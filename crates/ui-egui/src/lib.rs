//! L6: the RustGIS desktop front end, built around a ribbon with
//! contextual tabs, the Contents and Catalog panes, a Geoprocessing pane, the map view, the
//! attribute table and the status bar.
//!
//! The UI is thin: it reads [`Session`] state and runs commands by id ([`RustGisApp::run`]).
//! UI-only commands (`ui.*`: panes, dialogs, theme) are handled here.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[macro_use]
pub mod i18n;
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

/// Files read asynchronously (browser uploads, drops) waiting to be opened: (file name, bytes).
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// Platform hooks. The desktop app leaves them empty and uses native file dialogs; the web app
/// supplies an asynchronous file picker (results arrive in `inbox`) and browser downloads.
#[derive(Default)]
pub struct Services {
    /// Opens a file picker for `"data"` or `"project"`; the chosen file lands in `inbox`.
    pub open_async: Option<Box<dyn Fn(&str)>>,
    /// Hands bytes to the user as a download named `file_name`.
    pub download: Option<Box<dyn Fn(&str, Vec<u8>)>>,
    pub inbox: Option<Inbox>,
    /// Reads a saved preference (`language`, …).
    pub load_pref: Option<Box<dyn Fn(&str) -> Option<String>>>,
    /// Saves a preference.
    pub save_pref: Option<Box<dyn Fn(&str, &str)>>,
    /// Downloads a font by URL (web); the bytes land in `font_inbox`.
    pub fetch_font: Option<Box<dyn Fn(&str)>>,
    pub font_inbox: Option<Inbox>,
}

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
    /// Interface language preference: a language code or `auto`.
    pub language: String,
    /// The ribbon shows only its tabs; clicking a tab opens it until the next click elsewhere.
    pub ribbon_minimized: bool,
    /// A minimized ribbon temporarily opened from its tab strip.
    pub ribbon_peek: bool,
    pub command_search: Option<String>,
    pub about: bool,
    pub shortcuts: bool,
    /// Tab rectangles from the last frame (for the contextual group header).
    pub tab_rects: Vec<(String, egui::Rect)>,
    pub tab_strip_rect: Option<egui::Rect>,
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
            language: i18n::AUTO.into(),
            ribbon_minimized: false,
            ribbon_peek: false,
            command_search: None,
            about: false,
            shortcuts: false,
            tab_rects: Vec::new(),
            tab_strip_rect: None,
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
    pub services: Services,
    /// Extra font faces installed with the interface fonts: (name, face, region). The current
    /// language's region comes right after the interface font, so its glyph shapes win; the others
    /// are last-resort fallbacks. Faces are shared, so reinstalling copies nothing.
    extra_fonts: Vec<(String, std::sync::Arc<egui::FontData>, Option<rustgis_engine::render::fonts::CjkRegion>)>,
    /// The CJK region whose face was put first at the last font install.
    installed_region: Option<rustgis_engine::render::fonts::CjkRegion>,
    fonts_dirty: bool,
    fonts_requested: Vec<String>,
    /// Downloads that failed; cleared when a language is picked, which retries them.
    fonts_failed: Vec<String>,
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
            services: Services::default(),
            extra_fonts: Vec::new(),
            installed_region: None,
            fonts_dirty: false,
            fonts_requested: Vec::new(),
            fonts_failed: Vec::new(),
        }
    }

    pub fn with_services(mut self, services: Services) -> Self {
        self.services = services;
        if let Some(lang) = self.services.load_pref.as_ref().and_then(|load| load("language")) {
            self.ui.language = lang;
        }
        if let Some(m) = self.services.load_pref.as_ref().and_then(|load| load("ribbon_minimized")) {
            self.ui.ribbon_minimized = m == "true";
        }
        self
    }

    /// The language the interface is drawn in now.
    pub fn lang(&self) -> i18n::Lang {
        i18n::Lang::from_pref(&self.ui.language)
    }

    /// On the desktop, an installed CJK face as a last-resort fallback, so CJK text (language names
    /// in the picker, place names in data) renders whatever the interface language. The bytes are
    /// borrowed from the font cache, not copied.
    fn add_cjk_fallback(&mut self) {
        use rustgis_engine::render::fonts::{CjkRegion, cjk_font};
        if self.extra_fonts.iter().any(|(n, _, _)| n == "cjk-fallback") {
            return;
        }
        let found = [CjkRegion::SimplifiedChinese, CjkRegion::Japanese, CjkRegion::Korean].into_iter().find_map(cjk_font);
        if let Some((bytes, index)) = found {
            let mut face = egui::FontData::from_static(bytes);
            face.index = *index;
            self.extra_fonts.push(("cjk-fallback".into(), std::sync::Arc::new(face), None));
        }
    }

    /// The CJK region of the current language, if any.
    fn current_region(&self) -> Option<rustgis_engine::render::fonts::CjkRegion> {
        Self::script_region(self.lang().script())
    }

    /// The CJK font region a script needs, if any.
    fn script_region(script: i18n::Script) -> Option<rustgis_engine::render::fonts::CjkRegion> {
        use rustgis_engine::render::fonts::CjkRegion;
        match script {
            i18n::Script::Cjk(i18n::CjkFont::SimplifiedChinese) => Some(CjkRegion::SimplifiedChinese),
            i18n::Script::Cjk(i18n::CjkFont::Japanese) => Some(CjkRegion::Japanese),
            i18n::Script::Cjk(i18n::CjkFont::Korean) => Some(CjkRegion::Korean),
            i18n::Script::Alphabetic => None,
        }
    }

    /// Makes sure the current language's script has a font: an installed CJK face on the desktop,
    /// a download on the web.
    fn ensure_script_font(&mut self) {
        use rustgis_engine::render::fonts::{cjk_font, cjk_font_url};
        // Switching between CJK languages reorders the faces already loaded.
        if self.current_region() != self.installed_region {
            self.fonts_dirty = true;
        }
        let Some(region) = self.current_region() else { return };
        let name = format!("cjk-{region:?}");
        if self.extra_fonts.iter().any(|(n, _, _)| *n == name) || self.fonts_requested.contains(&name) || self.fonts_failed.contains(&name)
        {
            return;
        }
        self.fonts_requested.push(name.clone());
        if let Some((bytes, index)) = cjk_font(region) {
            let mut face = egui::FontData::from_static(bytes);
            face.index = *index;
            self.extra_fonts.push((name, std::sync::Arc::new(face), Some(region)));
            self.fonts_dirty = true;
        } else if let Some(fetch) = &self.services.fetch_font {
            fetch(cjk_font_url(region));
        } else {
            log::warn!("no {region:?} font installed; text in that script may not display");
        }
    }

    /// Opens a file by name and contents: a `.rgisx` project, or vector data.
    /// Failures are reported to the user (command errors through `Session::run`, others here).
    pub fn open_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<Value, String> {
        let Ok(text) = std::str::from_utf8(bytes) else {
            let e = crate::i18n::fmt("{name} is not a text file: RustGIS opens GeoJSON, CSV and .rgisx here", &[("name", name)]);
            self.session.notify(e.clone(), true);
            return Err(e);
        };
        if name.to_ascii_lowercase().ends_with(".rgisx") {
            let r = self.run("project.openText", json!({"text": text, "name": name}));
            self.map.invalidate();
            r
        } else {
            self.run("layer.addDataText", json!({"fileName": name, "text": text}))
        }
    }

    /// Offers bytes as a download (web) and reports whether a download hook exists.
    fn download(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        match &self.services.download {
            Some(d) => {
                d(name, bytes);
                self.session.notify(crate::i18n::fmt("Downloaded {name}", &[("name", name)]), false);
                true
            }
            None => false,
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
        if !matches!(id, "ui.menu" | "ui.ribbon" | "ui.language") {
            self.ui.ribbon_peek = false;
        }
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
            "ui.language" => {
                let code = p.get("code").and_then(Value::as_str).unwrap_or(i18n::AUTO);
                if code != i18n::AUTO && i18n::Lang::from_code(code).is_none() {
                    return Err(format!("unknown language '{code}'"));
                }
                self.ui.language = code.to_ascii_lowercase();
                self.fonts_failed.clear();
                if let Some(save) = &self.services.save_pref {
                    save("language", &self.ui.language);
                }
                self.map.invalidate();
            }
            "ui.ribbon" => {
                self.ui.ribbon_minimized = p.get("minimized").and_then(Value::as_bool).unwrap_or(!self.ui.ribbon_minimized);
                self.ui.ribbon_peek = false;
                if let Some(save) = &self.services.save_pref {
                    save("ribbon_minimized", if self.ui.ribbon_minimized { "true" } else { "false" });
                }
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
                if let Some(open) = &self.services.open_async {
                    open("data");
                } else if let Some(path) = dialogs::pick_open(&["geojson", "json", "csv", "shp"], tl!("Vector data")) {
                    return self.run("layer.addData", json!({"path": path}));
                }
            }
            "ui.open" => {
                if let Some(open) = &self.services.open_async {
                    open("project");
                    self.ui.backstage = false;
                } else if let Some(path) = dialogs::pick_open(&["rgisx"], tl!("RustGIS project")) {
                    let r = self.run("project.open", json!({"path": path}));
                    self.ui.backstage = false;
                    self.map.invalidate();
                    return r;
                }
            }
            "ui.save" | "ui.saveAs" if self.services.download.is_some() => {
                let text = serde_json::to_string(&self.session.project).map_err(|e| e.to_string())?;
                let name = format!("{}.rgisx", self.session.project.name);
                self.download(&name, text.into_bytes());
                self.session.dirty = false;
            }
            "ui.save" => {
                if self.session.path.is_some() {
                    return self.run("project.save", Value::Null);
                }
                return self.run("ui.saveAs", Value::Null);
            }
            "ui.saveAs" => {
                let name = format!("{}.rgisx", self.session.project.name);
                if let Some(path) = dialogs::pick_save(&name, &["rgisx"], tl!("RustGIS project")) {
                    return self.run("project.saveAs", json!({"path": path}));
                }
            }
            "ui.exportMap" if self.services.download.is_some() => {
                // The same last-resort panic guard `Session::run` gives commands.
                let session = &mut self.session;
                let png = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    rustgis_engine::cmd::share::map_png(session, None, None, Some(192.0))
                }))
                .unwrap_or_else(|_| Err("Export Map failed unexpectedly".into()));
                let png = match png {
                    Ok(p) => p,
                    Err(e) => {
                        self.session.notify(e.clone(), true);
                        return Err(e);
                    }
                };
                let name = format!("{}.png", self.session.map().name);
                self.download(&name, png);
            }
            "ui.exportLayer" if self.services.download.is_some() => {
                let l = self
                    .session
                    .active_layer
                    .and_then(|id| self.session.map().layer(id))
                    .ok_or("select a layer in the Contents pane first")?;
                let text = rustgis_engine::data::io::to_geojson(&l.dataset).to_string();
                let name = format!("{}.geojson", l.name);
                self.download(&name, text.into_bytes());
            }
            "ui.exportMap" => {
                let name = format!("{}.png", self.session.map().name);
                if let Some(path) = dialogs::pick_save(&name, &["png"], tl!("PNG image")) {
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
                if let Some(path) = dialogs::pick_save(&name, &["geojson", "csv"], tl!("GeoJSON or CSV")) {
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

    /// Per-frame work that doesn't draw (theme, fonts, files that arrived).
    pub fn logic(&mut self, ctx: &egui::Context) {
        i18n::set_current(self.lang());
        self.ensure_script_font();
        // Fonts downloaded by the web app.
        let fonts: Vec<(String, Vec<u8>)> = match &self.services.font_inbox {
            Some(inbox) => std::mem::take(&mut *inbox.lock().unwrap_or_else(|e| e.into_inner())),
            None => Vec::new(),
        };
        for (url, bytes) in fonts {
            use rustgis_engine::render::fonts::{CjkRegion, cjk_font_url, valid_font};
            let region =
                [CjkRegion::SimplifiedChinese, CjkRegion::Japanese, CjkRegion::Korean].into_iter().find(|r| cjk_font_url(*r) == url);
            if bytes.is_empty() || !valid_font(&bytes, 0) {
                // A failed download: say so; picking the language again retries.
                log::error!("{url}: no usable font downloaded");
                // Name the language whose font failed (not the current one), in English: its own
                // script has no font yet.
                let language =
                    i18n::Lang::all().find(|l| region.is_some_and(|r| Self::script_region(l.script()) == Some(r))).unwrap_or(self.lang());
                let msg = i18n::fmt(
                    "Couldn't download the font for {language}; pick the language again to retry.",
                    &[("language", language.english())],
                );
                self.session.notify(msg, true);
                if let Some(r) = region {
                    let name = format!("cjk-{r:?}");
                    self.fonts_requested.retain(|n| *n != name);
                    self.fonts_failed.push(name);
                }
                continue;
            }
            let name = format!("web-{}", url.rsplit('/').next().unwrap_or("font"));
            self.extra_fonts.retain(|(n, _, _)| *n != name);
            self.extra_fonts.push((name, std::sync::Arc::new(egui::FontData::from_owned(bytes)), region));
            self.fonts_dirty = true;
        }
        if self.fonts_dirty {
            self.fonts_dirty = false;
            self.installed_region = self.current_region();
            theme::install_fonts(ctx, &self.extra_fonts, self.installed_region);
            self.map.invalidate();
        }
        // Files from the async picker or browser drops.
        let arrived: Vec<(String, Vec<u8>)> = match &self.services.inbox {
            Some(inbox) => std::mem::take(&mut *inbox.lock().unwrap_or_else(|e| e.into_inner())),
            None => Vec::new(),
        };
        for (name, bytes) in arrived {
            let _ = self.open_bytes(&name, &bytes);
        }
        // Files dropped on the desktop window arrive as paths. (The web shell reads browser drops
        // asynchronously and passes them through the inbox instead.)
        if !cfg!(target_arch = "wasm32") {
            let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
            for f in dropped {
                let p = f.path().display().to_string();
                if p.is_empty() {
                    continue;
                }
                let cmd = if p.to_ascii_lowercase().ends_with(".rgisx") { "project.open" } else { "layer.addData" };
                let _ = self.run(cmd, json!({"path": p}));
                self.map.invalidate();
            }
        }
        if self.fonts_frames == 0 {
            self.add_cjk_fallback();
            self.installed_region = self.current_region();
            theme::install_fonts(ctx, &self.extra_fonts, self.installed_region);
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
        i18n::set_current(self.lang());
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
