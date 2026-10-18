//! The GPU camera fast path must yield one full re-render once the camera settles:
//! the scene's contents depend on the render-time camera (extent culling, the 0.35 px
//! decimation, label placement). Regression test for the PR #309 review (coderabbit +
//! github-actions, both flagging the missing settle render).
//! Planetary tile scenes also reuse uploaded images and preview layer changes immediately.

use rustgis_ui_egui::RustGisApp;
use serde_json::{Value, json};

fn wait_for_frame(h: &mut egui_kittest::Harness<'_, RustGisApp>) {
    for _ in 0..40 {
        h.step();
    }
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().map.drawing() {
        assert!(std::time::Instant::now() < end, "frame worker did not finish");
        std::thread::sleep(std::time::Duration::from_millis(2));
        h.step();
    }
}

fn serve_planet_tiles() {
    use rustgis_ui_egui::rustgis_engine::render::rustgis_tiles;
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_fn(256, 256, |x, y| image::Rgba([100 + (x / 2) as u8, 30 + (y / 8) as u8, 50, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("tile");
    rustgis_tiles::cache::set_fetcher(move |_, done| done(Ok(png.get_ref().clone())));
}

/// A wgpu harness sharing one render state with the app (the ui_shot pattern), so the GPU
/// backend engages. `None` on hosts without an adapter.
fn harness() -> Option<egui_kittest::Harness<'static, RustGisApp>> {
    let setup = egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew::without_display_handle());
    let instance = pollster::block_on(setup.new_instance());
    let rs = pollster::block_on(egui_wgpu::RenderState::create(
        &egui_wgpu::WgpuConfiguration { wgpu_setup: setup, ..Default::default() },
        &instance,
        None,
        egui_wgpu::RendererOptions::PREDICTABLE,
    ))
    .ok()?;
    let mut app = RustGisApp::with_sample();
    app.set_render_state(Some(rs.clone()));
    app.run("ui.map.gpu", json!({"on": true})).expect("gpu on");
    Some(
        egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 550.0))
            .with_pixels_per_point(1.0)
            .with_max_steps(10_000)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs))
            .build_ui_state(
                |ui, app: &mut RustGisApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            ),
    )
}

#[test]
fn settling_after_a_camera_animation_re_renders_fully() {
    let Some(mut h) = harness() else { return };
    for _ in 0..6 {
        h.step();
    }
    let app = h.state_mut();
    app.run("animation.add", Value::Null).expect("keyframe 1");
    app.run("map.setScale", json!({"scale": 20_000_000.0})).expect("zoom keyframe");
    app.run("animation.add", Value::Null).expect("keyframe 2");
    app.run("ui.animationPlay", Value::Null).expect("play");
    // Pump frames until the animation settles (playing is None again), then a few more.
    let mut settled = false;
    for _ in 0..9_000 {
        h.step();
        if h.state().ws.playing.is_none() {
            settled = true;
            break;
        }
    }
    assert!(settled, "the animation finished");
    for _ in 0..5 {
        h.step();
    }
    let app = h.state();
    let (cam, tex_cam) = (app.session.map().camera, app.map.full_frame_camera());
    assert_eq!((tex_cam.center, tex_cam.res), (cam.center, cam.res), "a full re-render followed the settled camera (tex_camera refreshed)");
}

