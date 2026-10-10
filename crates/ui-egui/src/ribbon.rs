//! The ribbon: the tab strip (with the Project button and the contextual Feature Layer or Raster
//! Layer tab group) and every tab's groups. Each control runs a command by id.

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use rustgis_engine::Tool;
use rustgis_engine::data::GeometryType;
use rustgis_engine::map::{Basemap, RasterRenderer, Renderer, Resampling, Stretch};
use serde_json::{Value, json};

use crate::theme::{Tokens, regular, semibold};
use crate::widgets::{CONTENT_H, LABEL_H, big, big_menu, col, group, menu_item, small, small_menu, text_w};
use crate::{RightPane, RustGisApp, icons};

pub const TABS: [&str; 7] = ["Map", "Insert", "Analysis", "View", "Edit", "Share", "Help"];
/// The contextual tabs for feature layers: (tab id, caption).
pub const FEATURE_TABS: &[(&str, &str)] = &[("Feature Layer", "Feature Layer"), ("Labeling", "Labeling"), ("Data", "Data")];
/// The contextual tabs for raster layers.
pub const RASTER_TABS: &[(&str, &str)] = &[("Raster Layer", "Raster Layer"), ("Raster Data", "Data")];

/// The contextual tab group for the layer selected in Contents: its caption (shown above its
/// tabs) and tabs.
pub fn context(app: &RustGisApp) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    let l = app.session.active_layer.and_then(|id| app.session.map().layer(id))?;
    if l.is_raster() {
        Some(("Raster Layer", RASTER_TABS))
    } else if l.geometry_type() != GeometryType::None {
        Some(("Feature Layer", FEATURE_TABS))
    } else {
        None
    }
}

/// Whether a tab id belongs to a contextual group.
pub fn is_context_tab(tab: &str) -> bool {
    FEATURE_TABS.iter().chain(RASTER_TABS).any(|(id, _)| *id == tab)
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let tabs = context(app).map(|(_, tabs)| tabs).unwrap_or(&[]);
    if is_context_tab(&app.ui.tab) && !tabs.iter().any(|(id, _)| *id == app.ui.tab) {
        // Switching between a feature and a raster layer keeps a contextual tab open.
        app.ui.tab = tabs.first().map(|(id, _)| id.to_string()).unwrap_or_else(|| "Map".into());
    }
    egui::Panel::top("tabs")
        .exact_size(32.0)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.tab_strip).inner_margin(egui::Margin { left: 8, right: 8, top: 0, bottom: 0 }))
        .show(ui, |ui| tab_strip(app, ui, &t));
    if app.ui.ribbon_minimized && !app.ui.ribbon_peek {
        return;
    }
    // A peeked ribbon closes on the next click outside the tab strip and the ribbon itself.
    // Popups opened from the ribbon (menus, combos, colour pickers) draw outside it, so a click in
    // one must not close it.
    let peek_started = app.ui.ribbon_peek && ui.input(|i| i.pointer.any_click()) && !egui::Popup::is_any_open(ui.ctx());
    let ribbon = egui::Panel::top("ribbon")
        .exact_size(CONTENT_H + LABEL_H + 12.0)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.ribbon).inner_margin(egui::Margin { left: 8, right: 8, top: 5, bottom: 3 }))
        .show(ui, |ui| {
            let r = ui.max_rect();
            let full = Rect::from_min_max(pos2(r.min.x - 8.0, r.max.y + 3.0), pos2(r.max.x + 8.0, r.max.y + 4.0));
            ui.painter().rect_filled(full, 0.0, t.border);
            egui::ScrollArea::horizontal().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                    match app.ui.tab.as_str() {
                        "Insert" => insert_tab(app, ui),
                        "Analysis" => analysis_tab(app, ui),
                        "View" => view_tab(app, ui),
                        "Edit" => edit_tab(app, ui),
                        "Share" => share_tab(app, ui),
                        "Help" => help_tab(app, ui),
                        "Feature Layer" => feature_layer_tab(app, ui),
                        "Labeling" => labeling_tab(app, ui),
                        "Data" => data_tab(app, ui),
                        "Raster Layer" => raster_layer_tab(app, ui),
                        "Raster Data" => raster_data_tab(app, ui),
                        _ => map_tab(app, ui),
                    }
                });
            });
        });
    if peek_started {
        let pos = ui.input(|i| i.pointer.interact_pos());
        let inside = pos.is_some_and(|p| ribbon.response.rect.contains(p) || app.ui.tab_strip_rect.is_some_and(|r| r.contains(p)));
        if !inside {
            app.ui.ribbon_peek = false;
        }
    }
}

