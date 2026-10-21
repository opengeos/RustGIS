//! Autocomplete for the Script pane (#662): a floating list of suggestions just under the cursor,
//! as in a code editor. ↑/↓ move the highlight, Enter or Tab accept it, Escape closes the list, or
//! a click accepts an item. What to offer at a cursor (`offer`) is worked out apart from the box
//! and tested on its own; this first step completes command ids (a line's first word).

use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::{TextEditOutput, TextEditState};
use egui::{Id, Key, Modifiers, Ui};
use rustgis_engine::cmd::workflows::refused_in_script;

/// The most suggestions the list shows.
const MAX_ITEMS: usize = 10;
/// The fewest characters typed before ids are offered (one would offer most of them).
const MIN_TYPED: usize = 2;

/// What to offer at a cursor: the word being typed (char indexes, `end` at the cursor) and what
/// can replace it.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub start: usize,
    pub end: usize,
    pub items: Vec<String>,
}

/// The ids that complete the command id being typed at char `at` of `script`: the cursor at the
/// end of a line's first word (comments aside), at least `MIN_TYPED` characters typed. `ids` are
/// the commands a script can run. Case aside, an id equal to the word isn't offered again.
pub fn offer(script: &str, at: usize, ids: &[String]) -> Option<Offer> {
    let chars: Vec<char> = script.chars().collect();
    // The cursor ends the word: no word character right after it.
    if chars.get(at).is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let before = chars.get(..at)?;
    let line_start = before.iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
    let typed = before.get(line_start..)?;
    let lead = typed.iter().take_while(|c| c.is_whitespace()).count();
    let word = typed.get(lead..)?;
    if word.len() < MIN_TYPED || word.iter().any(|c| c.is_whitespace()) || word.first() == Some(&'#') {
        return None;
    }
    let word: String = word.iter().collect();
    let lower = word.to_lowercase();
    let mut items: Vec<String> = ids
        .iter()
        .filter(|id| id.to_lowercase().starts_with(&lower) && !id.eq_ignore_ascii_case(&word) && !refused_in_script(id))
        .cloned()
        .collect();
    items.sort();
    items.dedup();
    items.truncate(MAX_ITEMS);
    (!items.is_empty()).then(|| Offer { start: line_start + lead, end: at, items })
}

/// The ids a script can run: the built-in commands and enabled plugins' (`plugin:…`), less those a
/// script refuses.
pub fn script_ids(session: &rustgis_engine::Session) -> Vec<String> {
    let builtin = session.registry.commands.iter().map(|c| c.id.to_string());
    let plugins = rustgis_engine::plugins::commands().into_iter().map(|c| c.id);
    builtin.chain(plugins).filter(|id| !refused_in_script(id)).collect()
}

/// The list's state between frames: the offer it shows and the highlighted item.
#[derive(Clone)]
struct Open {
    offer: Offer,
    selected: usize,
}

fn state_id(box_id: Id) -> Id {
    box_id.with("complete")
}

/// Where the offer Escape closed waits: it isn't shown again until what's typed changes.
fn dismissed_id(box_id: Id) -> Id {
    box_id.with("complete_dismissed")
}

/// The keys the box keeps while it has focus: code_editor's (the arrows, Tab to indent) and, with
/// the list open, Escape, which egui would otherwise take as "drop the focus" before `keys` can
/// read it as "close the list".
pub fn filter(ui: &Ui, box_id: Id) -> egui::EventFilter {
    let open = ui.data(|d| d.get_temp::<Open>(state_id(box_id))).is_some();
    egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: open }
}

/// Where the list was drawn last frame: a press on it takes the focus from the box (a click
/// elsewhere), so the list stays while the pointer is on it, for the click to complete.
fn list_rect_id(box_id: Id) -> Id {
    box_id.with("complete_rect")
}

/// Before the box sees the keys: with the list open and the box focused, ↑/↓ move the highlight,
/// Enter or Tab accept it (so Enter doesn't break the line, nor Tab indent), Escape closes the list
/// (and the box keeps the focus). Returns the item to accept, if any.
pub fn keys(ui: &Ui, box_id: Id) -> Option<(Offer, String)> {
    let key = state_id(box_id);
    let mut open = ui.data(|d| d.get_temp::<Open>(key))?;
    if !ui.memory(|m| m.has_focus(box_id)) {
        return None;
    }
    let n = open.offer.items.len();
    let (up, down, accept, close) = ui.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if close {
        ui.data_mut(|d| {
            d.remove::<Open>(key);
            d.insert_temp(dismissed_id(box_id), open.offer);
        });
        return None;
    }
    if accept {
        ui.data_mut(|d| d.remove::<Open>(key));
        let item = open.offer.items.get(open.selected)?.clone();
        return Some((open.offer, item));
    }
    if up || down {
        open.selected = if up { (open.selected + n - 1) % n } else { (open.selected + 1) % n };
        ui.data_mut(|d| d.insert_temp(key, open));
    }
    None
}

