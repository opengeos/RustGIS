//! Drawing a tile layer into a pixmap in Web Mercator.
//!
//! The style's layers draw in order; each one draws from every tile of its source in view.
//! A tile that hasn't arrived yet is stood in for by the nearest ancestor already in the cache
//! (or, for raster tiles, its children), clipped to the missing tile's square.

use std::collections::HashMap;
use std::sync::Arc;

use rustgis_geom::Extent;
use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Paint, PathBuilder, Pattern, Pixmap, PixmapPaint, Rect, SpreadMode, Stroke, StrokeDash,
    Transform,
};

use crate::cache::{self, Payload, State, Want};
use crate::mvt::GeomKind;
use crate::source::{Resolve, Spec, resolve};
use crate::sprite::{self, Load, Sprite};
use crate::style::{FeatureCtx, LayerKind, Rgba, Source, StyleLayer, Val};
use crate::template::{self, Placed, TileId};

/// Where the pixmap is: the Web Mercator point at its top-left corner and the metres per
/// (device) pixel, with the device pixels per logical point.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub left: f64,
    pub top: f64,
    pub m_per_px: f64,
    pub ppp: f32,
}

impl Frame {
    fn px(&self, x: f64, y: f64) -> (f32, f32) {
        (((x - self.left) / self.m_per_px) as f32, ((self.top - y) / self.m_per_px) as f32)
    }
    fn extent(&self, w: u32, h: u32) -> Extent {
        Extent::new(self.left, self.top - f64::from(h) * self.m_per_px, self.left + f64::from(w) * self.m_per_px, self.top)
    }
}

/// A label from a symbol layer, in logical points, for the renderer to place with its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: Rgba,
    pub halo: Option<Rgba>,
    pub bold: bool,
    /// Higher places first.
    pub priority: f32,
    /// Where the text sits against the point, in text-box sizes (`text-anchor`): `[0.5, 0]`
    /// puts its left edge on the point, `[0, 0.5]` its top edge.
    pub anchor: [f32; 2],
    /// How far the text is moved, in ems (`text-offset`).
    pub offset: [f32; 2],
    /// The icon drawn at the point (`icon-image`). A label may have an icon and no text.
    pub icon: Option<LabelIcon>,
}

/// An icon from the style's sprite, in logical points.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelIcon {
    pub image: Arc<Pixmap>,
    pub width: f32,
    pub height: f32,
    /// The icon's centre, from the point (`icon-offset`).
    pub dx: f32,
    pub dy: f32,
}

/// What drawing a layer found.
#[derive(Debug, Default)]
pub struct Drawn {
    pub labels: Vec<Label>,
    pub attribution: Vec<String>,
    /// Some of it is still on its way: drawing again later shows more.
    pub pending: bool,
    pub error: Option<String>,
}

/// One tile's worth of drawing: the tile data, where that tile is, and the square it stands in for.
struct Item {
    payload: Arc<Payload>,
    at: Extent,
    z: u8,
    clip: Extent,
}

