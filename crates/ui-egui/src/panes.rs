//! Docked panes: Contents and Catalog on the left; Geoprocessing, Symbology, History and Details
//! on the right.

use std::collections::HashMap;

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use rustgis_engine::Session;
use rustgis_engine::analysis::{ParamKind, TOOLS};
use rustgis_engine::data::GeometryType;
use rustgis_engine::map::{ClassMethod, Layer, Marker, Ramp, Renderer, Symbol};
use serde_json::{Value, json};

use crate::theme::{Tokens, c32, regular, rgba, semibold};
use crate::{LeftPane, RightPane, RustGisApp, icons};

pub fn show(app: &mut RustGisApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.pane).inner_margin(egui::Margin::ZERO);
    if let Some(left) = app.ui.left {
        egui::Panel::left("left_pane").resizable(true).default_size(270.0).size_range(200.0..=480.0).frame(frame).show(ui, |ui| {
            let (title, close) = match left {
                LeftPane::Contents => ("Contents", "ui.pane.contents"),
                LeftPane::Catalog => ("Catalog", "ui.pane.catalog"),
            };
            pane_header(app, ui, title, close, &t);
            match left {
                LeftPane::Contents => contents(app, ui, &t),
                LeftPane::Catalog => catalog(app, ui, &t),
            }
        });
    }
    if let Some(right) = app.ui.right {
        egui::Panel::right("right_pane").resizable(true).default_size(320.0).size_range(260.0..=560.0).frame(frame).show(ui, |ui| {
            let (title, close) = match right {
                RightPane::Geoprocessing => ("Geoprocessing", "ui.pane.geoprocessing"),
                RightPane::Symbology => ("Symbology", "ui.pane.symbology"),
                RightPane::History => ("History", "ui.pane.history"),
                RightPane::Details => ("Details", "ui.pane.details"),
            };
            pane_header(app, ui, title, close, &t);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| match right {
                    RightPane::Geoprocessing => geoprocessing(app, ui, &t),
                    RightPane::Symbology => symbology(app, ui, &t),
                    RightPane::History => history(app, ui, &t),
                    RightPane::Details => details(app, ui, &t),
                });
            });
        });
    }
}

fn pane_header(app: &mut RustGisApp, ui: &mut Ui, title: &str, close: &str, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.pane_header);
    ui.painter().line_segment([pos2(r.min.x, r.max.y - 0.5), pos2(r.max.x, r.max.y - 0.5)], Stroke::new(1.0, t.border));
    ui.painter().text(pos2(r.min.x + 12.0, r.center().y), Align2::LEFT_CENTER, title, semibold(13.5), t.text);
    let cr = Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(22.0, 22.0));
    let resp = ui.interact(cr, egui::Id::new(("pane_close", title)), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(cr, CornerRadius::same(4), t.hover);
    }
    icons::paint(ui.painter(), cr.shrink(5.0), "close", t.text_dim, t.accent);
    if resp.on_hover_text("Close").clicked() {
        let _ = app.run(close, Value::Null);
    }
}

// ---------------------------------------------------------------------------------------------
// Contents

/// Paints a symbol patch for a layer of geometry type `g`.
pub fn patch(p: &egui::Painter, r: Rect, s: &Symbol, g: GeometryType) {
    match g {
        GeometryType::Point => {
            let rad = (s.size.clamp(4.0, 14.0)) / 2.0;
            let c = r.center();
            match s.marker {
                Marker::Square => {
                    p.rect_filled(Rect::from_center_size(c, vec2(rad * 1.8, rad * 1.8)), 0.0, c32(s.fill));
                    p.rect_stroke(
                        Rect::from_center_size(c, vec2(rad * 1.8, rad * 1.8)),
                        0.0,
                        Stroke::new(s.stroke_width.min(2.0), c32(s.stroke)),
                        egui::StrokeKind::Middle,
                    );
                }
                Marker::Triangle => {
                    p.add(egui::Shape::convex_polygon(
                        vec![c + vec2(0.0, -rad), c + vec2(rad, rad * 0.8), c + vec2(-rad, rad * 0.8)],
                        c32(s.fill),
                        Stroke::new(s.stroke_width.min(2.0), c32(s.stroke)),
                    ));
                }
                Marker::Diamond => {
                    p.add(egui::Shape::convex_polygon(
                        vec![c + vec2(0.0, -rad), c + vec2(rad, 0.0), c + vec2(0.0, rad), c + vec2(-rad, 0.0)],
                        c32(s.fill),
                        Stroke::new(s.stroke_width.min(2.0), c32(s.stroke)),
                    ));
                }
                _ => {
                    p.circle_filled(c, rad, c32(s.fill));
                    p.circle_stroke(c, rad, Stroke::new(s.stroke_width.min(2.0), c32(s.stroke)));
                }
            }
        }
        GeometryType::Polyline => {
            let a = pos2(r.min.x + 2.0, r.center().y + 3.0);
            let b = pos2(r.center().x, r.center().y - 3.0);
            let c = pos2(r.max.x - 2.0, r.center().y + 1.0);
            p.add(egui::Shape::line(vec![a, b, c], Stroke::new(s.stroke_width.clamp(1.0, 4.0), c32(s.stroke))));
        }
        _ => {
            let rr = r.shrink(2.0);
            p.rect_filled(rr, CornerRadius::same(2), c32(s.fill));
            p.rect_stroke(rr, CornerRadius::same(2), Stroke::new(s.stroke_width.clamp(0.5, 2.0), c32(s.stroke)), egui::StrokeKind::Inside);
        }
    }
}