/// After the box: offers for the word at its cursor (a new offer keeps the highlight on the same
/// item, when it is still there), and the list drawn just under the cursor. Returns an item
/// clicked, to accept. The list closes when the box loses focus or nothing matches.
pub fn show(ui: &Ui, box_id: Id, out: &TextEditOutput, script: &str, ids: impl FnOnce() -> Vec<String>) -> Option<(Offer, String)> {
    let key = state_id(box_id);
    let on_list = ui
        .data(|d| d.get_temp::<egui::Rect>(list_rect_id(box_id)))
        .is_some_and(|r| ui.input(|i| i.pointer.hover_pos().is_some_and(|p| r.contains(p))));
    let offer = if out.response.response.has_focus() {
        out.cursor_range.and_then(|r| offer(script, r.primary.index.0, &ids()))
    } else if on_list {
        // Pressed on the list: the box let go of the focus (and maybe of its cursor), so the list
        // shown stays as it is until the click ends.
        ui.data(|d| d.get_temp::<Open>(key)).map(|k| k.offer)
    } else {
        None
    };
    let Some(offer) = offer else {
        ui.data_mut(|d| {
            d.remove::<Open>(key);
            d.remove::<egui::Rect>(list_rect_id(box_id));
        });
        return None;
    };
    // Closed with Escape: not again for the same word.
    if ui.data(|d| d.get_temp::<Offer>(dismissed_id(box_id))).is_some_and(|d| d == offer) {
        return None;
    }
    ui.data_mut(|d| d.remove::<Offer>(dismissed_id(box_id)));
    let kept = ui.data(|d| d.get_temp::<Open>(key));
    let selected =
        kept.and_then(|k| k.offer.items.get(k.selected).cloned()).and_then(|item| offer.items.iter().position(|i| *i == item)).unwrap_or(0);
    ui.data_mut(|d| d.insert_temp(key, Open { offer: offer.clone(), selected }));
    // Just under the cursor, or above it when the list would run off the bottom.
    let cursor = out.galley.pos_from_cursor(CCursor::new(offer.end)).translate(out.galley_pos.to_vec2());
    if !out.text_clip_rect.intersects(cursor) {
        return None;
    }
    let row = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y + 4.0;
    let height = row * offer.items.len() as f32 + 12.0;
    let screen = ui.ctx().content_rect();
    let y = if cursor.bottom() + height > screen.bottom() { cursor.top() - height } else { cursor.bottom() };
    let x = cursor.left().min(screen.right() - 260.0).max(screen.left());
    let mut clicked = None;
    let area = egui::Area::new(key.with("list")).order(egui::Order::Foreground).fixed_pos(egui::pos2(x, y)).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(220.0);
            for (i, item) in offer.items.iter().enumerate() {
                let r = ui.selectable_label(i == selected, egui::RichText::new(item).monospace());
                // Taken as the button goes down, as in a code editor: the press already takes the
                // focus from the box, and a click (down, then up a frame or more later) could find
                // the list gone by then.
                if r.clicked() || (r.hovered() && ui.input(|i| i.pointer.primary_pressed())) {
                    clicked = Some((offer.clone(), item.clone()));
                }
            }
        });
    });
    ui.data_mut(|d| d.insert_temp(list_rect_id(box_id), area.response.rect));
    if clicked.is_some() {
        ui.data_mut(|d| {
            d.remove::<Open>(key);
            d.remove::<egui::Rect>(list_rect_id(box_id));
        });
    }
    clicked
}

/// Puts `item` in place of the offer's word in `script`, the cursor after it, as one undo step of
/// the box (its own undo takes the pick back), and gives the box the focus again.
pub fn accept(ui: &Ui, box_id: Id, script: &mut String, offer: &Offer, item: &str) {
    let chars: Vec<char> = script.chars().collect();
    let (Some(before), Some(after)) = (chars.get(..offer.start), chars.get(offer.end..)) else { return };
    let mut new: String = before.iter().collect();
    new.push_str(item);
    new.extend(after);
    let mut state = TextEditState::load(ui.ctx(), box_id).unwrap_or_default();
    let mut undoer = state.undoer();
    let was = state.cursor.char_range().unwrap_or(CCursorRange::one(CCursor::new(offer.end)));
    undoer.add_undo(&(was, script.clone()));
    state.set_undoer(undoer);
    *script = new;
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(offer.start + item.chars().count()))));
    state.store(ui.ctx(), box_id);
    ui.memory_mut(|m| m.request_focus(box_id));
}

#[cfg(test)]
mod tests {
    use super::{Offer, offer};