fn tab_strip(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
        // The Project button opens the backstage.
        let pw = text_w(ui, tl!("Project"), semibold(12.5)) + 26.0;
        let (pr, presp) = ui.allocate_exact_size(vec2(pw, 26.0), Sense::click());
        let fill = if presp.hovered() { crate::theme::BRAND_DARK } else { crate::theme::BRAND };
        ui.painter().rect_filled(pr, CornerRadius::same(6), fill);
        ui.painter().text(pr.center(), Align2::CENTER_CENTER, tl!("Project"), semibold(12.5), egui::Color32::WHITE);
        if presp.on_hover_text(tl!("Open, save and create projects")).clicked() {
            app.ui.backstage = true;
        }
        ui.add_space(6.0);
        let mut tabs: Vec<(&str, &str)> = TABS.iter().map(|t| (*t, *t)).collect();
        if let Some((_, ctx)) = context(app) {
            tabs.extend(ctx.iter().copied());
        }
        // Tighter tabs when the window is too narrow for the usual padding (keeping room for the
        // collapse button on the right).
        let room = ui.available_width() - 40.0;
        let text: f32 = tabs.iter().map(|(_, c)| text_w(ui, tl!(*c), semibold(12.5))).sum();
        let pad = ((room - text) / tabs.len().max(1) as f32).clamp(8.0, 22.0);
        let mut rects = Vec::new();
        for (tab, caption) in tabs {
            let is_ctx = is_context_tab(tab);
            let active = app.ui.tab == tab;
            let font = if active { semibold(12.5) } else { regular(12.5) };
            let shown = tl!(caption);
            let w = text_w(ui, shown, font.clone()) + pad;
            let (r, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
            let body = Rect::from_min_max(pos2(r.min.x, r.min.y + 3.0), r.max);
            if is_ctx {
                ui.painter().rect_filled(body, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, t.context_tint);
            }
            if active {
                ui.painter().rect_filled(body, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, t.ribbon);
                ui.painter().rect_stroke(
                    body,
                    CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                    Stroke::new(1.0, t.border),
                    egui::StrokeKind::Inside,
                );
                ui.painter().rect_filled(
                    Rect::from_min_max(pos2(r.min.x + 1.0, r.max.y - 1.5), pos2(r.max.x - 1.0, r.max.y + 1.0)),
                    0.0,
                    t.ribbon,
                );
                let bar = if is_ctx { t.context } else { t.accent };
                ui.painter().rect_filled(
                    Rect::from_min_max(pos2(r.min.x + 8.0, r.max.y - 3.0), pos2(r.max.x - 8.0, r.max.y - 1.0)),
                    CornerRadius::same(1),
                    bar,
                );
            } else if resp.hovered() {
                ui.painter().rect_filled(body.shrink2(vec2(1.0, 2.0)), CornerRadius::same(5), t.hover);
            }
            let color = if is_ctx {
                t.context
            } else if active {
                t.text
            } else {
                t.text_dim
            };
            ui.painter().text(pos2(r.center().x, r.center().y + 1.5), Align2::CENTER_CENTER, shown, font, color);
            if resp.double_clicked() {
                let _ = app.run("ui.ribbon", Value::Null);
            } else if resp.clicked() {
                if app.ui.ribbon_minimized {
                    app.ui.ribbon_peek = !(app.ui.ribbon_peek && app.ui.tab == tab);
                }
                app.ui.tab = tab.to_string();
            }
            rects.push((tab.to_string(), r));
        }
        app.ui.tab_rects = rects;
        app.ui.tab_strip_rect = Some(ui.max_rect());
        // Right side: collapse/expand the ribbon, then the quick tool indicator.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (cr, cresp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
            if cresp.hovered() {
                ui.painter().rect_filled(cr, CornerRadius::same(5), t.hover);
            }
            let icon = if app.ui.ribbon_minimized { "chevron_down" } else { "up" };
            icons::paint(ui.painter(), cr.shrink(5.0), icon, t.text_dim, t.accent);
            let tip = if app.ui.ribbon_minimized { tl!("Expand the ribbon (Ctrl+F1)") } else { tl!("Collapse the ribbon (Ctrl+F1)") };
            if cresp.on_hover_text(tip).clicked() {
                let _ = app.run("ui.ribbon", Value::Null);
            }
            let tool = app.session.tool;
            if tool != Tool::Explore {
                let label = crate::i18n::fmt("{tool} tool active · Esc to stop", &[("tool", tl!(tool.label()))]);
                let w = text_w(ui, &label, regular(11.5)) + 18.0;
                let (r, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
                ui.painter().rect_filled(r, CornerRadius::same(11), t.checked);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, label, regular(11.5), t.accent_text);
                if resp.clicked() {
                    app.session.tool = Tool::Explore;
                    app.map.cancel_sketch();
                }
            }
        });
    });
}

fn tool_btn(ui: &mut Ui, app: &mut RustGisApp, icon: &str, label: &str, desc: &str, tool: Tool) {
    let checked = app.session.tool == tool;
    let r = big(ui, app, icon, label, desc, "tool.set", json!({"tool": tool.code()}), checked, false);
    if r.clicked() {
        app.map.cancel_sketch();
    }
}