fn contents(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    // Drawing order header.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(egui::RichText::new("Drawing Order").size(11.5).color(t.text_dim));
    });
    ui.add_space(2.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let map_name = app.session.map().name.clone();
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            icons::paint(ui.painter(), r, "map", t.icon, t.accent);
            ui.label(egui::RichText::new(&map_name).font(semibold(12.5)));
        });
        let layers: Vec<Layer> = app.session.map().layers.clone();
        let scale = app.session.scale();
        for l in &layers {
            layer_row(app, ui, l, scale, t);
        }
        // The basemap row at the bottom, like a desktop GIS.
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(30.0);
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            icons::paint(ui.painter(), r, "basemap", t.icon, t.accent);
            ui.label(egui::RichText::new(format!("Basemap · {}", app.session.map().basemap.label())).size(12.0).color(t.text_dim));
        });
        if layers.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("This map has no layers yet.").color(t.text_dim));
                if ui.button("Add Data…").clicked() {
                    let _ = app.run("ui.addData", Value::Null);
                }
                if ui.button("Add sample countries").clicked() {
                    let _ = app.run("layer.addSample", json!({"name": "countries"}));
                }
            });
        }
        // Standalone tables.
        let tables: Vec<String> = app.session.project.tables.iter().map(|t| t.name.clone()).collect();
        if !tables.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Standalone Tables").size(11.5).color(t.text_dim));
            });
            for name in tables {
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    icons::paint(ui.painter(), r, "table", t.icon, t.accent);
                    let resp = ui.add(egui::Label::new(egui::RichText::new(&name).size(12.0)).sense(Sense::click()));
                    if resp.double_clicked() || resp.clicked() {
                        app.table.standalone = Some(name.clone());
                        app.session.table_layer = None;
                    }
                });
            }
        }
    });
}

