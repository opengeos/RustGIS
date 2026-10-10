//! GeoTIFF reading and writing, including tiled and cloud-optimised files.
//!
//! Supports grey, RGB(A), palette and multiband images of any integer or floating-point pixel
//! type, stored in strips or tiles, chunky or planar, uncompressed or compressed with Deflate, LZW,
//! PackBits or JPEG. Georeferencing comes from the GeoTIFF tags (pixel scale and tie point, or a
//! transformation matrix without rotation) and the EPSG code in the GeoKey directory.

use std::io::{Read, Seek};

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use super::{ColorHint, Crs, GeoTransform, MAX_SAMPLES, Raster};
use crate::{DataError, Result};

fn err(e: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("GeoTIFF: {e}"))
}

/// The GeoTIFF parts of a TIFF's first image.
struct Geo {
    transform: GeoTransform,
    crs: Option<Crs>,
    /// An EPSG code that was named but isn't known.
    unknown_epsg: Option<u32>,
    geographic_model: bool,
    nodata: Option<f64>,
}

fn geo_tags<R: Read + Seek>(dec: &mut Decoder<R>) -> Result<Geo> {
    let doubles =
        |dec: &mut Decoder<R>, tag: u16| -> Option<Vec<f64>> { dec.find_tag(Tag::Unknown(tag)).ok().flatten()?.into_f64_vec().ok() };
    // GeoKey directory: a header of four shorts, then (key, location, count, value) entries.
    let keys: Vec<u16> = dec.find_tag(Tag::GeoKeyDirectoryTag).ok().flatten().and_then(|v| v.into_u16_vec().ok()).unwrap_or_default();
    let key = |k: u16| -> Option<u16> {
        keys.get(4..)?.as_chunks::<4>().0.iter().find(|[key, location, _, _]| *key == k && *location == 0).map(|[_, _, _, value]| *value)
    };
    let pixel_is_point = key(1025) == Some(2);
    let geographic_model = key(1024) == Some(2);
    // A user-defined (32767) projected code mustn't hide a usable geographic one.
    let usable = |c: &u16| *c != 32767 && *c != 0;
    let code = key(3072).filter(usable).or(key(2048).filter(usable)).map(u32::from);
    let crs = code.and_then(Crs::from_epsg);

    let transform = if let Some(m) = doubles(dec, 34264).filter(|m| m.len() >= 8) {
        let at = |i: usize| m.get(i).copied().unwrap_or(0.0);
        if at(1) != 0.0 || at(4) != 0.0 {
            return Err(err("rotated rasters aren't supported yet"));
        }
        GeoTransform { origin_x: at(3), origin_y: at(7), pixel_width: at(0), pixel_height: at(5) }
    } else {
        let scale = doubles(dec, 33550).filter(|s| s.len() >= 2).ok_or_else(|| err("the file isn't georeferenced"))?;
        let tie = doubles(dec, 33922).filter(|t| t.len() >= 6).ok_or_else(|| err("the file isn't georeferenced"))?;
        let s = |i: usize| scale.get(i).copied().unwrap_or(0.0);
        let t = |i: usize| tie.get(i).copied().unwrap_or(0.0);
        GeoTransform { origin_x: t(3) - t(0) * s(0), origin_y: t(4) + t(1) * s(1), pixel_width: s(0), pixel_height: -s(1) }
    };
    let transform = if pixel_is_point {
        GeoTransform {
            origin_x: transform.origin_x - transform.pixel_width / 2.0,
            origin_y: transform.origin_y - transform.pixel_height / 2.0,
            ..transform
        }
    } else {
        transform
    };
    let nodata = dec
        .find_tag(Tag::GdalNodata)
        .ok()
        .flatten()
        .and_then(|v| v.into_string().ok())
        .and_then(|s| s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).parse::<f64>().ok());
    Ok(Geo { transform, crs, unknown_epsg: code.filter(|_| crs.is_none()), geographic_model, nodata })
}

