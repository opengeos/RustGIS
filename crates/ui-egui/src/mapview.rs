//! The map view: the rendered map as a texture (re-rendered when the map or camera changes;
//! during pan/zoom the last frame is shifted and scaled for smoothness), the interactive tools,
//! sketches, labels, the identify pop-up, scale bar and north arrow, and the map view tabs.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use rustgis_engine::Tool;
use rustgis_engine::geom::{Coord, Extent, geodesic_length_m, geodesic_ring_area_m2, lonlat_to_merc, merc_to_lonlat};
use rustgis_engine::map::{Camera, Globe, Projection};
use rustgis_engine::render::{PlacedLabel, View};
use serde_json::{Value, json};

use crate::theme::{Tokens, c32, regular, semibold};
use crate::{RustGisApp, icons};

#[derive(Clone, Copy, PartialEq)]
struct Key {
    rev: u64,
    cam: (i64, i64, i64),
    size: [usize; 2],
    dark: bool,
    /// Rendered at reduced resolution while the globe was moving.
    draft: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Pan,
    Box,
}

pub struct Popup {
    pub info: Value,
    pub at: Coord,
}

pub struct MapView {
    texture: Option<egui::TextureHandle>,
    key: Option<Key>,
    tex_camera: Camera,
    labels: Vec<PlacedLabel>,
    last_interaction: f64,
    drag: Option<(Pos2, DragKind)>,
    sketch: Vec<Coord>,
    /// A finished measurement stays on screen until the next click.
    sketch_done: bool,
    pub popup: Option<Popup>,
    pub pointer_lonlat: Option<Coord>,
    pub dms: bool,
    pub render_ms: u64,
    pub notices_seen: usize,
    pub toasts: Vec<(String, bool, f64)>,
    camera_moved: bool,
}

impl Default for MapView {
    fn default() -> Self {
        MapView {
            texture: None,
            key: None,
            tex_camera: Camera::world(),
            labels: Vec::new(),
            last_interaction: -10.0,
            drag: None,
            sketch: Vec::new(),
            sketch_done: false,
            popup: None,
            pointer_lonlat: None,
            dms: false,
            render_ms: 0,
            notices_seen: 0,
            toasts: Vec::new(),
            camera_moved: false,
        }
    }
}

impl MapView {
    pub fn invalidate(&mut self) {
        self.key = None;
    }
    pub fn cancel_sketch(&mut self) {
        self.sketch.clear();
        self.sketch_done = false;
    }
    pub fn sketch_len(&self) -> usize {
        self.sketch.len()
    }
}