fn map_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Navigate", |ui| {
        tool_btn(
            ui,
            app,
            "explore",
            "Explore",
            "Pan with the mouse, zoom with the wheel and click features to see their attributes.",
            Tool::Explore,
        );
        big_menu(ui, app, "bookmark", "Bookmarks", "Go to a saved extent or save the current one.", |ui, app| {
            let names: Vec<String> = app.session.map().bookmarks.iter().map(|b| b.name.clone()).collect();
            if names.is_empty() {
                ui.label(egui::RichText::new(tl!("No bookmarks yet")).weak());
            }
            for n in names {
                if ui.button(n.as_str()).clicked() {
                    let _ = app.run("bookmark.go", json!({"name": n}));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(tl!("New Bookmark…")).clicked() {
                let _ = app.run("ui.dialog", json!({"name": "bookmark"}));
                ui.close();
            }
        });
        col(ui, |ui| {
            small(ui, app, "zoom_full", "Full Extent", "Zoom to the extent of all layers.", "map.zoomFull", Value::Null, false);
            small(ui, app, "zoom_in", "Fixed Zoom In", "Zoom in on the centre of the map.", "map.zoomIn", Value::Null, false);
            small(ui, app, "zoom_out", "Fixed Zoom Out", "Zoom out from the centre of the map.", "map.zoomOut", Value::Null, false);
        });
        col(ui, |ui| {
            small(ui, app, "prev_extent", "Previous", "Go back to the previous extent.", "map.prevExtent", Value::Null, false);
            small(ui, app, "next_extent", "Next", "Go forward to the next extent.", "map.nextExtent", Value::Null, false);
            small(ui, app, "goto_xy", "Go To XY", "Pan to a longitude/latitude.", "ui.dialog", json!({"name": "goToXY"}), false);
        });
    });
    group(ui, "Layer", |ui| {
        big_menu(ui, app, "basemap", "Basemap", "Change the map's backdrop.", basemap_gallery);
        big_menu(ui, app, "add_data", "Add\nData", "Add GeoJSON, CSV or Shapefile data to the map.", |ui, app| {
            menu_item(ui, app, "", "Data…", "ui.addData", Value::Null);
            ui.separator();
            ui.label(egui::RichText::new(tl!("Sample data (Natural Earth)")).small().weak());
            for (n, label) in
                [("countries", "World Countries"), ("cities", "Populated Places"), ("rivers", "Major Rivers"), ("lakes", "Major Lakes")]
            {
                menu_item(ui, app, "", label, "layer.addSample", json!({"name": n}));
            }
        });
    });
    group(ui, "Selection", |ui| {
        tool_btn(ui, app, "select", "Select", "Click or drag a box to select features. Shift adds, Ctrl removes.", Tool::Select);
        big(
            ui,
            app,
            "select_attr",
            "Select By\nAttributes",
            "Select features with a SQL expression.",
            "ui.dialog",
            json!({"name": "selectByAttributes"}),
            false,
            false,
        );
        big(
            ui,
            app,
            "select_loc",
            "Select By\nLocation",
            "Select features that intersect another layer.",
            "ui.gp.open",
            json!({"tool": "selectByLocation"}),
            false,
            false,
        );
        col(ui, |ui| {
            small(ui, app, "clear_sel", "Clear", "Clear the selection in every layer.", "select.clear", Value::Null, false);
            small(ui, app, "zoom_sel", "Zoom To", "Zoom to the selected features.", "select.zoomTo", Value::Null, false);
            small(ui, app, "table", "Attributes", "Open the attribute table of the selected layer.", "table.open", Value::Null, false);
        });
    });
    group(ui, "Inquiry", |ui| {
        big_menu(ui, app, "measure", "Measure", "Measure geodesic distances and areas on the map.", |ui, app| {
            if ui.button(tl!("Measure Distance")).clicked() {
                let _ = app.run("tool.set", json!({"tool": "measure"}));
                app.map.cancel_sketch();
                ui.close();
            }
            if ui.button(tl!("Measure Area")).clicked() {
                let _ = app.run("tool.set", json!({"tool": "measureArea"}));
                app.map.cancel_sketch();
                ui.close();
            }
        });
        col(ui, |ui| {
            small(ui, app, "locate", "Locate", "Find a place by coordinates.", "ui.dialog", json!({"name": "goToXY"}), false);
            small(
                ui,
                app,
                "zoom_box",
                "Zoom In",
                "Drag a box to zoom into.",
                "tool.set",
                json!({"tool": "zoomIn"}),
                app.session.tool == Tool::ZoomIn,
            );
        });
    });
    group(ui, "Labeling", |ui| {
        let on = app.session.active_layer.and_then(|id| app.session.map().layer(id)).is_some_and(|l| l.labels.enabled);
        big(
            ui,
            app,
            if on { "label" } else { "label_off" },
            "Label",
            "Turn labels on or off for the selected layer.",
            "labels.enable",
            Value::Null,
            on,
            false,
        );
    });
}

fn basemap_gallery(ui: &mut Ui, app: &mut RustGisApp) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("Basemaps")).font(semibold(12.5)));
    ui.add_space(4.0);
    egui::Grid::new("basemaps").spacing(vec2(8.0, 8.0)).show(ui, |ui| {
        for (i, b) in Basemap::ALL.into_iter().enumerate() {
            let (water, land, outline, _) = b.palette();
            let (r, resp) = ui.allocate_exact_size(vec2(92.0, 74.0), Sense::click());
            let sel = app.session.map().basemap == b;
            let thumb = Rect::from_min_size(r.min + vec2(4.0, 4.0), vec2(84.0, 48.0));
            ui.painter().rect_filled(thumb, CornerRadius::same(4), crate::theme::c32(water));
            // A little continent.
            let pts = [(0.18, 0.3), (0.42, 0.18), (0.6, 0.35), (0.55, 0.7), (0.3, 0.82), (0.15, 0.6)];
            let poly: Vec<egui::Pos2> =
                pts.iter().map(|(x, y)| pos2(thumb.min.x + x * thumb.width(), thumb.min.y + y * thumb.height())).collect();
            ui.painter().add(egui::Shape::convex_polygon(poly, crate::theme::c32(land), Stroke::new(1.0, crate::theme::c32(outline))));
            let poly2: Vec<egui::Pos2> = [(0.68, 0.22), (0.88, 0.3), (0.84, 0.55), (0.7, 0.5)]
                .iter()
                .map(|(x, y)| pos2(thumb.min.x + x * thumb.width(), thumb.min.y + y * thumb.height()))
                .collect();
            ui.painter().add(egui::Shape::convex_polygon(poly2, crate::theme::c32(land), Stroke::new(1.0, crate::theme::c32(outline))));
            if sel || resp.hovered() {
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::same(6),
                    Stroke::new(if sel { 2.0 } else { 1.0 }, t.accent),
                    egui::StrokeKind::Inside,
                );
            }
            ui.painter().text(pos2(r.center().x, r.max.y - 11.0), Align2::CENTER_CENTER, tl!(b.label()), regular(11.5), t.text);
            if resp.clicked() {
                let _ = app.run("map.basemap", json!({"name": b.code()}));
                ui.close();
            }
            if i % 3 == 2 {
                ui.end_row();
            }
        }
    });
}