fn to_f32(buf: DecodingResult) -> (Vec<f32>, &'static str) {
    match buf {
        DecodingResult::U8(v) => (v.into_iter().map(f32::from).collect(), "UInt8"),
        DecodingResult::I8(v) => (v.into_iter().map(f32::from).collect(), "Int8"),
        DecodingResult::U16(v) => (v.into_iter().map(f32::from).collect(), "UInt16"),
        DecodingResult::I16(v) => (v.into_iter().map(f32::from).collect(), "Int16"),
        DecodingResult::U32(v) => (v.into_iter().map(|x| x as f32).collect(), "UInt32"),
        DecodingResult::I32(v) => (v.into_iter().map(|x| x as f32).collect(), "Int32"),
        DecodingResult::U64(v) => (v.into_iter().map(|x| x as f32).collect(), "UInt64"),
        DecodingResult::I64(v) => (v.into_iter().map(|x| x as f32).collect(), "Int64"),
        DecodingResult::F16(v) => (v.into_iter().map(f32::from).collect(), "Float16"),
        DecodingResult::F32(v) => (v, "Float32"),
        DecodingResult::F64(v) => (v.into_iter().map(|x| x as f32).collect(), "Float64"),
    }
}

/// Bytes per sample of the current image (from its bits per sample).
fn sample_bytes<R: Read + Seek>(dec: &mut Decoder<R>) -> usize {
    let bits = dec.find_tag_unsigned_vec::<u16>(Tag::BitsPerSample).ok().flatten().and_then(|v| v.into_iter().max()).unwrap_or(8);
    (bits as usize).div_ceil(8).max(1)
}

fn samples<R: Read + Seek>(dec: &mut Decoder<R>) -> usize {
    dec.find_tag_unsigned::<u16>(Tag::SamplesPerPixel).ok().flatten().unwrap_or(1).max(1) as usize
}

