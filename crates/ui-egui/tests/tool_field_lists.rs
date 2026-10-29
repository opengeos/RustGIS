//! Exercise ordered lists through the real Tools pane, including run/history round trips.
use egui_kittest::{Harness, kittest::Queryable};
use rustgis_engine::{
    Project, Session,
    data::{Dataset, Field, FieldType, GeometryType, Value},
};
use rustgis_ui_egui::RustGisApp;
use serde_json::json;

/// A standalone table and a spatial layer with different schemas reveal incorrect field sourcing.
fn harness(tool: &str, values: serde_json::Value) -> Harness<'static, RustGisApp> {
    let mut session = Session::new(Project::default());
    let mut table = Dataset::new(
        "Records",
        GeometryType::None,
        vec![Field::new("region", FieldType::Text), Field::new("category", FieldType::Text), Field::new("amount", FieldType::Double)],
    );
    table.push(None, vec![Value::Text("North".into()), Value::Text("A".into()), Value::Float(14.3)]);
    table.push(None, vec![Value::Text("North".into()), Value::Text("A".into()), Value::Float(2.0)]);
    session.project.add_table(table);
    session.map_mut().add_dataset(Dataset::new("Target", GeometryType::Point, vec![Field::new("target_only", FieldType::Text)]));
    let mut app = RustGisApp::new(session);
    app.run("ui.gp.open", json!({"tool": tool, "values": values})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 1400.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

/// AccessKit labels distinguish the list's field and statistic selectors from other tool inputs.
fn choose(h: &mut Harness<'_, RustGisApp>, label: &str, value: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, label).click();
    h.run_steps(2);
    h.get_by_label(value).click_accesskit();
    h.run_steps(2);
}

#[test]
fn summary_list_runs_with_ordered_case_fields_and_reopens_from_history() {
    let mut h = harness("summaryStatistics", json!({"input": "Records", "case": ["region", "category"]}));
    h.get_by_label("Add statistic").click();
    h.run_steps(2);
    // There are existing case-field selectors, so choose the first list field (Statistics).
    h.get_all_by_role_and_label(egui::accesskit::Role::ComboBox, "Field").next().unwrap().click();
    h.run_steps(2);
    h.get_by_label("amount").click();
    h.run_steps(2);
    choose(&mut h, "Statistic", "SUM");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Run").click();
    h.run_steps(3);
    let history = h.state().session.project.gp_history.last().unwrap();
    assert_eq!(history.params["statistics"], json!([{"field":"amount","statistic":"SUM"}]));
    assert_eq!(history.params["case"], json!(["region", "category"]));
    let output = h.state().session.project.tables.last().unwrap();
    assert_eq!(output.dataset.value(0, 3).as_f64(), Some(16.3));
    let params = history.params.clone();
    h.state_mut().run("ui.gp.open", json!({"tool":"summaryStatistics", "values":params})).unwrap();
    h.run_steps(3);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["statistics"]).unwrap(), params["statistics"]);
}

#[test]
fn merge_rules_offer_join_fields_and_text_rules_and_preserve_delimiter() {
    let mut h =
        harness("spatialJoin", json!({"target":"Target", "join":"Records", "merge":[{"field":"region","rule":"join","delimiter":"; "}]}));
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Field").click();
    h.run_steps(2);
    assert!(h.query_by_label("amount").is_some());
    assert!(h.query_by_label("target_only").is_none());
    h.get_by_label("category").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Rule").click();
    h.run_steps(2);
    assert!(h.query_by_label("sum").is_none());
    assert!(h.query_by_label("join").is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    let rules: serde_json::Value = serde_json::from_str(&h.state().gp.values["merge"]).unwrap();
    assert_eq!(rules, json!([{"field":"category","rule":"join","delimiter":"; "}]));
}

#[test]
fn pivot_row_fields_can_be_added_reordered_and_removed() {
    let mut h = harness("pivotTable", json!({"input":"Records", "fields":"region"}));
    h.get_by_label("Add field").click();
    h.run_steps(2);
    h.get_all_by_role_and_label(egui::accesskit::Role::ComboBox, "Field").nth(1).unwrap().click();
    h.run_steps(2);
    assert!(h.query_by_label("region").is_none(), "a field cannot be listed twice");
    h.get_by_label("category").click();
    h.run_steps(2);
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, "Move up").nth(1).unwrap().click();
    h.run_steps(2);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["fields"]).unwrap(), json!(["category", "region"]));
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, "Remove").nth(1).unwrap().click();
    h.run_steps(2);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["fields"]).unwrap(), json!(["category"]));
}

#[test]
fn missing_fields_and_malformed_history_are_kept_until_edited() {
    let saved = json!([{"field":"missing","statistic":"P90","extension":42}]);
    let mut h = harness("summaryStatistics", json!({"input":"Records", "statistics":saved}));
    assert!(h.query_by_label("Field is missing from this input.").is_some());
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["statistics"]).unwrap(), saved);
    h.state_mut().run("ui.gp.open", json!({"tool":"summaryStatistics","values":{"statistics":"{broken"}})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("JSON value").is_some());
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["statistics"]).unwrap(), json!("{broken"));
}