fn layer_row(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, scale: f64, t: &Tokens) {
    let active = app.session.active_layer == Some(l.id);
    let row_h = 26.0;
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
    if active {
        ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), CornerRadius::same(5), t.selection);
    } else if resp.hovered() {
        ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), CornerRadius::same(5), t.hover);
    }
    let mut x = r.min.x + 10.0;
    // Expand arrow.
    let has_legend = !l.renderer.legend().is_empty();
    let ar = Rect::from_center_size(pos2(x + 6.0, r.center().y), vec2(14.0, 14.0));
    if has_legend {
        icons::paint(ui.painter(), ar, if l.expanded { "chevron_down" } else { "chevron_right" }, t.text_dim, t.accent);
    }
    let ar_resp = ui.interact(ar, egui::Id::new(("expand", l.id)), Sense::click());
    x += 16.0;
    // Visibility checkbox.
    let cb = Rect::from_center_size(pos2(x + 7.0, r.center().y), vec2(14.0, 14.0));
    let cb_resp = ui.interact(cb, egui::Id::new(("vis", l.id)), Sense::click());
    ui.painter().rect_filled(cb, CornerRadius::same(3), if l.visible { t.accent } else { t.input });
    ui.painter().rect_stroke(
        cb,
        CornerRadius::same(3),
        Stroke::new(1.0, if l.visible { t.accent } else { t.input_border }),
        egui::StrokeKind::Inside,
    );
    if l.visible {
        ui.painter().add(egui::Shape::line(
            vec![cb.min + vec2(3.0, 7.0), cb.min + vec2(6.0, 10.0), cb.min + vec2(11.0, 4.0)],
            Stroke::new(1.8, egui::Color32::WHITE),
        ));
    }
    x += 22.0;
    // Symbol patch (single symbol) or layer-type icon.
    let pr = Rect::from_center_size(pos2(x + 9.0, r.center().y), vec2(18.0, 16.0));
    if has_legend {
        let icon = match l.geometry_type() {
            GeometryType::Point => "point_layer",
            GeometryType::Polyline => "line_layer",
            _ => "polygon_layer",
        };
        icons::paint(ui.painter(), pr, icon, t.icon, t.accent);
    } else {
        patch(ui.painter(), pr, &l.renderer.base_symbol(), l.geometry_type());
    }
    x += 24.0;
    let out_of_range = !l.in_scale_range(scale);
    let color = if !l.visible || out_of_range { t.text_disabled } else { t.text };
    let name = if l.definition_query.is_some() { format!("{}  (filtered)", l.name) } else { l.name.clone() };
    ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, &name, if active { semibold(12.5) } else { regular(12.5) }, color);
    if !l.selection.is_empty() {
        let badge = format!("{}", l.selection.len());
        let bw = crate::widgets::text_w(ui, &badge, regular(10.5)) + 10.0;
        let br = Rect::from_center_size(pos2(r.max.x - 12.0 - bw / 2.0, r.center().y), vec2(bw, 16.0));
        ui.painter().rect_filled(br, CornerRadius::same(8), egui::Color32::from_rgb(0x00, 0xA8, 0xC0));
        ui.painter().text(br.center(), Align2::CENTER_CENTER, badge, regular(10.5), egui::Color32::WHITE);
    }
    if cb_resp.clicked() {
        let _ = app.run("layer.visibility", json!({"layer": l.id}));
    } else if ar_resp.clicked() {
        let _ = app.run("layer.expand", json!({"layer": l.id}));
    } else if resp.clicked() {
        let _ = app.run("layer.select", json!({"layer": l.id}));
    }
    if resp.double_clicked() {
        let _ = app.run("layer.select", json!({"layer": l.id}));
        app.ui.right = Some(RightPane::Symbology);
    }
    let tip = format!("{} · {} features · {}", l.geometry_type().label(), l.dataset.features.len(), l.renderer.kind_label());
    let resp = resp.on_hover_text(tip);
    resp.context_menu(|ui| {
        let _ = app.session.run("layer.select", &json!({"layer": l.id}));
        for (label, id) in [("Zoom To Layer", "layer.zoomTo"), ("Attribute Table", "table.open"), ("Symbology", "ui.pane.symbology")] {
            if ui.button(label).clicked() {
                let p = if id.starts_with("ui.") { Value::Null } else { json!({"layer": l.id}) };
                if id == "ui.pane.symbology" {
                    app.ui.right = Some(RightPane::Symbology);
                } else {
                    let _ = app.run(id, p);
                }
                ui.close();
            }
        }
        if ui.button(if l.labels.enabled { "Turn Labels Off" } else { "Label" }).clicked() {
            let _ = app.run("labels.enable", json!({"layer": l.id}));
            ui.close();
        }
        ui.separator();
        if ui.button("Move Up").clicked() {
            let _ = app.run("layer.moveUp", json!({"layer": l.id}));
            ui.close();
        }
        if ui.button("Move Down").clicked() {
            let _ = app.run("layer.moveDown", json!({"layer": l.id}));
            ui.close();
        }
        if ui.button("Rename…").clicked() {
            let _ = app.run("ui.dialog", json!({"name": "renameLayer"}));
            ui.close();
        }
        if ui.button("Definition Query…").clicked() {
            let _ = app.run("ui.dialog", json!({"name": "definitionQuery"}));
            ui.close();
        }
        if ui.button("Export Features…").clicked() {
            let _ = app.run("ui.exportLayer", Value::Null);
            ui.close();
        }
        ui.separator();
        if ui.button("Remove").clicked() {
            let _ = app.run("layer.remove", json!({"layer": l.id}));
            ui.close();
        }
    });
    // Legend classes.
    if l.expanded && has_legend {
        let field = l.renderer.field().unwrap_or("").to_string();
        ui.horizontal(|ui| {
            ui.add_space(74.0);
            ui.label(egui::RichText::new(field).size(11.0).color(t.text_dim));
        });
        for e in l.renderer.legend().iter().take(40) {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
            let pr = Rect::from_center_size(pos2(r.min.x + 84.0, r.center().y), vec2(18.0, 14.0));
            patch(ui.painter(), pr, &e.symbol, l.geometry_type());
            ui.painter().text(pos2(r.min.x + 100.0, r.center().y), Align2::LEFT_CENTER, &e.label, regular(11.5), t.text);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Catalog

fn catalog(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            egui::CollapsingHeader::new(egui::RichText::new("Maps").font(semibold(12.5))).default_open(true).show(ui, |ui| {
                let maps: Vec<String> = app.session.project.maps.iter().map(|m| m.name.clone()).collect();
                for (i, m) in maps.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        icons::paint(ui.painter(), r, "map", t.icon, t.accent);
                        let active = i == app.session.project.active_map;
                        let resp = ui.selectable_label(active, m);
                        if resp.clicked() {
                            let _ = app.run("map.activate", json!({"name": m}));
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Delete").clicked() {
                                let _ = app.run("map.remove", json!({"name": m}));
                                ui.close();
                            }
                        });
                    });
                }
            });
            egui::CollapsingHeader::new(egui::RichText::new("Toolboxes").font(semibold(12.5))).default_open(true).show(ui, |ui| {
                let mut boxes: Vec<&str> = TOOLS.iter().map(|t| t.toolbox.split(" › ").next().unwrap_or(t.toolbox)).collect();
                boxes.sort();
                boxes.dedup();
                for b in boxes {
                    egui::CollapsingHeader::new(b).default_open(false).show(ui, |ui| {
                        for tool in TOOLS.iter().filter(|x| x.toolbox.starts_with(b)) {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                                icons::paint(ui.painter(), r, "tools", t.icon, t.accent);
                                if ui.link(tool.name).on_hover_text(tool.summary).clicked() {
                                    let _ = app.run("ui.gp.open", json!({"tool": tool.id}));
                                }
                            });
                        }
                    });
                }
            });
            egui::CollapsingHeader::new(egui::RichText::new("Tables").font(semibold(12.5))).default_open(true).show(ui, |ui| {
                let tables: Vec<String> = app.session.project.tables.iter().map(|t| t.name.clone()).collect();
                if tables.is_empty() {
                    ui.label(egui::RichText::new("No standalone tables").small().weak());
                }
                for n in tables {
                    if ui.link(&n).clicked() {
                        app.table.standalone = Some(n.clone());
                        app.session.table_layer = None;
                    }
                }
            });
            egui::CollapsingHeader::new(egui::RichText::new("Folders").font(semibold(12.5))).default_open(true).show(ui, |ui| {
                folder_browser(app, ui, t);
            });
        });
    });
}