fn now(ui: &Ui) -> f64 {
    ui.input(|i| i.time)
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    view_tabs(app, ui, &t);
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    if w < 4.0 || h < 4.0 {
        return;
    }
    app.session.view_size = (w, h);
    let ppp = ui.ctx().pixels_per_point();
    let time = now(ui);

    interact(app, ui, &resp, rect);

    // Render when the map changed and we're not in the middle of a pan/zoom: meanwhile the flat
    // map's last frame is shifted and scaled. The globe can't be faked that way (turning and
    // zooming in perspective aren't shifts and scales), so while it moves it redraws every frame
    // at reduced resolution, then once in full when it settles.
    let cam = app.session.map().camera;
    let globe_view = app.session.map().projection == Projection::Globe;
    let dragging = app.map.drag.is_some_and(|(_, k)| k == DragKind::Pan);
    let moving = dragging || time - app.map.last_interaction < 0.18;
    let draft = globe_view && moving;
    let render_ppp = if draft { (ppp * 0.6).max(0.5) } else { ppp };
    let key = Key {
        rev: app.session.revision,
        cam: ((cam.center.x * 100.0) as i64, (cam.center.y * 100.0) as i64, (cam.res * 1000.0) as i64),
        size: [(w as f32 * ppp).round() as usize, (h as f32 * ppp).round() as usize],
        dark: t.dark,
        draft,
    };
    let interacting = !globe_view && moving;
    if globe_view && !moving && app.map.key.is_some_and(|k| k.draft) {
        ui.ctx().request_repaint();
    }
    let stale = app.map.key != Some(key);
    if stale && (!interacting || app.map.texture.is_none() || app.map.key.is_some_and(|k| k.rev != key.rev || k.size != key.size)) {
        let t0 = web_time::Instant::now();
        let view = View { width: w, height: h, pixels_per_point: render_ppp as f64, camera: cam, dark: t.dark };
        if let Some(r) = rustgis_engine::render::render(app.session.map_mut(), &view) {
            let size = [r.pixmap.width() as usize, r.pixmap.height() as usize];
            let img = egui::ColorImage::from_rgba_premultiplied(size, r.pixmap.data());
            match &mut app.map.texture {
                Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                None => app.map.texture = Some(ui.ctx().load_texture("map", img, egui::TextureOptions::LINEAR)),
            }
            app.map.labels = r.labels;
            app.map.tex_camera = cam;
            app.map.key = Some(key);
        }
        app.map.render_ms = t0.elapsed().as_millis() as u64;
        if app.map.camera_moved {
            app.session.record_extent();
            app.map.camera_moved = false;
        }
    } else if stale {
        ui.ctx().request_repaint();
    }

    // Draw the (possibly shifted) map texture.
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, t.canvas);
    let old = app.map.tex_camera;
    let e0 = old.extent(w, h);
    let to_screen = |c: &Camera, x: f64, y: f64| {
        let (sx, sy) = c.to_screen(Coord::new(x, y), w, h);
        pos2(rect.min.x + sx as f32, rect.min.y + sy as f32)
    };
    let tex_rect = if globe_view {
        // Where the old frame's centre now is, at the new zoom.
        let (old_g, new_g) = (Globe::new(&old, w, h), Globe::new(&cam, w, h));
        let s = (new_g.radius() / old_g.radius()) as f32;
        match new_g.forward(old.center_lonlat()) {
            Some((x, y)) if s.is_finite() => Rect::from_center_size(pos2(rect.min.x + x as f32, rect.min.y + y as f32), rect.size() * s),
            _ => rect,
        }
    } else {
        Rect::from_min_max(to_screen(&cam, e0.min_x, e0.max_y), to_screen(&cam, e0.max_x, e0.min_y))
    };
    if let Some(tex) = &app.map.texture {
        painter.image(tex.id(), tex_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    // Labels (moved with the texture; hidden while zooming, and while the globe moves, where
    // re-placing them every frame would make them flicker).
    let zooming = (old.res / cam.res - 1.0).abs() > 0.002;
    if !zooming && !(globe_view && moving) {
        let dx = tex_rect.min.x - rect.min.x;
        let dy = tex_rect.min.y - rect.min.y;
        for l in &app.map.labels {
            draw_label(&painter, l, pos2(rect.min.x + l.x + dx, rect.min.y + l.y + dy));
        }
    }
    overlays(app, ui, &painter, rect, &t);
    popup(app, ui, rect);
}

fn draw_label(p: &egui::Painter, l: &PlacedLabel, at: Pos2) {
    let font = if l.bold { semibold(l.size * 1.33) } else { regular(l.size * 1.33) };
    if let Some(h) = l.halo {
        let hc = c32(h);
        for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-0.8, -0.8), (0.8, 0.8), (-0.8, 0.8), (0.8, -0.8)] {
            p.text(at + vec2(dx, dy), Align2::CENTER_CENTER, &l.text, font.clone(), hc);
        }
    }
    p.text(at, Align2::CENTER_CENTER, &l.text, font, c32(l.color));
}

/// The globe as seen in this view, when the map is a globe.
fn globe(app: &RustGisApp, rect: Rect) -> Option<Globe> {
    let m = app.session.map();
    (m.projection == Projection::Globe).then(|| Globe::new(&m.camera, rect.width() as f64, rect.height() as f64))
}

