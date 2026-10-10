//! L3: the CPU map renderer.
//!
//! [`render`] draws a [`Map`] (basemap, graticule, layers, selection) into a premultiplied RGBA
//! `tiny_skia::Pixmap` and places labels without collisions. The desktop app shows the pixmap
//! as a texture and draws the placed labels with its own text engine; [`render_png`] rasterises
//! the labels too, for export and the command line.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod fonts;
pub mod logo;
mod raster;
mod text;

/// Natural Earth 1:110m sample data (public domain), fetched by `build.rs` from rustgis-assets.
pub mod sample_data {
    pub const COUNTRIES: &str = include_str!(concat!(env!("OUT_DIR"), "/countries.geojson"));
    pub const CITIES: &str = include_str!(concat!(env!("OUT_DIR"), "/cities.geojson"));
    pub const RIVERS: &str = include_str!(concat!(env!("OUT_DIR"), "/rivers.geojson"));
    pub const LAKES: &str = include_str!(concat!(env!("OUT_DIR"), "/lakes.geojson"));
}

use std::sync::OnceLock;

use rustgis_data::{Dataset, Geometry, GeometryType};
use rustgis_geom::{Coord, Extent, lonlat_to_merc};
use rustgis_map::globe::{V3, densify};
use rustgis_map::{Camera, Globe, Map, Marker, Projection, Rgba, Symbol};
use tiny_skia::{
    FillRule, GradientStop, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, SpreadMode, Stroke,
    Transform,
};

/// Selection highlight (cyan, as desktop GIS users expect).
pub const SELECTION: Rgba = Rgba::rgb(0x00, 0xE5, 0xFF);

/// Largest raster rendered (pixels per side).
pub const MAX_SIDE: u32 = 8192;

/// What to draw and at which size.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Size in logical points.
    pub width: f64,
    pub height: f64,
    /// Physical pixels per logical point.
    pub pixels_per_point: f64,
    pub camera: Camera,
    /// The interface is dark: the space around the globe is dark too.
    pub dark: bool,
}

impl View {
    pub fn pixel_size(&self) -> (u32, u32) {
        let w = (self.width * self.pixels_per_point).round().clamp(1.0, MAX_SIDE as f64) as u32;
        let h = (self.height * self.pixels_per_point).round().clamp(1.0, MAX_SIDE as f64) as u32;
        (w, h)
    }
    /// Logical screen position of a Web Mercator point.
    pub fn to_screen(&self, p: Coord) -> (f64, f64) {
        self.camera.to_screen(p, self.width, self.height)
    }
}

/// A placed label in logical points (centre of the text).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedLabel {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: Rgba,
    pub halo: Option<Rgba>,
    pub bold: bool,
}

pub struct Rendered {
    pub pixmap: Pixmap,
    pub labels: Vec<PlacedLabel>,
}

/// The built-in world outline used by every basemap (Natural Earth 1:110m, public domain).
fn world() -> &'static Vec<Geometry> {
    static W: OnceLock<Vec<Geometry>> = OnceLock::new();
    W.get_or_init(|| match rustgis_data::io::read_geojson(sample_data::COUNTRIES, "world") {
        Ok((ds, _)) => ds.features.into_iter().filter_map(|f| f.geometry).map(|g| g.map_coords(&lonlat_to_merc)).collect(),
        Err(e) => {
            log::warn!("built-in world outline unreadable: {e}");
            Vec::new()
        }
    })
}

/// The built-in world outline in longitude/latitude, for the globe.
fn world_lonlat() -> &'static Vec<Geometry> {
    static W: OnceLock<Vec<Geometry>> = OnceLock::new();
    W.get_or_init(|| world().iter().map(|g| g.map_coords(&rustgis_geom::merc_to_lonlat)).collect())
}

/// Largest angle between globe vertices before edges are bent along great circles (2°).
const GLOBE_STEP: f64 = 0.035;

/// The built-in world dataset (lon/lat), for the sample project.
pub fn world_dataset(name: &str, text: &str) -> Option<Dataset> {
    rustgis_data::io::read_geojson(text, name).ok().map(|(d, _)| d)
}

