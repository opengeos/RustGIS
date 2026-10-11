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
mod layoutview;
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

/// A result an asynchronous service fills in when it's done (the app polls it each frame).
pub type Slot<T> = std::sync::Arc<std::sync::Mutex<Option<T>>>;

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
    /// The browser's GeoLibre runner: lists the tools' manifests (a JSON array) into the slot.
    pub geolibre_manifests: Option<Box<dyn Fn(Slot<Result<String, String>>)>>,
    /// …and runs one tool in its in-memory `/work` folder, putting the new files and printed
    /// lines in the slot. The desktop app runs tools through the `geolibre` program instead.
    pub geolibre_run: Option<Box<dyn Fn(rustgis_engine::cmd::gp::ExternalRun, Slot<rustgis_engine::cmd::gp::ExternalOutput>)>>,
}

/// A new private folder for one print job: unique, created fresh (never an existing path) and,
/// on Unix, readable only by this user. Earlier jobs' folders from this run are removed first.
#[cfg(not(target_arch = "wasm32"))]
fn print_dir() -> Result<std::path::PathBuf, String> {
    let tmp = std::env::temp_dir();
    let prefix = format!("rustgis-print-{}-", std::process::id());
    if let Ok(rd) = std::fs::read_dir(&tmp) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) && e.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = tmp.join(format!("{prefix}{nanos}"));
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(target_arch = "wasm32")]
fn print_dir() -> Result<std::path::PathBuf, String> {
    Err("printing needs the desktop app".into())
}

/// Opens a file with the system's default application (native only).
#[cfg(not(target_arch = "wasm32"))]
fn open_with_system(path: &std::path::Path) -> Result<(), String> {
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(path).spawn().map(|_| ()).map_err(|e| format!("could not open {}: {e}", path.display()))
}