fn insert_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Project", |ui| {
        big(ui, app, "new_map", "New\nMap", "Add a new map to the project.", "map.new", Value::Null, false, false);
        big(ui, app, "toolbox", "Toolbox", "Browse the geoprocessing toolbox.", "ui.pane.geoprocessing", Value::Null, false, false);
        big(ui, app, "open", "Import\nProject", "Open a RustGIS project.", "ui.open", Value::Null, false, false);
        col(ui, |ui| {
            small(ui, app, "catalog", "Catalog", "Browse the project's maps, tables and folders.", "ui.pane.catalog", Value::Null, false);
            small(ui, app, "add_data", "Add Data", "Add vector data.", "ui.addData", Value::Null, false);
        });
    });
    group(ui, "Bookmarks", |ui| {
        big(
            ui,
            app,
            "bookmark",
            "New\nBookmark",
            "Save the current extent as a bookmark.",
            "ui.dialog",
            json!({"name": "bookmark"}),
            false,
            false,
        );
    });
    group(ui, "Data", |ui| {
        big(ui, app, "field", "Add\nField", "Add a field to the selected layer.", "ui.dialog", json!({"name": "addField"}), false, false);
        big(
            ui,
            app,
            "calculate",
            "Calculate\nField",
            "Compute a field's values.",
            "ui.dialog",
            json!({"name": "calculateField"}),
            false,
            false,
        );
    });
}

/// Analysis gallery entries: (tool id, icon, caption).
const GALLERY: &[(&str, &str, &str)] = &[
    ("buffer", "buffer", "Buffer"),
    ("clip", "clip", "Clip"),
    ("erase", "erase", "Erase"),
    ("dissolve", "dissolve", "Dissolve"),
    ("countPoints", "count", "Summarize\nWithin"),
    ("convexHull", "hull", "Bounding\nGeometry"),
    ("featureToPoint", "centroid", "Feature\nTo Point"),
    ("simplify", "simplify", "Simplify"),
    ("calculateGeometry", "calc_geom", "Calculate\nGeometry"),
    ("summaryStatistics", "stats", "Summary\nStatistics"),
    ("exportFeatures", "export_layer", "Export\nFeatures"),
];

fn analysis_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Geoprocessing", |ui| {
        big(
            ui,
            app,
            "history",
            "History",
            "See the tools you've run.",
            "ui.pane.history",
            Value::Null,
            app.ui.right == Some(RightPane::History),
            false,
        );
        big(
            ui,
            app,
            "toolbox",
            "Tools",
            "Find and run geoprocessing tools.",
            "ui.pane.geoprocessing",
            Value::Null,
            app.ui.right == Some(RightPane::Geoprocessing),
            false,
        );
    });
    group(ui, "Tools", |ui| {
        let t = Tokens::get(ui.ctx());
        // A framed gallery like a desktop GIS analysis gallery.
        let r = ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
            for (id, icon, label) in GALLERY {
                let tool = rustgis_engine::analysis::tool(id);
                let desc = tool.map(|t| t.summary).unwrap_or("");
                big(ui, app, icon, label, desc, "ui.gp.open", json!({"tool": id}), false, false);
            }
        });
        ui.painter().rect_stroke(r.response.rect.expand(1.0), CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
    });
    group(ui, "Selection", |ui| {
        big(
            ui,
            app,
            "select_attr",
            "By\nAttributes",
            "Select features with a SQL expression.",
            "ui.dialog",
            json!({"name": "selectByAttributes"}),
            false,
            false,
        );
        big(
            ui,
            app,
            "select_loc",
            "By\nLocation",
            "Select features by their spatial relationship.",
            "ui.gp.open",
            json!({"tool": "selectByLocation"}),
            false,
            false,
        );
    });
}

fn view_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Windows", |ui| {
        big(
            ui,
            app,
            "contents",
            "Contents",
            "Show or hide the Contents pane.",
            "ui.pane.contents",
            Value::Null,
            app.ui.left == Some(crate::LeftPane::Contents),
            false,
        );
        big(
            ui,
            app,
            "catalog",
            "Catalog\nPane",
            "Show or hide the Catalog pane.",
            "ui.pane.catalog",
            Value::Null,
            app.ui.left == Some(crate::LeftPane::Catalog),
            false,
        );
        big(
            ui,
            app,
            "geoprocessing",
            "Geo-\nprocessing",
            "Show or hide the Geoprocessing pane.",
            "ui.pane.geoprocessing",
            Value::Null,
            app.ui.right == Some(RightPane::Geoprocessing),
            false,
        );
        col(ui, |ui| {
            small(
                ui,
                app,
                "symbology",
                "Symbology",
                "Show the Symbology pane for the selected layer.",
                "ui.pane.symbology",
                Value::Null,
                app.ui.right == Some(RightPane::Symbology),
            );
            small(
                ui,
                app,
                "history",
                "History",
                "Geoprocessing history.",
                "ui.pane.history",
                Value::Null,
                app.ui.right == Some(RightPane::History),
            );
            small(
                ui,
                app,
                "identify",
                "Details",
                "Details of the selected layer.",
                "ui.pane.details",
                Value::Null,
                app.ui.right == Some(RightPane::Details),
            );
        });
        small(
            ui,
            app,
            "table",
            "Attribute Table",
            "Open the selected layer's table.",
            "table.open",
            Value::Null,
            app.session.table_layer.is_some(),
        );
    });
    group(ui, "Display", |ui| {
        let g = app.session.map().graticule;
        big(ui, app, "graticule", "Graticule", "Show latitude/longitude lines on the basemap.", "map.graticule", Value::Null, g, false);
        let dark = app.ui.dark;
        big(ui, app, "dark", "Dark\nMode", "Switch between light and dark themes.", "ui.theme", Value::Null, dark, false);
    });
    group(ui, "Navigation", |ui| {
        big(ui, app, "zoom_world", "World", "Zoom to the whole world.", "map.zoomWorld", Value::Null, false, false);
    });
    group(ui, "Language", |ui| language_menu(app, ui));
}

