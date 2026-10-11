//! Post-processing of the drawn map: day and night shading, weather, vignette, edges, tilt-shift,
//! grain, colour adjustments and colour vision simulation.

use rustgis_geom::Coord;
use rustgis_map::view::{ColorVision, ViewEffects, Weather};
use tiny_skia::{Pixmap, PremultipliedColorU8};

/// Applies the effects (and the colour vision simulation, when given) to a drawn map.
/// `lonlat(x, y)` gives the longitude/latitude under a pixel, if any.
pub fn apply(pm: &mut Pixmap, fx: &ViewEffects, vision: ColorVision, lonlat: &dyn Fn(f64, f64) -> Option<Coord>) {
    if let Some(t) = fx.sun_time {
        night(pm, t, lonlat);
    }
    if fx.tilt_shift {
        tilt_shift(pm);
    }
    if fx.edges {
        edges(pm);
    }
    weather(pm, fx.weather);
    let adjust = fx.brightness != 0.0 || fx.contrast != 0.0 || fx.saturation != 0.0 || fx.sepia != 0.0 || fx.vignette || fx.grain;
    let matrix = vision.matrix();
    if !adjust && matrix.is_none() {
        return;
    }
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let (b, c, sat, sepia) = (
        (fx.brightness / 100.0) as f32,
        (1.0 + fx.contrast / 100.0) as f32,
        (1.0 + fx.saturation / 100.0) as f32,
        (fx.sepia / 100.0).clamp(0.0, 1.0) as f32,
    );
    // sRGB to linear and back, from lookup tables (4096 steps) instead of powf per pixel.
    const STEPS: usize = 4096;
    let table = |f: fn(f32) -> f32| -> Vec<f32> { (0..STEPS).map(|i| f(i as f32 / (STEPS - 1) as f32)).collect() };
    let (lin_t, srgb_t) = if matrix.is_some() {
        (
            table(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }),
            table(|v| if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let look = |t: &[f32], v: f32| t.get((v.clamp(0.0, 1.0) * (STEPS - 1) as f32).round() as usize).copied().unwrap_or(v);
    let lin = |v: f32| look(&lin_t, v);
    let srgb = |v: f32| look(&srgb_t, v);
    let width = pm.width() as usize;
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let a = px.alpha();
        if a == 0 {
            continue;
        }
        let d = px.demultiply();
        let mut rgb = [d.red() as f32 / 255.0, d.green() as f32 / 255.0, d.blue() as f32 / 255.0];
        if adjust {
            for v in &mut rgb {
                *v = ((*v - 0.5) * c + 0.5 + b).clamp(0.0, 1.0);
            }
            let l = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            for v in &mut rgb {
                *v = (l + (*v - l) * sat).clamp(0.0, 1.0);
            }
            if sepia > 0.0 {
                let s = [
                    0.393 * rgb[0] + 0.769 * rgb[1] + 0.189 * rgb[2],
                    0.349 * rgb[0] + 0.686 * rgb[1] + 0.168 * rgb[2],
                    0.272 * rgb[0] + 0.534 * rgb[1] + 0.131 * rgb[2],
                ];
                for (v, s) in rgb.iter_mut().zip(s) {
                    *v = (*v * (1.0 - sepia) + s.min(1.0) * sepia).clamp(0.0, 1.0);
                }
            }
            let (x, y) = ((i % width) as f32, (i / width) as f32);
            if fx.vignette {
                let (dx, dy) = ((x / w - 0.5) * 2.0, (y / h - 0.5) * 2.0);
                let k = 1.0 - 0.42 * ((dx * dx + dy * dy) / 2.0).min(1.0);
                rgb.iter_mut().for_each(|v| *v *= k);
            }
            if fx.grain {
                let n = (hash(i as u64) as f32 / u32::MAX as f32 - 0.5) * 0.09;
                rgb.iter_mut().for_each(|v| *v = (*v + n).clamp(0.0, 1.0));
            }
        }
        if let Some(m) = matrix {
            let l = [lin(rgb[0]), lin(rgb[1]), lin(rgb[2])];
            for (k, row) in m.iter().enumerate() {
                if let Some(v) = rgb.get_mut(k) {
                    *v = srgb((row[0] * l[0] + row[1] * l[1] + row[2] * l[2]).clamp(0.0, 1.0));
                }
            }
        }
        let q = |v: f32| (v.clamp(0.0, 1.0) * a as f32).round() as u8;
        if let Some(p) = PremultipliedColorU8::from_rgba(q(rgb[0]), q(rgb[1]), q(rgb[2]), a) {
            *px = p;
        }
    }
}

fn hash(mut x: u64) -> u32 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    (x ^ (x >> 33)) as u32
}

/// Blends a pixel toward a colour (straight alpha `t` of 0–1).
fn mix(px: &mut PremultipliedColorU8, c: [u8; 3], t: f32) {
    let a = px.alpha() as f32;
    let m = |v: u8, target: u8| (v as f32 * (1.0 - t) + target as f32 * t * a / 255.0).round().clamp(0.0, a) as u8;
    if let Some(p) = PremultipliedColorU8::from_rgba(m(px.red(), c[0]), m(px.green(), c[1]), m(px.blue(), c[2]), px.alpha()) {
        *px = p;
    }
}