fn folder_browser(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let dir = app
        .table
        .folder
        .clone()
        .or_else(|| app.session.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())))
        .or_else(|| std::env::current_dir().ok());
    let Some(dir) = dir else { return };
    ui.horizontal(|ui| {
        if ui.small_button("↑").on_hover_text("Parent folder").clicked()
            && let Some(p) = dir.parent()
        {
            app.table.folder = Some(p.to_path_buf());
        }
        ui.label(egui::RichText::new(dir.display().to_string()).small().weak());
    });
    let Ok(rd) = std::fs::read_dir(&dir) else {
        ui.label(egui::RichText::new("Can't read this folder").small().weak());
        return;
    };
    let mut entries: Vec<(bool, String, std::path::PathBuf)> = rd
        .filter_map(|e| e.ok())
        .map(|e| (e.path().is_dir(), e.file_name().to_string_lossy().to_string(), e.path()))
        .filter(|(d, n, _)| {
            !n.starts_with('.')
                && (*d || ["geojson", "json", "csv", "shp", "rgisx"].iter().any(|x| n.to_lowercase().ends_with(&format!(".{x}"))))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    for (is_dir, name, path) in entries.into_iter().take(300) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            let icon = if is_dir {
                "add_folder"
            } else if name.ends_with(".rgisx") {
                "logo"
            } else {
                "add_data"
            };
            icons::paint(ui.painter(), r, icon, t.icon, t.accent);
            let resp = ui.add(egui::Label::new(egui::RichText::new(&name).size(12.0)).sense(Sense::click()));
            let resp = resp.on_hover_text(if is_dir { "Open folder" } else { "Double-click to add" });
            if resp.double_clicked() || (is_dir && resp.clicked()) {
                if is_dir {
                    app.table.folder = Some(path.clone());
                } else if name.ends_with(".rgisx") {
                    let _ = app.run("project.open", json!({"path": path.display().to_string()}));
                    app.map.invalidate();
                } else {
                    let _ = app.run("layer.addData", json!({"path": path.display().to_string()}));
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Geoprocessing

#[derive(Default)]
pub struct GpForm {
    pub tool: Option<&'static str>,
    pub values: HashMap<&'static str, String>,
    pub search: String,
    pub last: Option<(bool, Vec<String>)>,
}

impl GpForm {
    /// Opens a tool, pre-filling layer parameters from the active layer.
    pub fn open(&mut self, tool: Option<&'static str>, s: &Session) {
        self.tool = tool;
        self.values.clear();
        self.last = None;
        let Some(spec) = tool.and_then(rustgis_engine::analysis::tool) else { return };
        let active = s.active_layer.and_then(|id| s.map().layer(id));
        for p in spec.params {
            let v = match p.kind {
                ParamKind::Layer => active.map(|l| l.name.clone()),
                ParamKind::PolygonLayer => active
                    .filter(|l| l.geometry_type() == GeometryType::Polygon)
                    .or_else(|| s.map().layers.iter().find(|l| l.geometry_type() == GeometryType::Polygon))
                    .map(|l| l.name.clone()),
                ParamKind::PointLayer => active
                    .filter(|l| l.geometry_type() == GeometryType::Point)
                    .or_else(|| s.map().layers.iter().find(|l| l.geometry_type() == GeometryType::Point))
                    .map(|l| l.name.clone()),
                _ => serde_json::from_str::<Value>(p.default).ok().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())),
            };
            if let Some(v) = v {
                self.values.insert(p.name, v);
            }
        }
    }
}

fn geoprocessing(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(spec) = app.gp.tool.and_then(rustgis_engine::analysis::tool) else {
        // Tool finder.
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r, "search", t.text_dim, t.accent);
            ui.add(egui::TextEdit::singleline(&mut app.gp.search).hint_text("Find Tools").desired_width(f32::INFINITY));
        });
        ui.add_space(8.0);
        let q = app.gp.search.to_lowercase();
        let mut boxes: Vec<&str> = TOOLS.iter().map(|t| t.toolbox).collect();
        boxes.sort();
        boxes.dedup();
        for b in boxes {
            let tools: Vec<_> = TOOLS
                .iter()
                .filter(|x| x.toolbox == b && (q.is_empty() || x.name.to_lowercase().contains(&q) || x.summary.to_lowercase().contains(&q)))
                .collect();
            if tools.is_empty() {
                continue;
            }
            ui.label(egui::RichText::new(b).size(11.0).color(t.text_dim));
            for tool in tools {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(6), t.hover);
                }
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(pos2(r.min.x + 16.0, r.center().y), vec2(20.0, 20.0)),
                    tool_icon(tool.id),
                    t.icon,
                    t.accent,
                );
                ui.painter().text(pos2(r.min.x + 34.0, r.min.y + 13.0), Align2::LEFT_CENTER, tool.name, semibold(12.5), t.accent_text);
                let summary: String = tool.summary.chars().take(52).collect();
                ui.painter().text(pos2(r.min.x + 34.0, r.min.y + 29.0), Align2::LEFT_CENTER, summary, regular(11.0), t.text_dim);
                if resp.on_hover_text(tool.summary).clicked() {
                    app.gp.open(Some(tool.id), &app.session);
                }
            }
            ui.add_space(6.0);
        }
        return;
    };
    // Tool form.
    ui.horizontal(|ui| {
        if ui.button("←").on_hover_text("Back to tools").clicked() {
            app.gp.tool = None;
        }
        let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        icons::paint(ui.painter(), r, tool_icon(spec.id), t.icon, t.accent);
        ui.label(egui::RichText::new(spec.name).font(semibold(15.0)));
    });
    ui.label(egui::RichText::new(spec.summary).size(11.5).color(t.text_dim));
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    let layer_names = |filter: Option<GeometryType>| -> Vec<String> {
        let mut v: Vec<String> =
            app.session.map().layers.iter().filter(|l| filter.is_none_or(|g| l.geometry_type() == g)).map(|l| l.name.clone()).collect();
        if filter.is_none() {
            v.extend(app.session.project.tables.iter().map(|t| t.name.clone()));
        }
        v
    };
    let first_layer =
        spec.params.iter().find(|p| matches!(p.kind, ParamKind::Layer | ParamKind::PolygonLayer | ParamKind::PointLayer)).map(|p| p.name);
    for p in spec.params {
        ui.label(egui::RichText::new(format!("{}{}", p.label, if p.required { " *" } else { "" })).size(12.0));
        let cur = app.gp.values.get(p.name).cloned().unwrap_or_default();
        let options: Option<Vec<String>> = match p.kind {
            ParamKind::Layer => Some(layer_names(None)),
            ParamKind::PolygonLayer => Some(layer_names(Some(GeometryType::Polygon))),
            ParamKind::PointLayer => Some(layer_names(Some(GeometryType::Point))),
            ParamKind::Field | ParamKind::NumericField => {
                let lname = first_layer.and_then(|n| app.gp.values.get(n)).cloned().unwrap_or_default();
                let numeric = p.kind == ParamKind::NumericField;
                let ds = app
                    .session
                    .map()
                    .layer_by_name(&lname)
                    .map(|l| &l.dataset)
                    .or_else(|| app.session.project.tables.iter().find(|t| t.name == lname));
                let mut v: Vec<String> = ds
                    .map(|d| d.fields.iter().filter(|f| !numeric || f.ty.is_numeric()).map(|f| f.name.clone()).collect())
                    .unwrap_or_default();
                if !p.required {
                    v.insert(0, String::new());
                }
                Some(v)
            }
            ParamKind::Choice(c) => Some(c.iter().map(|s| s.to_string()).collect()),
            _ => None,
        };
        match options {
            Some(opts) => {
                egui::ComboBox::from_id_salt(("gp", p.name))
                    .selected_text(if cur.is_empty() { "—".to_string() } else { cur.clone() })
                    .width(ui.available_width() - 4.0)
                    .show_ui(ui, |ui| {
                        for o in opts {
                            if ui.selectable_label(o == cur, if o.is_empty() { "—" } else { &o }).clicked() {
                                app.gp.values.insert(p.name, o);
                            }
                        }
                    });
            }
            None => {
                let mut v = cur;
                let hint = if p.kind == ParamKind::Number { "number" } else { "" };
                if ui.add(egui::TextEdit::singleline(&mut v).hint_text(hint).desired_width(f32::INFINITY)).changed() {
                    app.gp.values.insert(p.name, v);
                }
            }
        }
        ui.add_space(4.0);
    }
    ui.add_space(8.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        let run = egui::Button::new(egui::RichText::new("Run").font(semibold(13.0)).color(egui::Color32::WHITE))
            .fill(t.green)
            .min_size(vec2(96.0, 30.0))
            .corner_radius(CornerRadius::same(6));
        if ui.add(run).clicked() {
            let mut params = serde_json::Map::new();
            for p in spec.params {
                if let Some(v) = app.gp.values.get(p.name).filter(|v| !v.trim().is_empty()) {
                    let val =
                        if p.kind == ParamKind::Number { v.trim().parse::<f64>().map(|n| json!(n)).unwrap_or(json!(v)) } else { json!(v) };
                    params.insert(p.name.to_string(), val);
                }
            }
            let r = app.run("gp.run", json!({"tool": spec.id, "params": Value::Object(params)}));
            app.gp.last = Some(match r {
                Ok(_) => (true, app.session.project.gp_history.last().map(|h| h.messages.clone()).unwrap_or_default()),
                Err(e) => (false, vec![e]),
            });
            app.map.invalidate();
        }
    });
    if let Some((ok, msgs)) = &app.gp.last {
        ui.add_space(8.0);
        let (fill, stroke, head) =
            if *ok { (t.green.gamma_multiply(0.12), t.green, "Completed") } else { (t.red.gamma_multiply(0.12), t.red, "Failed") };
        egui::Frame::NONE
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(head).font(semibold(12.5)).color(stroke));
                for m in msgs {
                    ui.label(egui::RichText::new(m).size(11.5));
                }
            });
    }
}