/// The longitude/latitude under a screen point (`None` off the globe).
fn screen_to_lonlat(app: &RustGisApp, rect: Rect, p: Pos2) -> Option<Coord> {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let (x, y) = ((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64);
    match globe(app, rect) {
        Some(g) => g.inverse(x, y),
        None => Some(merc_to_lonlat(app.session.map().camera.to_map(x, y, w, h))),
    }
}

/// The screen point of a longitude/latitude (`None` behind the globe).
fn lonlat_to_screen(app: &RustGisApp, rect: Rect, c: Coord) -> Option<Pos2> {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let (x, y) = match globe(app, rect) {
        Some(g) => g.forward(c)?,
        None => app.session.map().camera.to_screen(lonlat_to_merc(c), w, h),
    };
    Some(pos2(rect.min.x + x as f32, rect.min.y + y as f32))
}

/// Pans the flat map, or turns the globe, by a screen offset.
fn pan(app: &mut RustGisApp, dx: f64, dy: f64, w: f64, h: f64) {
    let m = app.session.map_mut();
    if m.projection == Projection::Globe {
        Globe::drag(&mut m.camera, dx, dy, w, h);
    } else {
        m.camera.pan_px(dx, dy);
    }
}

/// Zooms by `factor` about a screen point.
fn zoom(app: &mut RustGisApp, factor: f64, x: f64, y: f64, w: f64, h: f64) {
    let m = app.session.map_mut();
    if m.projection == Projection::Globe {
        Globe::zoom_at(&mut m.camera, factor, x, y, w, h);
    } else {
        m.camera.zoom_at(factor, x, y, w, h);
    }
}

fn interact(app: &mut RustGisApp, ui: &mut Ui, resp: &egui::Response, rect: Rect) {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let time = now(ui);
    let hover = resp.hover_pos();
    app.map.pointer_lonlat = hover.and_then(|p| screen_to_lonlat(app, rect, p));
    let tool = app.session.tool;

    // Wheel zoom about the pointer.
    if let Some(p) = hover {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
        if scroll.abs() > 0.1 {
            let f = (scroll as f64 * 0.0028).exp();
            zoom(app, f, (p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64, w, h);
            app.map.last_interaction = time;
            app.map.camera_moved = true;
            app.map.popup = None;
        }
    }
    // Arrow keys pan.
    if !ui.ctx().egui_wants_keyboard_input() {
        let (mut dx, mut dy) = (0.0, 0.0);
        ui.input(|i| {
            if i.key_down(egui::Key::ArrowLeft) {
                dx += 12.0;
            }
            if i.key_down(egui::Key::ArrowRight) {
                dx -= 12.0;
            }
            if i.key_down(egui::Key::ArrowUp) {
                dy += 12.0;
            }
            if i.key_down(egui::Key::ArrowDown) {
                dy -= 12.0;
            }
        });
        if dx != 0.0 || dy != 0.0 {
            pan(app, dx, dy, w, h);
            app.map.last_interaction = time;
            app.map.camera_moved = true;
        }
    }

    let shift = ui.input(|i| i.modifiers.shift);
    let middle = ui.input(|i| i.pointer.middle_down());
    // Drag start.
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let kind = if middle {
            DragKind::Pan
        } else {
            match tool {
                Tool::Explore if shift => DragKind::Box,
                Tool::Explore => DragKind::Pan,
                Tool::Select | Tool::ZoomIn => DragKind::Box,
                _ => DragKind::Pan,
            }
        };
        app.map.drag = Some((p, kind));
        app.map.popup = None;
    }
    if let Some((_, DragKind::Pan)) = app.map.drag {
        let d = resp.drag_delta();
        if d != egui::Vec2::ZERO {
            pan(app, d.x as f64, d.y as f64, w, h);
            app.map.camera_moved = true;
            app.map.last_interaction = time;
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
    if resp.drag_stopped() {
        if let (Some((start, DragKind::Box)), Some(end)) = (app.map.drag, resp.interact_pointer_pos()) {
            // A box reaching off the globe uses the corners that are on it.
            let corners = [start, end, pos2(start.x, end.y), pos2(end.x, start.y)];
            let mut e = Extent::EMPTY;
            for c in corners.iter().filter_map(|c| screen_to_lonlat(app, rect, *c)) {
                e.expand(c);
            }
            if e.is_empty() {
                // Nothing on the globe.
            } else if tool == Tool::Select {
                let mode = ui.input(|i| {
                    if i.modifiers.shift {
                        "add"
                    } else if i.modifiers.command {
                        "remove"
                    } else {
                        "new"
                    }
                });
                let _ = app.run(
                    "select.byRect",
                    json!({"minLon": e.min_x, "minLat": e.min_y, "maxLon": e.max_x, "maxLat": e.max_y, "mode": mode}),
                );
            } else if (start - end).length() > 6.0 {
                app.session.zoom_to(&e);
                app.map.camera_moved = true;
            }
        }
        app.map.drag = None;
        app.map.last_interaction = time - 1.0;
    }

    // Clicks.
    if resp.double_clicked() {
        if matches!(tool, Tool::CreateLine | Tool::CreatePolygon | Tool::Measure | Tool::MeasureArea) {
            finish_sketch(app);
        } else if let Some(p) = resp.interact_pointer_pos() {
            zoom(app, 2.0, (p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64, w, h);
            app.map.last_interaction = time;
            app.map.camera_moved = true;
            app.map.popup = None;
        }
        return;
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let Some(c) = screen_to_lonlat(app, rect, p) else {
            // A click off the globe.
            app.map.popup = None;
            return;
        };
        match tool {
            Tool::Explore => {
                let r = app.session.run("identify", &json!({"lon": c.x, "lat": c.y, "tolerancePx": 5}));
                app.map.popup = match r {
                    Ok(v) if !v.is_null() => {
                        if let (Some(id), Some(row)) = (v["layerId"].as_u64(), v["row"].as_u64()) {
                            let _ = app.session.run("select.rows", &json!({"layer": id, "rows": [row]}));
                        }
                        Some(Popup { info: v, at: c })
                    }
                    _ => {
                        let _ = app.session.run("select.clear", &Value::Null);
                        None
                    }
                };
            }
            Tool::Select => {
                let mode = ui.input(|i| {
                    if i.modifiers.shift {
                        "add"
                    } else if i.modifiers.command {
                        "remove"
                    } else {
                        "new"
                    }
                });
                let _ = app.run("select.atPoint", json!({"lon": c.x, "lat": c.y, "tolerancePx": 5, "mode": mode}));
            }
            Tool::ZoomIn => {
                zoom(app, 2.0, (p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64, w, h);
                app.map.last_interaction = time;
                app.map.camera_moved = true;
            }
            Tool::CreatePoint => {
                let _ = app.run("edit.addFeature", json!({"geometry": {"type": "Point", "coordinates": [c.x, c.y]}}));
            }
            Tool::Measure | Tool::MeasureArea | Tool::CreateLine | Tool::CreatePolygon => {
                if app.map.sketch_done {
                    app.map.cancel_sketch();
                }
                // A double-click arrives as two clicks first; ignore the duplicate vertex.
                let dup = app.map.sketch.last().and_then(|l| lonlat_to_screen(app, rect, *l)).is_some_and(|s| (s - p).length() < 3.0);
                if !dup {
                    app.map.sketch.push(c);
                }
            }
        }
    }
    resp.context_menu(|ui| {
        let c = app.map.pointer_lonlat;
        if ui.button(tl!("Zoom In")).clicked() {
            let _ = app.run("map.zoomIn", Value::Null);
            ui.close();
        }
        if ui.button(tl!("Zoom Out")).clicked() {
            let _ = app.run("map.zoomOut", Value::Null);
            ui.close();
        }
        if ui.button(tl!("Full Extent")).clicked() {
            let _ = app.run("map.zoomFull", Value::Null);
            ui.close();
        }
        ui.separator();
        if let Some(c) = c
            && ui.button(tl!("Copy Coordinates")).clicked()
        {
            ui.ctx().copy_text(format!("{:.6}, {:.6}", c.x, c.y));
            ui.close();
        }
        if ui.add_enabled(app.session.map().selection_count() > 0, egui::Button::new(tl!("Clear Selection"))).clicked() {
            let _ = app.run("select.clear", Value::Null);
            ui.close();
        }
    });
    if hover.is_some()
        && matches!(tool, Tool::Measure | Tool::MeasureArea | Tool::CreateLine | Tool::CreatePolygon)
        && !app.map.sketch.is_empty()
    {
        ui.ctx().request_repaint();
    }
}

fn finish_sketch(app: &mut RustGisApp) {
    let tool = app.session.tool;
    let pts = app.map.sketch.clone();
    match tool {
        Tool::Measure | Tool::MeasureArea => app.map.sketch_done = true,
        Tool::CreateLine if pts.len() >= 2 => {
            let coords: Vec<[f64; 2]> = pts.iter().map(|c| [c.x, c.y]).collect();
            let _ = app.run("edit.addFeature", json!({"geometry": {"type": "LineString", "coordinates": coords}}));
            app.map.cancel_sketch();
        }
        Tool::CreatePolygon if pts.len() >= 3 => {
            let mut coords: Vec<[f64; 2]> = pts.iter().map(|c| [c.x, c.y]).collect();
            if let Some(f) = coords.first().copied() {
                coords.push(f);
            }
            let _ = app.run("edit.addFeature", json!({"geometry": {"type": "Polygon", "coordinates": [coords]}}));
            app.map.cancel_sketch();
        }
        _ => {}
    }
}

fn overlays(app: &mut RustGisApp, ui: &mut Ui, p: &egui::Painter, rect: Rect, t: &Tokens) {
    let accent = t.accent;
    // Box drag.
    if let (Some((start, DragKind::Box)), Some(cur)) = (app.map.drag, ui.input(|i| i.pointer.latest_pos())) {
        let r = Rect::from_two_pos(start, cur);
        p.rect_filled(r, 0.0, accent.gamma_multiply(0.12));
        p.rect_stroke(r, 0.0, Stroke::new(1.5, accent), egui::StrokeKind::Middle);
    }
    // Sketch.
    if !app.map.sketch.is_empty() {
        let tool = app.session.tool;
        let mut pts: Vec<Coord> = app.map.sketch.clone();
        if !app.map.sketch_done
            && let Some(c) = app.map.pointer_lonlat
        {
            pts.push(c);
        }
        let closed = matches!(tool, Tool::MeasureArea | Tool::CreatePolygon);
        let screen: Vec<Pos2> = pts.iter().filter_map(|c| lonlat_to_screen(app, rect, *c)).collect();
        let color = if tool.creates() { Color32::from_rgb(0x16, 0x9A, 0x5A) } else { Color32::from_rgb(0xE5, 0x6A, 0x1A) };
        if closed && screen.len() >= 3 {
            p.add(egui::Shape::closed_line(screen.clone(), Stroke::new(2.0, color)));
        } else {
            p.add(egui::Shape::line(screen.clone(), Stroke::new(2.0, color)));
        }
        for s in &screen {
            p.circle_filled(*s, 3.5, Color32::WHITE);
            p.circle_stroke(*s, 3.5, Stroke::new(1.5, color));
        }
        if matches!(tool, Tool::Measure | Tool::MeasureArea) && pts.len() >= 2 {
            let text = if tool == Tool::Measure {
                let seg = pts.len().checked_sub(2).and_then(|i| pts.get(i..)).map(geodesic_length_m).unwrap_or(0.0);
                crate::i18n::fmt(
                    "Total {total}   ·   Segment {segment}",
                    &[
                        ("total", &rustgis_engine::geom::format_distance(geodesic_length_m(&pts))),
                        ("segment", &rustgis_engine::geom::format_distance(seg)),
                    ],
                )
            } else {
                let mut ring = pts.clone();
                if let Some(f) = ring.first().copied() {
                    ring.push(f);
                }
                crate::i18n::fmt(
                    "Area {area}   ·   Perimeter {perimeter}",
                    &[
                        ("area", &rustgis_engine::geom::format_area(geodesic_ring_area_m2(&ring))),
                        ("perimeter", &rustgis_engine::geom::format_distance(geodesic_length_m(&ring))),
                    ],
                )
            };
            let anchor = pos2(rect.center().x, rect.min.y + 22.0);
            let galley = p.layout_no_wrap(text, semibold(12.5), t.text);
            let bg = Rect::from_center_size(anchor, galley.size() + vec2(24.0, 12.0));
            p.rect_filled(bg, CornerRadius::same(14), t.menu);
            p.rect_stroke(bg, CornerRadius::same(14), Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
            p.galley(bg.center() - galley.size() / 2.0, galley, t.text);
        }
    }
    // Hint for the active tool.
    let hint = match app.session.tool {
        Tool::Measure => Some("Click to add points · double-click to finish · Esc to stop"),
        Tool::MeasureArea => Some("Click the corners of the area · double-click to finish"),
        Tool::CreatePoint => Some("Click the map to add a point"),
        Tool::CreateLine => Some("Click to add vertices · double-click to finish the line"),
        Tool::CreatePolygon => Some("Click to add vertices · double-click to close the polygon"),
        Tool::Select => Some("Click or drag a box · Shift adds · Ctrl removes"),
        Tool::ZoomIn => Some("Drag a box or click to zoom in"),
        Tool::Explore => None,
    };
    if let Some(h) = hint
        && (app.map.sketch.is_empty() || !matches!(app.session.tool, Tool::Measure | Tool::MeasureArea))
    {
        let g = p.layout_no_wrap(tl!(h).to_string(), regular(12.0), Color32::WHITE);
        let bg = Rect::from_center_size(pos2(rect.center().x, rect.max.y - 26.0), g.size() + vec2(22.0, 10.0));
        p.rect_filled(bg, CornerRadius::same(12), Color32::from_black_alpha(160));
        p.galley(bg.center() - g.size() / 2.0, g, Color32::WHITE);
    }
    scale_bar(app, p, rect, t);
    north_arrow(p, rect, t);
    // Attribution.
    let att = "Natural Earth";
    let g = p.layout_no_wrap(att.to_string(), regular(10.5), t.text_dim);
    let r = Rect::from_min_size(pos2(rect.max.x - g.size().x - 14.0, rect.max.y - g.size().y - 6.0), g.size() + vec2(10.0, 4.0));
    p.rect_filled(r, CornerRadius::same(3), t.ribbon.gamma_multiply(0.75));
    p.galley(r.min + vec2(5.0, 2.0), g, t.text_dim);
    // Navigation buttons (zoom in/out/full) in the top-right corner.
    let mut y = rect.min.y + 12.0;
    for (icon, id, tip) in
        [("zoom_in", "map.zoomIn", "Zoom in"), ("zoom_out", "map.zoomOut", "Zoom out"), ("zoom_full", "map.zoomFull", "Full extent")]
    {
        let r = Rect::from_min_size(pos2(rect.max.x - 44.0, y), vec2(32.0, 32.0));
        let resp = ui.interact(r, egui::Id::new(("nav", id)), Sense::click());
        p.rect_filled(r, CornerRadius::same(8), if resp.hovered() { t.hover } else { t.ribbon });
        p.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
        icons::paint(p, r.shrink(7.0), icon, t.icon, t.accent);
        if resp.on_hover_text(tl!(tip)).clicked() {
            let _ = app.run(id, Value::Null);
            app.map.popup = None;
        }
        y += 38.0;
    }
}

fn scale_bar(app: &RustGisApp, p: &egui::Painter, rect: Rect, t: &Tokens) {
    let cam = app.session.map().camera;
    let lat = cam.center_lonlat().y.to_radians().cos().abs().max(0.01);
    let m_per_pt = cam.res * lat;
    let d = rustgis_engine::geom::nice_distance(m_per_pt * 130.0);
    if d <= 0.0 {
        return;
    }
    let len = (d / m_per_pt) as f32;
    let x0 = rect.min.x + 18.0;
    let y0 = rect.max.y - 22.0;
    let bg = Rect::from_min_max(pos2(x0 - 8.0, y0 - 20.0), pos2(x0 + len + 10.0, y0 + 10.0));
    p.rect_filled(bg, CornerRadius::same(5), t.ribbon.gamma_multiply(0.85));
    for i in 0..4 {
        let seg = Rect::from_min_size(pos2(x0 + len * i as f32 / 4.0, y0), vec2(len / 4.0, 5.0));
        p.rect_filled(seg, 0.0, if i % 2 == 0 { t.text } else { t.ribbon });
    }
    p.rect_stroke(Rect::from_min_size(pos2(x0, y0), vec2(len, 5.0)), 0.0, Stroke::new(1.0, t.text), egui::StrokeKind::Middle);
    p.text(pos2(x0, y0 - 3.0), Align2::LEFT_BOTTOM, "0", regular(10.5), t.text);
    p.text(pos2(x0 + len, y0 - 3.0), Align2::RIGHT_BOTTOM, rustgis_engine::geom::format_distance(d), regular(10.5), t.text);
}

fn north_arrow(p: &egui::Painter, rect: Rect, t: &Tokens) {
    let c = pos2(rect.min.x + 30.0, rect.min.y + 34.0);
    p.circle_filled(c, 17.0, t.ribbon.gamma_multiply(0.85));
    p.circle_stroke(c, 17.0, Stroke::new(1.0, t.border_strong));
    p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -12.0), c + vec2(6.0, 6.0), c + vec2(0.0, 2.5)], t.text, Stroke::NONE));
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(0.0, -12.0), c + vec2(0.0, 2.5), c + vec2(-6.0, 6.0)],
        t.text.gamma_multiply(0.4),
        Stroke::NONE,
    ));
    p.text(c + vec2(0.0, 11.5), Align2::CENTER_CENTER, "N", semibold(8.5), t.text);
}

