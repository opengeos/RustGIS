//! The text form shared by a text graphic (Add Text) and a layout's text (the Format pane, #516):
//! its text, dynamic text, size, colour and boldness, laid out and limited the same in both, so
//! the same text can look the same on the map and on the page. It edits the values it is lent
//! and says what was settled; each caller runs its own command.

use egui::{Color32, Ui, vec2};

/// What the form edits. Colour and boldness are left out where the text doesn't have them yet.
pub struct Fields<'a> {
    pub text: &'a mut String,
    pub size: &'a mut f64,
    pub color: Option<&'a mut Color32>,
    pub bold: Option<&'a mut bool>,
}

/// What the user settled this frame.
#[derive(Debug, Default)]
pub struct Settled {
    /// The text is being typed: the caller keeps it as a draft, not saved yet.
    pub typing: bool,
    /// The text is done: its field lost focus, Enter was pressed, or a dynamic text was added.
    pub text: bool,
    /// The size changed and was let go (a drag ended, a value typed in).
    pub size: bool,
    pub color: bool,
    pub bold: bool,
}

/// The width the labels of a properties form take, so the controls of its grids line up in one
/// column (a longer label, in some languages, widens its own grid only).
pub const LABEL_W: f32 = 96.0;

/// The form, its fields told apart by `id` (an element, a dialog). `tokens` are the dynamic text
/// lists the text expands where it is shown, each a section of the Dynamic Text menu. `extra`
/// adds rows of the caller's own to the grid of Size, Color and Bold (a layout's type settings).
pub fn show(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    tokens: &[&[(&str, &str)]],
    f: Fields<'_>,
    extra: impl FnOnce(&mut Ui),
) -> Settled {
    let id = egui::Id::new(id);
    let mut out = Settled::default();
    ui.label(tl!("Text"));
    let hint = tl!("Shift+Enter starts a new line.");
    let r = ui.add(crate::dialogs::text_box(f.text).id_salt(id.with("text")).hint_text(hint).desired_rows(3).desired_width(f32::INFINITY));
    // Enter is done typing, as in the dialogs; Shift+Enter starts a line.
    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
        r.surrender_focus();
        out.text = true;
    }
    out.text |= r.lost_focus();
    // The values the text can show, as the ribbon's Dynamic Text menu lists them: one line here,
    // and what each is said in words.
    ui.menu_button(tl!("Insert Dynamic Text"), |ui| {
        for (i, list) in tokens.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            for &(token, what) in *list {
                if ui.button(format!("{}  {token}", tl!(what))).clicked() {
                    f.text.push_str(token);
                    out.text = true;
                    ui.close();
                }
            }
        }
    });
    out.typing = r.has_focus() && !out.text;
    egui::Grid::new(id.with("style")).num_columns(2).min_col_width(LABEL_W).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("Size"));
        let before = *f.size;
        let r = ui.add(egui::DragValue::new(f.size).range(rustgis_engine::map::graphics::FONT_SIZE).speed(0.2).suffix(" pt"));
        // Named for assistive technology, as its label sits in the other column.
        let size = *f.size;
        r.widget_info(|| egui::WidgetInfo { label: Some(tl!("Size").to_string()), ..egui::WidgetInfo::drag_value(true, size) });
        out.size = (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (*f.size - before).abs() > 1e-6;
        ui.end_row();
        if let Some(c) = f.color {
            ui.label(tl!("Color"));
            out.color = crate::widgets::color_button(ui, c).changed();
            ui.end_row();
        }
        if let Some(b) = f.bold {
            ui.label(tl!("Bold"));
            let r = ui.checkbox(b, "");
            let on = *b;
            r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, tl!("Bold")));
            out.bold = r.changed();
            ui.end_row();
        }
        extra(ui);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;

    #[derive(Default)]
    struct Form {
        text: String,
        size: f64,
        color: Color32,
        bold: bool,
        /// Whether colour and boldness are offered (a text graphic doesn't have them yet).
        styled: bool,
        settled: Vec<&'static str>,
    }

    fn harness(form: Form) -> egui_kittest::Harness<'static, Form> {
        egui_kittest::Harness::builder().build_ui_state(
            |ui, f: &mut Form| {
                let s = show(
                    ui,
                    "form",
                    &[rustgis_engine::map::graphics::TOKENS, rustgis_engine::map::layout::PAGE_TOKENS],
                    Fields {
                        text: &mut f.text,
                        size: &mut f.size,
                        color: f.styled.then_some(&mut f.color),
                        bold: f.styled.then_some(&mut f.bold),
                    },
                    |_| {},
                );
                for (on, what) in [(s.text, "text"), (s.size, "size"), (s.color, "color"), (s.bold, "bold")] {
                    if on {
                        f.settled.push(what);
                    }
                }
            },
            form,
        )
    }

    /// Every token listed is in the Dynamic Text menu, said in words; picking one adds it to the
    /// text, which is then settled.
    #[test]
    fn dynamic_text_is_added_from_every_list() {
        let mut h = harness(Form { text: "Page ".into(), size: 12.0, ..Form::default() });
        h.run();
        for (item, text) in [("Page number  {page}", "Page {page}"), ("Map scale  {scale}", "Page {page}{scale}")] {
            h.get_by_label("Insert Dynamic Text").click();
            h.run();
            h.get_by_label(item).click();
            h.run();
            assert_eq!(h.state().text, text);
        }
        assert_eq!(h.state().settled, ["text", "text"]);
        assert!(h.query_by_label("Page number  {page}").is_none(), "the menu closes");
    }

    /// Colour and boldness show only where the text has them; the size stays in the one range
    /// text is drawn at.
    #[test]
    fn colour_and_bold_show_only_when_lent() {
        let mut h = harness(Form { text: "T".into(), size: 12.0, ..Form::default() });
        h.run();
        assert!(h.query_by_label("Color").is_none() && h.query_by_label("Bold").is_none());
        let mut h = harness(Form { text: "T".into(), size: 12.0, styled: true, ..Form::default() });
        h.run();
        h.get_by_role_and_label(Role::CheckBox, "Bold").click();
        h.run();
        assert!(h.state().bold);
        assert_eq!(h.state().settled, ["bold"]);
    }
}