fn paint(c: Rgba, opacity: f32) -> Paint<'static> {
    let mut p = Paint::default();
    let a = (c.a() as f32 * opacity).round().clamp(0.0, 255.0) as u8;
    p.set_color_rgba8(c.r(), c.g(), c.b(), a);
    p.anti_alias = true;
    p
}

struct Ctx<'a> {
    view: &'a View,
    ppp: f32,
    /// Visible Mercator extent with a margin, for culling.
    cull: Extent,
    /// Set when drawing the globe; coordinates are then longitude/latitude, not Web Mercator.
    globe: Option<Globe>,
}

impl Ctx<'_> {
    /// Logical screen position of a coordinate, `None` when it is behind the globe.
    fn screen(&self, c: Coord) -> Option<(f64, f64)> {
        match &self.globe {
            Some(g) => g.forward(c),
            None => Some(self.view.to_screen(c)),
        }
    }

    fn px(&self, c: Coord) -> (f32, f32) {
        match self.screen(c) {
            Some((x, y)) => ((x * self.view.pixels_per_point) as f32, (y * self.view.pixels_per_point) as f32),
            None => (f32::NAN, f32::NAN),
        }
    }

    fn vpx(&self, g: &Globe, v: V3) -> (f32, f32) {
        let (x, y) = g.project(v);
        ((x * self.view.pixels_per_point) as f32, (y * self.view.pixels_per_point) as f32)
    }

    /// View vectors of a ring or line, with long edges bent along great circles.
    fn globe_vectors(g: &Globe, pts: &[Coord]) -> Vec<V3> {
        let mut out: Vec<V3> = Vec::with_capacity(pts.len());
        for c in pts {
            if !c.is_finite() {
                continue;
            }
            let v = g.to_view(*c);
            match out.last().copied() {
                Some(prev) => out.extend(densify(prev, v, GLOBE_STEP)),
                None => out.push(v),
            }
        }
        out
    }

    /// A polygon ring on the globe. Hidden vertices are moved radially onto the horizon circle at
    /// their own bearing, so the hidden part of the ring runs along the limb in its true order and
    /// encloses no area; a ring that is wholly hidden is skipped.
    fn globe_ring(&self, g: &Globe, pb: &mut PathBuilder, ring: &[Coord]) {
        let vs = Self::globe_vectors(g, ring);
        let h = g.horizon();
        if vs.len() < 3 || vs.iter().all(|v| v[2] <= h) {
            return;
        }
        let r = (1.0 - h * h).sqrt();
        let limb = |v: V3| {
            let a = v[1].atan2(v[0]);
            [r * a.cos(), r * a.sin(), h]
        };
        let mut out: Vec<V3> = Vec::with_capacity(vs.len() + 8);
        let push_limb = |out: &mut Vec<V3>, p: V3| {
            // Follow the limb between hidden points that are far apart in bearing.
            if let Some(last) = out.last().copied()
                && last[2] <= h + 1e-9
            {
                out.extend(g.horizon_arc(last, p));
            }
            out.push(p);
        };
        let mut prev: Option<V3> = None;
        for v in vs {
            let vis = v[2] > h;
            if let Some(p) = prev
                && (p[2] > h) != vis
            {
                push_limb(&mut out, g.horizon_crossing(p, v));
            }
            if vis {
                out.push(v)
            } else {
                push_limb(&mut out, limb(v))
            }
            prev = Some(v);
        }
        // A ring that ends and starts behind the horizon closes along the limb, not across it.
        if let (Some(first), Some(last)) = (out.first().copied(), out.last().copied())
            && first[2] <= h + 1e-9
            && last[2] <= h + 1e-9
        {
            out.extend(g.horizon_arc(last, first));
        }
        for (i, v) in out.iter().enumerate() {
            let (x, y) = self.vpx(g, *v);
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        if !out.is_empty() {
            pb.close();
        }
    }

    /// A line on the globe, broken where it goes behind the horizon.
    fn globe_line(&self, g: &Globe, pb: &mut PathBuilder, part: &[Coord]) {
        let vs = Self::globe_vectors(g, part);
        let h = g.horizon();
        let mut prev: Option<V3> = None;
        for v in vs {
            let vis = v[2] > h;
            match prev {
                Some(p) if p[2] > h && vis => {
                    let (x, y) = self.vpx(g, v);
                    pb.line_to(x, y);
                }
                Some(p) if p[2] > h => {
                    let (x, y) = self.vpx(g, g.horizon_crossing(p, v));
                    pb.line_to(x, y);
                }
                Some(p) if vis => {
                    let (x, y) = self.vpx(g, g.horizon_crossing(p, v));
                    pb.move_to(x, y);
                    let (x, y) = self.vpx(g, v);
                    pb.line_to(x, y);
                }
                None if vis => {
                    let (x, y) = self.vpx(g, v);
                    pb.move_to(x, y);
                }
                _ => {}
            }
            prev = Some(v);
        }
    }

    fn ring_path(&self, pb: &mut PathBuilder, ring: &[Coord], close: bool) {
        if let Some(g) = &self.globe {
            if close {
                self.globe_ring(g, pb, ring)
            } else {
                self.globe_line(g, pb, ring)
            }
            return;
        }
        // Drop vertices closer than ~0.35 px to the previous one: big speed-up when zoomed out.
        let mut last: Option<(f32, f32)> = None;
        let n = ring.len();
        for (i, c) in ring.iter().enumerate() {
            let (x, y) = self.px(*c);
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            match last {
                None => pb.move_to(x, y),
                Some((lx, ly)) => {
                    if (x - lx).abs() + (y - ly).abs() < 0.35 && i + 1 != n {
                        continue;
                    }
                    pb.line_to(x, y)
                }
            }
            last = Some((x, y));
        }
        if close && last.is_some() {
            pb.close();
        }
    }

    fn polygon(&self, pm: &mut Pixmap, polys: &[Vec<Vec<Coord>>], fill: Option<Paint>, stroke: Option<(Paint, f32)>) {
        let mut pb = PathBuilder::new();
        for rings in polys {
            for r in rings {
                self.ring_path(&mut pb, r, true);
            }
        }
        let Some(path) = pb.finish() else { return };
        if let Some(f) = fill {
            pm.fill_path(&path, &f, FillRule::EvenOdd, Transform::identity(), None);
        }
        if let Some((s, w)) = stroke
            && w > 0.0
        {
            let st = Stroke { width: w * self.ppp, line_join: LineJoin::Round, ..Stroke::default() };
            pm.stroke_path(&path, &s, &st, Transform::identity(), None);
        }
    }

    fn line(&self, pm: &mut Pixmap, parts: &[Vec<Coord>], p: &Paint, w: f32) {
        let mut pb = PathBuilder::new();
        for part in parts {
            self.ring_path(&mut pb, part, false);
        }
        let Some(path) = pb.finish() else { return };
        let st = Stroke { width: (w * self.ppp).max(0.5), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&path, p, &st, Transform::identity(), None);
    }

    fn marker(&self, pm: &mut Pixmap, c: Coord, s: &Symbol, opacity: f32, halo: Option<Rgba>) {
        let (x, y) = self.px(c);
        if !x.is_finite() || !y.is_finite() {
            return; // behind the globe
        }
        let r = (s.size.max(1.0) * self.ppp) / 2.0;
        let mut pb = PathBuilder::new();
        match s.marker {
            Marker::Circle => pb.push_circle(x, y, r),
            Marker::Square => {
                if let Some(rect) = tiny_skia::Rect::from_xywh(x - r * 0.9, y - r * 0.9, r * 1.8, r * 1.8) {
                    pb.push_rect(rect)
                }
            }
            Marker::Triangle => {
                pb.move_to(x, y - r * 1.1);
                pb.line_to(x + r, y + r * 0.75);
                pb.line_to(x - r, y + r * 0.75);
                pb.close();
            }
            Marker::Diamond => {
                pb.move_to(x, y - r * 1.15);
                pb.line_to(x + r * 0.9, y);
                pb.line_to(x, y + r * 1.15);
                pb.line_to(x - r * 0.9, y);
                pb.close();
            }
            Marker::Star => {
                for i in 0..10 {
                    let a = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
                    let rr = if i % 2 == 0 { r * 1.2 } else { r * 0.5 };
                    let (px, py) = (x + rr * a.cos(), y + rr * a.sin());
                    if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
                }
                pb.close();
            }
        }
        let Some(path) = pb.finish() else { return };
        if let Some(h) = halo {
            let st = Stroke { width: 3.5 * self.ppp, line_join: LineJoin::Round, ..Stroke::default() };
            pm.stroke_path(&path, &paint(h, 1.0), &st, Transform::identity(), None);
        }
        pm.fill_path(&path, &paint(s.fill, opacity), FillRule::Winding, Transform::identity(), None);
        if s.stroke_width > 0.0 && s.stroke.a() > 0 {
            let st = Stroke { width: s.stroke_width * self.ppp, line_join: LineJoin::Round, ..Stroke::default() };
            pm.stroke_path(&path, &paint(s.stroke, opacity), &st, Transform::identity(), None);
        }
    }

    fn feature(&self, pm: &mut Pixmap, g: &Geometry, s: &Symbol) {
        match g {
            Geometry::Point(c) => self.marker(pm, *c, s, 1.0, None),
            Geometry::MultiPoint(v) => v.iter().for_each(|c| self.marker(pm, *c, s, 1.0, None)),
            Geometry::Line(parts) => self.line(pm, parts, &paint(s.stroke, 1.0), s.stroke_width),
            Geometry::Polygon(polys) => self.polygon(pm, polys, Some(paint(s.fill, 1.0)), Some((paint(s.stroke, 1.0), s.stroke_width))),
        }
    }

    fn highlight(&self, pm: &mut Pixmap, g: &Geometry, s: &Symbol) {
        let cyan = paint(SELECTION, 1.0);
        match g {
            Geometry::Point(c) => self.marker(pm, *c, s, 1.0, Some(SELECTION)),
            Geometry::MultiPoint(v) => v.iter().for_each(|c| self.marker(pm, *c, s, 1.0, Some(SELECTION))),
            Geometry::Line(parts) => {
                self.line(pm, parts, &cyan, s.stroke_width + 3.0);
                self.line(pm, parts, &paint(s.stroke, 1.0), s.stroke_width);
            }
            Geometry::Polygon(polys) => self.polygon(pm, polys, Some(paint(SELECTION.with_alpha(40), 1.0)), Some((cyan, 2.0))),
        }
    }
}

