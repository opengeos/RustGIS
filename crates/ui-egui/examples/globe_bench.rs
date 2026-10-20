//! UI-thread pan/zoom benchmark of the globe with real tiles (needs the network): a vector
//! tile style on the Earth and a planetary basemap on the Moon, while tiles arrive and after.
//! `cargo run --release -p rustgis-ui-egui --example globe_bench -- [limit ms]`.
use rustgis_ui_egui::RustGisApp;
use rustgis_ui_egui::rustgis_engine::geom::{Coord, lonlat_to_merc};
use serde_json::json;

type H<'a> = egui_kittest::Harness<'a, RustGisApp>;

/// Steps until the map is drawn and every tile has arrived; false if that took over a minute.
fn settle(h: &mut H<'_>) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    for _ in 0..40 {
        h.step();
    }
    while (h.state().map.drawing() || rustgis_ui_egui::rustgis_engine::render::rustgis_tiles::cache::in_flight() > 0)
        && std::time::Instant::now() < end
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    !(h.state().map.drawing() || rustgis_ui_egui::rustgis_engine::render::rustgis_tiles::cache::in_flight() > 0)
}

/// Drags and zooms, returning each frame's time (ms).
fn navigate(h: &mut H<'_>) -> Vec<f64> {
    let mut ms = Vec::new();
    let mut timed = |h: &mut H<'_>| {
        let start = std::time::Instant::now();
        h.step();
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
        // Real time passes between frames, as tiles arrive.
        std::thread::sleep(std::time::Duration::from_millis(8));
    };
    let from = egui::pos2(850.0, 550.0);
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.step();
    for i in 1..=120 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + egui::vec2(i as f32 * 3.0, i as f32 * 0.5)));
        timed(h);
    }
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: from + egui::vec2(360.0, 60.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    for _ in 0..90 {
        h.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 6.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        });
        timed(h);
    }
    ms
}

fn report(name: &str, mut ms: Vec<f64>) -> f64 {
    let total: f64 = ms.iter().sum();
    let slow = ms.iter().filter(|&&t| t > 16.7).count();
    ms.sort_by(f64::total_cmp);
    let max = ms.last().copied().unwrap_or(0.0);
    println!(
        "{name:28} mean {:6.2}  p95 {:6.2}  max {max:7.2} ms/frame; {slow} of {} frames over 16.7 ms",
        total / ms.len() as f64,
        ms.get(ms.len() * 95 / 100).copied().unwrap_or(0.0),
        ms.len()
    );
    max
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let setup = egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew::without_display_handle());
    let instance = pollster::block_on(setup.new_instance());
    let rs = pollster::block_on(egui_wgpu::RenderState::create(
        &egui_wgpu::WgpuConfiguration { wgpu_setup: setup, ..Default::default() },
        &instance,
        None,
        egui_wgpu::RendererOptions::PREDICTABLE,
    ))?;
    let filter = std::env::var("BENCH").unwrap_or_default();
    let mut worst: f64 = 0.0;
    for (name, body, lon, lat, res) in [("earth-vector", "earth", 10.0, 45.0, 8_000.0), ("moon", "moon", 15.0, 25.0, 20_000.0)] {
        if !filter.is_empty() && !name.contains(&filter) {
            continue;
        }
        for gpu in [false, true] {
            let mut app = RustGisApp::with_sample();
            app.set_render_state(Some(rs.clone()));
            app.run("map.body", json!({"name": body}))?;
            app.run("map.projection", json!({"type": "globe"}))?;
            app.run("ui.map.gpu", json!({"on": gpu}))?;
            app.session.map_mut().layers.clear();
            if body == "earth" {
                app.run(
                    "map.addTiles",
                    json!({"url": "https://tiles.openfreemap.org/styles/liberty", "name": "Liberty", "replace": true}),
                )?;
            }
            app.session.map_mut().camera.center = lonlat_to_merc(Coord::new(lon, lat));
            app.session.map_mut().camera.res = res;
            let mut h = egui_kittest::Harness::builder()
                .with_size(egui::vec2(1480.0, 900.0))
                .with_pixels_per_point(2.0)
                .with_step_dt(1.0 / 60.0)
                .with_max_steps(100_000)
                .build_ui_state(
                    |ui, app: &mut RustGisApp| {
                        let ctx = ui.ctx().clone();
                        ctx.input_mut(|i| i.max_texture_side = 8192);
                        app.logic(&ctx);
                        app.ui(ui);
                    },
                    app,
                );
            // Cold: navigate as soon as the map opens, while its tiles arrive.
            for _ in 0..5 {
                h.step();
            }
            worst = worst.max(report(&format!("{name} gpu={gpu} cold"), navigate(&mut h)));
            if !settle(&mut h) {
                return Err(format!("{name}: the map didn't settle within a minute (a slow network?)").into());
            }
            worst = worst.max(report(&format!("{name} gpu={gpu} warm"), navigate(&mut h)));
        }
    }
    if let Some(limit) = std::env::args().nth(1) {
        let limit: f64 = limit.parse()?;
        if worst >= limit {
            return Err(format!("navigation stalls for {worst:.2} ms (limit {limit:.2})").into());
        }
    }
    Ok(())
}
