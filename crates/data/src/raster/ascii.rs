//! ASCII grids (`.asc`): a short header, then rows of numbers.

use super::{ColorHint, Crs, GeoTransform, MAX_SAMPLES, Raster};
use crate::{DataError, Result};

/// Reads an ASCII grid. Without a known coordinate system, coordinates that fit in degrees are
/// taken as WGS 84 longitude/latitude.
pub fn read(text: &str, name: &str, crs: Option<Crs>) -> Result<Raster> {
    let bad = |m: &str| DataError::Format(format!("ASCII grid: {m}"));
    let mut tokens = text.split_ascii_whitespace().peekable();
    let (mut ncols, mut nrows, mut x, mut y, mut cell, mut nodata) = (None, None, None, None, None, None);
    let (mut centre_x, mut centre_y) = (false, false);
    while let Some(key) = tokens.peek().map(|k| k.to_ascii_lowercase()) {
        if key.parse::<f64>().is_ok() {
            break;
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
            "cellsize" | "dx" | "dy" => cell = Some(value),
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
    let cell = cell.filter(|c| c.is_finite() && *c > 0.0).ok_or_else(|| bad("missing cellsize"))?;
    let (mut x, mut y) = (x.ok_or_else(|| bad("missing xllcorner"))?, y.ok_or_else(|| bad("missing yllcorner"))?);
    if centre_x {
        x -= cell / 2.0;
    }
    if centre_y {
        y -= cell / 2.0;
    }
    // Each value takes at least two characters, so the text bounds what to reserve.
    let mut data = Vec::with_capacity(n.min(text.len() / 2 + 1));
    for (i, t) in tokens.take(n).enumerate() {
        data.push(
            t.parse::<f32>().map_err(|_| bad(&format!("value {} ('{}') isn't a number", i + 1, t.chars().take(20).collect::<String>())))?,
        );
    }
    if data.len() != n {
        return Err(bad("fewer values than ncols × nrows"));
    }
    let top = y + cell * h as f64;
    let crs = match crs {
        Some(c) => c,
        None if x >= -180.0 && x + cell * w as f64 <= 180.0 && y >= -90.0 && top <= 90.0 => Crs::LonLat,
        None => return Err(bad("unknown coordinate system: put a .prj file next to it")),
    };
    let gt = GeoTransform { origin_x: x, origin_y: top, pixel_width: cell, pixel_height: -cell };
    Raster::new(name, w, h, vec![data], gt, crs, nodata, ColorHint::Gray, "Float32")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
