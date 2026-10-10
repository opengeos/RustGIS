//! L6: the RustGIS desktop front end, built around a ribbon with
//! contextual tabs, the Contents and Catalog panes, a Geoprocessing pane, the map view, the
//! attribute table and the status bar.
//!
//! The UI is thin: it reads [`Session`] state and runs commands by id ([`RustGisApp::run`]).
//! UI-only commands (`ui.*`: panes, dialogs, theme) are handled here.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod backstage;
mod chrome;
mod dialogs;
pub mod icons;
mod mapview;
mod panes;
mod ribbon;
mod table;
pub mod theme;
mod widgets;

use rustgis_engine::{Project, Session};
use serde_json::{Value, json};

pub use rustgis_engine;

/// Panes on the left of the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftPane {
    Contents,
    Catalog,
}

/// Panes on the right of the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightPane {
    Geoprocessing,
    Symbology,
    History,
    Details,
}

pub struct UiState {
    pub tab: String,
    pub backstage: bool,
    pub backstage_page: &'static str,
    pub left: Option<LeftPane>,
    pub right: Option<RightPane>,
    pub dark: bool,
    pub command_search: Option<String>,
    pub about: bool,
    pub shortcuts: bool,
    /// Tab rectangles from the last frame (for the contextual group header).
    pub tab_rects: Vec<(String, egui::Rect)>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            tab: "Map".into(),
            backstage: false,
            backstage_page: "open",
            left: Some(LeftPane::Contents),
            right: None,
            dark: false,
            command_search: None,
            about: false,
            shortcuts: false,
            tab_rects: Vec::new(),
        }
    }
}

pub struct RustGisApp {
    pub session: Session,
    pub ui: UiState,
    pub map: mapview::MapView,
    pub dialogs: dialogs::Dialogs,
    pub gp: panes::GpForm,
    pub table: table::TableState,
    fonts_frames: u32,
    applied_dark: Option<bool>,
    sent_title: String,
    pub quit_requested: bool,
    logo: Option<egui::TextureHandle>,
}

impl RustGisApp {
    pub fn new(session: Session) -> Self {
        let mut s = session;
        if s.active_layer.is_none() {
            s.active_layer = s.map().layers.first().map(|l| l.id);
        }
        RustGisApp {
            session: s,
            ui: UiState::default(),
            map: mapview::MapView::default(),
            dialogs: dialogs::Dialogs::default(),
            gp: panes::GpForm::default(),
            table: table::TableState::default(),
            fonts_frames: 0,
            applied_dark: None,
            sent_title: String::new(),
            quit_requested: false,
            logo: None,
        }
    }

    /// The RustGIS logo as a texture (rasterised once from `rustgis_render::logo`).
    pub fn logo(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.logo.is_none()
            && let Some(pm) = rustgis_engine::render::logo::pixmap(256)
        {
            let img = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
            self.logo = Some(ctx.load_texture("rustgis-logo", img, egui::TextureOptions::LINEAR));
        }
        self.logo.as_ref().map(|t| t.id())
    }

    /// Paints the logo into `rect`.
    pub fn paint_logo(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        if let Some(id) = self.logo(ui.ctx()) {
            ui.painter().image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
    }

    /// The sample "World Atlas" project.
    pub fn with_sample() -> Self {
        let mut s = Session::new(Project::default());
        let _ = s.run("project.sample", &Value::Null);
        s.dirty = false;
        RustGisApp::new(s)
    }

    /// Runs a command by id: `ui.*` here, everything else in the engine.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = if id.starts_with("ui.") { self.run_ui(id, &params) } else { self.session.run(id, &params) };
        if let Err(e) = &r {
            log::info!("{id}: {e}");
        }
        r
    }

