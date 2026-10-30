//! A layer its service draws on request (a WMS, a map service's export) asked for one picture
//! of the view instead of tiles (#1162): the service lays out its symbols and labels over the
//! whole view, where each tile would cut them at its edges, and is asked once a view instead of
//! for every tile.
//!
//! The picture reaches past the view, its middle on a grid and its detail rounded down to a
//! quarter of a power of two, so a small pan or zoom asks for nothing new; one on its way is
//! stood in for by the layer's last picture. Its size is in the map's logical pixels, as tiles
//! are asked for, so the service's symbols keep their size on a high-density screen.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rustgis_data::ogc::warp::{Grid, System, Warp};
use rustgis_data::raster::Transformer;
use rustgis_geom::Coord;
use rustgis_map::TileLayer;
use rustgis_map::projection::Flat;
use rustgis_tiles::cache::{self, Payload, State, Want};
use tiny_skia::Pixmap;

use crate::View;

/// Most pixels a side of a picture: what map servers usually draw at most.
const MAX_SIDE: u32 = 4096;
/// Most pixels a side once a service has refused a larger picture.
const SMALL_SIDE: u32 = 2048;
/// How much wider and taller than the view the picture is: room for the grid its middle is on
/// and for zooming out by a step before a new one is asked for.
const MARGIN: f64 = 1.8;
/// Steps a doubling of the map's detail is split into: the picture's units per pixel are the
/// map's, rounded down to one.
const STEPS: f64 = 4.0;
/// Pixels a picture's sides are rounded up to (so a view's size changing a little keeps it).
const ROUND: f64 = 256.0;
/// Most layers whose last picture (and refused size) is kept.
const MAX_KEPT: usize = 256;

/// How a layer is asked for a picture: its address with the box, size and system to fill in
/// (`{nbbox}`, `{nw}`, `{nh}`, `{ncrs}`), and the system the picture is in.
pub(crate) struct Request {
    base: String,
    system: System,
    /// From the system to longitude/latitude and back; `None` for Web Mercator on a Web
    /// Mercator map (the map's own units).
    transformer: Option<Arc<Transformer>>,
}

/// How `t` is asked for one picture on a flat map in `epsg` (3857 for Web Mercator): `None` when
/// it isn't to be, or can't be (its service doesn't offer it in that system): it is drawn in
/// tiles then.
pub(crate) fn of(t: &TileLayer, epsg: u16) -> Option<Request> {
    if !t.single_image || !t.on_request() {
        return None;
    }
    let mercator = || System { epsg: 3857, name: "EPSG:3857".into(), lat_first: false };
    if !t.url.contains(rustgis_data::ogc::warp::MARK) {
        // Asked for in Web Mercator by its box: on a Web Mercator map only.
        if epsg != 3857 {
            return None;
        }
        let base = rustgis_map::tiles::sized(&t.url, "{nw}", "{nh}")
            .replace("{bbox-epsg-3857}", "{nbbox}")
            .replace("{width}", "{nw}")
            .replace("{height}", "{nh}");
        return Some(Request { base, system: mercator(), transformer: None });
    }
    // A layer of a system of its own: asked for in the map's when its service offers it.
    let filled = t.url.replacen("{z}/{x}/{y}", "0/0/0", 1);
    let (base, _, w) = Warp::parse(&filled)?;
    if !matches!(w.grid, Grid::Map { .. }) {
        return None;
    }
    let system = w.system(epsg)?.clone();
    let base = Warp::map_template(base);
    if epsg == 3857 {
        return Some(Request { base, system, transformer: None });
    }
    let transformer = Some(rustgis_tiles::warp::transformer(epsg)?);
    Some(Request { base, system, transformer })
}