/// Reads a GeoTIFF. Images too large for [`MAX_SAMPLES`] are read from their largest overview that
/// fits, when the file has overviews.
pub fn read<R: Read + Seek>(r: R, name: &str) -> Result<Raster> {
    let limits = {
        let mut l = Limits::default();
        l.decoding_buffer_size = MAX_SAMPLES.saturating_mul(4);
        l.intermediate_buffer_size = 256 << 20;
        l.ifd_value_size = 64 << 20;
        l
    };
    let mut dec = Decoder::new(r).map_err(err)?.with_limits(limits);
    let (full_w, full_h) = dec.dimensions().map_err(err)?;
    let geo = geo_tags(&mut dec)?;
    let spp = samples(&mut dec);
    let bytes = sample_bytes(&mut dec);
    // The decoded image is held at its own pixel size before conversion to f32, so bound bytes
    // as well as samples.
    let fits = |w: u32, h: u32, spp: usize, bytes: usize| {
        let n = (w as usize).saturating_mul(h as usize).saturating_mul(spp);
        n <= MAX_SAMPLES && n.saturating_mul(bytes) <= MAX_SAMPLES.saturating_mul(4)
    };
    let (mut w, mut h) = (full_w, full_h);
    let mut overview = None;
    if !fits(w, h, spp, bytes) {
        let mut found = false;
        while dec.more_images() {
            dec.next_image().map_err(err)?;
            let subfile = dec.find_tag_unsigned::<u32>(Tag::NewSubfileType).ok().flatten().unwrap_or(0);
            if subfile & 4 != 0 {
                continue; // a transparency mask
            }
            let (ow, oh) = dec.dimensions().map_err(err)?;
            let (ospp, obytes) = (samples(&mut dec), sample_bytes(&mut dec));
            if fits(ow, oh, ospp, obytes) {
                (w, h) = (ow, oh);
                overview = Some((full_w, full_h));
                found = true;
                break;
            }
        }
        if !found {
            return Err(err(format!("{full_w} × {full_h} pixels is too large to open; build overviews for it first")));
        }
    }
    let ct = dec.colortype().map_err(err)?;
    let (nbands, color) = match ct {
        ColorType::Gray(_) => (1, ColorHint::Gray),
        ColorType::GrayA(_) => (2, ColorHint::GrayAlpha),
        ColorType::RGB(_) => (3, ColorHint::Rgb { alpha: false }),
        // The decoder turns JPEG-compressed YCbCr into RGB; other YCbCr data would stay YCbCr.
        ColorType::YCbCr(_) if dec.find_tag_unsigned::<u16>(Tag::Compression).ok().flatten() == Some(7) => {
            (3, ColorHint::Rgb { alpha: false })
        }
        ColorType::RGBA(_) => (4, ColorHint::Rgb { alpha: true }),
        ColorType::Palette(bits) => {
            let map: Vec<u16> = dec.find_tag(Tag::ColorMap).ok().flatten().and_then(|v| v.into_u16_vec().ok()).unwrap_or_default();
            let n = 1usize << bits.min(16);
            let c = |i: usize| (map.get(i).copied().unwrap_or(0) / 257) as u8;
            let table = if map.len() >= 3 * n { (0..n).map(|i| [c(i), c(n + i), c(2 * n + i), 255]).collect() } else { Vec::new() };
            (1, if table.is_empty() { ColorHint::Gray } else { ColorHint::Palette(table) })
        }
        ColorType::Multiband { num_samples, .. } => (num_samples.max(1) as usize, ColorHint::Gray),
        other => return Err(err(format!("{other:?} images aren't supported"))),
    };
    let planar = dec.find_tag_unsigned::<u16>(Tag::PlanarConfiguration).ok().flatten() == Some(2);
    let mut buf = DecodingResult::U8(Vec::new());
    dec.read_image_to_buffer(&mut buf).map_err(err)?;
    let (values, data_type) = to_f32(buf);
    let n = (w as usize) * (h as usize);
    if values.len() < n.saturating_mul(nbands) {
        return Err(err("couldn't read every band"));
    }
    let bands: Vec<Vec<f32>> = if nbands == 1 {
        let mut v = values;
        v.truncate(n);
        vec![v]
    } else if planar {
        values.chunks(n).take(nbands).map(<[f32]>::to_vec).collect()
    } else {
        // One pass over the interleaved pixels.
        let mut bands: Vec<Vec<f32>> = (0..nbands).map(|_| Vec::with_capacity(n)).collect();
        for px in values.chunks(nbands).take(n) {
            for (band, v) in bands.iter_mut().zip(px) {
                band.push(*v);
            }
        }
        drop(values);
        bands
    };

    let mut transform = geo.transform;
    if overview.is_some() {
        transform.pixel_width *= full_w as f64 / w.max(1) as f64;
        transform.pixel_height *= full_h as f64 / h.max(1) as f64;
    }
    let crs = match geo.crs {
        Some(c) => c,
        None => {
            let right = transform.origin_x + transform.pixel_width * w as f64;
            let bottom = transform.origin_y + transform.pixel_height * h as f64;
            let degrees = [transform.origin_x, right].iter().all(|x| (-180.0..=180.0).contains(x))
                && [transform.origin_y, bottom].iter().all(|y| (-90.0..=90.0).contains(y));
            if let Some(code) = geo.unknown_epsg {
                return Err(err(format!("the coordinate system EPSG:{code} isn't supported")));
            }
            if geo.geographic_model || degrees {
                Crs::LonLat
            } else {
                return Err(err("the file's coordinate system isn't recognised"));
            }
        }
    };
    let mut r = Raster::new(name, w, h, bands, transform, crs, geo.nodata, color, data_type)?;
    r.overview = overview;
    Ok(r)
}

