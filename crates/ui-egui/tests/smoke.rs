//! Runs frames of the whole UI (no GPU) through every tab, pane, dialog and theme.

use rustgis_ui_egui::RustGisApp;
use serde_json::json;

fn harness() -> egui_kittest::Harness<'static, RustGisApp> {
    egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        RustGisApp::with_sample(),
    )
}

#[test]
fn every_tab_pane_and_dialog_renders() {
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    for tab in ["Map", "Insert", "Analysis", "View", "Edit", "Share", "Plugins", "Help", "Feature Layer", "Labeling", "Data"] {
        h.state_mut().ui.tab = tab.into();
        h.step();
        h.step();
    }
    for (id, p) in [
        ("ui.pane.symbology", json!(null)),
        ("ui.pane.geoprocessing", json!(null)),
        ("ui.gp.open", json!({"tool": "buffer"})),
        ("ui.pane.history", json!(null)),
        ("ui.pane.details", json!(null)),
        ("ui.pane.catalog", json!(null)),
        ("table.open", json!(null)),
        ("ui.dialog", json!({"name": "selectByAttributes"})),
        ("ui.dialog", json!({"name": "goToXY"})),
        ("ui.dialog", json!({"name": "calculateField"})),
        ("ui.commandSearch", json!(null)),
        ("ui.about", json!(null)),
        ("ui.shortcuts", json!(null)),
        ("ui.theme", json!({"dark": true})),
        ("ui.backstage", json!({"open": true})),
    ] {
        h.state_mut().run(id, p).unwrap_or_else(|e| panic!("{id}: {e}"));
        h.step();
        h.step();
    }
    h.state_mut().ui.backstage = false;
    // Every language, the collapsed and peeked ribbon.
    for l in rustgis_ui_egui::i18n::Lang::all() {
        h.state_mut().run("ui.language", json!({"code": l.code()})).unwrap();
        h.step();
        h.step();
    }
    h.state_mut().run("ui.language", json!({"code": "auto"})).unwrap();
    assert!(h.state_mut().run("ui.language", json!({"code": "xx"})).is_err());
    h.state_mut().run("ui.ribbon", json!({"minimized": true})).unwrap();
    h.state_mut().ui.ribbon_peek = true;
    h.step();
    h.state_mut().run("ui.ribbon", json!({"minimized": false})).unwrap();
    h.step();
    for kind in ["unique", "graduated", "single"] {
        h.state_mut().run("layer.symbology", json!({"layer": "Countries", "type": kind, "field": "continent"})).ok();
        h.step();
    }
    h.state_mut().run("layer.select", json!({"layer": "Cities"})).unwrap();
    h.state_mut().run("layer.symbology", json!({"type": "proportional", "field": "pop_max"})).unwrap();
    h.state_mut().run("project.new", json!(null)).unwrap();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.state().session.map().layers.is_empty());
}

/// Plugins are loaded for the whole process, so the tests that look at them take turns.
static PLUGINS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn plugins_install_from_a_file_after_a_prompt() {
    use egui_kittest::kittest::Queryable;
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rustgis-ui-plugins-{}", std::process::id()));
    rustgis_ui_egui::rustgis_engine::plugins::store::set_dir(Some(dir.join("installed")));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ui-test.rgplugin");
    std::fs::write(&file, rustgis_ui_egui::rustgis_engine::plugins::testing::package("ui-test", &["edit"])).unwrap();
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().run("ui.installPlugin", json!({"path": file.display().to_string()})).unwrap();
    h.step();
    h.step();
    assert!(h.state().plugins.confirm.is_some(), "asks first");
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_none());
    h.get_by_label("Install").click();
    h.step();
    h.step();
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_some());
    // Its commands are on the Plugins tab, and run from there.
    h.state_mut().ui.tab = "Plugins".into();
    h.step();
    h.step();
    h.get_by_label("Hello").click();
    h.step();
    // Its tools: a button on the Plugins tab, and listed in the Geoprocessing pane.
    h.get_by_label("Make Points").click();
    h.step();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-test/m"));
    h.state_mut().gp.tool = None;
    h.step();
    h.step();
    // On the ribbon and in the pane's Plugins section.
    assert_eq!(h.query_all_by_label("Select Two").count(), 2);
    for tab in ["installed", "browse", "settings"] {
        // An unreachable registry is an error in the window, not a crash.
        h.state_mut().run("ui.plugins", json!({"open": true, "tab": tab, "registry": "http://localhost:9/registry.json"})).unwrap();
        h.step();
        h.step();
    }
    h.state_mut().run("ui.plugins", json!({"open": true, "tab": "installed"})).unwrap();
    h.step();
    h.get_by_label("Uninstall").click();
    h.step();
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plugins_can_have_a_tab_of_their_own() {
    use egui_kittest::kittest::Queryable;
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    use rustgis_ui_egui::rustgis_engine::plugins as rp;
    let mut m: serde_json::Value = serde_json::from_str(&rp::testing::manifest("ui-tab", &[])).unwrap();
    // Its own names, so the other tests (which share the loaded plugins) don't see them.
    m["commands"][0]["label"] = json!("Tab Hello");
    m["tools"][0]["name"] = json!("Tab Points");
    m["tools"][1]["name"] = json!("Tab Select");
    m["tab"] = json!({"label": "Hello Tab", "groups": [
        {"label": "Things", "items": ["h", "tool:m", {"menu": "More", "items": ["e", "tool:x"]}]}
    ]});
    let pkg = rp::testing::zip_of(&[("plugin.json", m.to_string().as_bytes()), ("plugin.wasm", &rp::testing::wasm())]);
    rp::load(rp::Package::from_zip(&pkg).unwrap(), "test").unwrap();
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    // Its tab is in the tab strip; its buttons are there, not on the Plugins tab.
    h.state_mut().ui.tab = "Plugins".into();
    h.step();
    h.step();
    assert_eq!(h.query_all_by_label("Tab Points").count(), 0);
    h.state_mut().ui.tab = "plugin:ui-tab".into();
    h.step();
    h.step();
    h.get_by_label("Tab Hello").click();
    h.step();
    h.get_by_label("Tab Points").click();
    h.step();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-tab/m"));
    h.get_by_label("More").click();
    for _ in 0..4 {
        h.step();
    }
    h.get_by_label("Tab Select").click();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-tab/x"));
    // Hidden, then shown again, from Help › Ribbon.
    h.state_mut().run("ui.customizeRibbon", json!({"tab": "plugin:ui-tab"})).unwrap();
    h.step();
    assert!(h.state().ui.hidden_tabs.contains(&"plugin:ui-tab".to_string()));
    h.state_mut().run("ui.customizeRibbon", json!({"tab": "plugin:ui-tab"})).unwrap();
    // A disabled plugin's tab goes, and the ribbon falls back to the Plugins tab.
    h.state_mut().ui.tab = "plugin:ui-tab".into();
    rp::set_enabled("ui-tab", false).unwrap();
    h.step();
    h.step();
    assert_eq!(h.state().ui.tab, "Plugins");
    rp::unload("ui-tab");
}
