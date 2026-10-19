//! The map's web tile layers, drawn over the basemap and under the layers.
//!
//! On the flat Web Mercator map tiles draw straight into the map. In any other projection, and
//! on the globe, they draw into a Web Mercator mosaic of the area in view, which is then warped
//! into place pixel by pixel.

use rustgis_geom::{Coord, Extent, MAX_LAT, lonlat_to_merc};
use rustgis_map::effects::Blend;
use rustgis_map::projection::Flat;
use rustgis_map::{Map, Rgba, TileKind, TileLayer};
use rustgis_tiles::{Frame, Kind, Spec};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use crate::raster::Place;
use crate::{PlacedIcon, PlacedLabel, View};

/// The largest side of the mosaic warped onto the globe or another projection.
const MOSAIC: f64 = 2048.0;

/// What drawing the tile layers found.
#[derive(Default)]
pub(crate) struct TileOutput {
    pub labels: Vec<(PlacedLabel, f32)>,
    pub attribution: Vec<String>,
    pub pending: bool,
    pub errors: Vec<String>,
}

/// How the tiles crate sees a tile layer.
pub fn spec(t: &TileLayer) -> Spec {
    Spec {
        kind: match t.kind {
            TileKind::Raster => Kind::Raster,
            TileKind::Vector => Kind::Vector,
            TileKind::Style => Kind::Style,
            TileKind::PmTiles => Kind::PmTiles,
            TileKind::MbTiles => Kind::MbTiles,
            TileKind::Cog => Kind::Cog,
        },
        url: t.url.clone(),
        subdomains: t.subdomains.clone(),
        min_zoom: t.min_zoom,
        max_zoom: t.max_zoom,
        tile_size: t.tile_size,
        tms: t.tms,
        attribution: t.attribution.clone(),
        layers: t.layers.clone(),
        style_layers: t.style_layers.clone(),
    }
}

/// Where a MapLibre style layer's root properties place the map; `Ok(None)` while the style is
/// still loading (asking starts loading it). Other tile layers have no view of their own.
pub fn style_view(t: &TileLayer) -> Result<Option<rustgis_tiles::style::StyleView>, String> {
    if t.kind != TileKind::Style {
        return Ok(Some(Default::default()));
    }
    match rustgis_tiles::source::resolve(&spec(t)) {
        rustgis_tiles::source::Resolve::Ready(r) => Ok(Some(r.style.view)),
        rustgis_tiles::source::Resolve::Pending => Ok(None),
        rustgis_tiles::source::Resolve::Failed(e) => Err(e),
    }
}