/// Draws the map for the screen. Layers are drawn bottom-up (the last layer in the list first).
/// On high-density screens rasters are computed at logical-pixel resolution, for speed.
pub fn render(map: &mut Map, view: &View) -> Option<Rendered> {
    render_with(map, view, false)
}

fn render_with(map: &mut Map, view: &View, full_res_rasters: bool) -> Option<Rendered> {
    let (w, h) = view.pixel_size();
    let mut pm = Pixmap::new(w, h)?;
    let globe = (map.projection == Projection::Globe).then(|| Globe::new(&view.camera, view.width, view.height));
    let ctx = Ctx { view, ppp: view.pixels_per_point as f32, cull: view.camera.extent(view.width, view.height).padded(0.05, 0.0), globe };
    let scale = view.camera.scale();
    // On the globe nothing is culled by the flat extent: the horizon clips instead.
    let visible = |e: &Extent| globe.is_some() || e.intersects(&ctx.cull);

    // Basemap.
    let (water, land, outline, grat) = map.basemap.palette();
    match &globe {
        Some(g) => globe_backdrop(&mut pm, g, map.basemap, water, view.dark, view.pixels_per_point as f32),
        None => pm.fill(tiny_skia::Color::from_rgba8(water.r(), water.g(), water.b(), 255)),
    }
    if land != water {
        let fill = paint(land, 1.0);
        let stroke = (outline.a() > 0).then(|| (paint(outline, 1.0), 0.6));
        let outlines = if globe.is_some() { world_lonlat() } else { world() };
        for g in outlines {
            if let Geometry::Polygon(polys) = g
                && visible(&g.extent())
            {
                ctx.polygon(&mut pm, polys, Some(fill.clone()), stroke.clone());
            }
        }
    }
    if map.graticule && grat.a() > 0 {
        let p = paint(grat, 1.0);
        let step = if view.camera.res > 8000.0 {
            30.0
        } else if view.camera.res > 2000.0 {
            15.0
        } else if view.camera.res > 400.0 {
            5.0
        } else {
            1.0
        };
        let to = |c: Coord| if globe.is_some() { c } else { lonlat_to_merc(c) };
        let (lat_max, lat_min) = if globe.is_some() { (90.0, -90.0) } else { (85.0, -85.0) };
        let mut lines = Vec::new();
        let mut lon = -180.0;
        while lon <= 180.0 {
            lines.push((0..=36).map(|i| to(Coord::new(lon, lat_min + (lat_max - lat_min) * i as f64 / 36.0))).collect::<Vec<_>>());
            lon += step;
        }
        let mut lat = -90.0 + step;
        while lat < 90.0 {
            lines.push((0..=72).map(|i| to(Coord::new(-180.0 + i as f64 * 5.0, lat))).collect::<Vec<_>>());
            lat += step;
        }
        ctx.line(&mut pm, &lines, &p, 0.6);
    }

    // Layers, bottom first.
    let mut label_candidates: Vec<(PlacedLabel, f32)> = Vec::new();
    let n_layers = map.layers.len();
    for li in (0..n_layers).rev() {
        let Some(layer) = map.layers.get_mut(li) else { continue };
        if !layer.visible || !layer.in_scale_range(scale) {
            continue;
        }
        let opacity = layer.opacity();
        if let Some(rl) = &layer.raster {
            if let Some(data) = &rl.data {
                raster::draw(&mut pm, data, &rl.style, view, opacity, full_res_rasters, globe.as_ref());
            }
            continue;
        }
        let rows = layer.visible_rows();
        if globe.is_none() {
            layer.projected(); // refresh the Web Mercator cache
        }
        // Everything below only reads the layer: borrow rather than copy its data each frame.
        let layer: &rustgis_map::Layer = layer;
        let (renderer, labels, selection, ds_ptr) = (&layer.renderer, &layer.labels, &layer.selection, &layer.dataset);
        // The globe draws the lon/lat geometry itself; the flat map its Web Mercator copy.
        let geoms: Vec<Option<&Geometry>> = if globe.is_some() {
            ds_ptr.features.iter().map(|f| f.geometry.as_ref()).collect()
        } else {
            layer.projected_cache().iter().map(Option::as_ref).collect()
        };
        let mut target = if opacity < 0.999 { Pixmap::new(w, h)? } else { Pixmap::new(1, 1)? };
        let direct = opacity >= 0.999;
        {
            let dest = if direct { &mut pm } else { &mut target };
            for (row, g) in geoms.iter().enumerate() {
                let Some(g) = g else { continue };
                if rows.get(row) != Some(&true) || !visible(&g.extent()) {
                    continue;
                }
                ctx.feature(dest, g, &renderer.symbol_for(ds_ptr, row));
            }
        }
        if !direct {
            pm.draw_pixmap(0, 0, target.as_ref(), &PixmapPaint { opacity, ..PixmapPaint::default() }, Transform::identity(), None);
        }
        for row in selection {
            if let Some(Some(g)) = geoms.get(*row)
                && rows.get(*row) == Some(&true)
            {
                ctx.highlight(&mut pm, g, &renderer.symbol_for(ds_ptr, *row));
            }
        }
        if labels.enabled
            && let Some(fi) = ds_ptr.field_index(&labels.field)
        {
            for (row, g) in geoms.iter().enumerate() {
                let Some(g) = g else { continue };
                if rows.get(row) != Some(&true) {
                    continue;
                }
                let v = ds_ptr.value(row, fi);
                if v.is_null() {
                    continue;
                }
                let Some(c) = g.label_point() else { continue };
                if globe.is_none() && !ctx.cull.contains(c) {
                    continue;
                }
                let Some((x, y)) = ctx.screen(c) else { continue };
                let dy =
                    if g.kind() == GeometryType::Point { -(renderer.symbol_for(ds_ptr, row).size / 2.0 + labels.size * 0.75) } else { 0.0 };
                // Bigger polygons get priority.
                let pri = match g {
                    Geometry::Polygon(_) => {
                        let e = g.extent();
                        (e.width() * e.height()).sqrt() as f32
                    }
                    _ => 0.0,
                } + (n_layers - li) as f32 * 1e9;
                label_candidates.push((
                    PlacedLabel {
                        text: v.to_string(),
                        x: x as f32,
                        y: y as f32 + dy,
                        size: labels.size,
                        color: labels.color,
                        halo: labels.halo.then_some(halo_for(labels.color)),
                        bold: labels.bold,
                    },
                    pri,
                ));
            }
        }
    }
    if let Some(g) = &globe {
        globe_shading(&mut pm, g, map.basemap, water, view.dark, view.pixels_per_point as f32);
    }
    let labels = place_labels(label_candidates, view.width as f32, view.height as f32);
    Some(Rendered { pixmap: pm, labels })
}