/// Draws `spec` into `pm` at `opacity`. Labels are collected when `labels` is set.
pub fn draw(pm: &mut Pixmap, spec: &Spec, opacity: f32, frame: &Frame, labels: bool) -> Drawn {
    let mut out = Drawn::default();
    let r = match resolve(spec) {
        Resolve::Ready(r) => r,
        Resolve::Pending => {
            out.pending = true;
            return out;
        }
        Resolve::Failed(e) => {
            out.error = Some(e);
            return out;
        }
    };
    out.attribution = r.attribution.clone();
    if !(frame.m_per_px.is_finite() && frame.m_per_px > 0.0) || opacity <= 0.0 {
        return out;
    }
    let (w, h) = (pm.width(), pm.height());
    let view = frame.extent(w, h);
    let zoom = template::style_zoom(frame.m_per_px * f64::from(frame.ppp));
    let mut layer_pm = if opacity < 0.999 { Pixmap::new(w, h) } else { None };
    let target: &mut Pixmap = match layer_pm.as_mut() {
        Some(p) => p,
        None => pm,
    };
    let mut items: HashMap<String, Vec<Item>> = HashMap::new();
    // The sprite, fetched the first time a layer needs it.
    let mut sprite_state: Option<Option<Arc<Sprite>>> = None;
    let mut sprite = |pending: &mut bool| -> Option<Arc<Sprite>> {
        sprite_state
            .get_or_insert_with(|| match r.sprite.as_deref().map(|b| sprite::load(b, frame.ppp >= 1.5)) {
                Some(Load::Ready(s)) => Some(s),
                Some(Load::Pending) => {
                    *pending = true;
                    None
                }
                Some(Load::Failed) | None => None,
            })
            .clone()
    };
    for (li, layer) in r.style.layers.iter().enumerate() {
        if !layer.visible_at(zoom) {
            continue;
        }
        if layer.kind == LayerKind::Background {
            if let (Some(c), Some(rect)) = (layer.background_color(zoom), Rect::from_xywh(0.0, 0.0, w as f32, h as f32)) {
                let mut p = Paint::default();
                p.set_color(color(c, layer.background_opacity(zoom)));
                target.fill_rect(rect, &p, Transform::identity(), None);
            }
            continue;
        }
        let Some(src_id) = layer.source.as_deref() else { continue };
        if !items.contains_key(src_id) {
            let Some(src) = r.style.sources.iter().find(|s| s.0 == src_id).map(|s| &s.1) else { continue };
            let list = tiles_for(src, &view, frame, zoom, &mut out.pending);
            items.insert(src_id.to_string(), list);
        }
        let Some(list) = items.get(src_id) else { continue };
        match layer.kind {
            LayerKind::Raster => {
                let o = layer.raster_opacity(zoom);
                for it in list {
                    draw_raster(target, it, frame, o);
                }
            }
            LayerKind::Fill | LayerKind::FillExtrusion | LayerKind::Line | LayerKind::Circle => {
                if !drawable(layer) {
                    continue;
                }
                // A pattern fill without its sprite isn't drawn: its fallback colour is black.
                let sheet = if pattern(layer).is_some() {
                    match sprite(&mut out.pending) {
                        Some(s) => Some(s),
                        None => continue,
                    }
                } else {
                    None
                };
                for it in list {
                    draw_vector(target, it, layer, frame, zoom, sheet.as_deref());
                }
            }
            LayerKind::Symbol if labels => {
                let sheet = if layer.has_layout("icon-image") { sprite(&mut out.pending) } else { None };
                for it in list {
                    // Later layers place first, as in MapLibre (styles list the most important
                    // labels last).
                    collect_labels(&mut out.labels, it, layer, frame, zoom, li as f32, sheet.as_deref(), (w as f32, h as f32));
                }
            }
            _ => {}
        }
    }
    if let Some(lp) = layer_pm {
        pm.draw_pixmap(0, 0, lp.as_ref(), &PixmapPaint { opacity, ..PixmapPaint::default() }, Transform::identity(), None);
    }
    out
}

/// The tiles of a source in view, with stand-ins for those not here yet.
fn tiles_for(src: &Source, view: &Extent, frame: &Frame, zoom: f32, pending: &mut bool) -> Vec<Item> {
    let (tiles, min_z, max_z, want, z) = match src {
        Source::Raster { tiles, tile_size, min_zoom, max_zoom, .. } => {
            let z = template::zoom_for(frame.m_per_px * f64::from(frame.ppp), *tile_size, *max_zoom);
            (tiles, *min_zoom, *max_zoom, Want::Image, z)
        }
        Source::Vector { tiles, min_zoom, max_zoom, .. } => {
            (tiles, *min_zoom, *max_zoom, Want::Vector, (zoom.floor() as u8).min(*max_zoom))
        }
        Source::Other => return Vec::new(),
    };
    if tiles.is_empty() || z < min_z || zoom + 0.5 < f32::from(min_z) {
        return Vec::new();
    }
    let url = |t: TileId| {
        let i = (t.x as usize).wrapping_add(t.y as usize) % tiles.len();
        tiles.get(i).map(|tpl| template::expand(tpl, t, 256, false, &[]))
    };
    let mut exact = Vec::new();
    let mut stand_ins = Vec::new();
    for p in template::covering(view, z) {
        let Some(u) = url(p.id) else { continue };
        match cache::get(&u, want) {
            State::Ready(payload) => exact.push(Item { payload, at: p.extent(), z: p.id.z, clip: p.extent() }),
            State::Failed(_) => {}
            State::Pending => {
                *pending = true;
                if let Some(it) = ancestor(&p, min_z, &url) {
                    stand_ins.push(it);
                } else if want == Want::Image && p.id.z < max_z {
                    for c in p.id.children() {
                        let cp = Placed { id: c, wx: p.wx * 2 + i64::from(c.x & 1) };
                        if let Some(payload) = url(c).and_then(|u| cache::peek(&u)) {
                            stand_ins.push(Item { payload, at: cp.extent(), z: c.z, clip: cp.extent() });
                        }
                    }
                }
            }
        }
    }
    // Stand-ins first, so the real tiles draw over their edges.
    stand_ins.extend(exact);
    stand_ins
}