/// Draws the map's visible tile layers, bottom first.
pub(crate) fn draw(pm: &mut Pixmap, map: &Map, view: &View, place: Place, labels: bool) -> TileOutput {
    let mut out = TileOutput::default();
    let layers: Vec<&TileLayer> = map.tiles.iter().rev().filter(|t| t.visible && t.opacity > 0.0).collect();
    if layers.is_empty() {
        return out;
    }
    let ppp = view.pixels_per_point as f32;
    let collect = |d: rustgis_tiles::Drawn, name: &str, out: &mut TileOutput| {
        out.pending |= d.pending;
        for a in d.attribution {
            if !out.attribution.contains(&a) {
                out.attribution.push(a);
            }
        }
        if let Some(e) = d.error {
            out.errors.push(format!("{name}: {e}"));
        }
        for l in d.labels {
            // Styles often label in two scripts ("Syria\nسوريا"): only the lines the font can
            // draw are kept, so a name never shows as boxes, and a label left with neither text
            // nor an icon is left out.
            let lines: Vec<&str> = l.text.lines().map(str::trim).filter(|s| !s.is_empty() && crate::text::covers(s, l.bold)).collect();
            if lines.is_empty() && l.icon.is_none() {
                continue;
            }
            let text = lines.join(" ");
            // The text sits against the point as the style anchors and offsets it (beside its
            // icon, for instance); without either it is centred on the point. Across, it is
            // placed by the edge it is anchored at, so each renderer lines it up with its own
            // measure of the width.
            let (th, em) = (l.size * 1.25, l.size * 1.33);
            let x = l.x + l.offset[0] * em;
            let y = l.y + l.anchor[1] * th + l.offset[1] * em;
            let icon = l.icon.map(|i| PlacedIcon { image: i.image, x: l.x + i.dx, y: l.y + i.dy, w: i.width, h: i.height });
            out.labels.push((
                PlacedLabel {
                    text,
                    x,
                    y,
                    size: l.size,
                    color: Rgba(l.color),
                    halo: l.halo.map(Rgba),
                    bold: l.bold,
                    icon,
                    align: -l.anchor[0],
                    framed: false,
                },
                l.priority,
            ));
        }
    };
    if let Place::Flat(Flat::Mercator, _) = place {
        let res = view.camera.res;
        let frame = Frame {
            left: view.camera.center.x - view.width / 2.0 * res,
            top: view.camera.center.y + view.height / 2.0 * res,
            m_per_px: res / view.pixels_per_point.max(0.01),
            ppp,
        };
        let mut scratch = None;
        for t in layers {
            let d = draw_layer(pm, t, &frame, labels, &mut scratch);
            collect(d, &t.name, &mut out);
        }
        return out;
    }
    // Elsewhere: a mosaic in Web Mercator, warped into place.
    let (w, h) = (pm.width(), pm.height());
    let k = view.pixels_per_point.max(0.01);
    let to_lonlat = |dx: f64, dy: f64| -> Option<Coord> {
        let (sx, sy) = (dx / k, dy / k);
        let c = match place {
            Place::Globe(g) => g.inverse(sx, sy)?,
            Place::Flat(flat, _) => flat.inverse(view.camera.to_map(sx, sy, view.width, view.height))?,
        };
        (c.is_finite() && c.y.abs() <= 90.0).then_some(c)
    };
    // Longitudes are unwrapped around the centre's, so a view across the antimeridian is one
    // stretch of an unwrapped world (the tiles past 180° are world copies) rather than two ends
    // of it with the strip between the last points sampled on either side left out.
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    let lon0 = to_lonlat(cx, cy).map_or(0.0, |c| c.x);
    let to_merc = |c: Coord| {
        let lon = c.x + 360.0 * ((lon0 - c.x) / 360.0).round();
        lonlat_to_merc(Coord::new(lon, c.y.clamp(-MAX_LAT, MAX_LAT)))
    };
    // The area in view, from a grid of points, and the detail needed at its centre.
    let n = 24;
    let grid: Vec<Vec<Option<Coord>>> = (0..=n)
        .map(|j| {
            (0..=n)
                .map(|i| to_lonlat(f64::from(w) * f64::from(i) / f64::from(n), f64::from(h) * f64::from(j) / f64::from(n)).map(to_merc))
                .collect()
        })
        .collect();
    let mut e = Extent::EMPTY;
    // The most a grid cell spans: the area in view reaches up to that past its outermost points
    // (the limb, the Web Mercator latitude limit, the meridian behind a pole).
    let (mut step_x, mut step_y) = (0.0f64, 0.0f64);
    let mut step = |a: Option<&Option<Coord>>, b: &Coord| {
        if let Some(Some(a)) = a {
            step_x = step_x.max((a.x - b.x).abs());
            step_y = step_y.max((a.y - b.y).abs());
        }
    };
    for (j, row) in grid.iter().enumerate() {
        for (i, c) in row.iter().enumerate() {
            let Some(c) = c else { continue };
            e.expand(*c);
            step(row.get(i + 1), c);
            step(grid.get(j + 1).and_then(|r| r.get(i)), c);
        }
    }
    if e.is_empty() {
        return out;
    }
    let x0 = lonlat_to_merc(Coord::new(lon0, 0.0)).x;
    // A pole in sight brings the latitudes up to it into view, whether or not the grid reached
    // them, and every longitude when it is in sight on the far side too (on the globe a pole is
    // a point; a world projection draws it as a line, which can run out of the view).
    let (vw, vh) = (f64::from(w) / k, f64::from(h) / k);
    let in_view = |lon: f64, lat: f64| {
        let pole = Coord::new(lon, lat);
        let at = match place {
            Place::Globe(g) => g.forward(pole),
            Place::Flat(flat, _) => flat.forward_checked(pole).map(|m| view.camera.to_screen(m, view.width, view.height)),
        };
        at.is_some_and(|(x, y)| x >= 0.0 && y >= 0.0 && x <= vw && y <= vh)
    };
    let (north, south) = (in_view(lon0, 90.0), in_view(lon0, -90.0));
    let all_lons = (north && in_view(lon0 + 180.0, 90.0)) || (south && in_view(lon0 + 180.0, -90.0));
    let min_y = if south { -HALF } else { (e.min_y - step_y).max(-HALF) };
    let max_y = if north { HALF } else { (e.max_y + step_y).min(HALF) };
    let e = if all_lons || e.width() > HALF {
        // Over HALF the world across: every longitude.
        Extent::new(x0 - HALF, min_y, x0 + HALF, max_y)
    } else {
        Extent::new((e.min_x - step_x).max(x0 - HALF), min_y, (e.max_x + step_x).min(x0 + HALF), max_y)
    };
    let detail = match (to_lonlat(cx, cy), to_lonlat(cx + 4.0, cy)) {
        (Some(a), Some(b)) => (to_merc(a).x - to_merc(b).x).hypot(to_merc(a).y - to_merc(b).y) / 4.0,
        _ => 0.0,
    };
    let m_per_px = detail.max(e.width() / MOSAIC).max(e.height() / MOSAIC).max(0.01);
    let (mw, mh) = ((e.width() / m_per_px).ceil().clamp(1.0, MOSAIC) as u32, (e.height() / m_per_px).ceil().clamp(1.0, MOSAIC) as u32);
    let Some(mut mosaic) = Pixmap::new(mw, mh) else { return out };
    // Pixels per point in the mosaic: line widths and the style's zoom then match the view's
    // centre once the mosaic is warped there.
    let mosaic_ppp = if detail > 0.0 { (detail * k / m_per_px) as f32 } else { ppp };
    let frame = Frame { left: e.min_x, top: e.max_y, m_per_px, ppp: mosaic_ppp.clamp(0.05, 8.0) };
    let mut scratch = None;
    for t in layers {
        let d = draw_layer(&mut mosaic, t, &frame, false, &mut scratch);
        collect(d, &t.name, &mut out);
    }
    warp(pm, &mosaic, &frame, &to_lonlat, &to_merc);
    out
}

