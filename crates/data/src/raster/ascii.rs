//! ASCII grids (`.asc`): a short header, then rows of numbers.

use super::{ColorHint, Crs, GeoTransform, MAX_SAMPLES, Raster};
use crate::{DataError, Result};

/// Reads an ASCII grid. Without a known coordinate system, coordinates that fit in degrees are
/// taken as WGS 84 longitude/latitude.
pub fn read(text: &str, name: &str, crs: Option<Crs>) -> Result<Raster> {
    let bad = |m: &str| DataError::Format(format!("ASCII grid: {m}"));
    let mut tokens = text.split_ascii_whitespace().peekable();
    let (mut ncols, mut nrows, mut x, mut y, mut cell, mut dx, mut dy, mut nodata) = (None, None, None, None, None, None, None, None);
    let (mut centre_x, mut centre_y) = (false, false);
    let mut entries = 0;
    while let Some(key) = tokens.peek().map(|k| k.to_ascii_lowercase()) {
        if key.parse::<f64>().is_ok() {
            break;
        }
        // A real header has about six entries; give up quickly on text that isn't a grid.
        entries += 1;
        if entries > 16 {
            return Err(bad("not an ASCII grid header"));
        }
        tokens.next();
        let value: f64 = tokens.next().and_then(|v| v.parse().ok()).ok_or_else(|| bad(&format!("bad value for {key}")))?;
        match key.as_str() {
            "ncols" => ncols = Some(value),
            "nrows" => nrows = Some(value),
            "xllcorner" => x = Some(value),
            "yllcorner" => y = Some(value),
            "xllcenter" => {
                x = Some(value);
                centre_x = true;
            }
            "yllcenter" => {
                y = Some(value);
                centre_y = true;
            }
            "cellsize" => cell = Some(value),
            "dx" => dx = Some(value),
            "dy" => dy = Some(value),
            "nodata_value" => nodata = Some(value),
            _ => {}
        }
    }
    let size = |v: Option<f64>, what: &str| -> Result<u32> {
        let v = v.ok_or_else(|| bad(&format!("missing {what}")))?;
        if v >= 1.0 && v <= u32::MAX as f64 && v.fract() == 0.0 { Ok(v as u32) } else { Err(bad(&format!("bad {what}"))) }
    };
    let (w, h) = (size(ncols, "ncols")?, size(nrows, "nrows")?);
    let n = (w as usize).checked_mul(h as usize).filter(|n| *n <= MAX_SAMPLES).ok_or_else(|| bad("too large"))?;
    // CELLSIZE takes precedence over DX/DY, as in GDAL's ASCII grid reader.
    let (dx, dy) = if let Some(cell) = cell { (Some(cell), Some(cell)) } else { (dx, dy) };
    let dx = dx.filter(|c| c.is_finite() && *c > 0.0).ok_or_else(|| bad("missing or invalid cell width"))?;
    let dy = dy.filter(|c| c.is_finite() && *c > 0.0).ok_or_else(|| bad("missing or invalid cell height"))?;
    let (mut x, mut y) = (x.ok_or_else(|| bad("missing xllcorner"))?, y.ok_or_else(|| bad("missing yllcorner"))?);
    if centre_x {
        x -= dx / 2.0;
    }
    if centre_y {
        y -= dy / 2.0;
    }
    // Each value takes at least two characters, so the text bounds what to reserve.
    let mut data = Vec::with_capacity(n.min(text.len() / 2 + 1));
    let (mut rounded, mut scratch) = (0u64, String::new());
    for (i, t) in tokens.take(n).enumerate() {
        let v =
            t.parse::<f32>().map_err(|_| bad(&format!("value {} ('{}') isn't a number", i + 1, t.chars().take(20).collect::<String>())))?;
        rounded += u64::from(!held(t, v, nodata, &mut scratch));
        data.push(v);
    }
    if data.len() != n {
        return Err(bad("fewer values than ncols × nrows"));
    }
    let top = y + dy * h as f64;
    let crs = match crs {
        Some(c) => c,
        None if x >= -180.0 && x + dx * w as f64 <= 180.0 && y >= -90.0 && top <= 90.0 => Crs::LonLat,
        None => return Err(bad("unknown coordinate system: put a .prj file next to it")),
    };
    let gt = GeoTransform { origin_x: x, origin_y: top, pixel_width: dx, pixel_height: -dy };
    let mut r = Raster::new(name, w, h, vec![data], gt, crs, nodata, ColorHint::Gray, "Float32")?;
    if rounded > 0 {
        r.rounded = vec![rounded];
    }
    Ok(r)
}