/// The globe's backdrop and lighting depend only on the view size, zoom and basemap, not on the
/// rotation, so they are drawn once and reused while the globe turns.
struct GlobeLayers {
    key: (u32, u32, u32, u32, u32, u32, rustgis_map::Basemap, [u8; 4], bool),
    backdrop: Pixmap,
    shading: Pixmap,
}

thread_local! {
    static GLOBE_LAYERS: std::cell::RefCell<Option<GlobeLayers>> = const { std::cell::RefCell::new(None) };
}

fn with_globe_layers(
    pm: &mut Pixmap,
    g: &Globe,
    basemap: rustgis_map::Basemap,
    water: Rgba,
    dark: bool,
    ppp: f32,
    f: impl FnOnce(&mut Pixmap, &GlobeLayers),
) {
    let (cx, cy) = g.center();
    let key = (
        pm.width(),
        pm.height(),
        ppp.to_bits(),
        (cx as f32).to_bits(),
        (cy as f32).to_bits(),
        (g.disc_radius() as f32).to_bits(),
        basemap,
        water.0,
        dark,
    );
    GLOBE_LAYERS.with(|cell| {
        let mut cache = cell.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.key != key) {
            let (Some(mut backdrop), Some(mut shading)) = (Pixmap::new(pm.width(), pm.height()), Pixmap::new(pm.width(), pm.height()))
            else {
                return;
            };
            draw_globe_backdrop(&mut backdrop, g, basemap, water, dark, ppp);
            draw_globe_shading(&mut shading, g, ppp);
            *cache = Some(GlobeLayers { key, backdrop, shading });
        }
        if let Some(c) = cache.as_ref() {
            f(pm, c);
        }
    });
}