/// Writes a raster as a Deflate-compressed GeoTIFF: RGB(A) bytes for 8-bit colour rasters,
/// otherwise `band` (0-based) as 32-bit floats.
pub fn write(r: &Raster, band: usize) -> Result<Vec<u8>> {
    use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype};
    let mut out = std::io::Cursor::new(Vec::new());
    let mut enc = TiffEncoder::new(&mut out).map_err(err)?.with_compression(Compression::Deflate(DeflateLevel::Balanced));
    let gt = r.transform;
    if gt.pixel_height > 0.0 || gt.pixel_width < 0.0 {
        return Err(err("only north-up rasters can be exported for now"));
    }
    let scale = [gt.pixel_width.abs(), gt.pixel_height.abs(), 0.0];
    let tie = [0.0, 0.0, 0.0, gt.origin_x, gt.origin_y, 0.0];
    let geographic = r.crs.is_geographic();
    let keys: [u16; 16] =
        [1, 1, 0, 3, 1024, 0, 1, if geographic { 2 } else { 1 }, 1025, 0, 1, 1, if geographic { 2048 } else { 3072 }, 0, 1, r.crs.epsg()];
    let (w, h) = (r.width, r.height);
    let bytes = |b: usize| -> Option<Vec<u8>> {
        let band = r.bands.get(b)?;
        let s = &band.stats;
        // Whole numbers within 0–255 (or no valid pixels at all) fit in a byte, and so must the
        // no-data value, which no-data pixels keep so the written GDAL_NODATA tag still matches.
        let whole_bytes = s.min >= 0.0 && s.max <= 255.0 && (s.categories.is_some() || r.data_type == "UInt8");
        let nodata = match r.nodata {
            Some(d) if (0.0..=255.0).contains(&d) && d.fract() == 0.0 => Some(d as u8),
            Some(_) => return None,
            None => None,
        };
        (whole_bytes || s.count == 0)
            .then(|| band.data.iter().map(|v| if r.is_valid(*v) { v.clamp(0.0, 255.0) as u8 } else { nodata.unwrap_or(0) }).collect())
    };
    let rgb = matches!(r.color, ColorHint::Rgb { .. }) && r.bands.len() >= 3;
    macro_rules! tags {
        ($img:expr) => {{
            let e = $img.encoder();
            e.write_tag(Tag::ModelPixelScaleTag, &scale[..]).map_err(err)?;
            e.write_tag(Tag::ModelTiepointTag, &tie[..]).map_err(err)?;
            e.write_tag(Tag::GeoKeyDirectoryTag, &keys[..]).map_err(err)?;
            if let Some(nd) = r.nodata {
                e.write_tag(Tag::GdalNodata, nd.to_string().as_str()).map_err(err)?;
            }
        }};
    }
    if rgb && let (Some(red), Some(green), Some(blue)) = (bytes(0), bytes(1), bytes(2)) {
        let alpha = if r.bands.len() >= 4 { bytes(3) } else { None };
        let n = (w as usize) * (h as usize);
        if let Some(a) = alpha {
            let mut data = Vec::with_capacity(n * 4);
            for i in 0..n {
                data.extend([red.get(i), green.get(i), blue.get(i), a.get(i)].map(|v| v.copied().unwrap_or(0)));
            }
            let mut img = enc.new_image::<colortype::RGBA8>(w, h).map_err(err)?;
            tags!(img);
            img.write_data(&data).map_err(err)?;
        } else {
            let mut data = Vec::with_capacity(n * 3);
            for i in 0..n {
                data.extend([red.get(i), green.get(i), blue.get(i)].map(|v| v.copied().unwrap_or(0)));
            }
            let mut img = enc.new_image::<colortype::RGB8>(w, h).map_err(err)?;
            tags!(img);
            img.write_data(&data).map_err(err)?;
        }
    } else {
        let b = r.bands.get(band).ok_or_else(|| err(format!("the raster has no band {}", band + 1)))?;
        let mut img = enc.new_image::<colortype::Gray32Float>(w, h).map_err(err)?;
        tags!(img);
        img.write_data(&b.data).map_err(err)?;
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::tests::small;

    #[test]
    fn float_round_trip() {
        let r = small();
        let bytes = write(&r, 0).unwrap();
        let back = read(std::io::Cursor::new(&bytes), "back").unwrap();
        assert_eq!((back.width, back.height), (4, 3));
        assert_eq!(back.crs, Crs::LonLat);
        assert_eq!(back.nodata, Some(-9999.0));
        assert_eq!(back.transform, r.transform);
        assert_eq!(back.value(0, 2, 1), Some(7.0));
        assert_eq!(back.value(0, 1, 1), None);
        assert_eq!(back.data_type, "Float32");
    }

    #[test]
    fn rgb_round_trip_in_utm() {
        let gt = GeoTransform { origin_x: 500_000.0, origin_y: 4_000_000.0, pixel_width: 30.0, pixel_height: -30.0 };
        let bands = vec![vec![10.0, 20.0], vec![30.0, 40.0], vec![50.0, 60.0]];
        let r =
            Raster::new("rgb", 2, 1, bands, gt, Crs::from_epsg(32617).unwrap(), None, ColorHint::Rgb { alpha: false }, "UInt8").unwrap();
        let back = read(std::io::Cursor::new(write(&r, 0).unwrap()), "back").unwrap();
        assert_eq!(back.band_count(), 3);
        assert_eq!(back.crs.epsg(), 32617);
        assert_eq!(back.color, ColorHint::Rgb { alpha: false });
        assert_eq!(back.value(2, 1, 0), Some(60.0));
        assert_eq!(back.data_type, "UInt8");
    }

    #[test]
    fn rgb_export_keeps_no_data() {
        let gt = GeoTransform { origin_x: 0.0, origin_y: 2.0, pixel_width: 1.0, pixel_height: -1.0 };
        let mk = |nodata: f64| {
            let bands = vec![vec![10.0, nodata as f32], vec![20.0, nodata as f32], vec![30.0, nodata as f32]];
            Raster::new("rgb", 2, 1, bands, gt, Crs::LonLat, Some(nodata), ColorHint::Rgb { alpha: false }, "UInt8").unwrap()
        };
        // A byte no-data value survives on the byte path.
        let back = read(std::io::Cursor::new(write(&mk(255.0), 0).unwrap()), "b").unwrap();
        assert_eq!((back.band_count(), back.value(0, 1, 0), back.value(0, 0, 0)), (3, None, Some(10.0)));
        // One that doesn't fit a byte keeps the file honest by writing floats (band 1 only).
        let back = read(std::io::Cursor::new(write(&mk(-9999.0), 0).unwrap()), "f").unwrap();
        assert_eq!((back.band_count(), back.value(0, 1, 0), back.nodata), (1, None, Some(-9999.0)));
    }

    #[test]
    fn south_up_export_is_refused() {
        let gt = GeoTransform { origin_x: 0.0, origin_y: 0.0, pixel_width: 1.0, pixel_height: 1.0 };
        let r = Raster::new("s", 1, 1, vec![vec![1.0]], gt, Crs::LonLat, None, ColorHint::Gray, "Float32").unwrap();
        assert!(write(&r, 0).is_err());
    }

    #[test]
    fn plain_tiff_is_not_georeferenced() {
        use tiff::encoder::{TiffEncoder, colortype};
        let mut out = std::io::Cursor::new(Vec::new());
        TiffEncoder::new(&mut out).unwrap().write_image::<colortype::Gray8>(2, 2, &[1, 2, 3, 4]).unwrap();
        let e = read(std::io::Cursor::new(out.into_inner()), "x").unwrap_err();
        assert!(e.to_string().contains("georeferenced"), "{e}");
    }

    #[test]
    fn junk_never_panics() {
        let r = small();
        let good = write(&r, 0).unwrap();
        for cut in [0, 8, 16, 100, good.len() / 2, good.len() - 1] {
            let _ = read(std::io::Cursor::new(&good[..cut]), "x");
        }
        let mut flipped = good.clone();
        for i in (8..flipped.len()).step_by(7) {
            flipped[i] ^= 0x5a;
        }
        let _ = read(std::io::Cursor::new(&flipped), "x");
    }
}
