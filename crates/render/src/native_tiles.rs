//! A WMS or WMTS layer on a map drawn in one of the systems its service offers it in (#769):
//! asked for in that system and drawn as the service gives it, not warped from Web Mercator. A
//! WMS layer is asked for in tiles of its own, 256 pixels a side, at the power of two of the map's
//! units per pixel nearest its detail (so they are kept and found again as the map moves); a WMTS
//! layer's tiles are its matrix's nearest the map's detail.

use std::sync::Arc;

use rustgis_data::ogc::warp::{Grid, Matrix, System, Warp};
use rustgis_data::raster::Transformer;
use rustgis_geom::Coord;
use rustgis_map::TileLayer;
use rustgis_map::projection::Flat;
use rustgis_tiles::cache::{self, Payload, State, Want};
use tiny_skia::{FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, SpreadMode, Transform};

use crate::View;

/// Pixels a side of a WMS layer's own tiles.
const SIDE: u32 = 256;
/// Most pictures one view asks for: past that, coarser ones.
const MAX_PICTURES: usize = 400;
/// Coarser levels looked at for a picture still on its way.
const STAND_IN_LEVELS: u32 = 3;

/// How a layer is drawn in the map's system.
pub(crate) struct Native {
    /// The service's address with the placeholders for the system.
    base: String,
    how: How,
    transformer: Arc<Transformer>,
}

enum How {
    Map(System),
    Tiles(Vec<Matrix>),
}

/// How a tile layer is drawn on a map in `epsg`: `None` when its service doesn't offer it in
/// that system (it is then warped from Web Mercator with the others).
pub(crate) fn of(t: &TileLayer, epsg: u16) -> Option<Native> {
    if t.blend != rustgis_map::effects::Blend::Normal || !t.url.contains(rustgis_data::ogc::warp::MARK) {
        return None;
    }
    // The address names the map tile it is for once filled in; any will do to read it.
    let filled = t.url.replacen("{z}/{x}/{y}", "0/0/0", 1);
    let (base, _, w) = Warp::parse(&filled)?;
    let how = match &w.grid {
        Grid::Map { .. } => How::Map(w.system(epsg)?.clone()),
        Grid::Tiles(ms) if w.epsg == epsg => How::Tiles(ms.clone()),
        Grid::Tiles(_) => return None,
    };
    let base = match how {
        How::Map(_) => Warp::map_template(base),
        How::Tiles(_) => base.to_string(),
    };
    Some(Native { base, how, transformer: rustgis_tiles::warp::transformer(epsg)? })
}

/// One picture to draw: its address, the area it shows (west, south, east, north in the
/// system), and those of coarser ones that stand in for it while it is on its way.
struct Picture {
    url: String,
    area: [f64; 4],
    stand_ins: Vec<(String, [f64; 4])>,
}