/// Draws one tile layer into `pm`: straight in, or (blended) on a layer of its own composited
/// with its blend mode over what's beneath. `scratch` holds that layer between calls, so the
/// blended layers of one draw share one allocation.
fn draw_layer(pm: &mut Pixmap, t: &TileLayer, frame: &Frame, labels: bool, scratch: &mut Option<Pixmap>) -> rustgis_tiles::Drawn {
    if t.blend == Blend::Normal {
        return rustgis_tiles::draw(pm, &spec(t), t.opacity, frame, labels);
    }
    let fits = scratch.as_ref().is_some_and(|s| s.width() == pm.width() && s.height() == pm.height());
    if fits {
        if let Some(s) = scratch.as_mut() {
            s.fill(tiny_skia::Color::TRANSPARENT);
        }
    } else {
        *scratch = Pixmap::new(pm.width(), pm.height());
    }
    let Some(own) = scratch.as_mut() else {
        log::debug!("no room to blend tile layer '{}' ({}x{}): drawing it unblended", t.name, pm.width(), pm.height());
        return rustgis_tiles::draw(pm, &spec(t), t.opacity, frame, labels);
    };
    let d = rustgis_tiles::draw(own, &spec(t), t.opacity, frame, labels);
    let paint = PixmapPaint { blend_mode: crate::blend_mode(t.blend), ..PixmapPaint::default() };
    pm.draw_pixmap(0, 0, own.as_ref(), &paint, Transform::identity(), None);
    d
}

/// Half the width (and height) of the Web Mercator world, metres.
const HALF: f64 = std::f64::consts::PI * rustgis_geom::MERCATOR_R;