fn edit_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Manage Edits", |ui| {
        big(ui, app, "save", "Save", "Save the project with your edits.", "ui.save", Value::Null, false, false);
        col(ui, |ui| {
            small(ui, app, "undo", "Undo", "Undo the last change.", "edit.undo", Value::Null, false);
            small(ui, app, "redo", "Redo", "Redo the change you undid.", "edit.redo", Value::Null, false);
        });
    });
    group(ui, "Features", |ui| {
        let gt = app.session.active_layer.and_then(|id| app.session.map().layer(id)).map(|l| l.geometry_type());
        let (tool, icon) = match gt {
            Some(GeometryType::Polyline) => (Tool::CreateLine, "line_tool"),
            Some(GeometryType::Polygon) => (Tool::CreatePolygon, "polygon_tool"),
            _ => (Tool::CreatePoint, "point_tool"),
        };
        let checked = app.session.tool.creates();
        let r = big(
            ui,
            app,
            icon,
            "Create",
            "Draw new features in the selected layer. Double-click to finish lines and polygons.",
            "tool.set",
            json!({"tool": tool.code()}),
            checked,
            false,
        );
        if r.clicked() {
            app.map.cancel_sketch();
        }
        big(ui, app, "delete", "Delete", "Delete the selected features.", "edit.delete", Value::Null, false, false);
    });
    group(ui, "Selection", |ui| {
        tool_btn(ui, app, "select", "Select", "Select features on the map.", Tool::Select);
        col(ui, |ui| {
            small(ui, app, "table", "Attributes", "Edit attributes in the table.", "table.open", Value::Null, false);
            small(ui, app, "clear_sel", "Clear", "Clear the selection.", "select.clear", Value::Null, false);
            small(ui, app, "zoom_sel", "Zoom To", "Zoom to the selection.", "select.zoomTo", Value::Null, false);
        });
    });
    group(ui, "Fields", |ui| {
        big(ui, app, "field", "Add\nField", "Add a field.", "ui.dialog", json!({"name": "addField"}), false, false);
        big(ui, app, "calculate", "Calculate", "Calculate field values.", "ui.dialog", json!({"name": "calculateField"}), false, false);
    });
}

fn share_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Output", |ui| {
        big(
            ui,
            app,
            "export_map",
            "Export\nMap",
            "Export the map view as a PNG image (with labels and a scale bar).",
            "ui.exportMap",
            Value::Null,
            false,
            false,
        );
        big(
            ui,
            app,
            "export_layer",
            "Export\nFeatures",
            "Export the selected layer to GeoJSON or CSV.",
            "ui.exportLayer",
            Value::Null,
            false,
            false,
        );
    });
    group(ui, "Save As", |ui| {
        big(ui, app, "save_as", "Project", "Save the project under a new name.", "ui.saveAs", Value::Null, false, false);
    });
}

fn help_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Help", |ui| {
        big(ui, app, "help", "About", "About RustGIS.", "ui.about", Value::Null, false, false);
        big(ui, app, "command", "Command\nSearch", "Find any command by name (Ctrl+K).", "ui.commandSearch", Value::Null, false, false);
        big(ui, app, "settings", "Shortcuts", "Keyboard shortcuts.", "ui.shortcuts", Value::Null, false, false);
    });
}

// ---------------------------------------------------------------------------------------------
// contextual tabs

const SCALES: [f64; 10] =
    [1_000.0, 10_000.0, 50_000.0, 100_000.0, 500_000.0, 1_000_000.0, 5_000_000.0, 10_000_000.0, 50_000_000.0, 100_000_000.0];

fn scale_label(s: Option<f64>) -> String {
    match s {
        None => tl!("<None>").into(),
        Some(v) => format!("1:{}", fmt_thousands(v)),
    }
}

pub fn fmt_thousands(v: f64) -> String {
    let s = format!("{:.0}", v.abs());
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if v < 0.0 { format!("-{out}") } else { out }
}

fn feature_layer_tab(app: &mut RustGisApp, ui: &mut Ui) {
    let Some(layer) = app.session.active_layer.and_then(|id| app.session.map().layer(id)).cloned() else { return };
    visibility_and_effects(app, ui, &layer);
    drawing_group(app, ui, &layer);
    layer_group(app, ui, &layer);
}

/// The Visibility Range and Effects groups shared by feature and raster layers.
fn visibility_and_effects(app: &mut RustGisApp, ui: &mut Ui, layer: &rustgis_engine::map::Layer) {
    group(ui, "Visibility Range", |ui| {
        ui.vertical(|ui| {
            ui.add_space(6.0);
            for (label, cur, key) in [("Out Beyond", layer.min_scale, "min"), ("In Beyond", layer.max_scale, "max")] {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!(label)).size(11.5));
                    egui::ComboBox::from_id_salt(format!("scale_{key}")).selected_text(scale_label(cur)).width(118.0).show_ui(ui, |ui| {
                        let mut choices: Vec<Option<f64>> = vec![None];
                        choices.extend(SCALES.iter().map(|s| Some(*s)));
                        for c in choices {
                            if ui.selectable_label(c == cur, scale_label(c)).clicked() {
                                let (mut min, mut max) = (layer.min_scale, layer.max_scale);
                                if key == "min" {
                                    min = c;
                                } else {
                                    max = c;
                                }
                                let _ = app.run("layer.scaleRange", json!({"min": min, "max": max}));
                            }
                        }
                    });
                });
            }
        });
        big(ui, app, "scale_range", "Clear\nLimits", "Draw the layer at every scale.", "layer.scaleRange", json!({}), false, false);
    });
    group(ui, "Effects", |ui| {
        ui.vertical(|ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let t = Tokens::get(ui.ctx());
                icons::paint(ui.painter(), Rect::from_min_size(ui.cursor().min, vec2(20.0, 20.0)), "transparency", t.icon, t.accent);
                ui.add_space(24.0);
                ui.label(egui::RichText::new(tl!("Transparency")).size(11.5));
            });
            let mut v = layer.transparency;
            ui.spacing_mut().slider_width = 130.0;
            let r = ui.add(egui::Slider::new(&mut v, 0.0..=100.0).suffix(" %").integer());
            live_drag(
                app,
                ui,
                &r,
                layer.transparency as f64,
                v as f64,
                |app, x| {
                    if let Some(l) = app.session.active_layer.and_then(|id| app.session.map_mut().layer_mut(id)) {
                        l.transparency = x as f32;
                    }
                },
                |app, x| {
                    let _ = app.run("layer.transparency", json!({"value": x}));
                },
            );
        });
    });
}