/// Draws the layer over `pm` (the view's pixels), its pictures asked for: whether some are still
/// on their way, and the error of one that failed.
pub(crate) fn draw(pm: &mut Pixmap, n: &Native, t: &TileLayer, view: &View, flat: &Flat) -> (bool, Option<String>) {
    let cam = view.camera;
    let (w, h, k) = (view.width, view.height, view.pixels_per_point.max(0.01));
    let tr = &n.transformer;
    let to_native = |m: Coord| flat.inverse(m).and_then(|ll| tr.from_lonlat(ll));
    // The view's area in the system, from its edges.
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for i in 0..=4 {
        for j in 0..=4 {
            let (sx, sy) = (w * f64::from(i) / 4.0, h * f64::from(j) / 4.0);
            if let Some(p) = to_native(cam.to_map(sx, sy, w, h)) {
                lo = (lo.0.min(p.x), lo.1.min(p.y));
                hi = (hi.0.max(p.x), hi.1.max(p.y));
            }
        }
    }
    let (Some(a), Some(b)) = (to_native(cam.to_map(w / 2.0, h / 2.0, w, h)), to_native(cam.to_map(w / 2.0 + 1.0, h / 2.0, w, h))) else {
        return (false, None);
    };
    // The system's units per pixel of the map's pixmap.
    let per_px = (b.x - a.x).hypot(b.y - a.y) / k;
    if !(lo.0 < hi.0 && lo.1 < hi.1 && per_px.is_finite() && per_px > 0.0) {
        return (false, None);
    }
    let area = [lo.0, lo.1, hi.0, hi.1];
    let pictures = match &n.how {
        How::Map(system) => map_pictures(&n.base, system, area, per_px),
        How::Tiles(ms) => matrix_pictures(&n.base, ms, area, per_px),
    };
    let place = |c: Coord| -> Option<(f32, f32)> {
        let m = tr.to_lonlat(c).and_then(|ll| flat.forward(ll))?;
        let (x, y) = cam.to_screen(m, w, h);
        Some(((x * k) as f32, (y * k) as f32))
    };
    let opacity = t.opacity.clamp(0.0, 1.0);
    let (mut pending, mut error) = (false, None);
    for p in pictures {
        match cache::get(&p.url, Want::Image) {
            State::Ready(payload) => {
                if let Payload::Image(img) = &*payload {
                    paint(pm, img, p.area, p.area, opacity, &place);
                }
            }
            State::Pending => {
                pending = true;
                // The nearest coarser picture here already, over this one's area (the cache's,
                // not a copy).
                let stand_in =
                    p.stand_ins.iter().find_map(|(u, a)| cache::peek(u).filter(|c| matches!(**c, Payload::Image(_))).map(|c| (c, *a)));
                if let Some((payload, at)) = stand_in
                    && let Payload::Image(img) = &*payload
                {
                    paint(pm, img, at, p.area, opacity, &place);
                }
            }
            // A picture the service doesn't have (a WMTS tile past its coverage) is left clear, as
            // the warped tiles leave it; any other failure is the layer's error.
            State::Failed(e) if rustgis_tiles::warp::missing(&e) => {}
            State::Failed(e) => error = Some(e),
        }
    }
    (pending, error)
}

/// A WMS layer's own tiles over `area`: at the power of two of the system's units per pixel at
/// or below `per_px`, coarser when there would be too many.
fn map_pictures(base: &str, system: &System, area: [f64; 4], per_px: f64) -> Vec<Picture> {
    let mut level = per_px.log2().floor() as i32;
    loop {
        let span = |l: i32| 2f64.powi(l) * f64::from(SIDE);
        let s = span(level);
        let (c0, c1) = ((area[0] / s).floor(), (area[2] / s).floor());
        let (r0, r1) = ((area[1] / s).floor(), (area[3] / s).floor());
        if !(c0.is_finite() && c1.is_finite() && r0.is_finite() && r1.is_finite()) {
            return Vec::new();
        }
        if (c1 - c0 + 1.0) * (r1 - r0 + 1.0) > MAX_PICTURES as f64 {
            level += 1;
            continue;
        }
        let at = |l: i32, c: f64, r: f64| {
            let s = span(l);
            [c * s, r * s, (c + 1.0) * s, (r + 1.0) * s]
        };
        let mut out = Vec::new();
        for r in (r0 as i64)..=(r1 as i64) {
            for c in (c0 as i64)..=(c1 as i64) {
                let a = at(level, c as f64, r as f64);
                let stand_ins = (1..=STAND_IN_LEVELS as i32)
                    .map(|up| {
                        let d = 2f64.powi(up);
                        let pa = at(level + up, (c as f64 / d).floor(), (r as f64 / d).floor());
                        (Warp::map_address(base, system, pa, SIDE, SIDE), pa)
                    })
                    .collect();
                out.push(Picture { url: Warp::map_address(base, system, a, SIDE, SIDE), area: a, stand_ins });
            }
        }
        return out;
    }
}

