//! The Messages pane groups a command's messages by its run and can undo back to before it.

use super::*;

#[test]
fn a_commands_messages_remember_its_run_and_undo_reaches_back_to_before_it() {
    let mut s = world();
    s.run("layer.addDataText", &json!({"fileName": "records.csv", "text": "n\n1\n2\n"})).unwrap();
    s.run("edit.enable", &json!({"on": true})).unwrap();
    let loss_records = |s: &Session| -> Vec<rustgis_data::diagnostics::Record> {
        s.diagnostics.records().into_iter().filter(|r| r.loss.is_some()).collect()
    };
    s.run("edit.calculateField", &json!({"table": "records", "field": "n", "expression": "n + 1.5"})).unwrap();
    let first = loss_records(&s);
    let [cut] = first.as_slice() else { panic!("one coded message: {first:?}") };
    assert_eq!(cut.source, "edit.calculateField");
    let run = cut.run.clone().expect("said by a command run");
    assert_eq!(run.target.as_deref(), Some("records"));
    assert_eq!(s.undo_steps_to(run.before), Some(1), "one step back to before it");

    s.run("edit.calculateField", &json!({"table": "records", "field": "n", "expression": "n + 0.5"})).unwrap();
    let both = loss_records(&s);
    let later = both.last().and_then(|r| r.run.clone()).unwrap();
    assert_ne!(later.id, run.id, "each run is its own");
    assert_eq!(s.undo_steps_to(run.before), Some(2), "back past the later run too");
    assert_eq!(s.undo_steps_to(later.before), Some(1));

    // Undone back to before the first run, its state is the current one.
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.undo_steps_to(run.before), Some(0));
    // A message no command said has no run.
    s.notify("said by the app", false);
    assert!(s.diagnostics.records().last().is_some_and(|r| r.run.is_none()));
}
