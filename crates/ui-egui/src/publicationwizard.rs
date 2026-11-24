//! Publication Layout Wizard: `layout.publication` asked step by step (size, style, texts, accent),
//! then made at once, as the New Layout › Publication Layout menu makes it (#2850).
use crate::{RustGisApp, i18n, theme::Tokens};
use rustgis_engine::Session;
use rustgis_engine::cmd::layout::{PUBLICATION_PROFILES, PUBLICATION_STYLES};
use rustgis_engine::map::layout::PAGES;
use serde_json::{Value, json};

/// The wizard's steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Size,
    Style,
    Texts,
    Accent,
    Review,
}

/// How a sheet is turned: `Auto` leaves it to `layout.publication` (the way the map comes out larger).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Auto,
    Portrait,
    Landscape,
}

/// What the wizard has asked so far: `layout.publication`'s fields, under the command's names.
pub struct Draft {
    pub step: Step,
    /// A page size's id (`a4`, from [`PAGES`]) or a figure's profile id (`figure-1col`).
    pub size: String,
    pub orientation: Orientation,
    pub style: String,
    pub title: String,
    pub subtitle: String,
    pub note: String,
    pub sources: String,
    pub author: String,
    /// No credit line at all (`"author": ""`), rather than the command's default one.
    pub no_author: bool,
    /// The style's colour, or `None` for the style's own.
    pub accent: Option<egui::Color32>,
}

impl Draft {
    /// What the menu makes: an A4 sheet turned as the map comes out larger, the classic style and
    /// the map's name as the title.
    pub fn new(s: &Session) -> Self {
        Self {
            step: Step::Size,
            size: "a4".into(),
            orientation: Orientation::Auto,
            style: "classic".into(),
            title: s.map().name.clone(),
            subtitle: String::new(),
            note: String::new(),
            sources: String::new(),
            author: String::new(),
            no_author: false,
            accent: None,
        }
    }

    /// Whether the size is a figure, which takes no style, texts or accent: the command draws the
    /// map edge to edge and leaves the rest to the caption.
    fn figure(&self) -> bool {
        PUBLICATION_PROFILES.iter().any(|p| !p.sheet && p.id == self.size)
    }

    /// The steps this draft goes through, in order.
    fn steps(&self) -> &'static [Step] {
        if self.figure() { &[Step::Size, Step::Review] } else { &[Step::Size, Step::Style, Step::Texts, Step::Accent, Step::Review] }
    }

    /// The step `by` places from this one, if there is one.
    fn moved(&self, by: isize) -> Option<Step> {
        let steps = self.steps();
        let at = steps.iter().position(|s| *s == self.step)?;
        steps.get(at.checked_add_signed(by)?).copied()
    }

    /// `layout.publication`'s parameters: the fields set; one left empty is left to the command's
    /// default, but for the title, which empty means none.
    pub fn params(&self) -> Value {
        let profile = match (self.figure(), self.orientation) {
            (true, _) | (false, Orientation::Auto) => self.size.clone(),
            (false, Orientation::Portrait) => format!("{}-portrait", self.size),
            (false, Orientation::Landscape) => format!("{}-landscape", self.size),
        };
        let mut p = json!({"profile": profile});
        if self.figure() {
            return p;
        }
        p["style"] = json!(self.style);
        p["title"] = json!(self.title.trim());
        for (key, text) in [("subtitle", &self.subtitle), ("note", &self.note), ("sources", &self.sources), ("author", &self.author)] {
            if !text.trim().is_empty() {
                p[key] = json!(text.trim());
            }
        }
        if self.no_author {
            p["author"] = json!("");
        }
        if let Some(c) = self.accent {
            p["accent"] = json!(format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()));
        }
        p
    }

    /// The size as the menu names it: `A4 (210 × 297 mm)`, or a figure's name.
    fn size_label(&self) -> String {
        size_label(&self.size)
    }
}

/// A page size's or a figure's name, as New Layout › Publication Layout lists it.
fn size_label(id: &str) -> String {
    if let Some((_, label, w, h)) = PAGES.iter().find(|p| p.0 == id) {
        return crate::ribbon::page_label(label, *w, *h);
    }
    let profile = PUBLICATION_PROFILES.iter().find(|p| p.id == id);
    crate::ribbon::publication_label(id)
        .map(str::to_string)
        .or_else(|| profile.map(|p| tl!(p.label).to_string()))
        .unwrap_or_else(|| id.to_string())
}

