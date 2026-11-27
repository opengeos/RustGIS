//! The wizard's review step.
use super::*;

/// The review step: what was picked, step by step.
pub(super) fn review_step(ui: &mut egui::Ui, d: &Draft) {
    let none = tl!("(none)");
    let or_none = |t: &str| if t.trim().is_empty() { none.to_string() } else { t.trim().to_string() };
    egui::Grid::new("publication_wizard_review").num_columns(2).spacing(egui::vec2(12.0, 6.0)).show(ui, |ui| {
        let mut row = |key: &str, value: String| {
            ui.label(egui::RichText::new(key).weak());
            ui.label(value);
            ui.end_row();
        };
        row(tl!("Size"), size_label(&d.size));
        if d.figure() {
            return;
        }
        let turned = match d.orientation {
            Orientation::Auto => tl!("Auto: the way the map comes out larger"),
            Orientation::Portrait => tl!("Portrait"),
            Orientation::Landscape => tl!("Landscape"),
        };
        row(tl!("Orientation"), turned.to_string());
        let style = crate::ribbon::publication_style(&d.style).map(|s| s.0.to_string()).unwrap_or_else(|| d.style.clone());
        row(tl!("Style"), style);
        row(tl!("Title"), or_none(&d.title));
        row(tl!("Subtitle"), or_none(&d.subtitle));
        row(tl!("Note"), or_none(&d.note));
        let sources = lines(&d.sources).join("\n");
        row(tl!("Sources"), or_none(&sources));
        let heading = |i: usize, or: &str| {
            let given = d.headings[i].trim();
            if given.is_empty() { or.to_string() } else { given.to_string() }
        };
        if !sources.is_empty() {
            row(tl!("Sources heading"), heading(0, PUBLICATION_HEADINGS[0]));
        }
        let typed = lines(&d.reference).join("\n");
        if d.no_reference {
            row(tl!("Reference"), none.to_string());
        } else {
            let default = tl!("The default: projection, datum and graticule, filled in on the sheet");
            row(tl!("Reference"), if typed.is_empty() { default.to_string() } else { typed });
            row(tl!("Reference heading"), heading(1, PUBLICATION_HEADINGS[1]));
        }
        if d.editorial() {
            row(tl!("Column heading"), heading(2, none));
        }
        let credit = if d.no_author {
            none.to_string()
        } else if d.author.trim().is_empty() {
            default_credit()
        } else {
            d.author.trim().to_string()
        };
        row(tl!("Credit line"), credit);
        let accent = d.accent.map(hex).unwrap_or_else(|| tl!("The style's own").to_string());
        row(tl!("Accent"), accent);
    });
    if d.figure() {
        figure_note(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::harness::*;
    use super::*;
    use egui_kittest::kittest::Queryable;

    /// The review says what will be drawn: the default credit line, dated when the layout is made, and for
    /// a figure, that its texts and accent are left to the caption.
    #[test]
    fn the_review_says_what_will_be_drawn() {
        let mut app = app();
        open(&mut app);
        app.dialogs.publication.as_mut().unwrap().step = Step::Review;
        let mut h = harness(app);
        assert!(h.query_by_label("Made with RustGIS · today's date").is_some());
        assert!(h.query_by_label("Auto: the way the map comes out larger").is_some(), "the turn, decided at Create");
        h.state_mut().dialogs.publication.as_mut().unwrap().size = "figure-1col".into();
        h.run_steps(2);
        assert!(h.query_by_label_contains("its caption").is_some(), "a figure's texts aren't drawn");
    }
}