/// The picture to ask for: its area (west, south, east, north in the system) and size in
/// pixels, for a view showing `area` at `res` units a logical pixel, `max_side` pixels a side at
/// most. `None` for a view with no size.
fn picture(area: [f64; 4], res: f64, max_side: u32) -> Option<([f64; 4], u32, u32)> {
    // The view's box in logical pixels: the same as the map zooms (a window's size).
    let (bw, bh) = ((area[2] - area[0]) / res, (area[3] - area[1]) / res);
    if !(bw.is_finite() && bh.is_finite() && bw > 0.0 && bh > 0.0 && res.is_finite() && res > 0.0) {
        return None;
    }
    let need = |b: f64| (b * MARGIN / ROUND).ceil() * ROUND;
    let (nw, nh) = (need(bw), need(bh));
    // Past the most a service draws: coarser, by whole steps.
    let over = (nw.max(nh) / f64::from(max_side)).max(1.0);
    let coarser = (over.log2() * STEPS).ceil();
    let shrink = 2f64.powf(coarser / STEPS);
    let level = (res.log2() * STEPS).floor() + coarser;
    let r = 2f64.powf(level / STEPS);
    let (w, h) = ((nw / shrink).ceil().min(f64::from(max_side)), (nh / shrink).ceil().min(f64::from(max_side)));
    // The middle on a grid of a sixth of the picture: the view stays inside it until it has
    // moved that far.
    let (sx, sy) = (w * r / 6.0, h * r / 6.0);
    let (cx, cy) = (((area[0] + area[2]) / 2.0 / sx).round() * sx, ((area[1] + area[3]) / 2.0 / sy).round() * sy);
    let (hw, hh) = (w * r / 2.0, h * r / 2.0);
    let out = [cx - hw, cy - hh, cx + hw, cy + hh];
    out.iter().all(|v| v.is_finite()).then_some((out, w as u32, h as u32))
}

/// How long a layer whose large picture failed is asked for smaller ones before a large one is
/// tried again (the failure may have been passing: a timeout, a busy server).
const SMALL_FOR: web_time::Duration = web_time::Duration::from_secs(300);

/// What is kept between frames, by the layer's address: its last picture drawn (the address
/// and area), and when a large picture of it last failed.
#[derive(Default)]
struct Kept {
    last: HashMap<String, (String, [f64; 4])>,
    small: HashMap<String, web_time::Instant>,
}

fn kept() -> std::sync::MutexGuard<'static, Kept> {
    static KEPT: OnceLock<Mutex<Kept>> = OnceLock::new();
    KEPT.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Half the width of the Web Mercator world, metres: what a service draws in it.
const HALF_WORLD: f64 = std::f64::consts::PI * rustgis_geom::MERCATOR_R;

/// A Web Mercator picture kept to the world: `None` where it reaches past the antimeridian (a
/// service draws nothing there: the layer is drawn in tiles, which wrap); past the poles' edge,
/// cut off there, with as many fewer pixels.
fn in_world(at: [f64; 4], w: u32, h: u32) -> Option<Option<([f64; 4], u32, u32)>> {
    if at[0] < -HALF_WORLD || at[2] > HALF_WORLD {
        return None;
    }
    let r = (at[3] - at[1]) / f64::from(h.max(1));
    let (s, n) = (at[1].max(-HALF_WORLD), at[3].min(HALF_WORLD));
    let rows = ((n - s) / r).round();
    if rows.is_nan() || rows < 1.0 {
        return Some(None);
    }
    Some(Some(([at[0], s, at[2], n], w, rows.min(f64::from(h)) as u32)))
}

/// Web Mercator metres a logical pixel at the middle of the view, whatever the map's projection
/// and its units: what a layer's zooms are worked out from.
fn mercator_res(view: &View, flat: &Flat) -> Option<f64> {
    let (cam, w, h) = (view.camera, view.width, view.height);
    let (a, b) = (cam.to_map(w / 2.0, h / 2.0, w, h), cam.to_map(w / 2.0 + 1.0, h / 2.0, w, h));
    let (a, b) = match flat {
        Flat::Mercator => (a, b),
        Flat::Custom(_) => (rustgis_geom::lonlat_to_merc(flat.inverse(a)?), rustgis_geom::lonlat_to_merc(flat.inverse(b)?)),
    };
    let r = (b.x - a.x).hypot(b.y - a.y);
    (r.is_finite() && r > 0.0).then_some(r)
}

/// Metres a pixel of a `tile_size`-pixel tile at `zoom` (a layer's zooms are its tiles').
fn zoom_res(zoom: u8, tile_size: u32) -> f64 {
    2.0 * HALF_WORLD / (f64::from(tile_size.max(1)) * 2f64.powi(i32::from(zoom)))
}