fn popup(app: &mut RustGisApp, ui: &mut Ui, rect: Rect) {
    let Some(pp) = &app.map.popup else { return };
    let t = Tokens::get(ui.ctx());
    let Some(anchor) = lonlat_to_screen(app, rect, pp.at) else { return };
    if !rect.contains(anchor) {
        return;
    }
    let info = pp.info.clone();
    let layer = info["layer"].as_str().unwrap_or("").to_string();
    let attrs = info["attributes"].as_object().cloned().unwrap_or_default();
    let title = if info["raster"].as_bool() == Some(true) {
        tl!("Pixel").to_string()
    } else {
        attrs.iter().find(|(k, _)| k.to_lowercase().contains("name")).and_then(|(_, v)| v.as_str()).unwrap_or(tl!("Feature")).to_string()
    };
    let p = ui.painter();
    p.circle_filled(anchor, 6.0, Color32::from_rgb(0x00, 0xE5, 0xFF).gamma_multiply(0.5));
    p.circle_stroke(anchor, 6.0, Stroke::new(2.0, Color32::from_rgb(0x00, 0xA8, 0xC0)));
    let pos = pos2((anchor.x + 14.0).min(rect.max.x - 330.0), (anchor.y - 40.0).clamp(rect.min.y + 8.0, rect.max.y - 300.0));
    let mut close = false;
    egui::Area::new(egui::Id::new("identify_popup")).fixed_pos(pos).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::NONE
            .fill(t.menu)
            .stroke(Stroke::new(1.0, t.border_strong))
            .corner_radius(CornerRadius::same(10))
            .shadow(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: t.shadow })
            .show(ui, |ui| {
                ui.set_width(310.0);
                egui::Frame::NONE
                    .fill(t.pane_header)
                    .corner_radius(CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 })
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(&title).font(semibold(14.0)));
                                ui.label(egui::RichText::new(&layer).size(11.0).color(t.text_dim));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                if ui.add(egui::Button::new(tl!("×")).frame(false)).clicked() {
                                    close = true;
                                }
                            });
                        });
                    });
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
                        egui::Grid::new("popup_grid").num_columns(2).striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                            for (k, v) in &attrs {
                                ui.label(egui::RichText::new(k).size(11.5).color(t.text_dim));
                                ui.label(egui::RichText::new(v.as_str().unwrap_or("")).size(11.5));
                                ui.end_row();
                            }
                        });
                    });
                });
            });
    });
    if close {
        app.map.popup = None;
    }
}

