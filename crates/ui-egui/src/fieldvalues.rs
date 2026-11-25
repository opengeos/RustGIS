//! Select By Attributes' Values list: the distinct values of a field, to pick from. A user who
//! clicked a field and wonders what it holds sees them without opening the table, and a click
//! writes the comparison, the value quoted as the expression needs it.
//!
//! The values are what the completion offers after `"field" = '` (`query::field_values`): read
//! from an evenly spread sample of at most `query::VALUES_READ` features, a streamed layer's from
//! the features in view. They are read once, when the list is opened for a field, and kept while
//! the dialog stays on that layer and field and the layer's rows don't change.

use std::sync::Weak;

use egui::Ui;
use egui::widgets::text_edit::TextEditState;
use rustgis_engine::data::query::{self, Completion, FieldValues, Kw, Place};
use rustgis_engine::data::{Dataset, Feature};

use crate::exprcomplete::{ValuesRead, field_ref, fields_key};

/// The Values list between frames, held by the dialog (`Dialogs::values`): which field it lists,
/// and the values read for it.
#[derive(Default)]
pub struct Form {
    /// The layer the field belongs to, by the name the dialog has it under.
    layer: String,
    /// The field last clicked, or the cursor was last in, as the layer names it.
    field: Option<String>,
    read: Option<Read>,
    /// Why there is no list to show, for the dialog to say under the buttons.
    said: Option<String>,
    /// How many times the values were read: once per list opened, not once per frame.
    #[cfg(test)]
    pub(crate) reads: usize,
}

/// The values of a field, as read, and the rows and fields they were read from: another set of
/// rows (an edit, a reload, a streamed layer's next view) or another type of field reads them
/// again.
struct Read {
    rows: Weak<Vec<Feature>>,
    fields: u64,
    got: FieldValues,
    /// What the list shows for each value: as the expression writes it.
    written: Vec<String>,
}

impl Form {
    /// Follows the layer the dialog is on: the list is of one layer's field, so another layer
    /// has none until a field of it is chosen.
    pub(crate) fn follow(&mut self, layer: &str) {
        if self.layer != layer {
            self.layer = layer.to_string();
            self.field = None;
            self.read = None;
            self.said = None;
        }
    }

    /// Lists `field` from now on. Another field drops the values read for the last.
    pub(crate) fn choose(&mut self, field: &str) {
        if self.field.as_deref() != Some(field) {
            self.field = Some(field.to_string());
            self.read = None;
            self.said = None;
        }
    }