#[test]
fn model_lists_accept_future_fields_and_undo_the_edit() {
    use rustgis_engine::project::{ModelNode, ModelNodeKind};
    let mut h = harness("summaryStatistics", json!({}));
    h.state_mut().run("ui.pane.geoprocessing", json!({})).unwrap();
    h.state_mut().run("ui.modelBuilder", json!({})).unwrap();
    h.state_mut().mb.graph.nodes.push(ModelNode {
        id: "summary".into(),
        kind: ModelNodeKind::Tool,
        tool: Some("summaryStatistics".into()),
        params: json!({}),
        ..Default::default()
    });
    h.state_mut().mb.selected = Some("summary".into());
    h.run_steps(3);
    h.get_by_label("Add statistic").click();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Field").focus();
    h.step();
    h.event(egui::Event::Text("future_amount".into()));
    h.run_steps(3);
    choose(&mut h, "Statistic", "P90");
    assert_eq!(h.state().mb.graph.nodes[0].params["statistics"], json!([{"field":"future_amount","statistic":"P90"}]));
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Remove").click();
    h.run_steps(3);
    assert_eq!(h.state().mb.graph.nodes[0].params["statistics"], json!([]));
    // ModelBuilder owns its undo stack; the actual shortcut must restore the list.
    h.event(egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
    h.run_steps(3);
    assert_eq!(h.state().mb.graph.nodes[0].params["statistics"], json!([{"field":"future_amount","statistic":"P90"}]));
}

#[test]
fn malformed_row_field_json_can_be_repaired_across_frames() {
    let mut h = harness("pivotTable", json!({"input":"Records", "fields":{"invalid":true}}));
    h.get_by_role_and_label(egui::accesskit::Role::MultilineTextInput, "JSON value").focus();
    h.step();
    h.event(egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
    h.event(egui::Event::Text("[".into()));
    h.run_steps(3);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::MultilineTextInput, "JSON value").is_some());
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["fields"]).unwrap(), json!({"invalid":true}));
    h.event(egui::Event::Text("\"region\"]".into()));
    h.run_steps(3);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["fields"]).unwrap(), json!(["region"]));
}

#[test]
fn model_merge_fields_follow_the_wired_join_input() {
    use rustgis_engine::project::{ModelEdge, ModelNode, ModelNodeKind};
    let mut h = harness("spatialJoin", json!({}));
    h.state_mut().run("ui.pane.geoprocessing", json!({})).unwrap();
    h.state_mut().run("ui.modelBuilder", json!({})).unwrap();
    h.state_mut().mb.graph.nodes.extend([
        ModelNode { id: "rows".into(), kind: ModelNodeKind::Input, layer: Some("Records".into()), ..Default::default() },
        ModelNode {
            id: "join".into(),
            kind: ModelNodeKind::Tool,
            tool: Some("spatialJoin".into()),
            params: json!({"target":"Target", "merge":[{"field":"region","rule":"first"}]}),
            ..Default::default()
        },
    ]);
    h.state_mut().mb.graph.edges.push(ModelEdge {
        from: "rows".into(),
        from_port: "out".into(),
        to: "join".into(),
        to_port: "join".into(),
    });
    h.state_mut().mb.selected = Some("join".into());
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Field").click();
    h.run_steps(3);
    assert!(h.query_by_label("amount").is_some());
    assert!(h.query_by_label("target_only").is_none());
}

#[test]
fn invalid_statistics_text_is_not_changed_into_an_empty_list() {
    let mut h = harness("summaryStatistics", json!({"input":"Records", "field":"amount", "statistics":"null"}));
    assert!(h.query_by_label("JSON value").is_some());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Run").click();
    h.run_steps(3);
    let result = h.state().gp.last.as_ref().unwrap();
    assert!(!result.0);
    assert!(result.1.iter().any(|message| message.contains("Statistics must be a list")));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["statistics"]).unwrap(), json!("null"));
}

#[test]
fn saved_percentile_alias_is_available_and_cannot_be_added_twice() {
    let saved = json!([{"field":"amount","statistic":"P01"}]);
    let mut h = harness("summaryStatistics", json!({"input":"Records","statistics":saved}));
    assert!(h.query_by_label("Statistic is not available for this field.").is_none());
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().gp.values["statistics"]).unwrap(), saved);
    h.get_by_label("Add statistic").click();
    h.run_steps(2);
    h.get_all_by_role_and_label(egui::accesskit::Role::ComboBox, "Field").nth(1).unwrap().click();
    h.run_steps(2);
    h.get_by_label("amount").click();
    h.run_steps(2);
    h.get_all_by_role_and_label(egui::accesskit::Role::ComboBox, "Statistic").nth(1).unwrap().click();
    h.run_steps(2);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "P1").is_none());
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "P2").is_some());
}