pub fn tool_icon(id: &str) -> &'static str {
    match id {
        "buffer" => "buffer",
        "clip" => "clip",
        "erase" => "erase",
        "dissolve" => "dissolve",
        "convexHull" => "hull",
        "featureToPoint" => "centroid",
        "simplify" => "simplify",
        "selectByLocation" => "select_loc",
        "selectByAttributes" => "select_attr",
        "countPoints" => "count",
        "calculateGeometry" => "calc_geom",
        "summaryStatistics" => "stats",
        "exportFeatures" => "export_layer",
        _ => "tools",
    }
}

// ---------------------------------------------------------------------------------------------
// Symbology

fn ramp_swatch(ui: &mut Ui, ramp: Ramp, w: f32, selected: bool, t: &Tokens) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click());
    let n = 24;
    for i in 0..n {
        let c = if ramp.is_qualitative() { ramp.pick(i * 10 / n, 10) } else { ramp.at(i as f64 / (n - 1) as f64) };
        let x0 = r.min.x + r.width() * i as f32 / n as f32;
        let x1 = r.min.x + r.width() * (i + 1) as f32 / n as f32 + 0.5;
        ui.painter().rect_filled(Rect::from_min_max(pos2(x0, r.min.y), pos2(x1, r.max.y)), 0.0, c32(c));
    }
    ui.painter().rect_stroke(
        r,
        CornerRadius::same(2),
        Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { t.accent } else { t.border_strong }),
        egui::StrokeKind::Outside,
    );
    resp
}