/// A dragged control with a live preview: `set` writes the value straight into the project while
/// dragging, and on release the pre-drag value is put back before `commit` runs the command, so
/// the command's undo snapshot holds the old value.
fn live_drag(
    app: &mut RustGisApp,
    ui: &Ui,
    resp: &egui::Response,
    before: f64,
    value: f64,
    set: impl Fn(&mut RustGisApp, f64),
    commit: impl FnOnce(&mut RustGisApp, f64),
) {
    let key = resp.id.with("live_drag_original");
    if resp.drag_started() {
        ui.data_mut(|d| d.insert_temp(key, before));
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        if let Some(original) = ui.data_mut(|d| d.remove_temp::<f64>(key)) {
            set(app, original);
        }
        commit(app, value);
    } else if resp.dragged() {
        set(app, value);
        app.session.revision = app.session.revision.wrapping_add(1);
    }
}

fn drawing_group(app: &mut RustGisApp, ui: &mut Ui, layer: &rustgis_engine::map::Layer) {
    group(ui, "Drawing", |ui| {
        big(
            ui,
            app,
            "symbology",
            "Symbology",
            "Open the Symbology pane.",
            "ui.pane.symbology",
            Value::Null,
            app.ui.right == Some(RightPane::Symbology),
            false,
        );
        let numeric: Vec<String> = layer.dataset.fields.iter().filter(|f| f.ty.is_numeric()).map(|f| f.name.clone()).collect();
        let text_f: Vec<String> = layer.dataset.fields.iter().map(|f| f.name.clone()).collect();
        let is_point = layer.geometry_type() == GeometryType::Point;
        big_menu(ui, app, renderer_icon(&layer.renderer), "Primary\nSymbology", "Choose how the layer is drawn.", move |ui, app| {
            if ui.button(tl!("Single Symbol")).clicked() {
                let _ = app.run("layer.symbology", json!({"type": "single"}));
                ui.close();
            }
            ui.menu_button(tl!("Unique Values"), |ui| {
                for f in &text_f {
                    if ui.button(f).clicked() {
                        let _ = app.run("layer.symbology", json!({"type": "unique", "field": f}));
                        ui.close();
                    }
                }
            });
            ui.menu_button(tl!("Graduated Colors"), |ui| {
                for f in &numeric {
                    if ui.button(f).clicked() {
                        let _ = app.run("layer.symbology", json!({"type": "graduated", "field": f}));
                        ui.close();
                    }
                }
            });
            if is_point {
                ui.menu_button(tl!("Proportional Symbols"), |ui| {
                    for f in &numeric {
                        if ui.button(f).clicked() {
                            let _ = app.run("layer.symbology", json!({"type": "proportional", "field": f}));
                            ui.close();
                        }
                    }
                });
            }
        });
        col(ui, |ui| {
            let mut c = crate::theme::c32(layer.renderer.base_symbol().fill);
            ui.horizontal(|ui| {
                if ui.color_edit_button_srgba(&mut c).changed() {
                    let _ = app.run("layer.color", json!({"color": crate::theme::rgba(c).hex()}));
                }
                ui.label(egui::RichText::new(tl!("Color")).size(11.5));
            });
            let mut w = layer.renderer.base_symbol().stroke_width;
            ui.horizontal(|ui| {
                if ui.add(egui::DragValue::new(&mut w).range(0.0..=10.0).speed(0.05).suffix(" pt")).changed() {
                    let _ = app.run("layer.outline", json!({"width": w}));
                }
                ui.label(egui::RichText::new(tl!("Outline")).size(11.5));
            });
            if layer.geometry_type() == GeometryType::Point {
                let mut s = layer.renderer.base_symbol().size;
                ui.horizontal(|ui| {
                    if ui.add(egui::DragValue::new(&mut s).range(1.0..=60.0).speed(0.1).suffix(" pt")).changed() {
                        let _ = app.run("layer.marker", json!({"size": s}));
                    }
                    ui.label(egui::RichText::new(tl!("Size")).size(11.5));
                });
            }
        });
    });
}

fn layer_group(app: &mut RustGisApp, ui: &mut Ui, layer: &rustgis_engine::map::Layer) {
    group(ui, "Layer", |ui| {
        big(ui, app, "zoom_full", "Zoom To\nLayer", "Zoom to the layer's extent.", "layer.zoomTo", Value::Null, false, false);
        col(ui, |ui| {
            if layer.is_raster() {
                small(
                    ui,
                    app,
                    "symbology",
                    "Symbology",
                    "Open the Symbology pane.",
                    "ui.pane.symbology",
                    Value::Null,
                    app.ui.right == Some(RightPane::Symbology),
                );
            } else {
                small(ui, app, "table", "Attribute Table", "Open the layer's table.", "table.open", Value::Null, false);
            }
            small(ui, app, "eye", "Visible", "Show or hide the layer.", "layer.visibility", Value::Null, layer.visible);
            small(ui, app, "delete", "Remove", "Remove the layer from the map.", "layer.remove", Value::Null, false);
        });
    });
}