/// A step's name, as its heading and the review give it.
fn step_name(step: Step) -> &'static str {
    match step {
        Step::Size => tl!("Size"),
        Step::Style => tl!("Style"),
        Step::Texts => tl!("Texts"),
        Step::Accent => tl!("Accent"),
        Step::Review => tl!("Review"),
    }
}

/// The size step: a sheet on any page size New Layout offers, turned as asked, or a figure.
fn size_step(ui: &mut egui::Ui, d: &mut Draft) {
    egui::Grid::new("publication_wizard_size").num_columns(2).spacing(egui::vec2(8.0, 8.0)).show(ui, |ui| {
        let label = ui.label(tl!("Size"));
        egui::ComboBox::from_id_salt("publication_wizard_size_pick")
            .selected_text(d.size_label())
            .width(260.0)
            .show_ui(ui, |ui| {
                for (id, ..) in PAGES {
                    ui.selectable_value(&mut d.size, (*id).to_string(), size_label(id));
                }
                ui.separator();
                for p in PUBLICATION_PROFILES.iter().filter(|p| !p.sheet) {
                    ui.selectable_value(&mut d.size, p.id.to_string(), size_label(p.id));
                }
            })
            .response
            .labelled_by(label.id);
        ui.end_row();
        ui.label(tl!("Orientation"));
        ui.add_enabled_ui(!d.figure(), |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.orientation, Orientation::Auto, tl!("Auto")).on_hover_text(tl!("The way the map comes out larger."));
                ui.radio_value(&mut d.orientation, Orientation::Portrait, tl!("Portrait"));
                ui.radio_value(&mut d.orientation, Orientation::Landscape, tl!("Landscape"));
            });
        });
        ui.end_row();
    });
    if d.figure() {
        ui.label(egui::RichText::new(tl!("A figure is the map edge to edge: its title and sources go in its caption.")).small().weak());
    }
}

/// The style step: each sheet style, by the name and description the menu gives it.
fn style_step(ui: &mut egui::Ui, d: &mut Draft) {
    for (id, name) in PUBLICATION_STYLES {
        let (name, about) = crate::ribbon::publication_style(id).unwrap_or_else(|| (tl!(*name), ""));
        ui.radio_value(&mut d.style, (*id).to_string(), name);
        ui.indent(*id, |ui| ui.label(egui::RichText::new(about).small().weak()));
    }
}

/// The texts step: the title, subtitle, note, sources and credit line.
fn texts_step(ui: &mut egui::Ui, d: &mut Draft) {
    egui::Grid::new("publication_wizard_texts").num_columns(2).spacing(egui::vec2(8.0, 8.0)).show(ui, |ui| {
        let line = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str| {
            let label = ui.label(label);
            ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(260.0)).labelled_by(label.id);
            ui.end_row();
        };
        line(ui, tl!("Title"), &mut d.title, tl!("No title"));
        line(ui, tl!("Subtitle"), &mut d.subtitle, "");
        let label = ui.label(tl!("Note"));
        ui.add(egui::TextEdit::multiline(&mut d.note).desired_rows(3).desired_width(260.0)).labelled_by(label.id);
        ui.end_row();
        line(ui, tl!("Sources"), &mut d.sources, "");
        let label = ui.label(tl!("Credit line"));
        ui.add_enabled(
            !d.no_author,
            egui::TextEdit::singleline(&mut d.author).hint_text(tl!("Made with RustGIS and the date")).desired_width(260.0),
        )
        .labelled_by(label.id);
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut d.no_author, tl!("No credit line"));
        ui.end_row();
    });
}

/// The accent step: the style's own colour, or one picked.
fn accent_step(ui: &mut egui::Ui, d: &mut Draft, own: egui::Color32) {
    let mut picked = d.accent.is_some();
    if ui.checkbox(&mut picked, tl!("Pick the accent colour")).changed() {
        d.accent = picked.then_some(own);
    }
    if let Some(c) = &mut d.accent {
        egui::color_picker::color_edit_button_srgba(ui, c, egui::color_picker::Alpha::Opaque);
    } else {
        ui.label(egui::RichText::new(tl!("The style's own colour.")).small().weak());
    }
}

