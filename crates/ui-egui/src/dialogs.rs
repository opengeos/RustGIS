//! Dialogs (Select By Attributes, Go To XY, bookmarks, fields, definition query, rename) and
//! native file pickers.

use egui::{Align2, vec2};
use rustgis_engine::Session;
use serde_json::{Value, json};

use crate::RustGisApp;
use crate::theme::{Tokens, semibold};

#[derive(Default)]
pub struct Dialogs {
    pub open: Option<&'static str>,
    pub layer: String,
    pub text: String,
    pub a: String,
    pub b: String,
    pub c: String,
    pub mode: &'static str,
    pub error: Option<String>,
    pub info: Option<String>,
}

impl Dialogs {
    pub fn open(&mut self, which: &str, s: &Session) -> Result<(), String> {
        let active = s.active_layer.and_then(|id| s.map().layer(id));
        let name: &'static str = match which {
            "selectByAttributes" => "selectByAttributes",
            "goToXY" => "goToXY",
            "bookmark" => "bookmark",
            "addField" => "addField",
            "calculateField" => "calculateField",
            "definitionQuery" => "definitionQuery",
            "renameLayer" => "renameLayer",
            other => return Err(format!("unknown dialog '{other}'")),
        };
        if matches!(name, "addField" | "calculateField" | "definitionQuery" | "renameLayer") && active.is_none() {
            return Err("select a layer in the Contents pane first".into());
        }
        if name == "selectByAttributes" && s.map().layers.is_empty() {
            return Err("the map has no layers".into());
        }
        *self = Dialogs { open: Some(name), mode: "new", ..Dialogs::default() };
        self.layer = active.or_else(|| s.map().layers.first()).map(|l| l.name.clone()).unwrap_or_default();
        match name {
            "goToXY" => {
                let c = s.map().camera.center_lonlat();
                self.a = format!("{:.4}", c.x);
                self.b = format!("{:.4}", c.y);
            }
            "bookmark" => self.text = format!("Bookmark {}", s.map().bookmarks.len() + 1),
            "addField" => self.b = "Double".into(),
            "calculateField" => self.a = active.and_then(|l| l.dataset.fields.last()).map(|f| f.name.clone()).unwrap_or_default(),
            "definitionQuery" => self.text = active.and_then(|l| l.definition_query.clone()).unwrap_or_default(),
            "renameLayer" => self.text = active.map(|l| l.name.clone()).unwrap_or_default(),
            _ => {}
        }
        Ok(())
    }
}

pub fn show(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(which) = app.dialogs.open else { return };
    let title = match which {
        "selectByAttributes" => "Select By Attributes",
        "goToXY" => "Go To XY",
        "bookmark" => "New Bookmark",
        "addField" => "Add Field",
        "calculateField" => "Calculate Field",
        "definitionQuery" => "Definition Query",
        _ => "Rename Layer",
    };
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut close = false;
    egui::Window::new(title).id(egui::Id::new("dialog")).open(&mut open).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, -40.0)).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        match which {
            "selectByAttributes" => close = select_by_attributes(app, ui, &t),
            "goToXY" => {
                egui::Grid::new("xy").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                    ui.label("Longitude (X)");
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.a).desired_width(200.0));
                    ui.end_row();
                    ui.label("Latitude (Y)");
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.b).desired_width(200.0));
                    ui.end_row();
                    ui.label("Scale 1:");
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.c).hint_text("keep current").desired_width(200.0));
                    ui.end_row();
                });
                close = buttons(app, ui, "Go", |app| {
                    let mut p = json!({"lon": app.dialogs.a.trim(), "lat": app.dialogs.b.trim()});
                    if let Ok(s) = app.dialogs.c.trim().replace(',', "").parse::<f64>() {
                        p["scale"] = json!(s);
                    }
                    app.run("map.goToXY", p).map(|_| ())
                });
            }
            "bookmark" => {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).desired_width(f32::INFINITY));
                close = buttons(app, ui, "Create", |app| app.run("bookmark.add", json!({"name": app.dialogs.text})).map(|_| ()));
            }
            "addField" => {
                egui::Grid::new("af").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                    ui.label("Field name");
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.a).desired_width(220.0));
                    ui.end_row();
                    ui.label("Data type");
                    egui::ComboBox::from_id_salt("af_type").selected_text(app.dialogs.b.clone()).show_ui(ui, |ui| {
                        for ty in ["Long", "Double", "Text", "Boolean"] {
                            ui.selectable_value(&mut app.dialogs.b, ty.to_string(), ty);
                        }
                    });
                    ui.end_row();
                });
                close = buttons(app, ui, "Add", |app| app.run("edit.addField", json!({"name": app.dialogs.a, "type": app.dialogs.b})).map(|_| ()));
            }
            "calculateField" => {
                let fields: Vec<String> = app.session.active_layer.and_then(|id| app.session.map().layer(id)).map(|l| l.dataset.fields.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Field");
                    egui::ComboBox::from_id_salt("cf_field").selected_text(app.dialogs.a.clone()).width(220.0).show_ui(ui, |ui| {
                        for f in &fields {
                            ui.selectable_value(&mut app.dialogs.a, f.clone(), f);
                        }
                    });
                });
                ui.label(format!("{} =", app.dialogs.a));
                ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).hint_text("e.g.  pop_est / 1000000   or   'Africa'   or   name").desired_width(f32::INFINITY));
                ui.label(egui::RichText::new("A value, a field, or two of them joined by + − × ÷. Applies to the selected rows, or every row when none is selected.").small().weak());
                close = buttons(app, ui, "Calculate", |app| app.run("edit.calculateField", json!({"field": app.dialogs.a, "expression": app.dialogs.text})).map(|_| ()));
            }
            "definitionQuery" => {
                ui.label(egui::RichText::new(format!("Only features matching this expression draw in {}.", app.dialogs.layer)).weak());
                ui.add(egui::TextEdit::multiline(&mut app.dialogs.text).hint_text("e.g. continent = 'Europe'").desired_rows(3).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                field_chips(app, ui);
                close = buttons(app, ui, "Apply", |app| app.run("layer.definitionQuery", json!({"where": app.dialogs.text})).map(|_| ()));
            }
            _ => {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).desired_width(f32::INFINITY));
                close = buttons(app, ui, "Rename", |app| app.run("layer.rename", json!({"name": app.dialogs.text})).map(|_| ()));
            }
        }
        if let Some(e) = &app.dialogs.error {
            ui.label(egui::RichText::new(e).color(t.red));
        }
        if let Some(i) = &app.dialogs.info {
            ui.label(egui::RichText::new(i).color(t.green));
        }
    });
    if !open || close {
        app.dialogs.open = None;
    }
}