/// Draws the layer over `pm` (the view's pixels) from one picture: whether it is still on its
/// way, and the error of one that failed. `None` when the view's picture can't be asked for (it
/// crosses the antimeridian, or the service failed to draw it): the layer is drawn in tiles
/// instead.
pub(crate) fn draw(pm: &mut Pixmap, req: &Request, t: &TileLayer, view: &View, flat: &Flat) -> Option<(bool, Option<String>)> {
    let nothing = Some((false, None));
    // Not closer out than its service draws it (as its tiles).
    let Some(merc) = mercator_res(view, flat) else { return nothing };
    if rustgis_tiles::template::zoom_for(merc, t.tile_size, rustgis_map::tiles::MAX_ZOOM) < t.min_zoom {
        return nothing;
    }
    // Closer in than its finest zoom, its picture there is stretched (as its finest tiles are).
    let finer = (zoom_res(t.max_zoom, t.tile_size) / merc).max(1.0);
    let cam = view.camera;
    let (w, h, k) = (view.width, view.height, view.pixels_per_point.max(0.01));
    let to_system = |m: Coord| -> Option<Coord> {
        match &req.transformer {
            None => Some(m),
            Some(tr) => flat.inverse(m).and_then(|ll| tr.from_lonlat(ll)),
        }
    };
    let place = |c: Coord| -> Option<(f32, f32)> {
        let m = match &req.transformer {
            None => c,
            Some(tr) => tr.to_lonlat(c).and_then(|ll| flat.forward(ll))?,
        };
        let (x, y) = cam.to_screen(m, w, h);
        Some(((x * k) as f32, (y * k) as f32))
    };
    // The view's area in the system, from its edges, and its units a logical pixel.
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for i in 0..=4 {
        for j in 0..=4 {
            let (sx, sy) = (w * f64::from(i) / 4.0, h * f64::from(j) / 4.0);
            if let Some(p) = to_system(cam.to_map(sx, sy, w, h)) {
                lo = (lo.0.min(p.x), lo.1.min(p.y));
                hi = (hi.0.max(p.x), hi.1.max(p.y));
            }
        }
    }
    let (Some(a), Some(b)) = (to_system(cam.to_map(w / 2.0, h / 2.0, w, h)), to_system(cam.to_map(w / 2.0 + 1.0, h / 2.0, w, h))) else {
        return nothing;
    };
    let res = (b.x - a.x).hypot(b.y - a.y) * finer;
    if !(lo.0 < hi.0 && lo.1 < hi.1) {
        return nothing;
    }
    let area = [lo.0, lo.1, hi.0, hi.1];
    // Nothing asked for outside the layer's bounds, when known.
    let bounds = t.bounds.and_then(|[bw, bs, be, bn]| match &req.transformer {
        None => {
            let (sw, ne) = (rustgis_geom::lonlat_to_merc(Coord::new(bw, bs)), rustgis_geom::lonlat_to_merc(Coord::new(be, bn)));
            Some([sw.x, sw.y, ne.x, ne.y])
        }
        Some(tr) => rustgis_data::ogc::warp::native_box(tr, [bw, bs, be, bn]),
    });
    if bounds.is_some_and(|l| l[0] > area[2] || l[2] < area[0] || l[1] > area[3] || l[3] < area[1]) {
        return nothing;
    }
    // Kept by the system too: a picture in another is no stand-in.
    let key = format!("{} {}", req.system.epsg, req.base);
    let opacity = t.opacity.clamp(0.0, 1.0);
    let blend = crate::blend_mode(t.blend);
    let paint = |pm: &mut Pixmap, img: &Pixmap, at: [f64; 4]| crate::native_tiles::paint_blended(pm, img, at, at, opacity, blend, &place);
    // Asked again smaller when the service refuses a large picture.
    for _ in 0..2 {
        let small = kept().small.get(&key).is_some_and(|at| at.elapsed() < SMALL_FOR);
        let Some((at, pw, ph)) = picture(area, res, if small { SMALL_SIDE } else { MAX_SIDE }) else { return nothing };
        let (at, pw, ph) = match req.system.epsg {
            3857 => match in_world(at, pw, ph)? {
                Some(p) => p,
                None => return nothing,
            },
            _ => (at, pw, ph),
        };
        let url = Warp::map_address(&req.base, &req.system, at, pw, ph);
        match cache::get(&url, Want::Image) {
            State::Ready(payload) => {
                if let Payload::Image(img) = &*payload {
                    paint(pm, img, at);
                    let mut k = kept();
                    if k.last.len() >= MAX_KEPT {
                        k.last.clear();
                    }
                    k.last.insert(key, (url, at));
                }
                return nothing;
            }
            State::Pending => {
                // The layer's last picture, where it still is in the cache, stands in.
                let last = kept().last.get(&key).cloned();
                if let Some((u, at)) = last
                    && let Some(payload) = cache::peek(&u)
                    && let Payload::Image(img) = &*payload
                {
                    paint(pm, img, at);
                }
                return Some((true, None));
            }
            // Asked again at most 2048 pixels a side for a while: some services draw no larger.
            State::Failed(_) if !small && pw.max(ph) > SMALL_SIDE => {
                let mut k = kept();
                if k.small.len() >= MAX_KEPT {
                    k.small.clear();
                }
                k.small.insert(key.clone(), web_time::Instant::now());
            }
            // A picture the service doesn't draw: its tiles may draw (and say what fails).
            State::Failed(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layer's zooms are its tiles': a 512-pixel tile at a zoom has the resolution of a
    /// 256-pixel one a zoom finer, so its picture is held, and its layer drawn, as its tiles are.
    #[test]
    fn zooms_are_in_the_layers_tile_size() {
        for z in 0..20 {
            assert!((zoom_res(z, 512) - zoom_res(z + 1, 256)).abs() < 1e-9 * zoom_res(z + 1, 256));
            assert_eq!(rustgis_tiles::template::zoom_for(zoom_res(z, 512), 512, 22), z);
        }
    }

    #[test]
    fn a_picture_reaches_past_the_view_and_holds_while_it_moves_a_little() {
        // A 1000 × 600 pixel view, 10 m a pixel.
        let res = 10.0;
        let view = |cx: f64, cy: f64, r: f64| [cx - 500.0 * r, cy - 300.0 * r, cx + 500.0 * r, cy + 300.0 * r];
        let (at, w, h) = picture(view(100_000.0, 200_000.0, res), res, MAX_SIDE).unwrap();
        // 1.8 times the view, rounded up to 256 pixels.
        assert_eq!((w, h), (2048, 1280));
        let r = (at[2] - at[0]) / f64::from(w);
        assert!(r <= res && r > res / 2f64.powf(1.0 / STEPS), "{r}");
        let v = view(100_000.0, 200_000.0, res);
        assert!(at[0] <= v[0] && at[1] <= v[1] && at[2] >= v[2] && at[3] >= v[3]);
        // Pans of less than half the grid either way and a small zoom ask for the same picture.
        let (cx, cy) = ((at[0] + at[2]) / 2.0, (at[1] + at[3]) / 2.0);
        let (gx, gy) = ((at[2] - at[0]) / 6.0, (at[3] - at[1]) / 6.0);
        for (dx, dy) in [(0.45, 0.0), (-0.45, 0.3), (0.2, -0.45)] {
            assert_eq!(picture(view(cx + dx * gx, cy + dy * gy, res), res, MAX_SIDE).unwrap().0, at, "{dx} {dy}");
        }
        assert_eq!(picture(view(cx, cy, res * 1.05), res * 1.05, MAX_SIDE).unwrap().0, at);
        // Past it, the next picture along.
        assert_ne!(picture(view(cx + 0.55 * gx, cy, res), res, MAX_SIDE).unwrap().0, at);
        // Panned and zoomed anywhere, the view is inside its picture.
        for i in 0..400 {
            let f = f64::from(i);
            let (cx, cy, r) = (f * 731.7 - 90_000.0, f * -517.3 + 33_000.0, res * 2f64.powf(f / 97.0 - 2.0));
            let v = view(cx, cy, r);
            let (at, w, h) = picture(v, r, MAX_SIDE).unwrap();
            assert!(at[0] <= v[0] && at[1] <= v[1] && at[2] >= v[2] && at[3] >= v[3], "{i}");
            assert!(w <= MAX_SIDE && h <= MAX_SIDE);
        }
    }

    #[test]
    fn a_view_wider_than_a_service_draws_is_asked_coarser() {
        // 3000 pixels wide: 5400 with the margin, past 4096 and 2048.
        let v = [0.0, 0.0, 3000.0, 1000.0];
        for max in [MAX_SIDE, SMALL_SIDE] {
            let (at, w, h) = picture(v, 1.0, max).unwrap();
            assert!(w <= max && h <= max, "{w} × {h}");
            assert!(at[0] <= v[0] && at[2] >= v[2] && at[1] <= v[1] && at[3] >= v[3]);
        }
        assert!(picture([0.0, 0.0, 0.0, 1.0], 1.0, MAX_SIDE).is_none());
        assert!(picture([0.0, 0.0, 1.0, 1.0], f64::NAN, MAX_SIDE).is_none());
    }

    #[test]
    fn a_web_mercator_picture_is_kept_to_the_world() {
        let h = HALF_WORLD;
        // Inside: as it is.
        assert_eq!(in_world([-10.0, -20.0, 10.0, 20.0], 100, 200), Some(Some(([-10.0, -20.0, 10.0, 20.0], 100, 200))));
        // Past the antimeridian (a Pacific view): tiles, which wrap.
        assert_eq!(in_world([h - 10.0, 0.0, h + 10.0, 20.0], 100, 100), None);
        assert_eq!(in_world([-h - 1.0, 0.0, 0.0, 20.0], 100, 100), None);
        // Past the top of the world: cut there, with as many fewer rows.
        let (at, w, rows) = in_world([0.0, h - 50.0, 100.0, h + 150.0], 100, 200).unwrap().unwrap();
        assert_eq!((at, w, rows), ([0.0, h - 50.0, 100.0, h], 100, 50));
        // Wholly past it: nothing.
        assert_eq!(in_world([0.0, h + 10.0, 100.0, h + 20.0], 100, 10), Some(None));
    }

    #[test]
    fn asked_for_by_its_own_address() {
        let wms = "https://m.example.org/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=a&STYLES=&CRS=EPSG:3857&BBOX={bbox-epsg-3857}&WIDTH=256&HEIGHT=256&FORMAT=image%2Fpng&TRANSPARENT=TRUE";
        let mut t = TileLayer::new("a", rustgis_map::TileKind::Raster, wms);
        assert!(of(&t, 3857).is_none(), "tiles unless asked");
        t.single_image = true;
        let r = of(&t, 3857).unwrap();
        assert_eq!(
            Warp::map_address(&r.base, &r.system, [1.0, 2.0, 3.0, 4.0], 800, 600),
            "https://m.example.org/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=a&STYLES=&CRS=EPSG:3857&BBOX=1,2,3,4&WIDTH=800&HEIGHT=600&FORMAT=image%2Fpng&TRANSPARENT=TRUE"
        );
        // Not on a map in another system: its service is asked in Web Mercator only.
        assert!(of(&t, 2056).is_none());
        let export = "https://s.example.org/arcgis/rest/services/x/MapServer/export?bbox={bbox-epsg-3857}&bboxSR=3857&imageSR=3857&size=256,256&format=png32&transparent=true&f=image";
        t.url = export.into();
        let r = of(&t, 3857).unwrap();
        assert_eq!(
            Warp::map_address(&r.base, &r.system, [1.0, 2.0, 3.0, 4.0], 800, 600),
            "https://s.example.org/arcgis/rest/services/x/MapServer/export?bbox=1,2,3,4&bboxSR=3857&imageSR=3857&size=800,600&format=png32&transparent=true&f=image"
        );
        // A template sized with `{width}` and `{height}`.
        t.url = "https://m.example.org/wms?BBOX={bbox-epsg-3857}&WIDTH={width}&HEIGHT={height}".into();
        let r = of(&t, 3857).unwrap();
        assert_eq!(
            Warp::map_address(&r.base, &r.system, [1.0, 2.0, 3.0, 4.0], 8, 6),
            "https://m.example.org/wms?BBOX=1,2,3,4&WIDTH=8&HEIGHT=6"
        );
        // Tiles cut from a tile set aren't drawn on request.
        t.url = "https://tile.example.org/{z}/{x}/{y}.png".into();
        assert!(of(&t, 3857).is_none());
    }
}
