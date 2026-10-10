//! RustGIS desktop app.
//!
//! `rustgis [project.rgisx | data.geojson|csv|shp …] [--sample]`
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use rustgis_engine::{Project, Session};
use rustgis_ui_egui::RustGisApp;
use serde_json::json;

struct App(RustGisApp);

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
    let mut app = if args.iter().any(|a| a == "--sample") || files.is_empty() {
        RustGisApp::with_sample()
    } else {
        RustGisApp::new(Session::new(Project::default()))
    };
    for f in files {
        let cmd = if f.to_ascii_lowercase().ends_with(".rgisx") { "project.open" } else { "layer.addData" };
        if let Err(e) = app.run(cmd, json!({"path": f})) {
            eprintln!("rustgis: {f}: {e}");
        }
    }
    app.session.dirty = false;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustGIS")
            .with_app_id("org.rustgis.RustGIS")
            .with_inner_size([1480.0, 920.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("RustGIS", options, Box::new(move |_cc| Ok(Box::new(App(app)))))
}