fn globe_backdrop(pm: &mut Pixmap, g: &Globe, basemap: rustgis_map::Basemap, water: Rgba, dark: bool, ppp: f32) {
    with_globe_layers(pm, g, basemap, water, dark, ppp, |pm, c| pm.data_mut().copy_from_slice(c.backdrop.data()));
}

fn globe_shading(pm: &mut Pixmap, g: &Globe, basemap: rustgis_map::Basemap, water: Rgba, dark: bool, ppp: f32) {
    with_globe_layers(pm, g, basemap, water, dark, ppp, |pm, c| {
        pm.draw_pixmap(0, 0, c.shading.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    });
}

/// The globe's surroundings and ocean: space (or a soft backdrop), an atmosphere glow, and the
/// ocean disc.
fn draw_globe_backdrop(pm: &mut Pixmap, g: &Globe, basemap: rustgis_map::Basemap, water: Rgba, dark_ui: bool, ppp: f32) {
    let dark = dark_ui || basemap == rustgis_map::Basemap::Midnight;
    let bg = if dark { Rgba::rgb(0x08, 0x0B, 0x10) } else { Rgba::rgb(0xEC, 0xEF, 0xF3) };
    pm.fill(tiny_skia::Color::from_rgba8(bg.r(), bg.g(), bg.b(), 255));
    let (cx, cy) = g.center();
    let (cx, cy, r) = (cx as f32 * ppp, cy as f32 * ppp, g.disc_radius() as f32 * ppp);
    if !(r.is_finite() && r > 0.5 && cx.is_finite() && cy.is_finite()) {
        return;
    }
    // Atmosphere: a glow fading out just beyond the limb.
    let glow_r = r * 1.07;
    let glow = if dark { (0x5A, 0x9B, 0xF0, 150) } else { (0x7F, 0xB4, 0xE8, 120) };
    if let Some(shader) = RadialGradient::new(
        Point::from_xy(cx, cy),
        0.0,
        Point::from_xy(cx, cy),
        glow_r,
        vec![
            GradientStop::new(0.0, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, glow.3)),
            GradientStop::new(r / glow_r, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, glow.3)),
            GradientStop::new(1.0, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, 0)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) && let Some(path) = PathBuilder::from_circle(cx, cy, glow_r)
    {
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        pm.fill_path(&path, &paint(water, 1.0), FillRule::Winding, Transform::identity(), None);
    }
}

