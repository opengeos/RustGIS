//! The wizard's placement step: which corner each surround takes.
use super::*;
use rustgis_engine::cmd::layout::{ARRANGEMENTS, Homes, corners_of, parse_arrangement, usable_saved_arrangement};

/// The arrangement `layout.publication` is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arrangement {
    /// The command's default.
    Best,
    Favourite,
    /// A built-in or saved arrangement, by name.
    Named(String),
    /// A corner for each surround.
    Custom(Homes),
}

impl Arrangement {
    /// The command's `"arrangement"`, or `None` to leave it to the command.
    pub(super) fn value(&self) -> Option<Value> {
        match self {
            Arrangement::Best => None,
            Arrangement::Favourite => Some(json!("favourite")),
            Arrangement::Named(name) => Some(json!(name)),
            Arrangement::Custom(homes) => Some(corners_of(*homes)),
        }
    }

    /// What making this the favourite saves: an arrangement chosen by name or corner by corner;
    /// `best` and the favourite itself aren't one.
    pub(super) fn as_favourite(&self) -> Option<Value> {
        matches!(self, Arrangement::Named(_) | Arrangement::Custom(_)).then(|| self.value()).flatten()
    }

    /// Its name, as the review says it.
    pub(super) fn label(&self) -> String {
        match self {
            Arrangement::Best => tl!("Best").to_string(),
            Arrangement::Favourite => tl!("Favourite").to_string(),
            Arrangement::Named(name) => i18n::display(built_in(name).unwrap_or(name)),
            Arrangement::Custom(_) => tl!("Custom").to_string(),
        }
    }

    /// The corners it gives, or `None` for `best`, which the command picks against the map.
    fn homes(&self, s: &Session) -> Option<Homes> {
        match self {
            Arrangement::Best => None,
            Arrangement::Favourite => Some(favourite_homes(s)),
            Arrangement::Named(name) => parse_arrangement(&json!(name), &s.layout_arrangements).ok(),
            Arrangement::Custom(homes) => Some(*homes),
        }
    }
}

/// A built-in arrangement's name as the step shows it, in logical order.
fn built_in(name: &str) -> Option<&'static str> {
    let labels = [tl_raw!("Classic"), tl_raw!("Mirrored"), tl_raw!("Legend right"), tl_raw!("Legend top")];
    ARRANGEMENTS.iter().zip(labels).find(|((n, _), _)| n.eq_ignore_ascii_case(name.trim())).map(|(_, label)| label)
}

/// The favourite's corners; one that can't be read is `classic`.
fn favourite_homes(s: &Session) -> Homes {
    parse_arrangement(&s.layout_favourite, &s.layout_arrangements).unwrap_or(ARRANGEMENTS[0].1)
}

/// The favourite as its option names it.
fn favourite_label(s: &Session) -> String {
    let name = s.layout_favourite.as_str().map_or(tl_raw!("custom corners"), |n| built_in(n).unwrap_or(n));
    i18n::fmt("Favourite: {name}", &[("name", name)])
}

/// The corners' names, in [`Homes`]'s numbering.
fn corner_labels() -> [&'static str; 4] {
    [tl_raw!("Top left"), tl_raw!("Top right"), tl_raw!("Bottom left"), tl_raw!("Bottom right")]
}

/// The surrounds' names, in [`Homes`]'s order.
fn surround_labels() -> [&'static str; 4] {
    [tl_raw!("Locator"), tl_raw!("North arrow"), tl_raw!("Legend"), tl_raw!("Scale bar")]
}