/// OK/Cancel row; returns true when the dialog should close.
fn buttons(app: &mut RustGisApp, ui: &mut egui::Ui, ok: &str, f: impl FnOnce(&mut RustGisApp) -> Result<(), String>) -> bool {
    let mut close = false;
    ui.add_space(4.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        let t = Tokens::get(ui.ctx());
        let okb = egui::Button::new(egui::RichText::new(ok).color(egui::Color32::WHITE).font(semibold(12.5)))
            .fill(t.accent)
            .min_size(vec2(84.0, 28.0));
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        if ui.add(okb).clicked() || enter {
            match f(app) {
                Ok(()) => close = true,
                Err(e) => app.dialogs.error = Some(e),
            }
        }
        if ui.add(egui::Button::new("Cancel").min_size(vec2(84.0, 28.0))).clicked() {
            close = true;
        }
    });
    close
}

fn field_chips(app: &mut RustGisApp, ui: &mut egui::Ui) {
    let fields: Vec<String> = app
        .session
        .map()
        .layer_by_name(&app.dialogs.layer)
        .map(|l| l.dataset.fields.iter().map(|f| f.name.clone()).collect())
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Fields:").small().weak());
        for f in fields.iter().take(24) {
            if ui.small_button(f).clicked() {
                if !app.dialogs.text.is_empty() && !app.dialogs.text.ends_with(' ') {
                    app.dialogs.text.push(' ');
                }
                app.dialogs.text.push_str(f);
                app.dialogs.text.push(' ');
            }
        }
    });
    ui.horizontal(|ui| {
        for op in ["=", "<>", ">", "<", "LIKE", "AND", "OR", "IS NULL"] {
            if ui.small_button(op).clicked() {
                app.dialogs.text.push_str(&format!("{op} "));
            }
        }
    });
}

fn select_by_attributes(app: &mut RustGisApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let layers: Vec<String> = app.session.map().layers.iter().map(|l| l.name.clone()).collect();
    egui::Grid::new("sba").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label("Input Rows");
        egui::ComboBox::from_id_salt("sba_layer").selected_text(app.dialogs.layer.clone()).width(260.0).show_ui(ui, |ui| {
            for l in &layers {
                ui.selectable_value(&mut app.dialogs.layer, l.clone(), l);
            }
        });
        ui.end_row();
        ui.label("Selection Type");
        egui::ComboBox::from_id_salt("sba_mode")
            .selected_text(match app.dialogs.mode {
                "add" => "Add To Current Selection",
                "remove" => "Remove From Current Selection",
                "subset" => "Select Subset From Current Selection",
                _ => "New Selection",
            })
            .width(260.0)
            .show_ui(ui, |ui| {
                for (m, l) in [
                    ("new", "New Selection"),
                    ("add", "Add To Current Selection"),
                    ("remove", "Remove From Current Selection"),
                    ("subset", "Select Subset From Current Selection"),
                ] {
                    ui.selectable_value(&mut app.dialogs.mode, m, l);
                }
            });
        ui.end_row();
    });
    ui.label(egui::RichText::new("Where").font(semibold(12.5)));
    ui.add(
        egui::TextEdit::multiline(&mut app.dialogs.text)
            .hint_text("e.g. pop_est > 100000000 AND continent = 'Asia'")
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    field_chips(app, ui);
    // Live validation.
    let check =
        app.session.map().layer_by_name(&app.dialogs.layer).map(|l| rustgis_engine::data::query::select(&app.dialogs.text, &l.dataset));
    match (&check, app.dialogs.text.trim().is_empty()) {
        (_, true) => {}
        (Some(Ok(rows)), _) => {
            ui.label(egui::RichText::new(format!("Valid expression · {} matching rows", rows.len())).small().color(t.green));
        }
        (Some(Err(e)), _) => {
            ui.label(egui::RichText::new(format!("Invalid: {e}")).small().color(t.red));
        }
        _ => {}
    }
    buttons(app, ui, "Apply", |app| {
        let layer = app.dialogs.layer.clone();
        app.run("select.byAttributes", json!({"layer": layer, "where": app.dialogs.text, "mode": app.dialogs.mode})).map(|_| ())
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn pick_open(exts: &[&str], label: &str) -> Option<String> {
    rfd::FileDialog::new().add_filter(label, exts).add_filter("All files", &["*"]).pick_file().map(|p| p.display().to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn pick_save(name: &str, exts: &[&str], label: &str) -> Option<String> {
    rfd::FileDialog::new().set_file_name(name).add_filter(label, exts).save_file().map(|p| p.display().to_string())
}

#[cfg(target_arch = "wasm32")]
pub fn pick_open(_: &[&str], _: &str) -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn pick_save(_: &str, _: &[&str], _: &str) -> Option<String> {
    None
}

#[allow(dead_code)]
fn unused(_: Value) {}
