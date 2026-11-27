//! The wizard's sources and reference step.
use super::*;

/// The most lines `layout.publication` takes in a list.
const MAX_LINES: usize = 50;

/// The sources step: the sources, the cartographic reference and their headings.
pub(super) fn sources_step(ui: &mut egui::Ui, d: &mut Draft, map: &Map) {
    egui::Grid::new("publication_wizard_sources").num_columns(2).spacing(egui::vec2(8.0, 8.0)).show(ui, |ui| {
        list(ui, d, tl!("Sources"), Field::Sources, "", 3, true);
        heading(ui, d, tl!("Sources heading"), 0, true);
        let on = !d.no_reference;
        list(ui, d, tl!("Reference"), Field::Reference, &PUBLICATION_DEFAULT_REFERENCE.join("\n"), 2, on);
        ui.label("");
        ui.checkbox(&mut d.no_reference, tl!("No cartographic reference"));
        ui.end_row();
        heading(ui, d, tl!("Reference heading"), 1, !d.no_reference);
    });
    statistic_menu(ui, d, map, Field::Sources);
    let mut fields = vec![Field::Sources, Field::Heading(0)];
    if !d.no_reference {
        fields.extend([Field::Reference, Field::Heading(1)]);
    }
    limit_note(ui, d, &fields);
}

/// A box taking one entry per line, with a note where it holds more than the command takes.
fn list(ui: &mut egui::Ui, d: &mut Draft, label: &str, field: Field, hint: &str, rows: usize, on: bool) {
    let caption = ui.label(label);
    let edit = egui::TextEdit::multiline(d.field_mut(field))
        .hint_text(hint)
        .desired_rows(rows)
        .char_limit(MAX_PUBLICATION_TEXT)
        .desired_width(field_width(ui));
    let r = ui.add_enabled(on, edit).labelled_by(caption.id);
    d.edited(&r, field);
    ui.end_row();
    if on && lines(d.field_mut(field)).count() > MAX_LINES {
        ui.label("");
        let note = i18n::fmt("At most {limit} lines.", &[("limit", &MAX_LINES.to_string())]);
        ui.label(egui::RichText::new(note).small().weak());
        ui.end_row();
    }
}

/// A section's heading, the command's own as its hint.
fn heading(ui: &mut egui::Ui, d: &mut Draft, label: &str, at: usize, on: bool) {
    let field = Field::Heading(at);
    let caption = ui.label(label);
    let edit = egui::TextEdit::singleline(d.field_mut(field))
        .hint_text(PUBLICATION_HEADINGS.get(at).copied().unwrap_or_default())
        .char_limit(MAX_PUBLICATION_TEXT)
        .desired_width(field_width(ui));
    let r = ui.add_enabled(on, edit).labelled_by(caption.id);
    d.edited(&r, field);
    ui.end_row();
}