fn raster_layer_tab(app: &mut RustGisApp, ui: &mut Ui) {
    let Some(layer) = app.session.active_layer.and_then(|id| app.session.map().layer(id)).cloned() else { return };
    let Some(rl) = layer.raster.clone() else { return };
    visibility_and_effects(app, ui, &layer);
    let Some(data) = rl.data.clone() else {
        group(ui, "Rendering", |ui| {
            ui.label(egui::RichText::new(tl!("The raster's data couldn't be read.")).size(11.5));
        });
        return;
    };
    let style = rl.style.clone();
    group(ui, "Rendering", |ui| {
        let n = data.band_count();
        let has_categories = data.bands.first().is_some_and(|b| b.stats.categories.is_some());
        let palette = matches!(data.color, rustgis_engine::data::raster::ColorHint::Palette(_));
        let kind = style.renderer.code();
        big_menu(ui, app, raster_renderer_icon(&style.renderer), "Symbology", "Choose how the raster is drawn.", move |ui, app| {
            for (code, label) in RasterRenderer::KINDS {
                let ok = match code {
                    "rgb" => n >= 3,
                    "unique" => has_categories,
                    "colormap" => palette,
                    _ => true,
                };
                let checked = kind == code;
                if ui.add_enabled(ok, egui::Button::selectable(checked, tl!(label))).clicked() {
                    let _ = app.run("raster.symbology", json!({"type": code}));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(tl!("Symbology Pane…")).clicked() {
                app.ui.right = Some(RightPane::Symbology);
                ui.close();
            }
        });
        let current = style.stretch;
        big_menu(ui, app, "stretch", "Stretch\nType", "Choose a contrast stretch for the raster layer.", move |ui, app| {
            for (code, label) in Stretch::KINDS {
                if ui.add(egui::Button::selectable(current.code() == code, tl!(label))).clicked() {
                    let _ = app.run("raster.stretch", json!({"type": code}));
                    ui.close();
                }
            }
        });
        col(ui, |ui| {
            small(
                ui,
                app,
                "dra",
                "DRA",
                "Dynamic range adjustment: stretch using the pixels in view.",
                "raster.dra",
                Value::Null,
                style.dra,
            );
            let res = style.resampling;
            small_menu(ui, app, "resample", "Resampling", "Choose a resampling type for the raster layer.", move |ui, app| {
                for r in Resampling::ALL {
                    if ui.add(egui::Button::selectable(r == res, tl!(r.label()))).clicked() {
                        let _ = app.run("raster.resampling", json!({"type": format!("{r:?}")}));
                        ui.close();
                    }
                }
            });
            let bands: Vec<String> = data.bands.iter().map(|b| b.name.clone()).collect();
            let cur = match style.renderer {
                RasterRenderer::Rgb { red, green, blue } => Some([red, green, blue]),
                _ => None,
            };
            small_menu(ui, app, "bands", "Band Combination", "Choose a band combination to display the raster layer.", move |ui, app| {
                if bands.len() < 3 {
                    ui.label(tl!("Needs three or more bands."));
                    return;
                }
                if ui.add(egui::Button::selectable(cur == Some([0, 1, 2]), tl!("Natural Color (1, 2, 3)"))).clicked() {
                    let _ = app.run("raster.bands", json!({"preset": "natural"}));
                    ui.close();
                }
                if bands.len() >= 4 && ui.add(egui::Button::selectable(cur == Some([3, 0, 1]), tl!("Color Infrared (4, 1, 2)"))).clicked() {
                    let _ = app.run("raster.bands", json!({"preset": "colorInfrared"}));
                    ui.close();
                }
                ui.separator();
                for (key, label, slot) in [("red", "Red", 0usize), ("green", "Green", 1), ("blue", "Blue", 2)] {
                    ui.menu_button(tl!(label), |ui| {
                        for (i, b) in bands.iter().enumerate() {
                            if ui
                                .add(egui::Button::selectable(
                                    cur.and_then(|c| c.get(slot).copied()) == Some(i),
                                    crate::panes::band_name(b),
                                ))
                                .clicked()
                            {
                                let _ = app.run("raster.bands", json!({ key: i + 1 }));
                                ui.close();
                            }
                        }
                    });
                }
            });
        });
    });
    group(ui, "Enhancement", |ui| {
        ui.vertical(|ui| {
            ui.add_space(2.0);
            for (key, label, icon, value, range, speed) in [
                ("brightness", "Brightness", "brightness", style.brightness, -100.0..=100.0, 0.5),
                ("contrast", "Contrast", "contrast", style.contrast, -100.0..=100.0, 0.5),
                ("gamma", "Gamma", "gamma", style.gamma, 0.1..=10.0, 0.01),
            ] {
                ui.horizontal(|ui| {
                    let t = Tokens::get(ui.ctx());
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    icons::paint(ui.painter(), r, icon, t.icon, t.accent);
                    ui.label(egui::RichText::new(tl!(label)).size(11.5));
                    ui.add_space((70.0 - text_w(ui, tl!(label), regular(11.5))).max(0.0));
                    let mut v = value;
                    let resp = ui.add(egui::DragValue::new(&mut v).range(range).speed(speed).max_decimals(2));
                    live_drag(
                        app,
                        ui,
                        &resp,
                        value,
                        v,
                        |app, x| {
                            let rl =
                                app.session.active_layer.and_then(|id| app.session.map_mut().layer_mut(id)).and_then(|l| l.raster.as_mut());
                            if let Some(rl) = rl {
                                match key {
                                    "brightness" => rl.style.brightness = x,
                                    "contrast" => rl.style.contrast = x,
                                    _ => rl.style.gamma = x,
                                }
                            }
                        },
                        |app, x| {
                            let _ = app.run("raster.enhance", json!({ key: x }));
                        },
                    );
                });
            }
        });
        big(
            ui,
            app,
            "undo",
            "Reset",
            "Reset brightness, contrast and gamma.",
            "raster.enhance",
            json!({"brightness": 0, "contrast": 0, "gamma": 1}),
            false,
            false,
        );
    });
    layer_group(app, ui, &layer);
}

fn raster_renderer_icon(r: &RasterRenderer) -> &'static str {
    match r {
        RasterRenderer::Stretch { .. } => "stretch",
        RasterRenderer::Rgb { .. } => "rgb",
        RasterRenderer::Classify { .. } => "classify",
        RasterRenderer::UniqueValues { .. } => "unique",
        RasterRenderer::Colormap => "raster_layer",
        RasterRenderer::ShadedRelief { .. } => "relief",
    }
}

fn raster_data_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Information", |ui| {
        big(
            ui,
            app,
            "identify",
            "Raster\nInformation",
            "Size, bands, statistics and coordinate system.",
            "ui.pane.details",
            Value::Null,
            app.ui.right == Some(RightPane::Details),
            false,
        );
    });
    group(ui, "Export", |ui| {
        big(ui, app, "export_layer", "Export\nRaster", "Save the raster as a GeoTIFF.", "ui.exportRaster", Value::Null, false, false);
    });
}