/// The nearest ancestor of a missing tile already in the cache, clipped to the missing tile.
fn ancestor(p: &Placed, min_z: u8, url: &dyn Fn(TileId) -> Option<String>) -> Option<Item> {
    for lv in 1..=p.id.z.saturating_sub(min_z).min(8) {
        let a = p.id.ancestor(lv)?;
        if let Some(payload) = url(a).and_then(|u| cache::peek(&u)) {
            let ap = Placed { id: a, wx: p.wx.div_euclid(1 << lv) };
            return Some(Item { payload, at: ap.extent(), z: a.z, clip: p.extent() });
        }
    }
    None
}

/// Whether a fill, line or circle layer can be drawn. Lines painted with a pattern can't (the
/// pattern would have to turn along the line), and the colour the spec falls back to, black,
/// would cover the map.
fn drawable(layer: &StyleLayer) -> bool {
    !layer.has_paint("line-pattern")
}

/// The paint property a fill layer takes its pattern from, if it has one.
fn pattern(layer: &StyleLayer) -> Option<&'static str> {
    ["fill-pattern", "fill-extrusion-pattern"].into_iter().find(|p| layer.has_paint(p))
}

fn color(c: Rgba, opacity: f32) -> tiny_skia::Color {
    let a = (f32::from(c[3]) / 255.0 * opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    tiny_skia::Color::from_rgba(f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, a)
        .unwrap_or(tiny_skia::Color::TRANSPARENT)
}

/// The square an item draws in, on whole pixels: neighbouring tiles then meet on a pixel edge,
/// so anti-aliased fills cut there neither leave a seam nor overlap.
fn clip_rect(it: &Item, frame: &Frame) -> Option<Rect> {
    let (x0, y0) = frame.px(it.clip.min_x, it.clip.max_y);
    let (x1, y1) = frame.px(it.clip.max_x, it.clip.min_y);
    Rect::from_ltrb(x0.round(), y0.round(), x1.round(), y1.round())
}

fn draw_raster(pm: &mut Pixmap, it: &Item, frame: &Frame, opacity: f32) {
    let Payload::Image(img) = &*it.payload else { return };
    let Some(clip) = clip_rect(it, frame) else { return };
    let (x0, y0) = frame.px(it.at.min_x, it.at.max_y);
    let side = (TileId::span(it.z) / frame.m_per_px) as f32;
    let k = side / img.width().max(1) as f32;
    if !(k.is_finite() && k > 0.0) {
        return;
    }
    let quality = if (k - 1.0).abs() < 0.01 { FilterQuality::Nearest } else { FilterQuality::Bilinear };
    let shader = Pattern::new(img.as_ref(), SpreadMode::Pad, quality, opacity.clamp(0.0, 1.0), Transform::from_row(k, 0.0, 0.0, k, x0, y0));
    // Without anti-aliasing, neighbouring tiles meet without a seam.
    let paint = Paint { shader, anti_alias: false, ..Paint::default() };
    // Only the part in the pixmap.
    let Some(clip) = clip.intersect(&Rect::from_ltrb(0.0, 0.0, pm.width() as f32, pm.height() as f32).unwrap_or(clip)) else { return };
    pm.fill_rect(clip, &paint, Transform::identity(), None);
}

/// A tile's geometry in pixels, clipped to the item's square (lines and polygons) or kept only
/// inside it (points).
fn to_px(it: &Item, layer_extent: u32, frame: &Frame) -> impl Fn(&[f32; 2]) -> (f32, f32) + use<> {
    let (x0, y0) = frame.px(it.at.min_x, it.at.max_y);
    let k = (TileId::span(it.z) / frame.m_per_px) as f32 / layer_extent.max(1) as f32;
    move |p: &[f32; 2]| (x0 + p[0] * k, y0 + p[1] * k)
}

fn draw_vector(pm: &mut Pixmap, it: &Item, layer: &StyleLayer, frame: &Frame, zoom: f32, sheet: Option<&Sprite>) {
    let Payload::Vector(tile) = &*it.payload else { return };
    let Some(src_layer) = layer.source_layer.as_deref() else { return };
    let Some(l) = tile.layers.iter().find(|l| l.name == src_layer) else { return };
    let Some(clip) = clip_rect(it, frame) else { return };
    let bounds = [clip.left(), clip.top(), clip.right(), clip.bottom()];
    let to = to_px(it, l.extent, frame);
    let ppp = frame.ppp;
    for f in &l.features {
        let ctx = FeatureCtx { zoom, kind: f.kind, id: f.id, props: &f.props };
        if !layer.matches(&ctx) {
            continue;
        }
        match layer.kind {
            LayerKind::Fill | LayerKind::FillExtrusion if f.kind == GeomKind::Polygon => {
                let Some(c) = layer.fill_color(&ctx) else { continue };
                let mut pb = PathBuilder::new();
                for ring in &f.geometry {
                    let pts: Vec<(f32, f32)> = ring.iter().map(&to).collect();
                    let clipped = clip_polygon(&pts, bounds);
                    push_ring(&mut pb, &clipped);
                }
                let Some(path) = pb.finish() else { continue };
                let opacity = layer.fill_opacity(&ctx);
                let mut paint = Paint::default();
                paint.set_color(color(c, opacity));
                paint.anti_alias = true;
                // A pattern from the sprite, repeated from a fixed point of the world so it
                // lines up across tiles and stays put as the map moves.
                let image = pattern(layer).and_then(|p| {
                    let name = layer.paint_value(p, &ctx)?;
                    sheet?.image(name.as_str()?, None)
                });
                if pattern(layer).is_some() && image.is_none() {
                    continue;
                }
                if let Some(img) = &image {
                    let s = frame.ppp / img.ratio;
                    let (iw, ih) = (img.pixmap.width() as f32 * s, img.pixmap.height() as f32 * s);
                    let (ox, oy) = frame.px(0.0, 0.0);
                    let at = Transform::from_row(s, 0.0, 0.0, s, ox.rem_euclid(iw.max(1.0)), oy.rem_euclid(ih.max(1.0)));
                    paint.shader =
                        Pattern::new(Pixmap::as_ref(&img.pixmap), SpreadMode::Repeat, FilterQuality::Bilinear, opacity.clamp(0.0, 1.0), at);
                }
                pm.fill_path(&path, &paint, FillRule::EvenOdd, Transform::identity(), None);
                // The outline follows the rings themselves, not where the tile cut them.
                if let Some(oc) = layer.fill_outline_color(&ctx) {
                    let mut ob = PathBuilder::new();
                    for ring in &f.geometry {
                        let mut pts: Vec<(f32, f32)> = ring.iter().map(&to).collect();
                        if let (Some(&a), Some(&b)) = (pts.first(), pts.last())
                            && a != b
                        {
                            pts.push(a);
                        }
                        for piece in clip_line(&pts, bounds) {
                            push_line(&mut ob, &piece);
                        }
                    }
                    if let Some(outline) = ob.finish() {
                        paint.set_color(color(oc, opacity));
                        pm.stroke_path(
                            &outline,
                            &paint,
                            &Stroke { width: ppp.max(1.0) * 0.75, ..Stroke::default() },
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
            LayerKind::Line if matches!(f.kind, GeomKind::LineString | GeomKind::Polygon) => {
                let Some(c) = layer.line_color(&ctx) else { continue };
                let width = layer.line_width(&ctx) * ppp;
                if !(width.is_finite() && width > 0.0) {
                    continue;
                }
                let mut pb = PathBuilder::new();
                for line in &f.geometry {
                    let pts: Vec<(f32, f32)> = line.iter().map(&to).collect();
                    for piece in clip_line(&pts, bounds) {
                        push_line(&mut pb, &piece);
                    }
                }
                let Some(path) = pb.finish() else { continue };
                let mut paint = Paint::default();
                paint.set_color(color(c, layer.line_opacity(&ctx)));
                paint.anti_alias = true;
                let dash = layer.line_dasharray(&ctx).and_then(|d| {
                    let mut v: Vec<f32> = d.iter().take(32).map(|x| (x * width.max(1.0)).max(0.5)).collect();
                    // An odd number of lengths repeats once over (as in SVG).
                    if !v.len().is_multiple_of(2) {
                        v.extend(v.clone());
                    }
                    StrokeDash::new(v, 0.0)
                });
                let stroke = Stroke {
                    width: width.min(512.0),
                    line_cap: match layer.line_cap(&ctx) {
                        crate::style::LineCap::Round => LineCap::Round,
                        crate::style::LineCap::Square => LineCap::Square,
                        crate::style::LineCap::Butt => LineCap::Butt,
                    },
                    line_join: match layer.line_join(&ctx) {
                        crate::style::LineJoin::Round => LineJoin::Round,
                        crate::style::LineJoin::Bevel => LineJoin::Bevel,
                        crate::style::LineJoin::Miter => LineJoin::Miter,
                    },
                    dash,
                    ..Stroke::default()
                };
                pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
            LayerKind::Circle if f.kind == GeomKind::Point => {
                let Some(c) = layer.circle_color(&ctx) else { continue };
                let r = layer.circle_radius(&ctx) * ppp;
                if !(r.is_finite() && r > 0.0) {
                    continue;
                }
                let opacity = layer.circle_opacity(&ctx);
                let stroke = layer.circle_stroke_color(&ctx).map(|sc| (sc, layer.circle_stroke_width(&ctx) * ppp));
                for pt in f.geometry.iter().flatten() {
                    let (x, y) = to(pt);
                    if !inside(x, y, bounds) {
                        continue;
                    }
                    let Some(path) = PathBuilder::from_circle(x, y, r.min(256.0)) else { continue };
                    let mut paint = Paint::default();
                    paint.set_color(color(c, opacity));
                    paint.anti_alias = true;
                    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                    if let Some((sc, sw)) = stroke
                        && sw > 0.0
                    {
                        paint.set_color(color(sc, opacity));
                        pm.stroke_path(&path, &paint, &Stroke { width: sw.min(64.0), ..Stroke::default() }, Transform::identity(), None);
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_labels(
    out: &mut Vec<Label>,
    it: &Item,
    layer: &StyleLayer,
    frame: &Frame,
    zoom: f32,
    layer_rank: f32,
    sheet: Option<&Sprite>,
    view: (f32, f32),
) {
    if out.len() >= 4000 || !matches!(layer.symbol_placement(&FeatureCtx::zoom_only(zoom)), crate::style::SymbolPlacement::Point) {
        return;
    }
    let Payload::Vector(tile) = &*it.payload else { return };
    let Some(src_layer) = layer.source_layer.as_deref() else { return };
    let Some(l) = tile.layers.iter().find(|l| l.name == src_layer) else { return };
    let Some(clip) = clip_rect(it, frame) else { return };
    // Within the tile's square and on screen: a tile reaches far past the view when zoomed in,
    // and labels collected there would only use up the allowance.
    let bounds = [clip.left().max(0.0), clip.top().max(0.0), clip.right().min(view.0), clip.bottom().min(view.1)];
    let to = to_px(it, l.extent, frame);
    let ppp = frame.ppp.max(0.1);
    for f in l.features.iter().filter(|f| f.kind == GeomKind::Point) {
        let ctx = FeatureCtx { zoom, kind: f.kind, id: f.id, props: &f.props };
        if !layer.matches(&ctx) {
            continue;
        }
        // Lines are kept: the renderer drops those its font can't draw (a name in another script).
        let text = layer.text_field(&ctx).map(|t| t.trim().to_string()).unwrap_or_default();
        let icon = sheet.and_then(|s| icon(s, layer, &ctx));
        if text.is_empty() && icon.is_none() {
            continue;
        }
        let Some(pt) = f.geometry.first().and_then(|g| g.first()) else { continue };
        let (x, y) = to(pt);
        if !inside(x, y, bounds) {
            continue;
        }
        let size = layer.text_size(&ctx).clamp(4.0, 64.0);
        let halo = layer.text_halo_color(&ctx).filter(|c| c[3] > 0 && layer.text_halo_width(&ctx) > 0.0);
        let bold = layer.text_font(&ctx).iter().any(|f| f.contains("Bold") || f.contains("Semibold") || f.contains("Medium"));
        let sort = layer.symbol_sort_key(&ctx).unwrap_or(0.0).clamp(-1e5, 1e5);
        out.push(Label {
            text: text.chars().take(80).collect(),
            x: x / ppp,
            y: y / ppp,
            size: size * 0.85,
            color: layer.text_color(&ctx).unwrap_or([0, 0, 0, 255]),
            halo,
            bold,
            priority: layer_rank * 1e5 - sort,
            anchor: text_anchor(layer, &ctx),
            offset: pair(layer.layout_value("text-offset", &ctx)).unwrap_or([0.0, 0.0]),
            icon,
        });
        if out.len() >= 4000 {
            return;
        }
    }
}

/// A label's icon (`icon-image`), when the sprite has it, sized by `icon-size` and moved by
/// `icon-offset`; an SDF icon takes `icon-color`.
fn icon(sheet: &Sprite, layer: &StyleLayer, ctx: &FeatureCtx) -> Option<LabelIcon> {
    let name = layer.layout_value("icon-image", ctx)?;
    let name = name.as_str().filter(|n| !n.is_empty())?;
    let img = sheet.image(name, layer.paint_value("icon-color", ctx).and_then(|c| c.as_color()))?;
    let k = layer.layout_value("icon-size", ctx).and_then(|v| v.as_f64()).filter(|v| v.is_finite()).unwrap_or(1.0).clamp(0.0, 8.0) as f32;
    let (w, h) = (img.pixmap.width() as f32 / img.ratio * k, img.pixmap.height() as f32 / img.ratio * k);
    if w < 0.5 || h < 0.5 {
        return None;
    }
    let [dx, dy] = pair(layer.layout_value("icon-offset", ctx)).unwrap_or([0.0, 0.0]);
    Some(LabelIcon { image: img.pixmap, width: w, height: h, dx: dx * k, dy: dy * k })
}

/// Two finite numbers (`[x, y]`), clamped to a sane range.
fn pair(v: Option<Val>) -> Option<[f32; 2]> {
    let Val::Arr(a) = v? else { return None };
    let n = |i: usize| a.get(i).and_then(Val::as_f64).filter(|v| v.is_finite()).map(|v| v.clamp(-1000.0, 1000.0) as f32);
    Some([n(0)?, n(1)?])
}

/// `text-anchor` as the text box's shift from the point, in box sizes.
fn text_anchor(layer: &StyleLayer, ctx: &FeatureCtx) -> [f32; 2] {
    let v = layer.layout_value("text-anchor", ctx);
    let (x, y) = match v.as_ref().and_then(Val::as_str).unwrap_or("center") {
        "left" => (0.5, 0.0),
        "right" => (-0.5, 0.0),
        "top" => (0.0, 0.5),
        "bottom" => (0.0, -0.5),
        "top-left" => (0.5, 0.5),
        "top-right" => (-0.5, 0.5),
        "bottom-left" => (0.5, -0.5),
        "bottom-right" => (-0.5, -0.5),
        _ => (0.0, 0.0),
    };
    [x, y]
}

fn inside(x: f32, y: f32, b: [f32; 4]) -> bool {
    x >= b[0] && x < b[2] && y >= b[1] && y < b[3]
}

fn push_ring(pb: &mut PathBuilder, pts: &[(f32, f32)]) {
    let mut first = true;
    for &(x, y) in pts {
        if !(x.is_finite() && y.is_finite()) {
            continue;
        }
        if first {
            pb.move_to(x, y);
            first = false;
        } else {
            pb.line_to(x, y);
        }
    }
    if !first {
        pb.close();
    }
}

fn push_line(pb: &mut PathBuilder, pts: &[(f32, f32)]) {
    if pts.len() < 2 {
        return;
    }
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
}

/// Sutherland–Hodgman: a ring clipped to a rectangle `[left, top, right, bottom]`.
pub(crate) fn clip_polygon(pts: &[(f32, f32)], b: [f32; 4]) -> Vec<(f32, f32)> {
    // Fully inside: nothing to clip.
    if pts.iter().all(|&(x, y)| x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]) {
        return pts.to_vec();
    }
    let mut poly = pts.to_vec();
    for edge in 0..4 {
        if poly.is_empty() {
            break;
        }
        let inside = |p: (f32, f32)| match edge {
            0 => p.0 >= b[0],
            1 => p.0 <= b[2],
            2 => p.1 >= b[1],
            _ => p.1 <= b[3],
        };
        let cross = |a: (f32, f32), c: (f32, f32)| -> (f32, f32) {
            let t = match edge {
                0 => (b[0] - a.0) / (c.0 - a.0),
                1 => (b[2] - a.0) / (c.0 - a.0),
                2 => (b[1] - a.1) / (c.1 - a.1),
                _ => (b[3] - a.1) / (c.1 - a.1),
            };
            let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
            (a.0 + (c.0 - a.0) * t, a.1 + (c.1 - a.1) * t)
        };
        let mut next = Vec::with_capacity(poly.len() + 4);
        let mut prev = poly.last().copied().unwrap_or((0.0, 0.0));
        for &p in &poly {
            match (inside(prev), inside(p)) {
                (true, true) => next.push(p),
                (true, false) => next.push(cross(prev, p)),
                (false, true) => {
                    next.push(cross(prev, p));
                    next.push(p);
                }
                (false, false) => {}
            }
            prev = p;
        }
        poly = next;
    }
    poly
}

/// A polyline clipped to a rectangle: the pieces inside it.
pub(crate) fn clip_line(pts: &[(f32, f32)], b: [f32; 4]) -> Vec<Vec<(f32, f32)>> {
    let mut out: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    for w in pts.windows(2) {
        let (Some(&a), Some(&c)) = (w.first(), w.get(1)) else { continue };
        match clip_segment(a, c, b) {
            Some((p, q)) => {
                if cur.last().is_none_or(|l| (l.0 - p.0).abs() > 1e-3 || (l.1 - p.1).abs() > 1e-3) {
                    if cur.len() >= 2 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    cur.push(p);
                }
                cur.push(q);
            }
            None => {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

/// Liang–Barsky.
fn clip_segment(a: (f32, f32), c: (f32, f32), b: [f32; 4]) -> Option<((f32, f32), (f32, f32))> {
    let (dx, dy) = (c.0 - a.0, c.1 - a.1);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-dx, a.0 - b[0]), (dx, b[2] - a.0), (-dy, a.1 - b[1]), (dy, b[3] - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(((a.0 + t0 * dx, a.1 + t0 * dy), (a.0 + t1 * dx, a.1 + t1 * dy)))
}

/// Area of a ring in tile units (for the tests).
#[cfg(test)]
fn area(r: &[(f32, f32)]) -> f32 {
    let mut s = 0.0;
    for i in 0..r.len() {
        let (a, b) = (r[i], r[(i + 1) % r.len()]);
        s += a.0 * b.1 - b.0 * a.1;
    }
    (s / 2.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_sprite_patterns() {
        let layer = |paint: serde_json::Value| {
            StyleLayer::parse(&serde_json::json!({"id": "a", "type": "fill", "source": "s", "source-layer": "l", "paint": paint})).unwrap()
        };
        let line =
            StyleLayer::parse(&serde_json::json!({"id": "l", "type": "line", "source": "s", "paint": {"line-pattern": "rail"}})).unwrap();
        assert!(!drawable(&line));
        let fill = layer(serde_json::json!({"fill-pattern": "pedestrian_polygon"}));
        assert!(drawable(&fill) && pattern(&fill) == Some("fill-pattern"));
        assert!(pattern(&layer(serde_json::json!({"fill-color": "#ccc"}))).is_none());
    }

    /// A one-layer tile ("l") holding `features`, as the item for tile 0/0/0.
    fn item(features: Vec<crate::mvt::Feature>) -> Item {
        let tile = crate::mvt::Tile { layers: vec![crate::mvt::Layer { name: "l".into(), extent: 4096, features }] };
        let at = TileId { z: 0, x: 0, y: 0 }.extent();
        Item { payload: Arc::new(Payload::Vector(tile)), at, z: 0, clip: at }
    }

    /// A sprite of two 4×4 images: red "red" and an SDF "dot".
    fn sheet() -> Sprite {
        let mut p = Pixmap::new(8, 4).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let index = serde_json::json!({
            "red": {"x": 0, "y": 0, "width": 4, "height": 4, "pixelRatio": 1},
            "dot": {"x": 4, "y": 0, "width": 4, "height": 4, "pixelRatio": 1, "sdf": true}
        });
        Sprite::new(&index, p)
    }

    /// The whole world in a 64×64 pixmap.
    fn world_frame() -> Frame {
        let half = crate::template::HALF_WORLD;
        Frame { left: -half, top: half, m_per_px: 2.0 * half / 64.0, ppp: 1.0 }
    }

    #[test]
    fn fills_with_sprite_patterns() {
        let square = crate::mvt::Feature {
            id: None,
            kind: GeomKind::Polygon,
            geometry: vec![vec![[0.0, 0.0], [4096.0, 0.0], [4096.0, 4096.0], [0.0, 4096.0], [0.0, 0.0]]],
            props: Vec::new(),
        };
        let layer = StyleLayer::parse(
            &serde_json::json!({"id": "a", "type": "fill", "source": "s", "source-layer": "l", "paint": {"fill-pattern": "red"}}),
        )
        .unwrap();
        let mut pm = Pixmap::new(64, 64).unwrap();
        draw_vector(&mut pm, &item(vec![square.clone()]), &layer, &world_frame(), 0.0, Some(&sheet()));
        let px = pm.pixel(32, 32).unwrap();
        assert_eq!((px.red(), px.green(), px.alpha()), (255, 0, 255));
        // Without the sprite, or with an image it lacks, nothing is drawn (not black).
        let mut pm = Pixmap::new(64, 64).unwrap();
        draw_vector(&mut pm, &item(vec![square.clone()]), &layer, &world_frame(), 0.0, None);
        let missing = StyleLayer::parse(
            &serde_json::json!({"id": "a", "type": "fill", "source": "s", "source-layer": "l", "paint": {"fill-pattern": "nope"}}),
        )
        .unwrap();
        draw_vector(&mut pm, &item(vec![square]), &missing, &world_frame(), 0.0, Some(&sheet()));
        assert_eq!(pm.pixel(32, 32).unwrap().alpha(), 0);
    }

    #[test]
    fn labels_points_with_icons() {
        let point = |name: &str| crate::mvt::Feature {
            id: None,
            kind: GeomKind::Point,
            geometry: vec![vec![[2048.0, 2048.0]]],
            props: vec![("name".into(), crate::mvt::Value::String(name.into())), ("class".into(), crate::mvt::Value::String("dot".into()))],
        };
        let layer = StyleLayer::parse(&serde_json::json!({"id": "a", "type": "symbol", "source": "s", "source-layer": "l",
            "layout": {"text-field": "{name}", "icon-image": "{class}", "icon-size": 2, "text-anchor": "left", "text-offset": [1, 0]},
            "paint": {"icon-color": "#0000ff"}}))
        .unwrap();
        let mut out = Vec::new();
        collect_labels(&mut out, &item(vec![point("A"), point("")]), &layer, &world_frame(), 0.0, 1.0, Some(&sheet()), (64.0, 64.0));
        assert_eq!(out.len(), 2, "an icon without text is a label too");
        let a = &out[0];
        assert_eq!((a.text.as_str(), a.anchor, a.offset), ("A", [0.5, 0.0], [1.0, 0.0]));
        let icon = a.icon.as_ref().unwrap();
        // `{class}` names the image; icon-size doubles it; the SDF icon takes icon-color.
        assert_eq!((icon.width, icon.height), (8.0, 8.0));
        let px = icon.image.pixel(1, 1).unwrap();
        assert_eq!((px.red(), px.blue()), (0, 255));
        assert!(out[1].text.is_empty() && out[1].icon.is_some());
    }

    #[test]
    fn clips_polygons_and_lines() {
        let sq = [(-5.0, -5.0), (15.0, -5.0), (15.0, 15.0), (-5.0, 15.0)];
        let c = clip_polygon(&sq, [0.0, 0.0, 10.0, 10.0]);
        assert!((area(&c) - 100.0).abs() < 1e-3);
        assert!(clip_polygon(&[(20.0, 20.0), (30.0, 20.0), (30.0, 30.0)], [0.0, 0.0, 10.0, 10.0]).is_empty());
        let pieces = clip_line(&[(-5.0, 5.0), (5.0, 5.0), (5.0, 15.0), (5.0, 20.0), (8.0, 8.0)], [0.0, 0.0, 10.0, 10.0]);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0], vec![(0.0, 5.0), (5.0, 5.0), (5.0, 10.0)]);
        assert!(clip_line(&[(f32::NAN, 0.0), (1.0, 1.0)], [0.0, 0.0, 10.0, 10.0]).len() <= 1);
    }
}