/// Light from the upper left: a soft highlight and a darker limb, so the globe reads as a sphere.
fn draw_globe_shading(pm: &mut Pixmap, g: &Globe, ppp: f32) {
    let (cx, cy) = g.center();
    let (cx, cy, r) = (cx as f32 * ppp, cy as f32 * ppp, g.disc_radius() as f32 * ppp);
    if !(r.is_finite() && r > 0.5 && cx.is_finite() && cy.is_finite()) {
        return;
    }
    let light = Point::from_xy(cx - r * 0.35, cy - r * 0.4);
    if let Some(shader) = RadialGradient::new(
        light,
        0.0,
        Point::from_xy(cx, cy),
        r * 1.02,
        vec![
            GradientStop::new(0.0, tiny_skia::Color::from_rgba8(255, 255, 255, 46)),
            GradientStop::new(0.45, tiny_skia::Color::from_rgba8(255, 255, 255, 0)),
            GradientStop::new(0.8, tiny_skia::Color::from_rgba8(0, 0, 0, 18)),
            GradientStop::new(1.0, tiny_skia::Color::from_rgba8(0, 0, 0, 70)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) && let Some(path) = PathBuilder::from_circle(cx, cy, r)
    {
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
}

/// A halo that contrasts with the text colour.
fn halo_for(text: Rgba) -> Rgba {
    let lum = 0.299 * text.r() as f32 + 0.587 * text.g() as f32 + 0.114 * text.b() as f32;
    if lum < 140.0 { Rgba::new(255, 255, 255, 225) } else { Rgba::new(20, 22, 26, 210) }
}

/// Greedy collision-free placement, highest priority first.
fn place_labels(mut cands: Vec<(PlacedLabel, f32)>, w: f32, h: f32) -> Vec<PlacedLabel> {
    cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut boxes: Vec<[f32; 4]> = Vec::new();
    let mut out = Vec::new();
    for (l, _) in cands.into_iter().take(5000) {
        let tw = text::measure(&l.text, l.size, l.bold);
        let th = l.size * 1.25;
        let b = [l.x - tw / 2.0 - 2.0, l.y - th / 2.0, l.x + tw / 2.0 + 2.0, l.y + th / 2.0];
        if b[0] < 0.0 || b[1] < 0.0 || b[2] > w || b[3] > h {
            continue;
        }
        if boxes.iter().any(|o| b[0] < o[2] && o[0] < b[2] && b[1] < o[3] && o[1] < b[3]) {
            continue;
        }
        boxes.push(b);
        out.push(l);
    }
    out
}

/// Renders a map with its labels, a scale bar and a north arrow to PNG bytes.
pub fn render_png(map: &mut Map, view: &View) -> Result<Vec<u8>, String> {
    let mut r = render_with(map, view, true).ok_or("could not allocate the image")?;
    for l in &r.labels {
        text::draw(&mut r.pixmap, l, view.pixels_per_point as f32);
    }
    text::scale_bar(&mut r.pixmap, view);
    r.pixmap.encode_png().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::{Field, FieldType, Value};

    #[test]
    fn renders_world_and_layers() {
        let mut m = Map::new("Map");
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        d.push(Some(Geometry::Point(Coord::new(2.35, 48.85))), vec![Value::Text("Paris".into())]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.labels.enabled = true;
            l.selection = vec![0];
        }
        let camera = Camera { center: lonlat_to_merc(Coord::new(2.35, 48.85)), res: 5000.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, dark: false };
        let r = render(&mut m, &view).unwrap();
        assert_eq!(r.labels.len(), 1);
        assert_eq!(r.labels[0].text, "Paris");
        let png = render_png(&mut m, &view).unwrap();
        assert!(png.len() > 1000);
        assert!(!world().is_empty());
    }

    #[test]
    fn degenerate_views_do_not_panic() {
        let mut m = Map::new("Map");
        for (w, h) in [(0.0, 0.0), (1e9, 1e9), (f64::NAN, 3.0)] {
            let view = View { width: w, height: h, pixels_per_point: 1.0, camera: Camera::world(), dark: false };
            let _ = render(&mut m, &view);
        }
    }
}
