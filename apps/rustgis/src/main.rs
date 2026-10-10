//! RustGIS desktop app.
//!
//! `rustgis [project.rgisx | data.geojson|csv|shp …] [--sample]`
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use rustgis_engine::{Project, Session};
use rustgis_ui_egui::RustGisApp;
use serde_json::json;

struct App(RustGisApp);

/// Preferences in `prefs.json` beside the platform's other app settings.
mod prefs {
    use std::path::PathBuf;

    use rustgis_ui_egui::Services;
    use serde_json::{Map, Value};

    fn dir() -> Option<PathBuf> {
        if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("RustGIS"))
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/RustGIS"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|d| d.join("rustgis"))
        }
    }

    fn read() -> Map<String, Value> {
        dir()
            .and_then(|d| std::fs::read_to_string(d.join("prefs.json")).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    pub fn services() -> Services {
        Services {
            load_pref: Some(Box::new(|key: &str| read().get(key).and_then(Value::as_str).map(str::to_string))),
            save_pref: Some(Box::new(|key: &str, value: &str| {
                let Some(d) = dir() else { return };
                let mut map = read();
                map.insert(key.to_string(), Value::String(value.to_string()));
                let text = Value::Object(map).to_string();
                if let Err(e) = std::fs::create_dir_all(&d).and_then(|_| std::fs::write(d.join("prefs.json"), text)) {
                    log::warn!("could not save preferences: {e}");
                }
            })),
            ..Services::default()
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
        if self.0.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("rustgis {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let app = if args.iter().any(|a| a == "--sample") || files.is_empty() {
        RustGisApp::with_sample()
    } else {
        RustGisApp::new(Session::new(Project::default()))
    };
    let mut app = app.with_services(prefs::services());
    for f in files {
        let cmd = if f.to_ascii_lowercase().ends_with(".rgisx") { "project.open" } else { "layer.addData" };
        if let Err(e) = app.run(cmd, json!({"path": f})) {
            eprintln!("rustgis: {f}: {e}");
        }
    }
    app.session.dirty = false;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("RustGIS")
        .with_app_id("org.rustgis.RustGIS")
        .with_inner_size([1480.0, 920.0])
        .with_min_inner_size([900.0, 560.0])
        .with_drag_and_drop(true);
    if let Some((size, rgba)) = rustgis_engine::render::logo::rgba(256) {
        viewport = viewport.with_icon(egui::IconData { rgba, width: size, height: size });
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("RustGIS", options, Box::new(move |_cc| Ok(Box::new(App(app)))))
}