/// The review step: what was picked, step by step.
fn review_step(ui: &mut egui::Ui, d: &Draft) {
    let none = tl!("(none)");
    let or_none = |t: &str| if t.trim().is_empty() { none.to_string() } else { t.trim().to_string() };
    egui::Grid::new("publication_wizard_review").num_columns(2).spacing(egui::vec2(12.0, 6.0)).show(ui, |ui| {
        let mut row = |key: &str, value: String| {
            ui.label(egui::RichText::new(key).weak());
            ui.label(value);
            ui.end_row();
        };
        row(tl!("Size"), d.size_label());
        if d.figure() {
            return;
        }
        let turned = match d.orientation {
            Orientation::Auto => tl!("Auto"),
            Orientation::Portrait => tl!("Portrait"),
            Orientation::Landscape => tl!("Landscape"),
        };
        row(tl!("Orientation"), turned.to_string());
        let style = crate::ribbon::publication_style(&d.style).map(|s| s.0.to_string()).unwrap_or_else(|| d.style.clone());
        row(tl!("Style"), style);
        row(tl!("Title"), or_none(&d.title));
        row(tl!("Subtitle"), or_none(&d.subtitle));
        row(tl!("Note"), or_none(&d.note));
        row(tl!("Sources"), or_none(&d.sources));
        let credit = if d.no_author {
            none.to_string()
        } else if d.author.trim().is_empty() {
            tl!("Made with RustGIS and the date").to_string()
        } else {
            d.author.trim().to_string()
        };
        row(tl!("Credit line"), credit);
        let accent =
            d.accent.map(|c| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())).unwrap_or_else(|| tl!("The style's own").to_string());
        row(tl!("Accent"), accent);
    });
}

/// The height every step takes: the style step's six styles and their descriptions.
const STEP_HEIGHT: f32 = 300.0;