/// The day of the year (0-based, fractional) for days since 1970-01-01, from the civil calendar.
fn day_of_year(days: f64) -> f64 {
    // Days from 1970-01-01 to January 1 of a year (proleptic Gregorian).
    let jan1 = |y: i64| -> i64 {
        let y1 = y - 1;
        365 * (y - 1970) + (y1.div_euclid(4) - 1969 / 4) - (y1.div_euclid(100) - 1969 / 100) + (y1.div_euclid(400) - 1969 / 400)
    };
    let d = days.floor() as i64;
    let mut y = 1970 + d.div_euclid(365);
    while jan1(y) > d {
        y -= 1;
    }
    while jan1(y + 1) <= d {
        y += 1;
    }
    days - jan1(y) as f64
}

/// The sun's elevation (degrees) at a place and time: an approximation good to about a degree
/// (declination from the day of the year, no equation of time), enough for shading.
pub fn sun_elevation(c: Coord, t: f64) -> f64 {
    sun_elevation_on(c, t, day_of_year((t / 86_400.0).clamp(-1e7, 1e7)))
}

/// [`sun_elevation`] with the day of the year already worked out.
fn sun_elevation_on(c: Coord, t: f64, doy: f64) -> f64 {
    let decl = (-23.44f64).to_radians() * ((360.0 / 365.0) * (doy + 10.0)).to_radians().cos();
    let utc_hours = (t / 3600.0).rem_euclid(24.0);
    let hour_angle = ((utc_hours - 12.0) * 15.0 + c.x).to_radians();
    let lat = c.y.to_radians();
    (lat.sin() * decl.sin() + lat.cos() * decl.cos() * hour_angle.cos()).asin().to_degrees()
}

/// Darkens the night side; twilight fades over the six degrees below the horizon.
fn night(pm: &mut Pixmap, t: f64, lonlat: &dyn Fn(f64, f64) -> Option<Coord>) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    // The day of the year is the same everywhere on the map.
    let doy = day_of_year((t / 86_400.0).clamp(-1e7, 1e7));
    let step = 4usize;
    let (gw, gh) = (w / step + 2, h / step + 2);
    let grid: Vec<f32> = (0..gh)
        .flat_map(|gy| (0..gw).map(move |gx| (gx, gy)))
        .map(|(gx, gy)| {
            lonlat((gx * step) as f64, (gy * step) as f64)
                .map_or(0.0, |c| (((-sun_elevation_on(c, t, doy)) / 6.0).clamp(0.0, 1.0) * 0.6) as f32)
        })
        .collect();
    let at = |x: usize, y: usize| -> f32 {
        let (gx, gy) = (x / step, y / step);
        let (fx, fy) = ((x % step) as f32 / step as f32, (y % step) as f32 / step as f32);
        let g = |a: usize, b: usize| grid.get(b * gw + a).copied().unwrap_or(0.0);
        let top = g(gx, gy) * (1.0 - fx) + g(gx + 1, gy) * fx;
        let bot = g(gx, gy + 1) * (1.0 - fx) + g(gx + 1, gy + 1) * fx;
        top * (1.0 - fy) + bot * fy
    };
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let t = at(i % w, i / w);
        if t > 0.0 && px.alpha() > 0 {
            mix(px, [8, 14, 40], t);
        }
    }
}

fn weather(pm: &mut Pixmap, wx: Weather) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    match wx {
        Weather::None => {}
        Weather::Haze | Weather::Fog => {
            let (base, extra) = if wx == Weather::Haze { (0.12, 0.18) } else { (0.3, 0.35) };
            for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
                // Thicker toward the top of the view (farther away).
                let t = base + extra * (1.0 - (i / w) as f32 / h.max(1) as f32);
                mix(px, [226, 231, 236], t);
            }
        }
        Weather::Rain | Weather::Snow => {
            let n = (w * h / if wx == Weather::Rain { 900 } else { 1400 }).min(20_000);
            let data = pm.pixels_mut();
            for k in 0..n {
                let (x0, y0) = ((hash(k as u64 * 2 + 1) as usize) % w.max(1), (hash(k as u64 * 2 + 2) as usize) % h.max(1));
                if wx == Weather::Rain {
                    for s in 0..10usize {
                        let (x, y) = (x0 + s / 3, y0 + s);
                        if x < w
                            && y < h
                            && let Some(px) = data.get_mut(y * w + x)
                        {
                            mix(px, [210, 220, 235], 0.45);
                        }
                    }
                } else {
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (x, y) = (x0 + dx, y0 + dy);
                        if x < w
                            && y < h
                            && let Some(px) = data.get_mut(y * w + x)
                        {
                            mix(px, [255, 255, 255], 0.85);
                        }
                    }
                }
            }
        }
    }
}