    /// Lists the field the box `id` (holding `text`, with the focus) has its cursor where a value
    /// compared with one of `ds`'s goes (`name = ▮`, `name IN ('a', ▮`): a field typed is a field
    /// chosen, as `query::complete_at` reads it.
    pub(crate) fn follow_box(&mut self, ui: &Ui, id: egui::Id, text: &str, ds: &Dataset) {
        let cursor = TextEditState::load(ui.ctx(), id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
        let field = cursor.and_then(|at| query::complete_at(text, at, ds)).and_then(|c| c.field);
        if let Some(field) = field.and_then(|i| ds.fields.get(i)) {
            self.choose(&field.name);
        }
    }

    /// The values of field `field` of `ds`, as written, and where they were read from, read
    /// unless they are held from the rows and fields `ds` has now. Says why there is nothing to
    /// list when there is none.
    fn list(&mut self, ds: &Dataset, field: usize) -> (Vec<String>, ValuesRead) {
        let (rows, fields) = (ds.features.allocation_key(), fields_key(ds));
        if !self.read.as_ref().is_some_and(|r| r.fields == fields && r.rows.ptr_eq(&rows)) {
            self.read = None;
        }
        #[cfg(test)]
        let reads = &mut self.reads;
        let read = self.read.get_or_insert_with(|| {
            #[cfg(test)]
            {
                *reads += 1;
            }
            let got = query::field_values(ds, field, query::VALUES_READ);
            let written = got.values.iter().map(|v| v.written.clone()).collect();
            Read { rows, fields, got, written }
        });
        let name = ds.fields.get(field).map(|f| f.name.as_str()).unwrap_or_default();
        self.said = read.written.is_empty().then(|| crate::i18n::fmt("{field} has no values to list", &[("field", &field_ref(name))]));
        (read.written.clone(), ValuesRead::of(&read.got, ds))
    }
}

/// The Values button, beside a clause's field buttons: opens the list of the chosen field's
/// values. Returns the field and the value accepted, as the expression writes it (`insert`
/// puts them in the clause). Without a field chosen, says to choose one.
pub(crate) fn button(ui: &mut Ui, form: &mut Form, ds: Option<&Dataset>) -> Option<(String, String)> {
    let label = tl!("Values ▾");
    let chosen = ds.zip(form.field.clone()).and_then(|(ds, name)| Some((ds, ds.fields.iter().position(|f| f.name == name)?, name)));
    let Some((ds, field, name)) = chosen else {
        if ui.button(label).clicked() {
            form.said = Some(tl!("Click a field, or put the cursor in one, to list its values").to_string());
        }
        return None;
    };
    let accepted = crate::exprcomplete::values_menu(ui, label, &name, || form.list(ds, field));
    accepted.map(|value| (name, value))
}

/// What the dialog says under its buttons about the list, when there is nothing to list.
pub(crate) fn said(ui: &mut Ui, form: &Form) {
    if let Some(said) = &form.said {
        ui.label(egui::RichText::new(said).small().weak());
    }
}

/// Adds `value` (as the expression writes it) of `field` to the end of the clause `text`, where
/// `query::complete_at` says the clause stands at its end (`ds` has the fields):
/// - where a value compared with a field goes (`name = `, `pop >`, `name IN (`, `pop BETWEEN `),
///   only the value, which `BETWEEN` takes for both ends (`BETWEEN 10 AND 10`, to edit) and a
///   bracket left open (`IN (`) is closed after;
/// - after a bare `IN` (a list is next), the value as a list of one (`IN ('Asia')`);
/// - anywhere else, the comparison with the field (`"Pop Est" = 10`).
///
/// A text or a quoted name begun and left open (`name = 'As`) is replaced by what is added, as the
/// completion does; a clause that can't be read at all gets the comparison, and the box says why
/// it doesn't read.
pub(crate) fn insert(text: &mut String, field: &str, value: &str, ds: &Dataset) {
    let mut at = at_end(text, ds);
    if let Some(open) = at.as_ref().filter(|c| c.place != Place::Code) {
        let keep = text.char_indices().nth(open.word.start).map_or(text.len(), |(i, _)| i);
        text.truncate(keep);
        at = at_end(text, ds);
    }
    let put = match &at {
        Some(c) if c.operand && c.field.is_some() => value.to_string(),
        Some(c) if !c.operand && c.symbols == ["("] => format!("({value})"),
        _ => format!("{} = {value}", field_ref(field)),
    };
    crate::dialogs::add_word(text, &put);
    // What the value left to finish (the end of the clause is where it was added).
    let then = at_end(text, ds);
    if then.as_ref().is_some_and(|c| !c.operand && c.keywords == [Kw::And]) {
        // `BETWEEN 10` needs `AND` and a second end.
        crate::dialogs::add_word(text, &format!("AND {value}"));
    }
    if at_end(text, ds).is_some_and(|c| c.symbols.contains(&")")) {
        text.pop();
        text.push_str(") ");
    }
}

/// What may go at the end of `text`, where a value is added: read with a space after it, so a
/// name or keyword typed last (`IN`) is a word complete, not one being typed.
fn at_end(text: &str, ds: &Dataset) -> Option<Completion> {
    let text = format!("{} ", text.trim_end());
    query::complete_at(&text, text.chars().count(), ds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn countries() -> Dataset {
        let app = crate::RustGisApp::with_sample();
        let layer = app.session.map().layer_by_name("Countries");
        layer.expect("the sample has Countries").dataset.clone()
    }

    /// The value goes where a value compared with a field goes, a list of one after a bare `IN`,
    /// and the comparison anywhere else; each result is the exact text, and reads on Countries.
    /// Every case is checked before the test fails, so one run shows them all.
    #[test]
    fn a_value_is_added_where_the_clause_takes_it_and_the_clause_still_reads() {
        let ds = countries();
        // (the clause, the field listed, the value listed, what the clause becomes)
        let cases = [
            // A clause starting, or a new one: the comparison, the field quoted if it needs it.
            ("", "continent", "'Asia'", "continent = 'Asia' "),
            ("continent = 'Asia' AND ", "pop_est", "10", "continent = 'Asia' AND pop_est = 10 "),
            ("continent = 'Asia' AND", "pop_est", "10", "continent = 'Asia' AND pop_est = 10 "),
            // After a comparison operator: the value alone, whatever field was listed.
            ("continent = ", "continent", "'Asia'", "continent = 'Asia' "),
            ("continent =", "continent", "'Asia'", "continent = 'Asia' "),
            ("pop_est >=", "pop_est", "10", "pop_est >= 10 "),
            ("continent LIKE", "continent", "'Asia'", "continent LIKE 'Asia' "),
            // A value with a quote is written as given (the list doubles it).
            ("", "name", "'O''Brien'", "name = 'O''Brien' "),
            ("name = ", "name", "'O''Brien'", "name = 'O''Brien' "),
            // In a list: the value, and the list closed.
            ("continent IN (", "continent", "'Asia'", "continent IN ('Asia') "),
            ("continent IN ('Asia',", "continent", "'Africa'", "continent IN ('Asia', 'Africa') "),
            // After a bare `IN` or `NOT IN`, a list is next.
            ("continent IN", "continent", "'Asia'", "continent IN ('Asia') "),
            ("continent not in ", "continent", "'Asia'", "continent not in ('Asia') "),
            // `BETWEEN` takes both ends: the value for each, to edit, or the upper end after `AND`.
            ("pop_est BETWEEN", "pop_est", "10", "pop_est BETWEEN 10 AND 10 "),
            ("pop_est BETWEEN 1 AND", "pop_est", "10", "pop_est BETWEEN 1 AND 10 "),
            ("(pop_est BETWEEN ", "pop_est", "10", "(pop_est BETWEEN 10 AND 10) "),
            // A text begun is replaced by the value.
            ("continent = 'As", "continent", "'Asia'", "continent = 'Asia' "),
        ];
        let mut wrong = Vec::new();
        for (clause, field, value, expected) in cases {
            let mut text = clause.to_string();
            insert(&mut text, field, value, &ds);
            if text != expected {
                wrong.push(format!("`{clause}` + {value} wrote `{text}`, not `{expected}`"));
            }
            if let Err(e) = query::select(&text, &ds) {
                wrong.push(format!("`{clause}` + {value} wrote `{text}`, which doesn't read: {e}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
        // A clause that doesn't read at all gets the comparison, and the box says why.
        let mut text = "pop_est > 1 ) AND".to_string();
        insert(&mut text, "continent", "'Asia'", &ds);
        assert_eq!(text, "pop_est > 1 ) AND continent = 'Asia' ");
    }

    /// The field a value typed at the cursor is compared with (what `follow_box` lists) is the
    /// one `complete_at` reads there, wherever in the clause; none outside a comparison.
    #[test]
    fn the_field_listed_is_the_one_the_value_at_the_cursor_is_compared_with() {
        let ds = countries();
        let of = |text: &str, cursor: usize| {
            query::complete_at(text, cursor, &ds).and_then(|c| c.field).and_then(|i| ds.fields.get(i)).map(|f| f.name.clone())
        };
        assert_eq!(of("continent = ", 12).as_deref(), Some("continent"));
        assert_eq!(of("pop_est > 1 AND continent IN ('a', ", 36).as_deref(), Some("continent"));
        assert_eq!(of("continent = 'x' AND pop_est > 1", 12).as_deref(), Some("continent"), "the text before the cursor only");
        assert_eq!(of("continent", 9), None, "no operator yet");
    }
}