#[cfg(target_arch = "wasm32")]
fn open_with_system(_: &std::path::Path) -> Result<(), String> {
    Err("printing needs the desktop app".into())
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
    /// A layout picture waiting for its image file (web).
    pub pending_layout_picture: bool,
    /// The corners of a picture graphic waiting for its image file.
    pub pending_picture: Option<Vec<rustgis_engine::geom::Coord>>,
    /// Text to put on the system clipboard at the end of the frame.
    pub copy_out: Option<String>,
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
            copy_out: None,
            pending_picture: None,
            pending_layout_picture: false,
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
    pub layout: layoutview::LayoutView,
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
            layout: layoutview::LayoutView::default(),
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
        // The browser's GeoLibre tools, loaded in an earlier visit: load them again (the runner
        // comes from the browser's cache).
        if self.services.load_pref.as_ref().and_then(|load| load("geolibre_web")).as_deref() == Some("on") {
            panes::start_geolibre_load(&mut self);
        }
        // A GeoLibre runner located earlier (Locate… in the Geoprocessing pane).
        if let Some(path) = self.services.load_pref.as_ref().and_then(|load| load("geolibre_runner")).filter(|p| !p.is_empty())
            && let Err(e) = self.session.run("gp.setGeolibre", &json!({"path": path}))
        {
            log::warn!("{e}");
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

    /// Adds the waiting picture graphic with its image (`path` or base64 `image`).
    fn add_picture(&mut self, mut p: Value) -> Result<Value, String> {
        let pts = self.ui.pending_picture.take().ok_or("drag the picture's rectangle on the map first")?;
        p["shape"] = json!("picture");
        p["points"] = json!(pts.iter().map(|c| [c.x, c.y]).collect::<Vec<_>>());
        let r = self.run("graphics.add", p);
        if r.is_ok() && !self.session.keep_tool {
            self.session.tool = rustgis_engine::Tool::Explore;
        }
        r
    }

    /// A layout as PDF or PNG bytes, guarded like `Session::run`.
    fn layout_bytes(&mut self, li: usize, pdf: bool, dpi: f64) -> Result<Vec<u8>, String> {
        let session = &mut self.session;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rustgis_engine::cmd::layout::layout_bytes(session, li, pdf, dpi, true)
        }))
        .unwrap_or_else(|_| Err("the layout export failed unexpectedly".into()));
        if let Err(e) = &r {
            self.session.notify(e.clone(), true);
        }
        r
    }

    /// Opens a file by name and contents: a `.rgisx` project, a raster, or vector data.
    /// Failures are reported to the user (command errors through `Session::run`, others here).
    pub fn open_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<Value, String> {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".rgislayout") {
            let text = std::str::from_utf8(bytes).map_err(|_| "not a RustGIS layout".to_string())?;
            return self.run("layout.import", json!({"text": text}));
        }
        if self.ui.pending_layout_picture && [".png", ".jpg", ".jpeg"].iter().any(|e| lower.ends_with(e)) {
            use base64::Engine;
            self.ui.pending_layout_picture = false;
            let image = base64::engine::general_purpose::STANDARD.encode(bytes);
            return self.run("layout.addElement", json!({"kind": "picture", "image": image}));
        }
        if self.ui.pending_picture.is_some() && [".png", ".jpg", ".jpeg"].iter().any(|e| lower.ends_with(e)) {
            use base64::Engine;
            return self.add_picture(json!({"image": base64::engine::general_purpose::STANDARD.encode(bytes)}));
        }
        if rustgis_engine::data::raster::is_raster_name(name) {
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            return self.run("layer.addDataBytes", json!({"fileName": name, "data": data}));
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            let e = crate::i18n::fmt("{name} is not a text file: RustGIS opens GeoJSON, CSV, GeoTIFF and .rgisx here", &[("name", name)]);
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
            // What a click with Explore does: identify there and show the popup.
            "ui.identify" => {
                // The engine checks the coordinates; the popup reuses them.
                let (lon, lat) = (p.get("lon").cloned().unwrap_or(Value::Null), p.get("lat").cloned().unwrap_or(Value::Null));
                let v = self.session.run("identify", &json!({"lon": lon, "lat": lat, "tolerancePx": 5}))?;
                if let (false, Some(lon), Some(lat)) = (v.is_null(), lon.as_f64(), lat.as_f64()) {
                    self.map.popup = Some(mapview::Popup { info: v, at: rustgis_engine::geom::Coord::new(lon, lat), index: 0 });
                } else {
                    self.map.popup = None;
                }
            }
            "ui.shortcuts" => self.ui.shortcuts = true,
            // Copy, cut and copy path also put their text on the system clipboard.
            "ui.copy" | "ui.cut" | "ui.copyPath" => {
                let cmd = match id {
                    "ui.copy" => "edit.copy",
                    "ui.cut" => "edit.cut",
                    _ => "layer.copyPath",
                };
                let v = self.run(cmd, p.clone())?;
                if let Some(t) = v.get("text").and_then(Value::as_str) {
                    self.ui.copy_out = Some(t.to_string());
                }
                if id == "ui.copyPath" {
                    self.session
                        .notify(i18n::fmt("Copied {path}", &[("path", v.get("text").and_then(Value::as_str).unwrap_or(""))]), false);
                }
                return Ok(v);
            }
            "ui.backstage" => self.ui.backstage = p.get("open").and_then(Value::as_bool).unwrap_or(!self.ui.backstage),
            "ui.tab" => {
                if let Some(t) = p.get("tab").and_then(Value::as_str) {
                    self.ui.tab = t.to_string();
                }
            }
            "ui.addData" => {
                if let Some(open) = &self.services.open_async {
                    open("data");
                } else if let Some(path) =
                    dialogs::pick_open(&["geojson", "json", "csv", "shp", "tif", "tiff", "asc"], tl!("Vector and raster data"))
                {
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
            // Layouts: export (PNG/PDF), print, save and import layout files, draw frames.
            "ui.exportLayout" | "ui.printLayout" => {
                let li = self.session.active_layout.ok_or("open a layout first")?;
                let name = self.session.project.layouts.get(li).map(|l| l.name.clone()).unwrap_or_default();
                let print = id == "ui.printLayout";
                let pdf_only = print || p.get("format").and_then(Value::as_str) == Some("pdf");
                if self.services.download.is_some() {
                    let bytes = self.layout_bytes(li, true, if print { 300.0 } else { 150.0 })?;
                    self.download(&format!("{name}.pdf"), bytes);
                } else if print {
                    // Printing goes through the system's PDF viewer, which owns the printers.
                    let bytes = self.layout_bytes(li, true, 300.0)?;
                    let safe: String =
                        name.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') { c } else { '_' }).collect();
                    let path = print_dir()?.join(format!("{safe}.pdf"));
                    rustgis_engine::data::io::write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
                    open_with_system(&path)?;
                    self.session.notify(i18n::fmt("Opened {name} for printing", &[("name", &path.display().to_string())]), false);
                } else {
                    let exts: &[&str] = if pdf_only { &["pdf"] } else { &["pdf", "png"] };
                    if let Some(path) = dialogs::pick_save(&format!("{name}.pdf"), exts, tl!("PDF or PNG")) {
                        return self.run("layout.export", json!({"layout": li, "path": path, "dpi": 200}));
                    }
                }
            }
            "ui.saveLayout" => {
                let li = self.session.active_layout.ok_or("open a layout first")?;
                let l = self.session.project.layouts.get(li).cloned().ok_or("open a layout first")?;
                if self.services.download.is_some() {
                    let text = serde_json::to_string_pretty(&l).map_err(|e| e.to_string())?;
                    self.download(&format!("{}.rgislayout", l.name), text.into_bytes());
                } else if let Some(path) = dialogs::pick_save(&format!("{}.rgislayout", l.name), &["rgislayout"], tl!("RustGIS layout")) {
                    return self.run("layout.save", json!({"layout": li, "path": path}));
                }
            }
            "ui.importLayout" => {
                if let Some(open) = &self.services.open_async {
                    open("layout");
                } else if let Some(path) = dialogs::pick_open(&["rgislayout"], tl!("RustGIS layout")) {
                    return self.run("layout.import", json!({"path": path}));
                }
            }
            "ui.layoutPicture" => {
                let li = self.session.active_layout.ok_or("open a layout first")?;
                if let Some(open) = &self.services.open_async {
                    self.ui.pending_layout_picture = true;
                    self.ui.pending_picture = None;
                    open("picture");
                } else if let Some(path) = dialogs::pick_open(&["png", "jpg", "jpeg"], tl!("Pictures")) {
                    let v = self.run("layout.addElement", json!({"layout": li, "kind": "picture", "path": path}))?;
                    self.layout.selected = v["element"].as_u64().map(|v| v as u32);
                }
            }
            "ui.layoutSelect" => self.layout.selected = p.get("element").and_then(Value::as_u64).map(|v| v as u32),
            "ui.layoutTool" => {
                self.layout.tool = match p.get("shape").and_then(Value::as_str) {
                    Some("polygon") => Some("polygon"),
                    Some("lasso") => Some("lasso"),
                    _ => None,
                };
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
            "ui.exportRaster" if self.services.download.is_some() => {
                // The web app: encode once and download.
                match rustgis_engine::cmd::raster::geotiff_bytes(&self.session, &Value::Null) {
                    Ok((name, bytes)) => {
                        self.download(&format!("{name}.tif"), bytes);
                    }
                    Err(e) => {
                        self.session.notify(e.clone(), true);
                        return Err(e);
                    }
                }
            }
            "ui.exportRaster" => {
                let name = self
                    .session
                    .active_layer
                    .and_then(|id| self.session.map().layer(id))
                    .map(|l| format!("{}.tif", l.name))
                    .unwrap_or_default();
                if let Some(path) = dialogs::pick_save(&name, &["tif", "tiff"], tl!("GeoTIFF")) {
                    return self.run("share.exportRaster", json!({"path": path}));
                }
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
                // The text graphic dialog remembers where the map was clicked.
                if which == "graphicText" {
                    self.dialogs.a = p.get("lon").and_then(Value::as_f64).map(|v| v.to_string()).unwrap_or_default();
                    self.dialogs.b = p.get("lat").and_then(Value::as_f64).map(|v| v.to_string()).unwrap_or_default();
                }
            }
            // Picks the image for a picture graphic whose corners were just dragged.
            "ui.addPicture" => {
                if self.ui.pending_picture.is_none() {
                    return Err("drag the picture's rectangle on the map first".into());
                }
                if let Some(open) = &self.services.open_async {
                    self.ui.pending_layout_picture = false;
                    open("picture");
                } else if let Some(path) = dialogs::pick_open(&["png", "jpg", "jpeg"], tl!("Pictures")) {
                    return self.add_picture(json!({"path": path}));
                } else {
                    self.ui.pending_picture = None;
                }
            }
            "ui.quit" => self.quit_requested = true,
            // Buttons that act through their own click handler (menus, layout inserts).
            "ui.menu" => {}
            other => return Err(format!("unknown command '{other}'")),
        }
        Ok(Value::Null)
    }

    /// Per-frame work that doesn't draw (theme, fonts, files that arrived).
    pub fn logic(&mut self, ctx: &egui::Context) {
        i18n::set_current(self.lang());
        panes::poll_geolibre(self, ctx);
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
        if let Some(text) = self.ui.copy_out.take() {
            ctx.copy_text(text);
        }
        let title = format!("{}{} - RustGIS", self.session.project.name, if self.session.dirty { " •" } else { "" });
        if title != self.sent_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.sent_title = title;
        }
    }
}