fn luminance(p: &PremultipliedColorU8) -> f32 {
    0.299 * p.red() as f32 + 0.587 * p.green() as f32 + 0.114 * p.blue() as f32
}

/// Darkens strong luminance edges (Sobel).
fn edges(pm: &mut Pixmap) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    if w < 3 || h < 3 {
        return;
    }
    let lum: Vec<f32> = pm.pixels().iter().map(luminance).collect();
    let l = |x: usize, y: usize| lum.get(y * w + x).copied().unwrap_or(0.0);
    let data = pm.pixels_mut();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx = l(x + 1, y - 1) + 2.0 * l(x + 1, y) + l(x + 1, y + 1) - l(x - 1, y - 1) - 2.0 * l(x - 1, y) - l(x - 1, y + 1);
            let gy = l(x - 1, y + 1) + 2.0 * l(x, y + 1) + l(x + 1, y + 1) - l(x - 1, y - 1) - 2.0 * l(x, y - 1) - l(x + 1, y - 1);
            let g = (gx * gx + gy * gy).sqrt() / 1020.0;
            if g > 0.08
                && let Some(px) = data.get_mut(y * w + x)
            {
                mix(px, [30, 30, 34], (g * 1.6).min(0.85));
            }
        }
    }
}

/// Blurs the top and bottom of the view, leaving a sharp band across the middle.
fn tilt_shift(pm: &mut Pixmap) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    if w < 4 || h < 4 {
        return;
    }
    let src: Vec<[f32; 4]> = pm.pixels().iter().map(|p| [p.red() as f32, p.green() as f32, p.blue() as f32, p.alpha() as f32]).collect();
    let r = (h / 90).clamp(2, 12);
    // Separable box blur (horizontal, then vertical).
    let mut tmp = vec![[0.0f32; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            let mut acc = [0.0f32; 4];
            for xx in lo..=hi {
                if let Some(s) = src.get(y * w + xx) {
                    acc.iter_mut().zip(s).for_each(|(a, b)| *a += b);
                }
            }
            let n = (hi - lo + 1) as f32;
            if let Some(t) = tmp.get_mut(y * w + x) {
                *t = acc.map(|v| v / n);
            }
        }
    }
    let data = pm.pixels_mut();
    for y in 0..h {
        let band = ((y as f32 / h as f32 - 0.5).abs() - 0.18).max(0.0) / 0.32;
        let t = band.min(1.0);
        if t <= 0.0 {
            continue;
        }
        for x in 0..w {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
            let mut acc = [0.0f32; 4];
            for yy in lo..=hi {
                if let Some(s) = tmp.get(yy * w + x) {
                    acc.iter_mut().zip(s).for_each(|(a, b)| *a += b);
                }
            }
            let n = (hi - lo + 1) as f32;
            let (Some(orig), Some(px)) = (src.get(y * w + x), data.get_mut(y * w + x)) else { continue };
            let mut v = [0u8; 4];
            for ((slot, b), o) in v.iter_mut().zip(&acc).zip(orig) {
                *slot = (o * (1.0 - t) + b / n * t).round().clamp(0.0, 255.0) as u8;
            }
            // Premultiplied: no colour channel may exceed alpha.
            let a = v[3];
            let c = |i: usize| v.get(i).copied().unwrap_or(0).min(a);
            if let Some(p) = PremultipliedColorU8::from_rgba(c(0), c(1), c(2), a) {
                *px = p;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_and_effects() {
        // Noon at Greenwich around the March equinox: the sun is high; midnight it's below.
        let noon = 1_710_936_000.0; // 2024-03-20 12:00 UTC
        assert!(sun_elevation(Coord::new(0.0, 0.0), noon) > 80.0);
        assert!(sun_elevation(Coord::new(0.0, 0.0), noon + 43_200.0) < -80.0);
        let mut pm = Pixmap::new(40, 40).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(200, 60, 60, 255));
        let fx = ViewEffects { vignette: true, sepia: 50.0, weather: Weather::Fog, ..ViewEffects::default() };
        apply(&mut pm, &fx, ColorVision::Achromatopsia, &|_, _| None);
        let c = pm.pixel(20, 20).unwrap();
        assert!(c.red().abs_diff(c.green()) < 3 && c.green().abs_diff(c.blue()) < 3, "no colour left: {c:?}");
        let corner = pm.pixel(0, 39).unwrap();
        assert!(corner.red() < c.red(), "vignette darkens corners");
    }

    #[test]
    fn days_of_the_year_follow_the_calendar() {
        assert_eq!(day_of_year(0.0), 0.0); // 1970-01-01
        assert_eq!(day_of_year(365.0), 0.0); // 1971-01-01
        assert_eq!(day_of_year(365.0 * 2.0 + 366.0), 0.0); // 1973-01-01 (1972 was a leap year)
        assert_eq!(day_of_year(19_723.0), 0.0); // 2024-01-01
        assert_eq!(day_of_year(19_723.0 + 365.0), 365.0); // 2024-12-31
        assert_eq!(day_of_year(-1.0), 364.0); // 1969-12-31
    }
}