fn renderer_icon(r: &Renderer) -> &'static str {
    match r {
        Renderer::Single(_) => "single",
        Renderer::Unique { .. } => "unique",
        Renderer::Graduated { .. } => "graduated",
        Renderer::Proportional { .. } => "proportional",
    }
}

fn labeling_tab(app: &mut RustGisApp, ui: &mut Ui) {
    let Some(layer) = app.session.active_layer.and_then(|id| app.session.map().layer(id)).cloned() else { return };
    group(ui, "Layer", |ui| {
        let on = layer.labels.enabled;
        big(ui, app, if on { "label" } else { "label_off" }, "Label", "Turn labels on or off.", "labels.enable", Value::Null, on, false);
    });
    group(ui, "Label Class", |ui| {
        ui.vertical(|ui| {
            ui.add_space(8.0);
            let fields: Vec<String> = layer.dataset.fields.iter().map(|f| f.name.clone()).collect();
            if let Some(f) = crate::widgets::ribbon_combo(ui, "label_field", "Field", &layer.labels.field, &fields, 140.0) {
                let _ = app.run("labels.field", json!({"field": f}));
            }
        });
    });
    group(ui, "Text Symbol", |ui| {
        col(ui, |ui| {
            let mut size = layer.labels.size;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Size")).size(11.5));
                if ui.add(egui::DragValue::new(&mut size).range(4.0..=48.0).speed(0.1).suffix(" pt")).changed() {
                    let _ = app.run("labels.style", json!({"size": size}));
                }
            });
            let mut c = crate::theme::c32(layer.labels.color);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Color")).size(11.5));
                if ui.color_edit_button_srgba(&mut c).changed() {
                    let _ = app.run("labels.style", json!({"color": crate::theme::rgba(c).hex()}));
                }
            });
        });
        col(ui, |ui| {
            small(ui, app, "label", "Bold", "Bold label text.", "labels.style", json!({"bold": !layer.labels.bold}), layer.labels.bold);
            small(
                ui,
                app,
                "single",
                "Halo",
                "Draw a halo around label text.",
                "labels.style",
                json!({"halo": !layer.labels.halo}),
                layer.labels.halo,
            );
        });
    });
}

fn data_tab(app: &mut RustGisApp, ui: &mut Ui) {
    group(ui, "Definition Query", |ui| {
        let has = app.session.active_layer.and_then(|id| app.session.map().layer(id)).is_some_and(|l| l.definition_query.is_some());
        big(
            ui,
            app,
            "def_query",
            "Definition\nQuery",
            "Limit which features draw with a SQL expression.",
            "ui.dialog",
            json!({"name": "definitionQuery"}),
            has,
            false,
        );
    });
    group(ui, "Table", |ui| {
        big(ui, app, "table", "Attribute\nTable", "Open the layer's attribute table.", "table.open", Value::Null, false, false);
    });
    group(ui, "Selection", |ui| {
        col(ui, |ui| {
            small(ui, app, "select", "All", "Select every feature.", "select.all", Value::Null, false);
            small(ui, app, "select_loc", "Switch", "Invert the selection.", "select.switch", Value::Null, false);
            small(ui, app, "clear_sel", "Clear", "Clear the selection.", "select.clear", Value::Null, false);
        });
        big(
            ui,
            app,
            "add_data",
            "Layer From\nSelection",
            "Make a new layer of the selected features.",
            "select.layerFromSelection",
            Value::Null,
            false,
            false,
        );
    });
    group(ui, "Data Design", |ui| {
        big(ui, app, "field", "Fields", "Add a field.", "ui.dialog", json!({"name": "addField"}), false, false);
        big(ui, app, "calculate", "Calculate", "Calculate field values.", "ui.dialog", json!({"name": "calculateField"}), false, false);
    });
    group(ui, "Export", |ui| {
        small_menu(ui, app, "export_layer", "Export", "Export the layer.", |ui, app| {
            menu_item(ui, app, "", "Export Features…", "ui.exportLayer", Value::Null);
            menu_item(ui, app, "", "Export Features (tool)", "ui.gp.open", json!({"tool": "exportFeatures"}));
        });
    });
}

/// A language's own name, or its English name when no installed font can draw it (CJK names on the
/// web before a CJK font has been downloaded).
fn shown_name(ui: &Ui, l: crate::i18n::Lang) -> &'static str {
    if ui.ctx().fonts_mut(|f| f.has_glyphs(&regular(12.5), l.name())) { l.name() } else { l.english() }
}

/// The interface language picker (View › Language).
fn language_menu(app: &mut RustGisApp, ui: &mut Ui) {
    let current = app.lang();
    let auto = app.ui.language == crate::i18n::AUTO;
    let label = if auto { format!("{} ({})", tl!("Automatic"), shown_name(ui, current)) } else { shown_name(ui, current).to_string() };
    big_menu(ui, app, "globe_lang", &label, "Choose the interface language.", move |ui, app| {
        if ui.selectable_label(auto, tl!("Automatic (system language)")).clicked() {
            let _ = app.run("ui.language", json!({"code": crate::i18n::AUTO}));
            ui.close();
        }
        ui.separator();
        for l in crate::i18n::Lang::all() {
            let sel = !auto && l == current;
            if ui.selectable_label(sel, shown_name(ui, l)).on_hover_text(l.english()).clicked() {
                let _ = app.run("ui.language", json!({"code": l.code()}));
                ui.close();
            }
        }
    });
}
