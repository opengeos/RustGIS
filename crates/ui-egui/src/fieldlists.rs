//! One ordered-list editor for field names and field/statistic objects in tool parameters.
use egui::Ui;
use rustgis_engine::{
    analysis::{ParamKind, StatisticList},
    data::Field,
};
use serde_json::{Value, json};

/// Named input supplying a list's fields. Other parameter kinds use their existing editors.
pub(crate) fn input(kind: ParamKind) -> Option<&'static str> {
    match kind {
        ParamKind::Fields { input } | ParamKind::FieldStatistics { input, .. } => Some(input),
        _ => None,
    }
}

/// Reads a Tools-pane list value, stored as JSON so legacy strings retain their type as well
/// as their contents. An empty form value is absent, as for other optional parameters.
pub(crate) fn form_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

/// Edits a list without changing its JSON until a user acts. Unknown fields and statistics are
/// shown as saved; malformed or oversized values remain editable as JSON rather than discarded.
/// `None` fields means an upstream model input is not known yet, so names can be typed.
pub(crate) fn show(ui: &mut Ui, kind: ParamKind, value: &Value, fields: Option<&[Field]>) -> Option<Value> {
    let statistics = match kind {
        ParamKind::FieldStatistics { statistics, .. } => Some(statistics),
        _ => None,
    };
    let decoded = if statistics.is_some() {
        match value {
            Value::String(s) if s.trim().is_empty() => Value::Null,
            Value::String(s) => match serde_json::from_str(s) {
                Ok(array @ Value::Array(_)) => array,
                _ => value.clone(),
            },
            _ => value.clone(),
        }
    } else {
        value.clone()
    };
    let mut rows = match &decoded {
        Value::Null => Vec::new(),
        Value::String(s) if statistics.is_none() => {
            if s.trim().is_empty() {
                Vec::new()
            } else {
                vec![json!(s)]
            }
        }
        Value::Array(rows)
            if rows.len() <= 256
                && rows.iter().all(|r| match statistics {
                    None => r.is_string(),
                    Some(s) => {
                        r.is_object()
                            && r.get("field").is_some_and(Value::is_string)
                            && r.get(s.key()).is_some_and(Value::is_string)
                            && r.get("delimiter").is_none_or(|v| v.is_null() || v.is_string())
                    }
                }) =>
        {
            rows.clone()
        }
        _ => {
            ui.label(tl!("This value cannot be shown as a list. Edit its JSON below."));
            // Keep incomplete JSON as an editor draft. In particular a field-list string is a
            // valid legacy value, so storing an unfinished JSON fragment as a string would
            // replace this editor with a field picker before the user could finish typing.
            let draft = ui.scope_id().with(("json_draft", value.to_string()));
            let mut text = ui
                .ctx()
                .data_mut(|d| d.get_temp::<String>(draft))
                .unwrap_or_else(|| value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string()));
            let label = ui.label(tl!("JSON value"));
            if ui.add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY)).labelled_by(label.id).changed() {
                if let Ok(value) = serde_json::from_str(&text) {
                    ui.ctx().data_mut(|d| d.remove::<String>(draft));
                    return Some(value);
                }
                ui.ctx().data_mut(|d| d.insert_temp(draft, text));
            }
            return None;
        }
    };
    let mut changed = false;
    let mut action = None;
    let count = rows.len();
    for i in 0..count {
        ui.push_id(i, |ui| {
            ui.group(|ui| {
                let Some(row) = rows.get(i) else { return };
                let mut name = match statistics {
                    None => row.as_str(),
                    Some(_) => row.get("field").and_then(Value::as_str),
                }
                .unwrap_or_default()
                .to_string();
                let old_name = name.clone();
                let available: Option<Vec<String>> = fields.map(|fields| {
                    fields
                        .iter()
                        .filter(|f| {
                            // Field lists and merge rules allow a field once. Summary allows several statistics.
                            statistics == Some(StatisticList::Summary)
                                || !rows.iter().enumerate().any(|(j, r)| {
                                    let n = if statistics.is_some() { r.get("field").and_then(Value::as_str) } else { r.as_str() };
                                    i != j && n.is_some_and(|n| n.trim().eq_ignore_ascii_case(&f.name))
                                })
                        })
                        .map(|f| f.name.clone())
                        .collect()
                });
                field(ui, &mut name, available.as_deref());
                if fields.is_some_and(|f| !name.is_empty() && !f.iter().any(|f| f.name.eq_ignore_ascii_case(name.trim()))) {
                    ui.label(tl!("Field is missing from this input."));
                }
                let other_pairs: Vec<(String, String)> = rows
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .filter_map(|(_, r)| {
                        Some((
                            r.get("field")?.as_str()?.trim().to_string(),
                            StatisticList::Summary.canonical_name(r.get("statistic")?.as_str()?)?,
                        ))
                    })
                    .collect();
                let Some(row) = rows.get_mut(i) else { return };
                if name != old_name {
                    changed = true;
                    if let Some(obj) = row.as_object_mut() {
                        obj.insert("field".into(), json!(name));
                    } else {
                        *row = json!(name);
                    }
                }
                if let Some(s) = statistics {
                    let ty = fields.and_then(|f| f.iter().find(|f| f.name.eq_ignore_ascii_case(name.trim()))).map(|f| f.ty);
                    let mut choices = s.choices(ty);
                    if s == StatisticList::Summary {
                        choices.retain(|stat| {
                            !other_pairs
                                .iter()
                                .any(|(field, other)| field.eq_ignore_ascii_case(name.trim()) && other.eq_ignore_ascii_case(stat))
                        });
                    }
                    let mut stat = row.get(s.key()).and_then(Value::as_str).unwrap_or_default().to_string();
                    let before = stat.clone();
                    let identity = s.canonical_name(&stat);
                    let label = ui.label(if s == StatisticList::Merge { tl!("Rule") } else { tl!("Statistic") });
                    egui::ComboBox::from_id_salt("statistic")
                        .selected_text(if stat.is_empty() { "—" } else { &stat })
                        .width((ui.available_width() - 8.0).max(40.0))
                        .show_ui(ui, |ui| {
                            for c in &choices {
                                if ui.selectable_label(identity.as_ref() == Some(c), tl!(c)).clicked() {
                                    stat = c.clone();
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    if !stat.is_empty() && !choices.iter().any(|choice| identity.as_ref() == Some(choice)) {
                        ui.label(tl!("Statistic is not available for this field."));
                    }
                    if let Some(obj) = row.as_object_mut() {
                        if stat != before {
                            obj.insert(s.key().into(), json!(stat));
                            changed = true;
                        }
                        if s == StatisticList::Merge && stat.trim().eq_ignore_ascii_case("join") {
                            let mut delimiter = obj.get("delimiter").and_then(Value::as_str).unwrap_or(", ").to_string();
                            let label = ui.label(tl!("Delimiter"));
                            if ui
                                .add(egui::TextEdit::singleline(&mut delimiter).desired_width(f32::INFINITY))
                                .labelled_by(label.id)
                                .changed()
                            {
                                obj.insert("delimiter".into(), json!(delimiter));
                                changed = true;
                            }
                        }
                    }
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new(tl!("Move up"))).clicked() {
                        action = Some((i, Some(i - 1)));
                    }
                    if ui.add_enabled(i + 1 < count, egui::Button::new(tl!("Move down"))).clicked() {
                        action = Some((i, Some(i + 1)));
                    }
                    if ui.button(tl!("Remove")).clicked() {
                        action = Some((i, None));
                    }
                });
            });
        });
    }
    if let Some((i, to)) = action {
        if let Some(to) = to {
            rows.swap(i, to);
        } else {
            rows.remove(i);
        }
        changed = true;
    }
    if ui
        .add_enabled(
            rows.len() < 256,
            egui::Button::new(match statistics {
                Some(StatisticList::Merge) => tl!("Add rule"),
                Some(StatisticList::Summary) => tl!("Add statistic"),
                None => tl!("Add field"),
            }),
        )
        .clicked()
    {
        rows.push(match statistics {
            Some(s) => {
                let mut obj = serde_json::Map::new();
                obj.insert("field".into(), json!(""));
                obj.insert(s.key().into(), json!(""));
                Value::Object(obj)
            }
            None => json!(""),
        });
        changed = true;
    }
    changed.then_some(Value::Array(rows))
}

/// Selects known field names, or accepts a name for an input only available when a model runs.
fn field(ui: &mut Ui, name: &mut String, offered: Option<&[String]>) {
    let label = ui.label(tl!("Field"));
    match offered {
        Some(names) => {
            egui::ComboBox::from_id_salt("field")
                .selected_text(if name.is_empty() { "—" } else { name.as_str() })
                .width((ui.available_width() - 8.0).max(40.0))
                .show_ui(ui, |ui| {
                    for n in names {
                        if ui.selectable_label(name.trim().eq_ignore_ascii_case(n), n).clicked() {
                            *name = n.clone();
                            ui.close();
                        }
                    }
                })
                .response
                .labelled_by(label.id);
        }
        None => {
            ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY)).labelled_by(label.id);
        }
    }
}
