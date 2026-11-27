//! The Messages pane groups a command's messages by its run and can undo back to before it.

use super::*;

#[test]
fn a_commands_messages_remember_its_run_and_undo_reaches_back_past_it() {
    let mut s = world();
    s.run("layer.addDataText", &json!({"fileName": "records.csv", "text": "n\n1\n2\n"})).unwrap();
    s.run("edit.enable", &json!({"on": true})).unwrap();
    let last_run = |s: &Session| -> rustgis_data::diagnostics::Run {
        s.diagnostics.records().into_iter().rev().find(|r| r.loss.is_some()).and_then(|r| r.run).expect("a coded message from a run")
    };
    s.run("edit.calculateField", &json!({"table": "records", "field": "n", "expression": "n + 1.5"})).unwrap();
    let first = last_run(&s);
    assert_eq!(first.target.as_deref(), Some("records"));
    let after = first.after.expect("finished");
    assert_ne!(after, first.before, "it changed the project");
    assert_eq!(s.undo_steps_back_past(first.before, after), Some(1), "one step back past it");

    s.run("edit.calculateField", &json!({"table": "records", "field": "n", "expression": "n + 0.5"})).unwrap();
    let later = last_run(&s);
    assert_ne!(later.id, first.id, "each run is its own");
    assert_eq!(s.undo_steps_back_past(first.before, after), Some(2), "back past the later run too");
    assert_eq!(s.undo_steps_back_past(later.before, later.after.unwrap()), Some(1));

    // Both undone, then another edit from the state before the first: the first run's change is
    // gone from the history, so its message offers nothing, not the new edit.
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.state(), first.before);
    s.run("edit.calculateField", &json!({"table": "records", "field": "n", "expression": "n * 2"})).unwrap();
    assert_eq!(s.undo_steps_back_past(first.before, after), None);
    // A run whose step joined an earlier one's (a held key nudging an element) has no step of its
    // own: Undo would land before the earlier command, so its message offers nothing.
    s.merge_undo("nudge");
    s.run("edit.calculateField", &json!({"table": "records", "field": "n", "expression": "n + 1.5"})).unwrap();
    s.merge_undo("nudge");
    s.run("edit.calculateField", &json!({"table": "records", "field": "n", "expression": "n + 2.5"})).unwrap();
    let joined = last_run(&s);
    assert_eq!(s.undo_steps_back_past(joined.before, joined.after.unwrap()), None);
    // An undo makes no step: a message it says offers no Undo to before it.
    s.run("edit.undo", &json!({})).unwrap();
    let undone = s.diagnostics.records().into_iter().rev().find_map(|r| r.run.filter(|run| run.id > later.id));
    assert!(undone.as_ref().is_none_or(|run| run.after.is_none()), "{undone:?}");
    // A message no command said has no run.
    s.notify("said by the app", false);
    assert!(s.diagnostics.records().last().is_some_and(|r| r.run.is_none()));
}