    fn ids() -> Vec<String> {
        ["labels.style", "labels.placement", "labels.class.add", "map.zoomFull", "ui.pane.labeling", "script.run", "plugin:demo/hello"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn items(o: Option<Offer>) -> Vec<String> {
        o.map(|o| o.items).unwrap_or_default()
    }

    #[test]
    fn a_lines_first_word_offers_the_ids_it_starts() {
        let s = "map.zoomFull\n  labels.";
        let o = offer(s, s.chars().count(), &ids()).unwrap();
        assert_eq!(o.items, ["labels.class.add", "labels.placement", "labels.style"]);
        assert_eq!((o.start, o.end), (15, 22), "the word, after the indent");
        assert_eq!(items(offer("LABELS.ST", 9, &ids())), ["labels.style"], "case aside");
        assert_eq!(items(offer("plu", 3, &ids())), ["plugin:demo/hello"], "plugins' too");
    }

    #[test]
    fn only_a_lines_first_word_at_the_cursor_is_completed() {
        let none = |s: &str, at: usize| offer(s, at, &ids()).is_none();
        assert!(none("labels.style {\"la", 17), "in the JSON");
        assert!(none("# labels", 8), "in a comment");
        assert!(none("l", 1), "one character");
        assert!(none("labels.st", 4), "the cursor mid-word");
        assert!(none("labels.style", 12), "the id typed whole");
        assert!(none("ui.pa", 5) && none("script.r", 8), "ids a script refuses");
        assert!(none("nothing", 7), "no match");
        assert!(none("map", 99), "a cursor past the text");
    }

    #[test]
    fn char_indexes_with_multibyte_text_and_crlf() {
        let s = "# Bogotá 🌎\r\nlabels.pl";
        let at = s.chars().count();
        let o = offer(s, at, &ids()).unwrap();
        assert_eq!(o.items, ["labels.placement"]);
        let start: String = s.chars().skip(o.start).take(o.end - o.start).collect();
        assert_eq!(start, "labels.pl");
    }

    #[test]
    fn the_list_is_capped() {
        let many: Vec<String> = (0..30).map(|i| format!("map.cmd{i:02}")).collect();
        assert_eq!(items(offer("map.cm", 6, &many)).len(), super::MAX_ITEMS);
    }

    /// The pane with an empty script, its box focused.
    fn pane() -> egui_kittest::Harness<'static, crate::RustGisApp> {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(500.0, 700.0)).build_ui_state(
            |ui, app: &mut crate::RustGisApp| crate::analysispane::script_pane(app, ui, &crate::theme::Tokens::light()),
            crate::RustGisApp::with_sample(),
        );
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();
        h.run();
        h
    }

    fn type_in(h: &mut egui_kittest::Harness<'static, crate::RustGisApp>, text: &str) {
        h.event(egui::Event::Text(text.into()));
        h.run();
    }

    /// Typing a line's first word opens the list; Enter accepts the highlighted id, with no line
    /// break, and the list closes.
    #[test]
    fn enter_accepts_the_highlighted_id() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.get_by_label("labels.style");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().workflow.script, "labels.style");
        assert!(h.query_by_label("labels.style").is_none(), "the list closed");
    }

    /// The arrows move the highlight; Tab accepts it too, inserting no tab.
    #[test]
    fn the_arrows_move_and_tab_accepts() {
        let mut h = pane();
        type_in(&mut h, "labels.");
        h.key_press(egui::Key::ArrowDown);
        h.run();
        h.key_press(egui::Key::Tab);
        h.run();
        let second = super::offer("labels.", 7, &super::script_ids(&h.state().session)).unwrap().items[1].clone();
        assert_eq!(h.state().workflow.script, second, "the second item, and no tab");
    }

    /// Escape closes the list without touching the text, and it stays closed for that word.
    #[test]
    fn escape_closes_the_list_for_that_word() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(h.query_by_label("labels.style").is_none());
        assert_eq!(h.state().workflow.script, "labels.sty");
        h.run();
        assert!(h.query_by_label("labels.style").is_none(), "not opened again");
        type_in(&mut h, "l");
        h.get_by_label("labels.style");
    }

    /// A click accepts an item, and the box keeps the focus. Pressed and let go in frames of their
    /// own, as a real click is (`click()` does both in one frame, which hid that the press takes
    /// the focus from the box first).
    #[test]
    fn a_click_accepts_an_item() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "map.zoomF");
        let at = h.get_by_label("map.zoomFull").rect().center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.event(egui::Event::PointerMoved(at));
        h.step();
        h.event(button(true));
        h.step();
        h.step();
        h.event(button(false));
        h.run();
        assert_eq!(h.state().workflow.script, "map.zoomFull");
        assert!(h.get_by_role(egui::accesskit::Role::MultilineTextInput).is_focused());
    }

    /// The box's own undo takes a pick back.
    #[test]
    fn a_pick_is_one_undo_step_of_the_box() {
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.key_press(egui::Key::Enter);
        h.run();
        let state = egui::widgets::text_edit::TextEditState::load(&h.ctx, egui::Id::new("script_text")).unwrap();
        let now = (state.cursor.char_range().unwrap(), h.state().workflow.script.clone());
        let mut undoer = state.undoer();
        let back = undoer.undo(&now).map(|(_, text)| text.clone());
        assert_eq!(back.as_deref(), Some("labels.sty"));
    }
}