#[test]
fn zooming_a_planet_with_raster_tiles_reuses_the_gpu_frame() {
    use rustgis_ui_egui::rustgis_engine::map::{TileKind, TileLayer};
    serve_planet_tiles();
    let Some(mut h) = harness() else { return };
    h.state_mut().run("map.body", json!({"name": "moon", "basemap": false})).expect("Moon");
    h.state_mut().session.map_mut().layers.clear();
    h.state_mut().session.map_mut().graticule = false;
    h.state_mut().session.map_mut().tiles.push(TileLayer::new("Moon", TileKind::Raster, "https://planet.test/{z}/{x}/{y}.png"));
    wait_for_frame(&mut h);
    assert!(h.output().shapes.iter().any(|s| matches!(s.shape, egui::Shape::Callback(_))), "the GPU backend engaged");
    let before = h.state().map.full_frame_camera();
    for _ in 0..8 {
        h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(600.0, 350.0)));
        h.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 2.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        });
        h.step();
    }
    assert_ne!(h.state().session.map().camera.res, before.res, "zoom engaged");
    assert_eq!(h.state().map.full_frame_camera(), before, "navigation must reuse the uploaded image instead of redrawing every frame");
    // Exercise the actual blit shader as well: a shifted/scaled image must sample the same
    // ground as an exact CPU render, even when its rectangle extends past the window.
    let image = h.render().expect("moving GPU image");
    let rect = h.state().map.view_rect.expect("map rectangle").0;
    let view = rustgis_ui_egui::rustgis_engine::render::View {
        width: rect.width() as f64,
        height: rect.height() as f64,
        pixels_per_point: 1.0,
        camera: h.state().session.map().camera,
        sky: true,
    };
    let expected = rustgis_ui_egui::rustgis_engine::render::render(&mut h.state().session.map().clone(), &view).expect("CPU reference");
    let mut error = 0u32;
    let mut samples = 0u32;
    for y in (rect.height() as u32 / 3..rect.height() as u32 * 2 / 3).step_by(11) {
        for x in (rect.width() as u32 / 3..rect.width() as u32 * 2 / 3).step_by(11) {
            let actual = image.get_pixel(rect.min.x.round() as u32 + x, rect.min.y.round() as u32 + y);
            let expected = expected.pixmap.pixel(x, y).expect("reference pixel");
            error += actual[0].abs_diff(expected.red()) as u32 + actual[1].abs_diff(expected.green()) as u32;
            samples += 2;
        }
    }
    assert!(error < samples * 6, "cached GPU image moved to the wrong ground: mean color error {}", error / samples);
    wait_for_frame(&mut h);
    assert_eq!(h.state().map.full_frame_camera(), h.state().session.map().camera, "settling sharpens the image");
}

#[test]
fn planet_layer_toggles_show_in_the_next_frame_and_stale_jobs_do_not_restore_them() {
    use rustgis_ui_egui::rustgis_engine::{
        data::{GeometryType, io::read_geojson},
        map::{Renderer, Rgba, Symbol, TileKind, TileLayer},
    };
    serve_planet_tiles();
    let Some(mut h) = harness() else { return };
    h.state_mut().run("map.body", json!({"name": "moon", "basemap": false})).expect("Moon");
    let m = h.state_mut().session.map_mut();
    m.layers.clear();
    m.graticule = false;
    m.tiles.push(TileLayer::new("Moon", TileKind::Raster, "https://planet.test/{z}/{x}/{y}.png"));
    let (data, _) = read_geojson(r#"{"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[-30,-20],[50,-20],[50,55],[-30,55],[-30,-20]]]}}"#, "Overlay").expect("overlay");
    let id = m.add_dataset(data);
    m.layer_mut(id).expect("layer").renderer =
        Renderer::Single(Symbol { fill: Rgba::rgb(20, 220, 20), ..Symbol::default_for(GeometryType::Polygon, 0) });
    for projection in ["globe", "mercator"] {
        for gpu in [false, true] {
            h.state_mut().run("map.projection", json!({"type": projection})).expect("projection");
            h.state_mut().run("ui.map.gpu", json!({"on": gpu})).expect("backend");
            wait_for_frame(&mut h);
            for visible in [true, false, true, false] {
                h.state_mut().run("layer.visibility", json!({"layer": id, "visible": visible})).expect("toggle");
                h.step();
                assert!(h.state().map.drawing(), "exact drawing runs off the UI thread");
                let image = h.render().expect("visibility preview");
                let center = h.state().map.view_rect.expect("map").0.center();
                let p = image.get_pixel(center.x.round() as u32, center.y.round() as u32);
                assert_eq!(
                    u16::from(p[1]) > u16::from(p[0]) * 2,
                    visible,
                    "{projection} GPU={gpu} visibility did not update immediately: {p:?}"
                );
            }
            wait_for_frame(&mut h);
            let image = h.render().expect("settled frame");
            let center = h.state().map.view_rect.expect("map").0.center();
            let p = image.get_pixel(center.x.round() as u32, center.y.round() as u32);
            assert!(p[0] > p[1], "an old worker restored a hidden overlay: {p:?}");
        }
    }
}