/// Whether `v`, `text` read as `f32`, is the number `text` writes: always for up to 6 significant
/// digits (which `f32` keeps); past that, when `v` written back out is `text`'s number, as
/// `123.4567` is and `1234567.891` and `16777217` aren't. NaN and the no-data value count as held.
/// `scratch` is reused for writing `v` out.
fn held(text: &str, v: f32, nodata: Option<f64>, scratch: &mut String) -> bool {
    use std::fmt::Write;
    let Ok(wide) = text.parse::<f64>() else { return true };
    if v.is_nan() || f64::from(v) == wide || nodata == Some(wide) {
        return true;
    }
    // Overflowed to infinity, or into f32's subnormals, which keep fewer digits.
    if !v.is_finite() || v.abs() < f32::MIN_POSITIVE {
        return false;
    }
    let mantissa = text.as_bytes().split(|b| matches!(b, b'e' | b'E')).next().unwrap_or_default();
    let significant = |b: &u8| b.is_ascii_digit() && *b != b'0';
    let digits = match (mantissa.iter().position(significant), mantissa.iter().rposition(significant)) {
        (Some(a), Some(b)) => mantissa.get(a..=b).map_or(0, |d| d.iter().filter(|c| c.is_ascii_digit()).count()),
        _ => 0,
    };
    if digits <= 6 {
        return true;
    }
    scratch.clear();
    let _ = write!(scratch, "{v}");
    scratch.parse::<f64>().is_ok_and(|back| back == wide)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rectangular_pixels_and_rejects_incomplete_sizes() {
        let corner = "ncols 1\nnrows 2\nxllcorner 10\nyllcorner 20\n{}\n1\n2\n";
        for spacing in ["dx 2\ndy 3", "dy 3\ndx 2"] {
            let r = read(&corner.replace("{}", spacing), "g", None).unwrap();
            assert_eq!((r.transform.origin_x, r.transform.origin_y), (10.0, 26.0));
            assert_eq!((r.transform.pixel_width, r.transform.pixel_height), (2.0, -3.0));
            assert_eq!(r.value(0, 0, 0), Some(1.0));
            assert_eq!(r.value(0, 0, 1), Some(2.0));
            assert_eq!(r.cell_size(), (2.0, 3.0));
            assert_eq!(r.pixel_at(rustgis_geom::Coord::new(11.0, 24.0)), Some((0, 0)));
            let upload = super::super::read_bytes("g.asc", corner.replace("{}", spacing).as_bytes()).unwrap();
            assert_eq!(upload.cell_size(), (2.0, 3.0));
        }
        let center = "ncols 1\nnrows 2\nxllcenter 10\nyllcenter 20\ndx 2\ndy 3\n1\n2\n";
        let r = read(center, "g", None).unwrap();
        assert_eq!((r.transform.origin_x, r.transform.origin_y), (9.0, 24.5));
        assert_eq!((r.transform.pixel_width, r.transform.pixel_height), (2.0, -3.0));
        let mixed = center.replace("yllcenter", "yllcorner");
        let r = read(&mixed, "g", None).unwrap();
        assert_eq!((r.transform.origin_x, r.transform.origin_y), (9.0, 26.0));
        // When both forms are present, CELLSIZE takes precedence, as in GDAL.
        for spacing in ["cellsize 4\ndx 2\ndy 3", "dy 3\ndx 2\ncellsize 4"] {
            let r = read(&corner.replace("{}", spacing), "g", None).unwrap();
            assert_eq!((r.transform.pixel_width, r.transform.pixel_height, r.transform.origin_y), (4.0, -4.0, 28.0));
        }
        for spacing in ["dx 2", "dy 3", "dx 0\ndy 3", "dx 2\ndy -3", "dx NaN\ndy 3", "dx 2\ndy inf", "cellsize 0", "cellsize NaN"] {
            assert!(read(&corner.replace("{}", spacing), "g", None).is_err(), "{spacing}");
        }
        // An unprojected grid extending beyond longitude 180 cannot be inferred as WGS 84.
        let beyond = "ncols 2\nnrows 1\nxllcorner 177\nyllcorner 0\ndx 2\ndy 1\n1 2\n";
        assert!(read(beyond, "g", None).is_err());
        assert!(read(beyond, "g", Some(Crs::WebMercator)).is_ok());
        let at_edge = beyond.replace("177", "176");
        assert!(read(&at_edge, "g", None).is_ok());
        let north = "ncols 1\nnrows 1\nxllcorner 0\nyllcorner 89\ndx 1\ndy 2\n1\n";
        assert!(read(north, "g", None).is_err());
        assert!(read(&north.replace("89", "88"), "g", None).is_ok());
    }

    #[test]
    fn reads_grid() {
        let text = "ncols 3\nnrows 2\nxllcorner 10\nyllcorner 40\ncellsize 1\nNODATA_value -9999\n1 2 3\n4 -9999 6\n";
        let r = read(text, "g", None).unwrap();
        assert_eq!((r.width, r.height), (3, 2));
        assert_eq!(r.value(0, 0, 0), Some(1.0));
        assert_eq!(r.value(0, 1, 1), None);
        assert_eq!(r.transform.origin_y, 42.0);
        assert!(read("ncols 3\nnrows 2\n", "g", None).is_err());
        assert!(read("ncols 1e12\nnrows 1e12\nxllcorner 0\nyllcorner 0\ncellsize 1\n", "g", None).is_err());
        assert!(read("ncols 1\nnrows 1\nxllcorner 5e5\nyllcorner 5e6\ncellsize 10\n1", "g", None).is_err());
        assert!(read("ncols 2\nnrows 1\nxllcorner 0\nyllcorner 0\ncellsize 1\n1 x", "g", None).is_err(), "non-numbers");
        // Centre and corner coordinates per axis.
        let r = read("ncols 1\nnrows 1\nxllcenter 10.5\nyllcorner 40\ncellsize 1\n7", "g", None).unwrap();
        assert_eq!((r.transform.origin_x, r.transform.origin_y), (10.0, 41.0));
    }

    #[test]
    fn values_f32_cannot_hold_are_counted() {
        let grid =
            |values: &str| format!("ncols 4\nnrows 2\nxllcorner 10\nyllcorner 40\ncellsize 1\nNODATA_value -9999.123456789\n{values}");
        // Up to 7 digits that f32 writes back the same, a whole number at 2^24, the no-data value
        // and long zero runs are held.
        let clean = read(&grid("123.4567 16777216 -9999.123456789 12.3\n0.000100 1.5e-3 -0 1000000000"), "g", None).unwrap();
        assert!(clean.rounded.is_empty(), "{:?}", clean.rounded);
        // Past 2^24, too many digits, past f32's range and in its subnormals, they change.
        let lossy = read(&grid("16777217 1234567.891 1e39 1e-40\n0.123456789 1 2 3"), "g", None).unwrap();
        assert_eq!(lossy.rounded, vec![5]);
    }
}