fn symbology(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(l) = app.session.active_layer.and_then(|id| app.session.map().layer(id)).cloned() else {
        ui.label(egui::RichText::new("Select a layer in the Contents pane.").color(t.text_dim));
        return;
    };
    let g = l.geometry_type();
    ui.label(egui::RichText::new(&l.name).font(semibold(14.0)));
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Primary symbology").size(11.5).color(t.text_dim));
    let kinds: Vec<(&str, &str)> = if g == GeometryType::Point {
        vec![
            ("single", "Single Symbol"),
            ("unique", "Unique Values"),
            ("graduated", "Graduated Colors"),
            ("proportional", "Proportional Symbols"),
        ]
    } else {
        vec![("single", "Single Symbol"), ("unique", "Unique Values"), ("graduated", "Graduated Colors")]
    };
    let cur_kind = l.renderer.kind_label();
    ui.horizontal_wrapped(|ui| {
        for (code, label) in &kinds {
            let icon = match *code {
                "single" => "single",
                "unique" => "unique",
                "graduated" => "graduated",
                _ => "proportional",
            };
            let sel = cur_kind == *label;
            let (r, resp) = ui.allocate_exact_size(vec2(68.0, 56.0), Sense::click());
            ui.painter().rect_filled(
                r,
                CornerRadius::same(6),
                if sel {
                    t.checked
                } else if resp.hovered() {
                    t.hover
                } else {
                    t.pane
                },
            );
            ui.painter().rect_stroke(
                r,
                CornerRadius::same(6),
                Stroke::new(1.0, if sel { t.accent } else { t.border }),
                egui::StrokeKind::Inside,
            );
            icons::paint(
                ui.painter(),
                Rect::from_center_size(pos2(r.center().x, r.min.y + 19.0), vec2(24.0, 24.0)),
                icon,
                t.icon,
                t.accent,
            );
            let short = label.split(' ').next().unwrap_or(label);
            ui.painter().text(pos2(r.center().x, r.max.y - 11.0), Align2::CENTER_CENTER, short, regular(11.0), t.text);
            if resp.on_hover_text(*label).clicked() && !sel {
                let _ = app.run("layer.symbology", json!({"type": code}));
            }
        }
    });
    ui.add_space(8.0);
    let numeric: Vec<String> = l.dataset.fields.iter().filter(|f| f.ty.is_numeric()).map(|f| f.name.clone()).collect();
    let all: Vec<String> = l.dataset.fields.iter().map(|f| f.name.clone()).collect();
    match &l.renderer {
        Renderer::Single(s) => symbol_editor(app, ui, *s, g, t),
        Renderer::Unique { field, classes, ramp, .. } => {
            field_combo(app, ui, "Field", field, &all, "unique");
            ramp_picker(app, ui, *ramp, t);
            ui.add_space(6.0);
            ui.label(egui::RichText::new(format!("{} classes", classes.len())).size(11.5).color(t.text_dim));
            legend_list(ui, &l, t);
        }
        Renderer::Graduated { field, breaks, ramp, method, .. } => {
            field_combo(app, ui, "Field", field, &numeric, "graduated");
            ui.horizontal(|ui| {
                ui.label("Method");
                egui::ComboBox::from_id_salt("sym_method").selected_text(method.label()).width(180.0).show_ui(ui, |ui| {
                    for m in ClassMethod::ALL {
                        if ui.selectable_label(m == *method, m.label()).clicked() {
                            let _ = app.run("layer.symbology", json!({"type": "graduated", "field": field, "method": format!("{m:?}"), "classes": breaks.len(), "ramp": format!("{ramp:?}")}));
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Classes");
                let mut k = breaks.len();
                if ui.add(egui::DragValue::new(&mut k).range(1..=12)).changed() {
                    let _ = app.run("layer.symbology", json!({"type": "graduated", "field": field, "method": format!("{method:?}"), "classes": k, "ramp": format!("{ramp:?}")}));
                }
            });
            ramp_picker(app, ui, *ramp, t);
            ui.add_space(6.0);
            legend_list(ui, &l, t);
        }
        Renderer::Proportional { field, base, .. } => {
            field_combo(app, ui, "Field", field, &numeric, "proportional");
            symbol_editor(app, ui, *base, g, t);
            legend_list(ui, &l, t);
        }
    }
    ui.add_space(10.0);
    ui.separator();
    ui.label(egui::RichText::new("Layer").size(11.5).color(t.text_dim));
    let mut tr = l.transparency;
    ui.horizontal(|ui| {
        ui.label("Transparency");
        ui.spacing_mut().slider_width = 150.0;
        let r = ui.add(egui::Slider::new(&mut tr, 0.0..=100.0).suffix(" %").integer());
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            let _ = app.run("layer.transparency", json!({"value": tr}));
        }
    });
}

fn field_combo(app: &mut RustGisApp, ui: &mut Ui, label: &str, cur: &str, fields: &[String], kind: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(("sym_field", kind)).selected_text(cur).width(200.0).show_ui(ui, |ui| {
            for f in fields {
                if ui.selectable_label(f == cur, f).clicked() {
                    let _ = app.run("layer.symbology", json!({"type": kind, "field": f}));
                }
            }
        });
    });
}

fn ramp_picker(app: &mut RustGisApp, ui: &mut Ui, cur: Ramp, t: &Tokens) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Color scheme").size(11.5).color(t.text_dim));
    let w = (ui.available_width() - 8.0) / 2.0;
    egui::Grid::new("ramps").spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        for (i, r) in Ramp::ALL.into_iter().enumerate() {
            if ramp_swatch(ui, r, w, r == cur, t).on_hover_text(r.label()).clicked() {
                let _ = app.run("layer.ramp", json!({"ramp": format!("{r:?}")}));
            }
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
}

fn legend_list(ui: &mut Ui, l: &Layer, t: &Tokens) {
    egui::Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(6)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            for e in l.renderer.legend().iter().take(64) {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
                patch(
                    ui.painter(),
                    Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(18.0, 14.0)),
                    &e.symbol,
                    l.geometry_type(),
                );
                ui.painter().text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, &e.label, regular(11.5), t.text);
            }
        },
    );
}

