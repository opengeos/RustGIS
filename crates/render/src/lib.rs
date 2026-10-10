//! L3: the CPU map renderer.
//!
//! [`render`] draws a [`Map`] (basemap, graticule, layers, selection) into a premultiplied RGBA
//! `tiny_skia::Pixmap` and places labels without collisions. The desktop app shows the pixmap
//! as a texture and draws the placed labels with its own text engine; [`render_png`] rasterises
//! the labels too, for export and the command line.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod fonts;
mod text;

use std::sync::OnceLock;

use rustgis_data::{Dataset, Geometry, GeometryType};
use rustgis_geom::{Coord, Extent, lonlat_to_merc};
use rustgis_map::{Camera, Map, Marker, Rgba, Symbol};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

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
    W.get_or_init(|| {
        let text = include_str!("../../../assets/data/countries.geojson");
        match rustgis_data::io::read_geojson(text, "world") {
            Ok((ds, _)) => ds.features.into_iter().filter_map(|f| f.geometry).map(|g| g.map_coords(&lonlat_to_merc)).collect(),
            Err(e) => {
                log::warn!("built-in world outline unreadable: {e}");
                Vec::new()
            }
        }
    })
}

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
}

impl Ctx<'_> {
    fn px(&self, c: Coord) -> (f32, f32) {
        let (x, y) = self.view.to_screen(c);
        ((x * self.view.pixels_per_point) as f32, (y * self.view.pixels_per_point) as f32)
    }

    fn ring_path(&self, pb: &mut PathBuilder, ring: &[Coord], close: bool) {
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

/// Draws the map. Layers are drawn bottom-up (the last layer in the list first).
pub fn render(map: &mut Map, view: &View) -> Option<Rendered> {
    let (w, h) = view.pixel_size();
    let mut pm = Pixmap::new(w, h)?;
    let ctx = Ctx { view, ppp: view.pixels_per_point as f32, cull: view.camera.extent(view.width, view.height).padded(0.05, 0.0) };
    let scale = view.camera.scale();

    // Basemap.
    let (water, land, outline, grat) = map.basemap.palette();
    pm.fill(tiny_skia::Color::from_rgba8(water.r(), water.g(), water.b(), 255));
    if land != water {
        let fill = paint(land, 1.0);
        let stroke = (outline.a() > 0).then(|| (paint(outline, 1.0), 0.6));
        for g in world() {
            if let Geometry::Polygon(polys) = g
                && g.extent().intersects(&ctx.cull)
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
        let mut lines = Vec::new();
        let mut lon = -180.0;
        while lon <= 180.0 {
            lines.push((0..=34).map(|i| lonlat_to_merc(Coord::new(lon, -85.0 + i as f64 * 5.0))).collect::<Vec<_>>());
            lon += step;
        }
        let mut lat = -90.0 + step;
        while lat < 90.0 {
            lines.push((0..=72).map(|i| lonlat_to_merc(Coord::new(-180.0 + i as f64 * 5.0, lat))).collect::<Vec<_>>());
            lat += step;
        }
        ctx.line(&mut pm, &lines, &p, 0.6);
    }

    // Layers, bottom first.
    let mut label_candidates: Vec<(PlacedLabel, f32)> = Vec::new();
    for li in (0..map.layers.len()).rev() {
        let Some(layer) = map.layers.get_mut(li) else { continue };
        if !layer.visible || !layer.in_scale_range(scale) {
            continue;
        }
        let rows = layer.visible_rows();
        let opacity = layer.opacity();
        let renderer = layer.renderer.clone();
        let labels = layer.labels.clone();
        let selection: Vec<usize> = layer.selection.clone();
        let ds_ptr = layer.dataset.clone();
        let geoms = layer.projected().to_vec();
        let mut target = if opacity < 0.999 { Pixmap::new(w, h)? } else { Pixmap::new(1, 1)? };
        let direct = opacity >= 0.999;
        {
            let dest = if direct { &mut pm } else { &mut target };
            for (row, g) in geoms.iter().enumerate() {
                let Some(g) = g else { continue };
                if rows.get(row) != Some(&true) || !g.extent().intersects(&ctx.cull) {
                    continue;
                }
                ctx.feature(dest, g, &renderer.symbol_for(&ds_ptr, row));
            }
        }
        if !direct {
            pm.draw_pixmap(0, 0, target.as_ref(), &PixmapPaint { opacity, ..PixmapPaint::default() }, Transform::identity(), None);
        }
        for row in &selection {
            if let Some(Some(g)) = geoms.get(*row)
                && rows.get(*row) == Some(&true)
            {
                ctx.highlight(&mut pm, g, &renderer.symbol_for(&ds_ptr, *row));
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
                if !ctx.cull.contains(c) {
                    continue;
                }
                let (x, y) = view.to_screen(c);
                let dy = if g.kind() == GeometryType::Point {
                    -(renderer.symbol_for(&ds_ptr, row).size / 2.0 + labels.size * 0.75)
                } else {
                    0.0
                };
                // Bigger polygons get priority.
                let pri = match g {
                    Geometry::Polygon(_) => {
                        let e = g.extent();
                        (e.width() * e.height()).sqrt() as f32
                    }
                    _ => 0.0,
                } + (map.layers.len() - li) as f32 * 1e9;
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
    let labels = place_labels(label_candidates, view.width as f32, view.height as f32);
    Some(Rendered { pixmap: pm, labels })
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
    let mut r = render(map, view).ok_or("could not allocate the image")?;
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
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera };
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
            let view = View { width: w, height: h, pixels_per_point: 1.0, camera: Camera::world() };
            let _ = render(&mut m, &view);
        }
    }
}