/// Draws the mosaic over `pm`: each pixel takes the mosaic pixel under its longitude/latitude.
fn warp(pm: &mut Pixmap, mosaic: &Pixmap, frame: &Frame, to_lonlat: &dyn Fn(f64, f64) -> Option<Coord>, to_merc: &dyn Fn(Coord) -> Coord) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let (mw, mh) = (mosaic.width() as usize, mosaic.height() as usize);
    // The latitudes past the Web Mercator limit, clamped to it, land a hair outside the mosaic
    // (the poles' caps), and its last row can hang past the world's south edge, where no tile
    // draws: the row above it is the edge then.
    let (fw, fh) = (mw as f64, (mh as f64).min(((frame.top + HALF) / frame.m_per_px).floor().max(1.0)));
    let src = mosaic.data();
    let dst = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let Some(c) = to_lonlat(x as f64 + 0.5, y as f64 + 0.5) else { continue };
            let m = to_merc(c);
            let (mx, my) = ((m.x - frame.left) / frame.m_per_px, (frame.top - m.y) / frame.m_per_px);
            // Within a pixel of the mosaic's edge takes the edge (`fw`, `fh`).
            if !(mx > -1.0 && my > -1.0 && mx < fw + 1.0 && my < fh + 1.0) {
                continue;
            }
            let (mx, my) = (mx.max(0.0).min(fw - 1.0) as usize, my.max(0.0).min(fh - 1.0) as usize);
            let si = (my * mw + mx) * 4;
            let di = (y * w + x) * 4;
            let (Some(s), Some(d)) = (src.get(si..si + 4), dst.get_mut(di..di + 4)) else { continue };
            // Source-over, premultiplied.
            let a = u32::from(s[3]);
            if a == 0 {
                continue;
            }
            for i in 0..4 {
                d[i] = (u32::from(s[i]) + u32::from(d[i]) * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rustgis_geom::{Coord, lonlat_to_merc};
    use rustgis_map::{Camera, Map, Projection, TileKind, TileLayer};

    use crate::View;

    /// Serves tiles and documents for `https://tiles.test/…` without a network.
    fn serve() {
        rustgis_tiles::cache::set_fetcher(|url, done| {
            if url.starts_with("https://tiles.test/red/") {
                let mut pm = tiny_skia::Pixmap::new(256, 256).unwrap();
                pm.fill(tiny_skia::Color::from_rgba8(220, 20, 20, 255));
                done(Ok(pm.encode_png().unwrap()));
            } else if url == "https://tiles.test/style.json" {
                let style = r##"{"version": 8, "sources": {}, "layers": [{"id": "bg", "type": "background", "paint": {"background-color": "#2050e0"}}]}"##;
                done(Ok(style.as_bytes().to_vec()));
            } else {
                done(Err("404".into()));
            }
        });
    }

    fn pixel(map: &mut Map) -> [u8; 4] {
        let camera = Camera { center: lonlat_to_merc(Coord::new(10.0, 50.0)), res: 2000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 200.0, height: 100.0, pixels_per_point: 1.0, camera, sky: false };
        let png = crate::render_png(map, &view).unwrap();
        let pm = tiny_skia::Pixmap::decode_png(&png).unwrap();
        let p = pm.pixel(100, 50).unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    #[test]
    fn draws_raster_tiles_and_styles() {
        serve();
        let mut m = Map::new("Map");
        m.graticule = false;
        let mut t = TileLayer::new("Red", TileKind::Raster, "https://tiles.test/red/{z}/{x}/{y}.png");
        t.attribution = "© Test".into();
        m.tiles.push(t);
        assert_eq!(pixel(&mut m), [220, 20, 20, 255]);
        // Half transparent over the basemap.
        m.tiles[0].opacity = 0.5;
        let p = pixel(&mut m);
        assert!(p[0] > 200 && (100..180).contains(&p[1]), "{p:?}");
        // A style on top, and the globe.
        m.tiles.insert(0, TileLayer::new("Blue", TileKind::Style, "https://tiles.test/style.json"));
        assert_eq!(pixel(&mut m), [0x20, 0x50, 0xE0, 255]);
        m.projection = Projection::Globe;
        let p = pixel(&mut m);
        // Shaded a little on the globe.
        assert!(p[2] > 150 && p[0] < 90, "{p:?}");
        // A layer that can't load draws nothing and says why.
        m.projection = Projection::WebMercator;
        m.tiles = vec![TileLayer::new("Gone", TileKind::Style, "https://tiles.test/missing.json")];
        let camera = Camera { center: lonlat_to_merc(Coord::new(10.0, 50.0)), res: 2000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 200.0, height: 100.0, pixels_per_point: 1.0, camera, sky: false };
        let r = crate::render_complete(&mut m, &view).unwrap();
        assert!(!r.tiles_pending);
        assert_eq!(r.tile_errors.len(), 1, "{:?}", r.tile_errors);
    }

    /// A view across the antimeridian on the globe draws the tiles on both sides of it: the
    /// mosaic once ran from the westernmost point sampled east of 180° to the easternmost west
    /// of it, and left out the strip between 180° and the nearest sample (#223).
    #[test]
    fn the_globe_draws_tiles_across_the_antimeridian() {
        serve();
        let mut m = Map::new("Map");
        m.graticule = false;
        m.projection = Projection::Globe;
        m.tiles.push(TileLayer::new("Red", TileKind::Raster, "https://tiles.test/red/{z}/{x}/{y}.png"));
        for lon in [175.0, 179.5, -178.0] {
            let camera = Camera { center: lonlat_to_merc(Coord::new(lon, 0.0)), res: 20_000.0, bearing: 0.0, pitch: 0.0 };
            let view = View { width: 200.0, height: 200.0, pixels_per_point: 1.0, camera, sky: false };
            let pm = tiny_skia::Pixmap::decode_png(&crate::render_png(&mut m, &view).unwrap()).unwrap();
            // The middle of the view, well inside the globe's edge (5° is 28 pixels here).
            for y in (60..140).step_by(10) {
                for x in 40..160 {
                    let p = pm.pixel(x, y).unwrap();
                    assert!(p.red() > 120 && p.green() < 80 && p.blue() < 80, "centred on {lon}: ({x}, {y}) is {p:?}");
                }
            }
        }
    }

    /// With a pole in sight the globe's mosaic is the whole world wide, too many tiles at the
    /// zoom of the view's centre: the coarser zoom that fits is drawn, where the tiles farthest
    /// from the mosaic's centre (the far meridian, the polar rows) were left out (#223).
    #[test]
    fn the_globe_draws_tiles_around_a_pole() {
        serve();
        let mut m = Map::new("Map");
        m.graticule = false;
        m.projection = Projection::Globe;
        m.tiles.push(TileLayer::new("Red", TileKind::Raster, "https://tiles.test/red/{z}/{x}/{y}.png"));
        let camera = Camera { center: lonlat_to_merc(Coord::new(0.0, 80.0)), res: 40_000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 600.0, height: 600.0, pixels_per_point: 2.0, camera, sky: false };
        let pm = tiny_skia::Pixmap::decode_png(&crate::render_png(&mut m, &view).unwrap()).unwrap();
        // The pole and the meridian beyond it, up the middle of the view.
        for y in (20..700).step_by(20) {
            for x in (560..640).step_by(4) {
                let p = pm.pixel(x, y).unwrap();
                assert!(p.red() > 120 && p.green() < 80 && p.blue() < 80, "({x}, {y}) is {p:?}");
            }
        }
        // Around the south pole, and a small globe whose grid of points misses the pole near
        // its edge.
        for (lon, lat, res, pole) in [(-110.0, -55.0, 40_000.0, -89.9), (120.0, 20.0, 150_000.0, 89.9), (120.0, 20.0, 150_000.0, 86.0)] {
            let camera = Camera { center: lonlat_to_merc(Coord::new(lon, lat)), res, bearing: 0.0, pitch: 0.0 };
            let view = View { width: 600.0, height: 600.0, pixels_per_point: 2.0, camera, sky: false };
            let pm = tiny_skia::Pixmap::decode_png(&crate::render_png(&mut m, &view).unwrap()).unwrap();
            let g = rustgis_map::globe::Globe::new(&camera, 600.0, 600.0);
            let (x, y) = g.forward(Coord::new(lon, pole)).unwrap();
            let p = pm.pixel((x * 2.0) as u32, (y * 2.0) as u32).unwrap();
            assert!(p.red() > 120 && p.green() < 80 && p.blue() < 80, "({lon}, {lat}): {pole}° is {p:?}");
        }
    }

    #[test]
    fn blends_tile_layers_with_those_beneath() {
        serve();
        let mut m = Map::new("Map");
        m.graticule = false;
        m.tiles.push(TileLayer::new("Red", TileKind::Raster, "https://tiles.test/red/{z}/{x}/{y}.png"));
        let mut blue = TileLayer::new("Blue", TileKind::Style, "https://tiles.test/style.json");
        blue.blend = rustgis_map::effects::Blend::Multiply;
        m.tiles.insert(0, blue);
        // Multiplied: red × blue, not the blue on top.
        let p = pixel(&mut m);
        assert!(p[0] < 40 && p[1] < 15 && p[2] < 30 && p[3] == 255, "{p:?}");
        // Screened: lighter than either.
        m.tiles[0].blend = rustgis_map::effects::Blend::Screen;
        let p = pixel(&mut m);
        assert!(p[0] > 220 && p[1] > 80 && p[2] > 0xE0, "{p:?}");
        // And in the mosaic warped onto another projection (the globe shades it a little).
        m.tiles[0].blend = rustgis_map::effects::Blend::Multiply;
        m.projection = Projection::Globe;
        let p = pixel(&mut m);
        assert!(p[0] < 60 && p[1] < 40 && p[2] < 60, "{p:?}");
    }
}