fn symbol_editor(app: &mut RustGisApp, ui: &mut Ui, s: Symbol, g: GeometryType, t: &Tokens) {
    egui::Frame::NONE.fill(t.pane_header).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(44.0, 36.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(4), t.ribbon);
            patch(ui.painter(), r.shrink(6.0), &s, g);
            ui.label(egui::RichText::new("Symbol").font(semibold(12.5)));
        });
        egui::Grid::new("symbol_grid").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
            if g != GeometryType::Polyline {
                ui.label("Color");
                let mut c = c32(s.fill);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    let _ = app.run("layer.color", json!({"color": rgba(c).hex()}));
                }
                ui.end_row();
            }
            ui.label(if g == GeometryType::Polyline { "Line color" } else { "Outline color" });
            let mut c = c32(s.stroke);
            if ui.color_edit_button_srgba(&mut c).changed() {
                let key = if g == GeometryType::Polyline { "layer.color" } else { "layer.outline" };
                let _ = app.run(key, json!({"color": rgba(c).hex()}));
            }
            ui.end_row();
            ui.label(if g == GeometryType::Polyline { "Line width" } else { "Outline width" });
            let mut w = s.stroke_width;
            if ui.add(egui::DragValue::new(&mut w).range(0.0..=12.0).speed(0.05).suffix(" pt")).changed() {
                let _ = app.run("layer.outline", json!({"width": w}));
            }
            ui.end_row();
            if g == GeometryType::Point {
                ui.label("Size");
                let mut sz = s.size;
                if ui.add(egui::DragValue::new(&mut sz).range(1.0..=60.0).speed(0.1).suffix(" pt")).changed() {
                    let _ = app.run("layer.marker", json!({"size": sz}));
                }
                ui.end_row();
                ui.label("Shape");
                egui::ComboBox::from_id_salt("marker_shape").selected_text(s.marker.label()).show_ui(ui, |ui| {
                    for m in Marker::ALL {
                        if ui.selectable_label(m == s.marker, m.label()).clicked() {
                            let _ = app.run("layer.marker", json!({"shape": m.label()}));
                        }
                    }
                });
                ui.end_row();
            }
        });
    });
}