/// A WMTS layer's tiles over `area`: of its coarsest matrix with as much detail as `per_px`
/// (else its finest), coarser when there would be too many.
fn matrix_pictures(base: &str, ms: &[Matrix], area: [f64; 4], per_px: f64) -> Vec<Picture> {
    let Some(mut i) = ms.iter().position(|m| m.cell <= per_px * 1.25).or(ms.len().checked_sub(1)) else { return Vec::new() };
    let tile_area = |m: &Matrix, c: u32, r: u32| {
        let (tw, th) = (m.cell * f64::from(m.tile_w), m.cell * f64::from(m.tile_h));
        let x0 = m.left + f64::from(c) * tw;
        let y1 = m.top - f64::from(r) * th;
        [x0, y1 - th, x0 + tw, y1]
    };
    let address = |m: &Matrix, c: u32, r: u32| {
        let id = rustgis_data::ogc::warp::matrix_in_address(&m.id);
        base.replace("{nm}", &id).replace("{ncol}", &c.to_string()).replace("{nrow}", &r.to_string())
    };
    // The tile of `m` at a point, if it has one there.
    let tile_at = |m: &Matrix, x: f64, y: f64| -> Option<(u32, u32)> {
        let c = ((x - m.left) / (m.cell * f64::from(m.tile_w))).floor();
        let r = ((m.top - y) / (m.cell * f64::from(m.tile_h))).floor();
        (c >= 0.0 && r >= 0.0 && c < f64::from(m.cols) && r < f64::from(m.rows)).then_some((c as u32, r as u32))
    };
    loop {
        let Some(m) = ms.get(i) else { return Vec::new() };
        let (tw, th) = (m.cell * f64::from(m.tile_w), m.cell * f64::from(m.tile_h));
        let (mut c0, mut c1) = (((area[0] - m.left) / tw).floor().max(0.0), ((area[2] - m.left) / tw).floor().min(f64::from(m.cols) - 1.0));
        let (mut r0, mut r1) = (((m.top - area[3]) / th).floor().max(0.0), ((m.top - area[1]) / th).floor().min(f64::from(m.rows) - 1.0));
        if !(c0 <= c1 && r0 <= r1) {
            return Vec::new();
        }
        if (c1 - c0 + 1.0) * (r1 - r0 + 1.0) > MAX_PICTURES as f64 {
            if i > 0 {
                i -= 1;
                continue;
            }
            // Even its coarsest matrix has too many there: those nearest the view's centre.
            let half = ((MAX_PICTURES as f64).sqrt() / 2.0).floor();
            let (mc, mr) = (((c0 + c1) / 2.0).floor(), ((r0 + r1) / 2.0).floor());
            (c0, c1) = ((mc - half).max(c0), (mc + half - 1.0).min(c1));
            (r0, r1) = ((mr - half).max(r0), (mr + half - 1.0).min(r1));
        }
        let mut out = Vec::new();
        for r in (r0 as u32)..=(r1 as u32) {
            for c in (c0 as u32)..=(c1 as u32) {
                let a = tile_area(m, c, r);
                let mid = ((a[0] + a[2]) / 2.0, (a[1] + a[3]) / 2.0);
                let stand_ins = (1..=STAND_IN_LEVELS as usize)
                    .filter_map(|up| {
                        let p = ms.get(i.checked_sub(up)?)?;
                        let (pc, pr) = tile_at(p, mid.0, mid.1)?;
                        Some((address(p, pc, pr), tile_area(p, pc, pr)))
                    })
                    .collect();
                out.push(Picture { url: address(m, c, r), area: a, stand_ins });
            }
        }
        return out;
    }
}

