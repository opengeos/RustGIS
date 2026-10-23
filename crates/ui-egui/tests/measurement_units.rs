//! Headless interaction tests for the units users choose and the results they see.
use egui_kittest::kittest::Queryable;
use rustgis_engine::{Project, Session, data::Geometry, geom::lonlat_to_merc};
use rustgis_ui_egui::RustGisApp;
use serde_json::json;

fn harness() -> egui_kittest::Harness<'static, RustGisApp> {
    let mut app = RustGisApp::new(Session::new(Project::default()));
    app.run("layer.template", json!({"template": "lines", "name": "Paths"})).unwrap();
    app.run("edit.addFeature", json!({"geometry": {"type": "LineString", "coordinates": [[0, 0], [0, 0.001]]}})).unwrap();
    app.run("select.rows", json!({"layer": "Paths", "rows": [0]})).unwrap();
    egui_kittest::Harness::builder().with_size(egui::vec2(1800.0, 1000.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
}

#[test]
fn editing_pane_moves_in_feet() {
    let mut h = harness();
    h.state_mut().run("ui.pane.editing", json!({})).unwrap();
    h.state_mut().forms.dx = "500".into();
    h.state_mut().forms.dy = "0".into();
    for _ in 0..3 {
        h.step();
    }
    h.get_by_value("Meters").click();
    h.step();
    h.get_by_label("Feet").click();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Move").click();
    h.step();
    let Geometry::Line(parts) = h.state().session.map().layer_by_name("Paths").unwrap().dataset.features[0].geometry.as_ref().unwrap()
    else {
        panic!("line")
    };
    assert!((lonlat_to_merc(parts[0][0]).x - 152.4).abs() < 1e-6);
}

#[test]
fn editing_pane_sets_constant_elevation_in_feet() {
    let mut h = harness();
    h.state_mut().run("ui.pane.editing", json!({})).unwrap();
    h.state_mut().forms.z = "1000".into();
    for _ in 0..3 {
        h.step();
    }
    h.get_by_value("Meters").click();
    h.step();
    h.get_by_label("Feet").click();
    h.step();
    h.get_by_label("Constant").click();
    h.step();
    assert_eq!(h.state().session.map().editing.settings.elevation, rustgis_engine::map::design::Elevation::Constant(304.8));
}

fn click_map(h: &mut egui_kittest::Harness<'_, RustGisApp>, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    for _ in 0..3 {
        h.step();
    }
}

fn painted_text(h: &egui_kittest::Harness<'_, RustGisApp>) -> Vec<String> {
    fn collect(shape: &egui::Shape, text: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => text.push(t.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = Vec::new();
    for shape in &h.output().shapes {
        collect(&shape.shape, &mut text);
    }
    text
}

#[test]
fn measure_menu_switches_live_distance_and_area_readouts_to_imperial() {
    let mut h = harness();
    h.state_mut().run("map.goToXY", json!({"lon": 0, "lat": 0, "scale": 1000})).unwrap();
    h.state_mut().run("tool.set", json!({"tool": "measure"})).unwrap();
    for _ in 0..3 {
        h.step();
    }
    let center = h.state().map.view_rect.unwrap().0.center();
    click_map(&mut h, center - egui::vec2(50.0, 0.0));
    click_map(&mut h, center + egui::vec2(50.0, 0.0));
    assert!(painted_text(&h).iter().any(|s| s.starts_with("Total ") && s.contains(" m")));
    h.get_by_label("Measure").click();
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Imperial").click();
    h.step();
    // At 1:1000, 100 screen points span about 28 m, or 92 ft at the equator.
    let text = painted_text(&h);
    assert!(text.iter().any(|s| s.starts_with("Total 92 ft")), "{text:?}");
    assert!(text.iter().any(|s| s == "100 ft"), "scale bar: {text:?}");
    h.state_mut().run("tool.set", json!({"tool": "measureArea"})).unwrap();
    h.state_mut().map.cancel_sketch();
    h.step();
    click_map(&mut h, center - egui::vec2(50.0, 50.0));
    click_map(&mut h, center + egui::vec2(50.0, -50.0));
    click_map(&mut h, center + egui::vec2(50.0, 50.0));
    let text = painted_text(&h);
    assert!(text.iter().any(|s| s.starts_with("Area ") && s.contains("ft²") && s.contains("Perimeter") && s.ends_with("ft")), "{text:?}");
    for (scale, unit) in [(10_000, "acres"), (1_000_000, "mi²")] {
        h.state_mut().map.cancel_sketch();
        h.state_mut().run("map.goToXY", json!({"lon": 0, "lat": 0, "scale": scale})).unwrap();
        for _ in 0..3 {
            h.step();
        }
        click_map(&mut h, center - egui::vec2(50.0, 50.0));
        click_map(&mut h, center + egui::vec2(50.0, -50.0));
        click_map(&mut h, center + egui::vec2(50.0, 50.0));
        let text = painted_text(&h);
        assert!(text.iter().any(|s| s.starts_with("Area ") && s.contains(unit)), "{unit}: {text:?}");
    }
}

#[test]
fn measurement_unit_choice_is_saved_restored_and_available_to_scripts() {
    use rustgis_ui_egui::Services;
    use std::sync::{Arc, Mutex};
    let saved = Arc::new(Mutex::new(Vec::new()));
    let writes = saved.clone();
    let mut h = harness();
    h.state_mut().services.save_pref = Some(Box::new(move |k, v| writes.lock().unwrap().push((k.to_string(), v.to_string()))));
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Measure").click();
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Imperial").click();
    h.step();
    assert!(saved.lock().unwrap().contains(&("measurement_units".into(), "imperial".into())));
    let mut restored = RustGisApp::new(Session::new(Project::default())).with_services(Services {
        load_pref: Some(Box::new(|k| (k == "measurement_units").then(|| "imperial".into()))),
        ..Services::default()
    });
    assert!(restored.ui.measurement_imperial);
    restored.run("ui.measurementUnits", json!({"units": "metric"})).unwrap();
    assert!(!restored.ui.measurement_imperial);
    assert!(restored.run("ui.measurementUnits", json!({"units": "unknown"})).is_err());
    assert!(!restored.ui.measurement_imperial);
}