// ---------------------------------------------------------------------------------------------
// History and Details

fn history(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let hist = app.session.project.gp_history.clone();
    if hist.is_empty() {
        ui.label(egui::RichText::new("Tools you run appear here.").color(t.text_dim));
        return;
    }
    for h in hist.iter().rev() {
        egui::Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.5, if h.ok { t.green } else { t.red });
                    ui.label(egui::RichText::new(&h.tool).font(semibold(12.5)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(format!("{} ms", h.millis)).size(11.0).color(t.text_dim));
                    });
                });
                for m in &h.messages {
                    ui.label(egui::RichText::new(m).size(11.0).color(t.text_dim));
                }
            },
        );
        ui.add_space(4.0);
    }
    if ui.button("Clear History").clicked() {
        let _ = app.run("gp.clearHistory", Value::Null);
    }
}

fn details(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(l) = app.session.active_layer.and_then(|id| app.session.map().layer(id)) else {
        ui.label(egui::RichText::new("Select a layer in the Contents pane.").color(t.text_dim));
        return;
    };
    let e = l.extent();
    ui.label(egui::RichText::new(&l.name).font(semibold(14.0)));
    egui::Grid::new("details").num_columns(2).striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
        let rows = [
            ("Geometry", l.geometry_type().label().to_string()),
            ("Features", l.dataset.features.len().to_string()),
            ("Fields", l.dataset.fields.len().to_string()),
            ("Selected", l.selection.len().to_string()),
            ("Symbology", l.renderer.kind_label().to_string()),
            ("Source", l.dataset.source.clone().unwrap_or_else(|| "In project".into())),
            ("West", format!("{:.4}°", e.min_x)),
            ("East", format!("{:.4}°", e.max_x)),
            ("South", format!("{:.4}°", e.min_y)),
            ("North", format!("{:.4}°", e.max_y)),
            ("Definition query", l.definition_query.clone().unwrap_or_else(|| "—".into())),
        ];
        for (k, v) in rows {
            ui.label(egui::RichText::new(k).color(t.text_dim));
            ui.label(v);
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    ui.label(egui::RichText::new("Fields").font(semibold(12.5)));
    for f in &l.dataset.fields {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&f.name).monospace().size(11.5));
            ui.label(egui::RichText::new(f.ty.label()).size(11.0).color(t.text_dim));
        });
    }
}