    fn run_ui(&mut self, id: &str, p: &Value) -> Result<Value, String> {
        let toggle_right = |ui: &mut UiState, pane: RightPane| ui.right = if ui.right == Some(pane) { None } else { Some(pane) };
        match id {
            "ui.pane.contents" => self.ui.left = if self.ui.left == Some(LeftPane::Contents) { None } else { Some(LeftPane::Contents) },
            "ui.pane.catalog" => self.ui.left = if self.ui.left == Some(LeftPane::Catalog) { None } else { Some(LeftPane::Catalog) },
            "ui.pane.geoprocessing" => toggle_right(&mut self.ui, RightPane::Geoprocessing),
            "ui.pane.symbology" => {
                if self.session.active_layer.is_none() {
                    return Err("select a layer in the Contents pane first".into());
                }
                toggle_right(&mut self.ui, RightPane::Symbology)
            }
            "ui.pane.history" => toggle_right(&mut self.ui, RightPane::History),
            "ui.pane.details" => toggle_right(&mut self.ui, RightPane::Details),
            "ui.gp.open" => {
                let tool = p.get("tool").and_then(Value::as_str).and_then(rustgis_engine::analysis::tool);
                self.gp.open(tool.map(|t| t.id), &self.session);
                self.ui.right = Some(RightPane::Geoprocessing);
            }
            "ui.theme" => self.ui.dark = p.get("dark").and_then(Value::as_bool).unwrap_or(!self.ui.dark),
            "ui.commandSearch" => self.ui.command_search = Some(String::new()),
            "ui.about" => self.ui.about = true,
            "ui.shortcuts" => self.ui.shortcuts = true,
            "ui.backstage" => self.ui.backstage = p.get("open").and_then(Value::as_bool).unwrap_or(!self.ui.backstage),
            "ui.tab" => {
                if let Some(t) = p.get("tab").and_then(Value::as_str) {
                    self.ui.tab = t.to_string();
                }
            }
            "ui.addData" => {
                if let Some(path) = dialogs::pick_open(&["geojson", "json", "csv", "shp"], "Vector data") {
                    return self.run("layer.addData", json!({"path": path}));
                }
            }
            "ui.open" => {
                if let Some(path) = dialogs::pick_open(&["rgisx"], "RustGIS project") {
                    let r = self.run("project.open", json!({"path": path}));
                    self.ui.backstage = false;
                    self.map.invalidate();
                    return r;
                }
            }
            "ui.save" => {
                if self.session.path.is_some() {
                    return self.run("project.save", Value::Null);
                }
                return self.run("ui.saveAs", Value::Null);
            }
            "ui.saveAs" => {
                let name = format!("{}.rgisx", self.session.project.name);
                if let Some(path) = dialogs::pick_save(&name, &["rgisx"], "RustGIS project") {
                    return self.run("project.saveAs", json!({"path": path}));
                }
            }
            "ui.exportMap" => {
                let name = format!("{}.png", self.session.map().name);
                if let Some(path) = dialogs::pick_save(&name, &["png"], "PNG image") {
                    return self.run("share.exportMap", json!({"path": path, "dpi": 192}));
                }
            }
            "ui.exportLayer" => {
                let name = self
                    .session
                    .active_layer
                    .and_then(|id| self.session.map().layer(id))
                    .map(|l| format!("{}.geojson", l.name))
                    .unwrap_or_default();
                if let Some(path) = dialogs::pick_save(&name, &["geojson", "csv"], "GeoJSON or CSV") {
                    return self.run("share.exportLayer", json!({"path": path}));
                }
            }
            "ui.dialog" => {
                let which = p.get("name").and_then(Value::as_str).unwrap_or("");
                self.dialogs.open(which, &self.session)?;
            }
            "ui.quit" => self.quit_requested = true,
            other => return Err(format!("unknown command '{other}'")),
        }
        Ok(Value::Null)
    }

    /// Per-frame work that doesn't draw (theme, fonts).
    pub fn logic(&mut self, ctx: &egui::Context) {
        if self.fonts_frames == 0 {
            theme::install_fonts(ctx);
        }
        if self.applied_dark != Some(self.ui.dark) {
            let t = if self.ui.dark { theme::Tokens::dark() } else { theme::Tokens::light() };
            theme::apply(ctx, &t);
            self.applied_dark = Some(self.ui.dark);
            self.map.invalidate();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Fonts installed by `logic` take effect on the next frame.
        if self.fonts_frames < 2 {
            self.fonts_frames += 1;
            ctx.request_repaint();
            return;
        }
        let t = theme::Tokens::get(&ctx);
        chrome::shortcuts(self, &ctx);
        if self.ui.backstage {
            backstage::show(self, ui);
        } else {
            chrome::header(self, ui);
            ribbon::show(self, ui);
            chrome::status_bar(self, ui);
            panes::show(self, ui);
            table::show(self, ui);
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
                mapview::show(self, ui);
            });
        }
        dialogs::show(self, &ctx);
        chrome::command_search(self, &ctx);
        chrome::about(self, &ctx);
        chrome::notices(self, &ctx);
        let title = format!("{}{} - RustGIS", self.session.project.name, if self.session.dirty { " •" } else { "" });
        if title != self.sent_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.sent_title = title;
        }
    }
}