/// Draws a picture showing `area` of the system over the part of the pixmap `clip` (an area of
/// the system within it) covers, `place` putting a point of the system on the pixmap.
fn paint(pm: &mut Pixmap, img: &Pixmap, area: [f64; 4], clip: [f64; 4], opacity: f32, place: &dyn Fn(Coord) -> Option<(f32, f32)>) {
    let corner = |x: f64, y: f64| place(Coord::new(x, y));
    let (Some(tl), Some(tr), Some(bl)) = (corner(area[0], area[3]), corner(area[2], area[3]), corner(area[0], area[1])) else { return };
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    if iw <= 0.0 || ih <= 0.0 {
        return;
    }
    let transform = Transform::from_row((tr.0 - tl.0) / iw, (tr.1 - tl.1) / iw, (bl.0 - tl.0) / ih, (bl.1 - tl.1) / ih, tl.0, tl.1);
    let mut path = PathBuilder::new();
    let quad = [(clip[0], clip[3]), (clip[2], clip[3]), (clip[2], clip[1]), (clip[0], clip[1])];
    for (i, (x, y)) in quad.iter().enumerate() {
        let Some((px, py)) = corner(*x, *y) else { return };
        if i == 0 { path.move_to(px, py) } else { path.line_to(px, py) }
    }
    path.close();
    let Some(path) = path.finish() else { return };
    let shader = Pattern::new(img.as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, opacity, transform);
    // Without anti-aliasing, neighbouring pictures meet without a seam.
    let paint = Paint { shader, anti_alias: false, ..Paint::default() };
    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wms_layers_own_tiles() {
        let system = System { epsg: 2056, name: "EPSG:2056".into(), lat_first: false };
        // 1.5 m a pixel: tiles of 1 m pixels, 256 m a side.
        let p = map_pictures(
            "https://m.example.org/wms?CRS={ncrs}&BBOX={nbbox}&WIDTH={nw}&HEIGHT={nh}",
            &system,
            [2_600_000.0, 1_200_000.0, 2_600_600.0, 1_200_300.0],
            1.5,
        );
        assert_eq!(p.len(), 3 * 2);
        assert_eq!(p[0].area, [2_599_936.0, 1_199_872.0, 2_600_192.0, 1_200_128.0]);
        assert_eq!(p[0].url, "https://m.example.org/wms?CRS=EPSG:2056&BBOX=2599936,1199872,2600192,1200128&WIDTH=256&HEIGHT=256");
        // Its stand-ins: the tiles twice, four and eight times as coarse containing it.
        assert_eq!(p[0].stand_ins.len(), STAND_IN_LEVELS as usize);
        let a = p[0].stand_ins[0].1;
        assert!(a[0] <= p[0].area[0] && a[2] >= p[0].area[2] && a[2] - a[0] == 512.0);
        // A view wider than its tiles allow: coarser ones.
        let wide = map_pictures("x{nbbox}", &system, [0.0, 0.0, 1e6, 1e6], 1.0);
        assert!(!wide.is_empty() && wide.len() <= MAX_PICTURES);
    }

    #[test]
    fn a_wmts_layers_tiles() {
        let m = |id: &str, cell: f64| Matrix {
            id: id.into(),
            left: 2_420_000.0,
            top: 1_350_000.0,
            cell,
            tile_w: 256,
            tile_h: 256,
            cols: 4000,
            rows: 4000,
        };
        let ms = [m("a", 4.0), m("b", 2.0), m("c", 1.0)];
        let p =
            matrix_pictures("https://t.example.org/{nm}/{nrow}/{ncol}.png", &ms, [2_600_000.0, 1_200_000.0, 2_600_600.0, 1_200_300.0], 2.1);
        assert!(p.iter().all(|p| p.url.starts_with("https://t.example.org/b/")), "{}", p[0].url);
        assert!(p[0].stand_ins.first().is_some_and(|(u, _)| u.starts_with("https://t.example.org/a/")));
        // Finer than its finest: the finest.
        let p =
            matrix_pictures("https://t.example.org/{nm}/{nrow}/{ncol}.png", &ms, [2_600_000.0, 1_200_000.0, 2_600_100.0, 1_200_100.0], 0.1);
        assert!(p.iter().all(|p| p.url.starts_with("https://t.example.org/c/")));
        // Outside the matrix: nothing.
        assert!(matrix_pictures("x", &ms, [0.0, 0.0, 10.0, 10.0], 1.0).is_empty());
        // More than a view takes even of its coarsest matrix: those nearest the centre.
        let fine = [m("f", 0.1)];
        let p = matrix_pictures("x/{nm}/{ncol}/{nrow}", &fine, [2_500_000.0, 1_100_000.0, 2_700_000.0, 1_300_000.0], 0.1);
        assert!(!p.is_empty() && p.len() <= MAX_PICTURES, "{}", p.len());
    }
}