/// View tabs above the map, one per map in the project, like document tabs.
fn view_tabs(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.tab_strip);
    ui.painter().line_segment([pos2(r.min.x, r.max.y - 0.5), pos2(r.max.x, r.max.y - 0.5)], Stroke::new(1.0, t.border));
    let mut x = r.min.x + 6.0;
    let names: Vec<String> = app.session.project.maps.iter().map(|m| m.name.clone()).collect();
    let active = app.session.project.active_map;
    for (i, n) in names.iter().enumerate() {
        let w = crate::widgets::text_w(ui, n, regular(12.0)) + 46.0;
        let tr = Rect::from_min_size(pos2(x, r.min.y + 4.0), vec2(w, r.height() - 4.0));
        let resp = ui.interact(tr, egui::Id::new(("maptab", i)), Sense::click());
        let is = i == active;
        if is {
            ui.painter().rect_filled(tr, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, t.ribbon);
            ui.painter().rect_filled(
                Rect::from_min_max(tr.min, pos2(tr.max.x, tr.min.y + 2.0)),
                CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                t.accent,
            );
        } else if resp.hovered() {
            ui.painter().rect_filled(tr, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, t.hover);
        }
        icons::paint(ui.painter(), Rect::from_center_size(pos2(tr.min.x + 15.0, tr.center().y), vec2(15.0, 15.0)), "map", t.icon, t.accent);
        ui.painter().text(
            pos2(tr.min.x + 28.0, tr.center().y),
            Align2::LEFT_CENTER,
            n,
            if is { semibold(12.0) } else { regular(12.0) },
            if is { t.text } else { t.text_dim },
        );
        if resp.clicked() && !is {
            let _ = app.run("map.activate", json!({"name": n}));
            app.map.popup = None;
        }
        x += w + 2.0;
    }
    let pr = Rect::from_min_size(pos2(x + 2.0, r.min.y + 6.0), vec2(22.0, 20.0));
    let resp = ui.interact(pr, egui::Id::new("maptab_new"), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(pr, CornerRadius::same(4), t.hover);
    }
    ui.painter().text(pr.center(), Align2::CENTER_CENTER, "+", regular(15.0), t.text_dim);
    if resp.on_hover_text(tl!("New Map")).clicked() {
        let _ = app.run("map.new", Value::Null);
    }
}