#[cfg(test)]
mod tests {
    use super::harness::*;
    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};
    use rustgis_engine::map::layout::ElementKind;

    type Wizard = egui_kittest::Harness<'static, RustGisApp>;

    fn draft() -> Draft {
        Draft::new(&app().session)
    }

    /// The wizard on `step`, over the sample project (its map has vector layers).
    fn at(step: Step, set: impl FnOnce(&mut Draft)) -> Wizard {
        let mut app = app();
        open(&mut app);
        let d = app.dialogs.publication.as_mut().unwrap();
        d.step = step;
        set(d);
        harness(app)
    }

    fn wizard(h: &Wizard) -> &Draft {
        h.state().dialogs.publication.as_ref().expect("the wizard is open")
    }

    /// The first vector layer of the active map, which the statistic menu lists.
    fn layer(h: &Wizard) -> String {
        crate::textform::StatLayer::of_map(h.state().session.map()).first().expect("a vector layer").name.clone()
    }

    /// Insert Statistic › the first layer › Count.
    fn insert_count(h: &mut Wizard) -> String {
        let name = layer(h);
        let token = format!("{{count:{name}}}");
        for label in ["Insert Statistic", &name, &format!("Count  {token}")] {
            h.get_by_label(label).click();
            h.run_steps(4);
        }
        token
    }

    /// Sources go one per line, trimmed, the empty lines left out; none at all sends no key.
    #[test]
    fn sources_are_sent_one_per_line() {
        let mut d = draft();
        d.sources = "A\n\n B \n".into();
        assert_eq!(d.params()["sources"], json!(["A", "B"]));
        d.sources = "  \n ".into();
        assert!(d.params().get("sources").is_none());
    }

    /// An empty reference is left to the command, one typed goes line by line, and none at all is
    /// an empty list.
    #[test]
    fn the_reference_is_the_commands_lines_or_none() {
        let mut d = draft();
        assert!(d.params().get("reference").is_none());
        d.reference = "X\nY".into();
        assert_eq!(d.params()["reference"], json!(["X", "Y"]));
        d.no_reference = true;
        assert_eq!(d.params()["reference"], json!([]));
    }

    /// Headings are sent up to the last one given, a gap filled with the command's own; the
    /// column's only for the editorial style.
    #[test]
    fn headings_are_sent_up_to_the_last_one_given() {
        let mut d = draft();
        assert!(d.params().get("headings").is_none());
        d.headings[0] = " S ".into();
        assert!(d.params().get("headings").is_none(), "no sources, so no section to head");
        d.sources = "IBGE".into();
        assert_eq!(d.params()["headings"], json!(["S"]));
        d.headings = [String::new(), "Ref".into(), String::new()];
        assert_eq!(d.params()["headings"], json!([PUBLICATION_HEADINGS[0], "Ref"]));
        d.headings = [String::new(), String::new(), "Col".into()];
        assert!(d.params().get("headings").is_none(), "no column on a classic sheet");
        d.style = "editorial".into();
        assert_eq!(d.params()["headings"], json!([PUBLICATION_HEADINGS[0], PUBLICATION_HEADINGS[1], "Col"]));
    }

    /// A section left out takes no heading with it: not the reference's where there is none.
    #[test]
    fn a_section_left_out_sends_no_heading() {
        let mut d = draft();
        d.headings = [String::new(), "Ref".into(), String::new()];
        d.no_reference = true;
        assert!(d.params().get("headings").is_none());
        d.style = "editorial".into();
        d.headings[2] = "Col".into();
        assert_eq!(d.params()["headings"], json!([PUBLICATION_HEADINGS[0], PUBLICATION_HEADINGS[1], "Col"]));
    }

    /// A figure takes none of them.
    #[test]
    fn a_figure_takes_no_sources_reference_or_headings() {
        let mut d = draft();
        (d.sources, d.reference, d.no_reference) = ("A".into(), "B".into(), true);
        d.headings[0] = "S".into();
        d.size = "figure-1col".into();
        assert_eq!(d.params(), json!({"profile": "figure-1col"}));
    }

    /// The step has its fields, and No cartographic reference turns the reference's box off.
    #[test]
    fn the_step_asks_for_the_sources_the_reference_and_their_headings() {
        let mut h = at(Step::Sources, |_| {});
        for label in ["Sources", "Sources heading", "Reference", "Reference heading"] {
            assert!(h.query_by_label(label).is_some(), "{label}");
        }
        assert!(!h.get_by_label("Reference").accesskit_node().is_disabled());
        h.get_by_role_and_label(Role::CheckBox, "No cartographic reference").click();
        h.run_steps(4);
        assert!(wizard(&h).no_reference);
        assert!(h.get_by_label("Reference").accesskit_node().is_disabled());
    }

    /// The column's heading is asked with the texts, for the editorial style alone.
    #[test]
    fn only_an_editorial_sheet_is_asked_its_columns_heading() {
        let mut h = at(Step::Texts, |_| {});
        assert!(h.query_by_label("Column heading").is_none());
        h.state_mut().dialogs.publication.as_mut().unwrap().style = "editorial".into();
        h.run_steps(4);
        assert!(h.query_by_label("Column heading").is_some());
    }

    /// A statistic goes to the field last edited, here the subtitle.
    #[test]
    fn a_statistic_goes_to_the_field_last_edited() {
        let mut h = at(Step::Texts, |d| d.subtitle = "In ".into());
        h.get_by_label("Subtitle").focus();
        h.run_steps(2);
        let token = insert_count(&mut h);
        assert_eq!(wizard(&h).subtitle, format!("In {token}"));
        assert!(wizard(&h).note.is_empty());
    }

    /// Each step keeps its own: a field edited on the sources step leaves the texts' last one.
    #[test]
    fn each_step_keeps_the_field_last_edited_on_it() {
        let mut h = at(Step::Texts, |_| {});
        h.get_by_label("Subtitle").focus();
        h.run_steps(2);
        click(&mut h, "Next");
        h.get_by_label("Sources heading").focus();
        h.run_steps(2);
        click(&mut h, "Back");
        let token = insert_count(&mut h);
        assert_eq!(wizard(&h).subtitle, token);
        assert!(wizard(&h).note.is_empty());
        click(&mut h, "Next");
        insert_count(&mut h);
        assert_eq!(wizard(&h).headings[0], token);
    }

    /// With nothing edited on the step it goes to the note, or to the sources on their step, a
    /// field edited on another step left alone.
    #[test]
    fn with_no_field_edited_a_statistic_goes_to_the_steps_own() {
        let mut h = at(Step::Texts, |_| {});
        let token = insert_count(&mut h);
        assert_eq!(wizard(&h).note, token);
        let mut h = at(Step::Sources, |d| d.targets[0] = Some(Field::Subtitle));
        let token = insert_count(&mut h);
        assert_eq!(wizard(&h).sources, token);
        assert!(wizard(&h).subtitle.is_empty());
    }

    /// A statistic put in the title is a title typed: a map opened afterwards doesn't replace it.
    #[test]
    fn a_statistic_in_the_title_keeps_the_title() {
        let mut h = at(Step::Texts, |d| d.targets[0] = Some(Field::Title));
        insert_count(&mut h);
        assert!(wizard(&h).title_typed);
    }

    /// A statistic that would take its field past the limit isn't added, and that is said.
    #[test]
    fn a_statistic_past_the_limit_isnt_added_and_its_said() {
        let full = "x".repeat(MAX_PUBLICATION_TEXT - 3);
        // A one-line field: a box this full of text would push the menu out of the step's view.
        let mut h = at(Step::Sources, |d| (d.headings[0], d.targets[1]) = (full.clone(), Some(Field::Heading(0))));
        let limit = format!("Each field takes at most {MAX_PUBLICATION_TEXT} characters.");
        assert!(h.query_by_label(&limit).is_none());
        insert_count(&mut h);
        assert_eq!(wizard(&h).headings[0], full);
        assert!(wizard(&h).sources.is_empty());
        assert!(h.query_by_label(&limit).is_some());
    }

    /// A map with no vector layers has no statistic menu.
    #[test]
    fn a_map_without_vector_layers_has_no_statistic_menu() {
        for step in [Step::Texts, Step::Sources] {
            let mut h = at(step, |_| {});
            assert!(h.query_by_label("Insert Statistic").is_some(), "{step:?}");
            h.state_mut().run("map.new", json!({"name": "Empty"})).unwrap();
            h.run_steps(4);
            assert!(h.query_by_label("Insert Statistic").is_none(), "{step:?}");
        }
    }

    /// More than the command's 50 lines is said under the box.
    #[test]
    fn more_than_fifty_lines_are_said() {
        let said = "At most 50 lines.";
        let mut h = at(Step::Sources, |d| d.sources = "a\n".repeat(50));
        assert!(h.query_by_label(said).is_none());
        h.state_mut().dialogs.publication.as_mut().unwrap().reference = "a\n".repeat(51);
        h.run_steps(4);
        assert!(h.query_by_label(said).is_some());
    }

    /// The review lists the sources, the reference and the headings, the defaults where nothing
    /// was typed and "(none)" for no reference.
    #[test]
    fn the_review_lists_sources_reference_and_headings() {
        let mut h = at(Step::Review, |d| d.sources = "IBGE\nANA".into());
        assert!(h.query_by_label("IBGE\nANA").is_some());
        let default = "The default: projection, datum and graticule, filled in on the sheet";
        assert!(h.query_by_label(default).is_some(), "said in words, not as tokens");
        assert!(h.query_by_label(PUBLICATION_HEADINGS[1]).is_some());
        let d = h.state_mut().dialogs.publication.as_mut().unwrap();
        d.no_reference = true;
        d.style = "editorial".into();
        d.headings = ["Fontes".into(), String::new(), "About".into()];
        h.run_steps(4);
        assert!(h.query_by_label(default).is_none());
        assert!(h.query_by_label("Fontes").is_some() && h.query_by_label("About").is_some());
    }

    /// Create puts the sources, the reference and the heading given on the sheet, and Undo takes
    /// it away.
    #[test]
    fn create_puts_them_on_the_sheet() {
        let mut h = at(Step::Review, |d| {
            (d.sources, d.reference) = ("IBGE\nANA".into(), "UTM 23S".into());
            d.headings[0] = "Fontes".into();
        });
        let before = h.state().session.project.layouts.len();
        click(&mut h, "Create");
        let layout = h.state().session.project.layouts.last().unwrap();
        let text = |name: &str| {
            layout.elements.iter().find(|e| e.name == name).and_then(|e| match &e.kind {
                ElementKind::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
        };
        assert_eq!(text("Sources").as_deref(), Some("IBGE\nANA"));
        assert_eq!(text("Reference").as_deref(), Some("UTM 23S"));
        assert!(layout.elements.iter().any(|e| matches!(&e.kind, ElementKind::Text { text, .. } if text.eq_ignore_ascii_case("Fontes"))));
        h.state_mut().run("edit.undo", Value::Null).unwrap();
        assert_eq!(h.state().session.project.layouts.len(), before);
    }
}
