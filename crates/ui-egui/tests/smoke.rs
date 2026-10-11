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

#[test]
fn browse_lists_thousands_of_plugins_quickly() {
    use egui_kittest::kittest::Queryable;
    use rustgis_ui_egui::rustgis_engine::plugins::registry;
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    let sha = "a".repeat(64);
    let list: Vec<serde_json::Value> = (0..5000)
        .map(|i| {
            let cat = ["Vector", "Raster", "Terrain"][i % 3];
            json!({"id": format!("fake-{i:04}"), "name": format!("Plugin {i}"), "version": "1.0.0",
                "description": format!("Does many things with data for plugin number {i}. ").repeat(3), "author": format!("Author {}", i % 97),
                "categories": [cat, "Utilities"], "package": format!("https://packages.example.com/fake-{i}.rgplugin"),
                "sha256": sha, "size": 123_456, "permissions": ["edit"]})
        })
        .collect();
    let entries = registry::parse(&json!({"plugins": list}).to_string(), "https://plugins.rustgis.app/registry.json").unwrap();
    assert_eq!(entries.len(), 5000);
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().plugins.set_registry(Some(Ok(entries)));
    h.state_mut().run("ui.plugins", json!({"open": true, "tab": "browse"})).unwrap();
    h.step();
    let _ = h.get_by_label("5000 plugins");
    // Only the visible rows are laid out, so a frame stays cheap with thousands of plugins
    // (about 1 ms in a release build; it was over 100 ms when every card was drawn). The bound
    // is loose for debug builds on busy machines.
    let t0 = std::time::Instant::now();
    for _ in 0..10 {
        h.step();
    }
    let per_frame = t0.elapsed() / 10;
    assert!(per_frame < std::time::Duration::from_millis(100), "{per_frame:?} per frame");
    // Search and the category filter narrow the list.
    h.state_mut().plugins.filter = "plugin 4999".into();
    h.step();
    h.step();
    let _ = h.get_by_label("1 of 5000 plugins");
    h.state_mut().plugins.filter = String::new();
    h.state_mut().plugins.category = "Terrain".into();
    h.step();
    h.step();
    let _ = h.get_by_label("1666 of 5000 plugins");
    // A new registry of the same size replaces what is listed (the cache follows the registry,
    // not its allocation).
    h.state_mut().plugins.category = String::new();
    let two = |a: &str, b: &str| {
        let list: Vec<serde_json::Value> = [a, b]
            .iter()
            .map(|n| json!({"id": n.to_lowercase(), "name": n, "version": "1.0.0", "package": "https://packages.example.com/x.rgplugin", "sha256": sha}))
            .collect();
        registry::parse(&json!({"plugins": list}).to_string(), "https://plugins.rustgis.app/registry.json").unwrap()
    };
    h.state_mut().plugins.set_registry(Some(Ok(two("Alpha", "Beta"))));
    h.step();
    h.step();
    let _ = h.get_by_label("Alpha");
    h.state_mut().plugins.set_registry(Some(Ok(two("Gamma", "Delta"))));
    h.step();
    h.step();
    let _ = h.get_by_label("Gamma");
    assert!(h.query_by_label("Alpha").is_none());
    h.state_mut().plugins.set_registry(None);
    h.state_mut().plugins.open = false;
}
