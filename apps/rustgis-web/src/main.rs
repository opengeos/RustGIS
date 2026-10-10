//! RustGIS in the browser.
//!
//! Runs the same [`rustgis_ui_egui::RustGisApp`] as the desktop app through eframe's web runner
//! (WebGPU where available, WebGL2 otherwise). Build with `trunk build --release` from this
//! directory (output in `dist/web`).
//!
//! Differences from the desktop app: Open and Add Data use the browser's file picker (GeoJSON,
//! CSV and `.rgisx`; Shapefiles need the desktop app), files can be dropped on the page, and
//! Save and Export download. URL flags: `?webgl` forces WebGL2; `?blank` starts an empty project;
//! `?lang=de` sets the interface language.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("rustgis-web only runs in the browser: build it with `trunk build --release` in apps/rustgis-web");
}