/// The placement step: the arrangements to pick from, and what goes in each corner.
pub(super) fn arrange_step(ui: &mut egui::Ui, s: &Session, d: &mut Draft) {
    let mut option = |ui: &mut egui::Ui, choice: Arrangement, label: &str| {
        let response = ui.radio(d.arrangement == choice, label);
        if response.clicked() {
            d.arrangement = choice;
        }
        response
    };
    ui.horizontal_wrapped(|ui| {
        option(ui, Arrangement::Best, tl!("Best"))
            .on_hover_text(tl!("The arrangement that moves the surrounds least off their corners and covers the map least."));
        option(ui, Arrangement::Favourite, &favourite_label(s));
    });
    ui.horizontal_wrapped(|ui| {
        for (name, _) in ARRANGEMENTS {
            let label = i18n::display(built_in(name).unwrap_or(name));
            option(ui, Arrangement::Named(name.into()), &label);
        }
        // Those saved with `layout.arrangement` that this version reads, under their own names.
        for (name, _) in s.layout_arrangements.iter().filter(|(n, c)| usable_saved_arrangement(n, c)) {
            option(ui, Arrangement::Named(name.clone()), &i18n::display(name));
        }
    });
    let custom = matches!(d.arrangement, Arrangement::Custom(_));
    if ui.radio(custom, tl!("Custom")).clicked() && !custom {
        d.arrangement = Arrangement::Custom(d.arrangement.homes(s).unwrap_or_else(|| favourite_homes(s)));
    }
    if let Arrangement::Custom(homes) = &mut d.arrangement {
        corner_pickers(ui, homes);
    }
    if let Some(homes) = d.arrangement.homes(s) {
        ui.add_space(4.0);
        egui::Grid::new("publication_wizard_corners").num_columns(2).spacing(egui::vec2(24.0, 6.0)).show(ui, |ui| {
            for (corner, name) in corner_labels().into_iter().enumerate() {
                if let Some(surround) = homes.iter().position(|&h| h == corner) {
                    let values = [("corner", name), ("surround", surround_labels()[surround])];
                    ui.group(|ui| ui.label(i18n::fmt("{corner}: {surround}", &values)));
                }
                if corner % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    }
    // The styles with a column keep the locator and legend in it (`cmd/layout/publication`).
    if !d.figure() && matches!(d.style.as_str(), "panel" | "editorial" | "modular") {
        let note = tl!("The locator and legend sit in this style's column: only the north arrow and scale bar take these corners.");
        ui.label(egui::RichText::new(note).small().weak());
    }
    ui.add_space(4.0);
    ui.add_enabled_ui(d.arrangement.as_favourite().is_some(), |ui| {
        ui.checkbox(&mut d.make_favourite, tl!("Make this my favourite"))
            .on_hover_text(tl!("Create also keeps it with the app's preferences as the favourite arrangement."));
    });
}

/// A corner for each surround; one picked that another has swaps the two, so each keeps a corner
/// of its own.
fn corner_pickers(ui: &mut egui::Ui, homes: &mut Homes) {
    egui::Grid::new("publication_wizard_custom").num_columns(2).spacing(egui::vec2(8.0, 6.0)).show(ui, |ui| {
        for (surround, name) in surround_labels().into_iter().enumerate() {
            let label = ui.label(i18n::display(name));
            let corners = corner_labels();
            egui::ComboBox::from_id_salt(("publication_wizard_corner", surround))
                .selected_text(i18n::display(corners[homes[surround]]))
                .width(field_width(ui))
                .show_ui(ui, |ui| {
                    for (corner, name) in corners.into_iter().enumerate() {
                        if ui.selectable_label(homes[surround] == corner, i18n::display(name)).clicked()
                            && let Some(other) = homes.iter().position(|&h| h == corner)
                        {
                            homes.swap(surround, other);
                        }
                    }
                })
                .response
                .labelled_by(label.id);
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::super::harness::*;
    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};
    use rustgis_engine::cmd::layout::{ARRANGEMENTS, parse_arrangement};

    type Wizard = egui_kittest::Harness<'static, RustGisApp>;

    /// The wizard on its placement step.
    fn placement(prepare: impl FnOnce(&mut RustGisApp)) -> Wizard {
        let mut app = app();
        open(&mut app);
        app.dialogs.publication.as_mut().unwrap().step = Step::Placement;
        prepare(&mut app);
        harness(app)
    }

    fn draft(h: &Wizard) -> &Draft {
        h.state().dialogs.publication.as_ref().expect("the wizard is open")
    }

    fn pick(h: &mut Wizard, label: &str) {
        h.get_by_role_and_label(Role::RadioButton, label).click();
        h.run_steps(4);
    }

    /// What the 2 × 2 grid says, corner by corner.
    fn shown(h: &Wizard, text: &str) -> bool {
        h.query_by_label(text).is_some()
    }

    /// A new draft asks for no arrangement, as before the step, and makes no favourite.
    #[test]
    fn a_new_draft_leaves_the_arrangement_to_the_command() {
        let app = app();
        let d = Draft::new(&app.session);
        assert_eq!((d.arrangement.clone(), d.make_favourite), (Arrangement::Best, false));
        assert!(d.params().get("arrangement").is_none());
    }

    /// A sheet and a figure both send the arrangement chosen.
    #[test]
    fn the_arrangement_chosen_is_sent_for_a_sheet_and_a_figure() {
        let app = app();
        for size in ["a4", "figure-1col"] {
            let mut d = Draft::new(&app.session);
            d.size = size.into();
            d.arrangement = Arrangement::Named("mirrored".into());
            assert_eq!(d.params()["arrangement"], "mirrored", "{size}");
            d.arrangement = Arrangement::Favourite;
            assert_eq!(d.params()["arrangement"], "favourite", "{size}");
        }
    }

    /// Each arrangement by name sends its name and shows its corners.
    #[test]
    fn each_built_in_arrangement_sends_its_name_and_shows_its_corners() {
        let mut h = placement(|_| {});
        for ((name, homes), label) in ARRANGEMENTS.iter().zip(["Classic", "Mirrored", "Legend right", "Legend top"]) {
            pick(&mut h, label);
            assert_eq!(draft(&h).params()["arrangement"], *name);
            for (surround, corner) in ["Locator", "North arrow", "Legend", "Scale bar"].iter().zip(homes) {
                let corner = ["Top left", "Top right", "Bottom left", "Bottom right"][*corner];
                assert!(shown(&h, &format!("{corner}: {surround}")), "{name}: {surround} at {corner}");
            }
        }
    }

    /// The favourite is named: classic unset, a name, a saved one, or corners of its own.
    #[test]
    fn the_favourite_is_listed_with_its_name() {
        let h = placement(|_| {});
        assert!(h.query_by_role_and_label(Role::RadioButton, "Favourite: Classic").is_some());
        let own = json!({"locator": "bottom-right", "northArrow": "top-left", "legend": "top-right", "scaleBar": "bottom-left"});
        for (favourite, label) in [(json!("mirrored"), "Favourite: Mirrored"), (own, "Favourite: custom corners")] {
            let mut h = placement(|app| app.session.layout_favourite = favourite);
            pick(&mut h, label);
            assert_eq!(draft(&h).params()["arrangement"], "favourite");
        }
    }

    /// Each saved arrangement is listed under its name; with none saved, there is nothing more.
    #[test]
    fn saved_arrangements_are_listed_by_name() {
        let corners = json!({"locator": "bottom-right", "northArrow": "top-left", "legend": "top-right", "scaleBar": "bottom-left"});
        let h = placement(|_| {});
        assert!(h.query_by_role_and_label(Role::RadioButton, "Atlas").is_none());
        let mut h = placement(|app| {
            app.session.layout_arrangements.insert("Atlas".into(), corners.clone());
        });
        pick(&mut h, "Atlas");
        assert_eq!(draft(&h).params()["arrangement"], "Atlas");
        assert!(shown(&h, "Top right: Legend"));
    }

    /// Custom starts from the corners of what was selected: the favourite's for Best and
    /// Favourite.
    #[test]
    fn custom_starts_from_the_corners_selected() {
        let mut h = placement(|app| app.session.layout_favourite = json!("mirrored"));
        pick(&mut h, "Custom");
        assert_eq!(draft(&h).arrangement, Arrangement::Custom(ARRANGEMENTS[1].1), "from Best: the favourite's");
        pick(&mut h, "Legend top");
        pick(&mut h, "Custom");
        assert_eq!(draft(&h).arrangement, Arrangement::Custom(ARRANGEMENTS[3].1));
        let params = draft(&h).params();
        assert_eq!(parse_arrangement(&params["arrangement"], &Default::default()), Ok(ARRANGEMENTS[3].1));
    }

    /// Giving a surround a corner another has swaps the two, whichever corner it is.
    #[test]
    fn picking_a_taken_corner_swaps_the_two_surrounds() {
        for (corner, name) in ["Top left", "Top right", "Bottom left", "Bottom right"].iter().enumerate() {
            let mut h = placement(|app| app.dialogs.publication.as_mut().unwrap().arrangement = Arrangement::Custom([0, 1, 2, 3]));
            // The legend (at 2) is given each corner in turn.
            h.get_by_role_and_label(Role::ComboBox, "Legend").click();
            h.run_steps(4);
            h.get_by_role_and_label(Role::Button, name).click();
            h.run_steps(4);
            let mut want = [0, 1, 2, 3];
            want.swap(2, corner);
            assert_eq!(draft(&h).arrangement, Arrangement::Custom(want), "{name}");
        }
    }

    /// The styles with a column say that the locator and legend sit in it.
    #[test]
    fn column_styles_say_which_surrounds_take_the_corners() {
        for (style, column) in
            [("classic", false), ("banner", false), ("panel", true), ("editorial", true), ("modular", true), ("poster", false)]
        {
            let h = placement(|app| app.dialogs.publication.as_mut().unwrap().style = style.into());
            assert_eq!(h.query_by_label_contains("sit in this style's column").is_some(), column, "{style}");
        }
        let h = placement(|app| {
            let d = app.dialogs.publication.as_mut().unwrap();
            (d.style, d.size) = ("panel".into(), "figure-1col".into());
        });
        assert!(h.query_by_label_contains("sit in this style's column").is_none(), "a figure has no column");
    }

    /// Only a chosen arrangement can be made the favourite.
    #[test]
    fn make_favourite_is_off_for_best_and_favourite() {
        let mut h = placement(|_| {});
        let off = |h: &Wizard| h.get_by_role_and_label(Role::CheckBox, "Make this my favourite").accesskit_node().is_disabled();
        assert!(off(&h), "Best");
        pick(&mut h, "Favourite: Classic");
        assert!(off(&h), "Favourite");
        pick(&mut h, "Mirrored");
        assert!(!off(&h));
        pick(&mut h, "Custom");
        assert!(!off(&h));
    }

    /// The wizard on its review, with `arrangement` to be made the favourite.
    fn creating(arrangement: Arrangement, prepare: impl FnOnce(&mut Draft)) -> Wizard {
        let mut app = app();
        open(&mut app);
        let d = app.dialogs.publication.as_mut().unwrap();
        (d.step, d.arrangement, d.make_favourite) = (Step::Review, arrangement, true);
        prepare(d);
        harness(app)
    }

    /// Create sets the favourite, by name or by corners; Undo takes the layout, not the
    /// favourite.
    #[test]
    fn create_makes_the_chosen_arrangement_the_favourite() {
        let custom = [3, 0, 1, 2];
        for (arrangement, want) in [
            (Arrangement::Named("mirrored".into()), json!("mirrored")),
            (Arrangement::Custom(custom), rustgis_engine::cmd::layout::corners_of(custom)),
        ] {
            let mut h = creating(arrangement, |_| {});
            let before = h.state().session.project.layouts.len();
            click(&mut h, "Create");
            assert_eq!(h.state().session.project.layouts.len(), before + 1);
            assert_eq!(h.state().session.layout_favourite, want);
            assert_eq!(h.state().ui.tab, "Publish");
            h.state_mut().run("edit.undo", Value::Null).unwrap();
            assert_eq!(h.state().session.project.layouts.len(), before);
            assert_eq!(h.state().session.layout_favourite, want, "a preference, not undone");
        }
    }

    /// A flag left ticked behind Best or Favourite sets nothing.
    #[test]
    fn create_leaves_the_favourite_for_best_and_favourite() {
        for arrangement in [Arrangement::Best, Arrangement::Favourite] {
            let mut h = creating(arrangement, |_| {});
            click(&mut h, "Create");
            assert_eq!(h.state().session.layout_favourite, json!("classic"));
        }
    }

    /// A layout the command refuses changes no favourite and keeps the wizard open: an unknown
    /// style, or a saved arrangement removed meanwhile.
    #[test]
    fn a_refused_layout_leaves_the_favourite() {
        for (arrangement, style) in [(Arrangement::Named("mirrored".into()), "unknown"), (Arrangement::Named("Gone".into()), "classic")] {
            let mut h = creating(arrangement, |d| d.style = style.into());
            let before = h.state().session.project.layouts.len();
            click(&mut h, "Create");
            assert_eq!(h.state().session.project.layouts.len(), before, "nothing made");
            assert_eq!(h.state().session.layout_favourite, json!("classic"));
            assert_eq!(h.state().dialogs.open, Some("publicationWizard"));
        }
    }

    /// The review says the placement, of a sheet and of a figure.
    #[test]
    fn the_review_says_the_placement() {
        for size in ["a4", "figure-1col"] {
            let h = creating(Arrangement::Named("legendTop".into()), |d| d.size = size.into());
            assert!(h.query_by_label("Legend top").is_some(), "{size}");
        }
    }
}