/// Shows the wizard while it is open; Create runs `layout.publication` as the menu does, and the
/// wizard stays open on a layout the command refuses.
pub fn show(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(mut d) = app.dialogs.publication.take() else {
        app.dialogs.open = None;
        return;
    };
    let t = Tokens::get(ctx);
    let (mut open, mut close, mut create) = (true, false, false);
    egui::Window::new(tl!("Publication Layout Wizard"))
        .id(egui::Id::unique("publication_wizard"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .auto_sized()
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(380.0);
            let steps = d.steps();
            let at = steps.iter().position(|s| *s == d.step).unwrap_or(0);
            let heading = i18n::fmt(
                "Step {n} of {count}: {step}",
                &[("n", &(at + 1).to_string()), ("count", &steps.len().to_string()), ("step", step_name(d.step))],
            );
            ui.label(egui::RichText::new(heading).strong());
            ui.separator();
            // Every step as tall as the tallest (the styles and their descriptions), so the
            // window keeps its size and Next stays under the pointer from step to step.
            // `set_min_height`, not `allocate_ui_with_layout`, which in egui 0.37 keeps only what
            // was drawn.
            ui.vertical(|ui| {
                ui.set_min_height(STEP_HEIGHT);
                match d.step {
                    Step::Size => size_step(ui, &mut d),
                    Step::Style => style_step(ui, &mut d),
                    Step::Texts => texts_step(ui, &mut d),
                    Step::Accent => accent_step(ui, &mut d, t.accent),
                    Step::Review => review_step(ui, &d),
                }
            });
            ui.separator();
            let size = egui::vec2(84.0, 28.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    match d.moved(1) {
                        Some(next) => {
                            if crate::widgets::dialog_button(ui, tl!("Next"), Some(t.accent), size).clicked() {
                                d.step = next;
                            }
                        }
                        None => create = crate::widgets::dialog_button(ui, tl!("Create"), Some(t.accent), size).clicked(),
                    }
                    let back = d.moved(-1);
                    if ui.add_enabled_ui(back.is_some(), |ui| crate::widgets::dialog_button(ui, tl!("Back"), None, size).clicked()).inner {
                        d.step = back.unwrap_or(d.step);
                    }
                    close = crate::widgets::dialog_button(ui, tl!("Cancel"), None, size).clicked();
                });
            });
        });
    // A layout the command refuses is said (a map with no extent), and the wizard stays.
    if create && crate::ribbon::make_publication(app, d.params()) {
        close = true;
    }
    if !open || close {
        app.dialogs.open = None;
    } else {
        app.dialogs.publication = Some(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::accesskit::Role;
    use egui::vec2;
    use egui_kittest::kittest::{NodeT, Queryable};

    fn app() -> RustGisApp {
        RustGisApp::with_sample()
    }

    fn open(app: &mut RustGisApp) {
        app.run("ui.dialog", json!({"name": "publicationWizard"})).unwrap();
    }

    fn harness(app: RustGisApp) -> egui_kittest::Harness<'static, RustGisApp> {
        // The dialogs alone: the ribbon has a Next of its own (the next extent).
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 1000.0)).with_max_steps(64).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                crate::dialogs::show(app, &ui.ctx().clone());
            },
            app,
        );
        h.run_steps(4);
        h
    }

    fn click(h: &mut egui_kittest::Harness<'static, RustGisApp>, label: &str) {
        h.get_by_role_and_label(Role::Button, label).click();
        h.run_steps(4);
    }

    fn step(h: &egui_kittest::Harness<'static, RustGisApp>) -> Step {
        h.state().dialogs.publication.as_ref().expect("the wizard is open").step
    }

    /// It opens on its first step with the menu's defaults: an A4 sheet turned as the map comes
    /// out larger, the classic style, the map's name as the title and nothing else set; opened
    /// again while open, it keeps what was entered.
    #[test]
    fn opens_on_the_menus_defaults_and_keeps_its_draft() {
        let mut app = app();
        open(&mut app);
        let name = app.session.map().name.clone();
        let d = app.dialogs.publication.as_mut().expect("opened");
        assert_eq!((d.step, d.size.as_str(), d.orientation, d.style.as_str()), (Step::Size, "a4", Orientation::Auto, "classic"));
        assert_eq!(d.title, name);
        assert!(d.subtitle.is_empty() && d.note.is_empty() && d.sources.is_empty() && d.author.is_empty());
        assert!((d.accent, d.no_author) == (None, false));
        assert_eq!(app.dialogs.open, Some("publicationWizard"));
        app.dialogs.publication.as_mut().unwrap().subtitle = "Kept".into();
        open(&mut app);
        assert_eq!(app.dialogs.publication.as_ref().unwrap().subtitle, "Kept", "not started over");
    }

    /// What Create runs: the fields set, under the command's names; what is left empty is left to
    /// the command's default, but for the title, which empty means none.
    #[test]
    fn create_sends_the_fields_set_and_nothing_else() {
        let app = app();
        let mut d = Draft::new(&app.session);
        d.title = "Rivers".into();
        assert_eq!(d.params(), json!({"profile": "a4", "style": "classic", "title": "Rivers"}));
        d.orientation = Orientation::Landscape;
        d.size = "letter".into();
        d.style = "panel".into();
        d.title.clear();
        d.subtitle = "2026".into();
        d.note = "A note".into();
        d.sources = "Natural Earth".into();
        d.author = "J. G.".into();
        d.accent = Some(egui::Color32::from_rgb(0x0B, 0x1B, 0x2B));
        assert_eq!(
            d.params(),
            json!({"profile": "letter-landscape", "style": "panel", "title": "", "subtitle": "2026", "note": "A note",
                "sources": "Natural Earth", "author": "J. G.", "accent": "#0B1B2B"})
        );
        d.orientation = Orientation::Portrait;
        d.no_author = true;
        assert_eq!(d.params()["profile"], "letter-portrait");
        assert_eq!(d.params()["author"], "", "no credit line");
        // A figure takes no style, texts or accent: the engine draws none of them on it.
        d.size = "figure-1col".into();
        assert_eq!(d.params(), json!({"profile": "figure-1col"}));
    }

    /// A sheet goes through the five steps; a figure goes from its size to the review, the steps
    /// it takes nothing from skipped both ways. Back is off on the first step; Next becomes
    /// Create on the last.
    #[test]
    fn a_sheet_takes_every_step_and_a_figure_skips_what_it_doesnt_draw() {
        let mut app = app();
        open(&mut app);
        let mut h = harness(app);
        assert!(h.get_by_role_and_label(Role::Button, "Back").accesskit_node().is_disabled(), "Back is off");
        for want in [Step::Style, Step::Texts, Step::Accent, Step::Review] {
            click(&mut h, "Next");
            assert_eq!(step(&h), want);
        }
        assert!(h.query_by_role_and_label(Role::Button, "Next").is_none());
        assert!(h.query_by_role_and_label(Role::Button, "Create").is_some());
        for want in [Step::Accent, Step::Texts, Step::Style, Step::Size] {
            click(&mut h, "Back");
            assert_eq!(step(&h), want);
        }
        h.state_mut().dialogs.publication.as_mut().unwrap().size = "figure-2col".into();
        h.run_steps(2);
        click(&mut h, "Next");
        assert_eq!(step(&h), Step::Review);
        click(&mut h, "Back");
        assert_eq!(step(&h), Step::Size);
    }

    /// The style step lists every sheet style, by the names and descriptions the menu gives them.
    #[test]
    fn the_style_step_lists_every_style_as_the_menu_names_it() {
        let mut app = app();
        open(&mut app);
        app.dialogs.publication.as_mut().unwrap().step = Step::Style;
        let h = harness(app);
        for (id, _) in rustgis_engine::cmd::layout::PUBLICATION_STYLES {
            let (name, about) = crate::ribbon::publication_style(id).unwrap();
            assert!(h.query_by_label(name).is_some(), "{id}");
            assert!(h.query_by_label(about).is_some(), "{id}");
        }
    }

    /// Create makes the layout in one step, closes the wizard and opens the Publish tab, as the
    /// menu does; Undo takes the layout away.
    #[test]
    fn create_makes_the_layout_and_undo_takes_it_away() {
        let mut app = app();
        open(&mut app);
        {
            let d = app.dialogs.publication.as_mut().unwrap();
            d.style = "banner".into();
            d.subtitle = "From the wizard".into();
            d.step = Step::Review;
        }
        let before = app.session.project.layouts.len();
        let mut h = harness(app);
        click(&mut h, "Create");
        let app = h.state();
        assert_eq!(app.session.project.layouts.len(), before + 1);
        let l = app.session.project.layouts.last().unwrap();
        assert!(
            l.elements.iter().any(
                |e| matches!(&e.kind, rustgis_engine::map::layout::ElementKind::Text { text, .. } if text.contains("From the wizard"))
            ),
            "the subtitle given"
        );
        assert_eq!((app.dialogs.open, app.ui.tab.as_str()), (None, "Publish"));
        assert!(app.dialogs.publication.is_none());
        h.state_mut().run("edit.undo", Value::Null).unwrap();
        assert_eq!(h.state().session.project.layouts.len(), before);
    }

    /// A layout the command refuses (here a style it doesn't know) is said, and the wizard stays
    /// open on its review with nothing made.
    #[test]
    fn a_refused_layout_is_said_and_the_wizard_stays_open() {
        let mut app = app();
        open(&mut app);
        {
            let d = app.dialogs.publication.as_mut().unwrap();
            d.style = "unknown".into();
            d.step = Step::Review;
        }
        let before = app.session.project.layouts.len();
        let mut h = harness(app);
        let said = h.state().session.notices.len();
        click(&mut h, "Create");
        let app = h.state();
        assert_eq!(app.session.project.layouts.len(), before, "nothing made");
        assert!(app.session.notices[said..].iter().any(|n| n.text.contains("unknown")), "said");
        assert_eq!(app.dialogs.open, Some("publicationWizard"));
        assert_eq!(app.dialogs.publication.as_ref().map(|d| d.step), Some(Step::Review));
    }

    /// Next stays where it was from one step to the next: every step is as tall as the tallest.
    #[test]
    fn next_stays_under_the_pointer_from_step_to_step() {
        let mut app = app();
        open(&mut app);
        let mut h = harness(app);
        let at = |h: &egui_kittest::Harness<'static, RustGisApp>| h.get_by_role_and_label(Role::Button, "Next").rect();
        let first = at(&h);
        for _ in 0..3 {
            click(&mut h, "Next");
            assert_eq!(at(&h), first, "{:?}", step(&h));
        }
    }

    /// Cancel closes the wizard with nothing made, and the next opening starts over.
    #[test]
    fn cancel_makes_nothing_and_the_next_opening_starts_over() {
        let mut app = app();
        open(&mut app);
        app.dialogs.publication.as_mut().unwrap().subtitle = "Dropped".into();
        let before = app.session.project.layouts.len();
        let mut h = harness(app);
        click(&mut h, "Cancel");
        assert_eq!(h.state().dialogs.open, None);
        assert_eq!(h.state().session.project.layouts.len(), before);
        h.state_mut().run("ui.dialog", json!({"name": "publicationWizard"})).unwrap();
        assert!(h.state().dialogs.publication.as_ref().unwrap().subtitle.is_empty());
    }
}
