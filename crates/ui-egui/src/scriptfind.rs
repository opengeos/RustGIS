//! Find and replace in the Script pane (#2996): what the find bar looks for in the script, and the
//! text a replace leaves. Positions are characters, as the box's cursor counts them. The matching
//! is plain text (no patterns), and worked out apart from the box so it is tested on its own.

use std::ops::Range;

/// The most matches kept: a script this long with this many hits is not read by eye, and the
/// highlight is laid out each time the text changes.
pub const MAX_MATCHES: usize = 10_000;

/// Where `query` is in a script.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    /// Each match, in characters, in order and apart from each other.
    pub ranges: Vec<Range<usize>>,
    /// More than [`MAX_MATCHES`] hits: the rest are not kept.
    pub capped: bool,
}

/// Every place `query` is in `text`, from the start, a match taken before the next is looked for
/// (so `aa` is twice in `aaaa`). `case`: capitals count; otherwise a letter is its lower case.
/// `whole`: only a match with no letter, digit or `_` against it on either side (a whole word, as a
/// code editor's Match Whole Word takes it). An empty query is nowhere.
pub fn matches(text: &str, query: &str, case: bool, whole: bool) -> Found {
    // A letter whose lower case is several characters (`İ`) stays as it is, so it matches only
    // itself and the positions stay the text's.
    let fold = |c: char| {
        let mut lower = c.to_lowercase();
        match (lower.next(), lower.next()) {
            (Some(l), None) if !case => l,
            _ => c,
        }
    };
    let text: Vec<char> = text.chars().map(fold).collect();
    let query: Vec<char> = query.chars().map(fold).collect();
    let mut found = Found::default();
    if query.is_empty() {
        return found;
    }
    let mut i = 0;
    while i + query.len() <= text.len() {
        let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
        let apart = !whole || (!word(i.checked_sub(1).and_then(|b| text.get(b))) && !word(text.get(i + query.len())));
        if apart && text.get(i..i + query.len()) == Some(&query[..]) {
            if found.ranges.len() == MAX_MATCHES {
                found.capped = true;
                break;
            }
            found.ranges.push(i..i + query.len());
            i += query.len();
        } else {
            i += 1;
        }
    }
    found
}

/// The match after (or, `back`, before) `current` of `len`, going round at the ends; the first
/// (the last, `back`) when none is current. None when there are no matches.
pub fn step(len: usize, current: Option<usize>, back: bool) -> Option<usize> {
    let last = len.checked_sub(1)?;
    Some(match (current, back) {
        (None, false) => 0,
        (None, true) => last,
        (Some(c), false) => {
            if c.min(last) == last {
                0
            } else {
                c + 1
            }
        }
        (Some(c), true) => {
            if c.min(last) == 0 {
                last
            } else {
                c.min(last) - 1
            }
        }
    })
}

/// The index of the first match at or after the character `from`, else the first (it goes round).
pub fn first_from(found: &Found, from: usize) -> Option<usize> {
    let at = found.ranges.iter().position(|r| r.start >= from);
    at.or(if found.ranges.is_empty() { None } else { Some(0) })
}

/// `text` with the characters in `range` replaced by `with`.
pub fn replace_one(text: &str, range: Range<usize>, with: &str) -> String {
    let n = text.chars().count();
    let (start, end) = (range.start.min(n), range.end.min(n));
    let end = end.max(start);
    text.chars().take(start).chain(with.chars()).chain(text.chars().skip(end)).collect()
}

/// `text` with every match of `found` replaced by `with`.
pub fn replace_all(text: &str, found: &Found, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    let mut at = 0;
    for r in &found.ranges {
        out.extend(chars.by_ref().take(r.start.saturating_sub(at)));
        out.push_str(with);
        // The matched characters are skipped.
        chars.by_ref().take(r.end.saturating_sub(r.start)).for_each(drop);
        at = r.end.max(r.start);
    }
    out.extend(chars);
    out
}

/// The find bar's state, kept in the context under the box's id: it isn't saved with the project.
#[derive(Clone, Default)]
struct Bar {
    open: bool,
    query: String,
    replace: String,
    case: bool,
    /// Only whole words match.
    whole: bool,
    /// The match the next and previous go from, an index of `found`.
    current: Option<usize>,
    /// Where a new search starts picking its match: the cursor when the bar opened, the match last
    /// stepped to, or the end of a replacement.
    anchor: usize,
    /// Pick the match at the anchor when the matches are next worked out.
    reseek: bool,
    /// Scroll the current match into view in the box's next frame.
    scroll: bool,
    /// Put the focus in the query field, its text selected.
    focus_query: bool,
    /// The Replace row is shown.
    replace_open: bool,
    found: std::sync::Arc<Found>,
    /// What `found` was worked out for: the script's hash, the query and the case.
    key: Option<(u64, String, bool, bool)>,
}

impl Bar {
    /// The matches again, if the script, the query or the case changed, and the current one picked.
    /// `cursor`: the box's cursor, where a current match lost to an edit of the text is picked again.
    fn refresh(&mut self, script: &str, cursor: usize) {
        let key = (egui::util::hash(script), self.query.clone(), self.case, self.whole);
        if self.key.as_ref() != Some(&key) {
            self.found = std::sync::Arc::new(matches(script, &self.query, self.case, self.whole));
            self.key = Some(key);
            if !self.reseek {
                // The text changed under the bar (typed in the box): the matches shifted, so the
                // current one is the first from the cursor, where the edit is. Not scrolled to.
                self.current = first_from(&self.found, cursor);
            }
        }
        if self.reseek {
            self.current = first_from(&self.found, self.anchor);
            self.scroll = self.current.is_some();
            self.reseek = false;
        }
    }

    /// The bar moved to the match `at` of the found ones.
    fn go(&mut self, at: Option<usize>) {
        self.current = at;
        if let Some(r) = at.and_then(|i| self.found.ranges.get(i)) {
            self.anchor = r.start;
            self.scroll = true;
        }
    }
}

/// What the box draws of the bar: the matches, the current one and whether to scroll to it.
pub struct Shown {
    pub found: std::sync::Arc<Found>,
    pub current: Option<usize>,
    pub scroll: bool,
}

enum Action {
    Next,
    Previous,
    Replace,
    ReplaceAll,
    Close,
}

/// The room the bar leaves at the box's edges, and the widths it takes: a button's side, what the
/// count takes, the toggle that shows Replace, and the least and most the bar is wide (a narrower
/// box has the bar as wide as it is).
const EDGE: f32 = 8.0;
const COUNT_WIDTH: f32 = 52.0;
const TOGGLE_WIDTH: f32 = 16.0;
const MIN_BAR: f32 = 230.0;
const MAX_BAR: f32 = 380.0;
const GAP: f32 = 4.0;

/// The most characters of a query: what the field takes, and so of a selection that fills it when
/// the bar opens (a longer one is a passage, not a word to look for). Each change of the script or
/// the query costs the script's length times the query's, on the UI thread.
const QUERY_MAX: usize = 200;

/// Before the box: Ctrl+F (⌘F) in the box or in the bar opens it, with the selection in its field;
/// the matches are worked out, and what the box has to draw of them is returned, None while the bar
/// is closed. The bar itself is drawn over the box once it has been laid out ([`show`]).
pub fn prepare(ui: &egui::Ui, box_id: egui::Id, script: &str) -> Option<Shown> {
    let id = box_id.with("find");
    let (query_id, replace_id) = (box_id.with("find_query"), box_id.with("find_replace"));
    let mut bar: Bar = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let in_box = ui.memory(|m| m.has_focus(box_id));
    let in_bar = ui.memory(|m| m.has_focus(query_id) || m.has_focus(replace_id));
    // Ctrl+F (⌘F) opens the bar; Ctrl+H (⌥⌘F on a Mac, where ⌘H hides the app) opens it with the
    // Replace row, as a code editor's do. Not with Alt held, but on a Mac for ⌥⌘F: elsewhere
    // Ctrl+Alt is AltGr, and AltGr+F types a letter on many layouts, which egui's Ctrl+F
    // pattern would otherwise take as its own.
    let (here, mac) = (in_box || in_bar, ui.ctx().os().is_mac());
    let alt = ui.input(|i| i.modifiers.alt);
    let find = here && !alt && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F));
    let replace = here
        && ui.input_mut(|i| {
            (!alt && !mac && i.consume_key(egui::Modifiers::COMMAND, egui::Key::H))
                || (mac && alt && i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::ALT, egui::Key::F))
        });
    if find || replace {
        if in_box && let Some(r) = crate::widgets::cursor_range(ui.ctx(), box_id) {
            let (a, b) = (r.primary.index.0.min(r.secondary.index.0), r.primary.index.0.max(r.secondary.index.0));
            let picked: String = script.chars().skip(a).take(b - a).collect();
            if !picked.is_empty() && picked.chars().count() <= QUERY_MAX && !picked.contains(['\n', '\r']) {
                bar.query = picked;
            }
            bar.anchor = a;
        }
        bar.open = true;
        bar.replace_open |= replace;
        bar.reseek = true;
        bar.focus_query = true;
    }
    if !bar.open {
        ui.data_mut(|d| d.insert_temp(id, bar));
        return None;
    }
    let cursor = crate::widgets::cursor_range(ui.ctx(), box_id).map_or(bar.anchor, |r| r.primary.index.0);
    bar.refresh(script, cursor);
    // F3 and Shift+F3, as in a code editor: the next and the previous match, with the focus where it was.
    // Shift first: a pattern without Shift may take a press that has it.
    if (in_box || in_bar) && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F3)) {
        bar.go(step(bar.found.ranges.len(), bar.current, true));
    } else if (in_box || in_bar) && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F3)) {
        bar.go(step(bar.found.ranges.len(), bar.current, false));
    }
    let shown = Shown { found: bar.found.clone(), current: bar.current, scroll: std::mem::take(&mut bar.scroll) };
    ui.data_mut(|d| d.insert_temp(id, bar));
    Some(shown)
}

/// The toggles inside the right end of the query field `input`, as in a code editor: Match Case
/// (`Aa`) and Match Whole Word (`ab` under a bracket). Each is `on` or off; the one clicked is
/// returned.
fn field_toggles(ui: &mut egui::Ui, id: egui::Id, input: egui::Rect, case: bool, whole: bool) -> Option<Toggle> {
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut clicked = None;
    let toggles = [(Toggle::Word, whole, tl!("Match Whole Word")), (Toggle::Case, case, tl!("Match Case"))];
    for (k, (toggle, on, label)) in toggles.into_iter().enumerate() {
        let r =
            egui::Rect::from_min_size(egui::pos2(input.right() - 26.0 - 24.0 * k as f32, input.center().y - 9.0), egui::vec2(22.0, 18.0));
        let resp = ui.interact(r, id.with(k), egui::Sense::click());
        if on {
            ui.painter().rect_filled(r, egui::CornerRadius::same(3), t.accent.gamma_multiply(0.3));
            ui.painter().rect_stroke(r, egui::CornerRadius::same(3), egui::Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, egui::CornerRadius::same(3), t.hover);
        }
        let ink = if on { t.text } else { t.text_dim };
        let c = r.center();
        match toggle {
            Toggle::Case => {
                ui.painter().text(c, egui::Align2::CENTER_CENTER, "Aa", egui::FontId::proportional(12.0), ink);
            }
            Toggle::Word => {
                ui.painter().text(c - egui::vec2(0.0, 1.5), egui::Align2::CENTER_CENTER, "ab", egui::FontId::proportional(11.0), ink);
                // A bracket under the word, as a code editor draws it.
                let (y, stroke) = (c.y + 6.0, egui::Stroke::new(1.2, ink));
                let (l, rr) = (c.x - 7.0, c.x + 7.0);
                ui.painter().line_segment([egui::pos2(l, y - 2.5), egui::pos2(l, y)], stroke);
                ui.painter().line_segment([egui::pos2(l, y), egui::pos2(rr, y)], stroke);
                ui.painter().line_segment([egui::pos2(rr, y), egui::pos2(rr, y - 2.5)], stroke);
            }
        }
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, label.to_string()));
        if resp.on_hover_text(label).clicked() {
            clicked = Some(toggle);
        }
    }
    clicked
}

/// A toggle inside the query field.
#[derive(Clone, Copy)]
enum Toggle {
    Case,
    Word,
}

/// The find bar, drawn over the top right of the box (`area`, as laid out), as in a code editor: a
/// field with Match case inside it, the count and the previous, next and close buttons; and,
/// under the arrow at its left, a field for what to replace with and the Replace and Replace All
/// buttons. Does what it's asked: a replace changes `script` as one undo step of the box, which
/// `running` forbids.
pub fn show(ui: &mut egui::Ui, box_id: egui::Id, script: &mut String, running: bool, area: egui::Rect) {
    use egui::text::{CCursor, CCursorRange};
    let id = box_id.with("find");
    let (query_id, replace_id) = (box_id.with("find_query"), box_id.with("find_replace"));
    let mut bar: Bar = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    if !bar.open {
        return;
    }
    let total = bar.found.ranges.len();
    let capped = bar.found.capped;
    let width = (area.width() - 2.0 * EDGE).clamp(MIN_BAR, MAX_BAR);
    let pos = egui::pos2((area.right() - EDGE - width).max(area.left()), area.top() + EDGE);
    // The popup's own margin (`Frame::popup`) and its stroke.
    let margin = 6.0;
    let field = width - 2.0 * margin - 2.0 - TOGGLE_WIDTH - COUNT_WIDTH - 3.0 * crate::widgets::FLAT_BUTTON - 5.0 * GAP;
    let mut todo = None;
    crate::completion::popup(ui, box_id.with("find_area"), pos, area.height(), |ui| {
        ui.set_width(width - 2.0 * margin - 2.0);
        ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
        ui.horizontal_top(|ui| {
            // The arrow that shows or hides the Replace row, as tall as the rows it covers.
            let tall = if bar.replace_open { 2.0 * crate::widgets::FLAT_BUTTON + GAP } else { crate::widgets::FLAT_BUTTON };
            let (r, toggle) = ui.allocate_exact_size(egui::vec2(TOGGLE_WIDTH, tall), egui::Sense::click());
            let t = crate::theme::Tokens::get(ui.ctx());
            if toggle.hovered() {
                ui.painter().rect_filled(r, egui::CornerRadius::same(3), t.hover);
            }
            let icon = if bar.replace_open { "chevron_down" } else { "chevron_right" };
            let box_ =
                egui::Rect::from_center_size(r.center_top() + egui::vec2(0.0, crate::widgets::FLAT_BUTTON / 2.0), egui::vec2(14.0, 14.0));
            crate::icons::paint(ui.painter(), box_, icon, t.text, t.accent);
            toggle.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, bar.replace_open, tl!("Toggle Replace")));
            if toggle.on_hover_text(tl!("Toggle Replace")).clicked() {
                bar.replace_open = !bar.replace_open;
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let query = ui.add(
                        egui::TextEdit::singleline(&mut bar.query)
                            .id(query_id)
                            .desired_width(field)
                            .char_limit(QUERY_MAX)
                            .margin(egui::Margin { left: 6, right: 54, top: 3, bottom: 3 })
                            .hint_text(tl!("Find")),
                    );
                    let toggled = field_toggles(ui, query_id.with("toggle"), query.rect, bar.case, bar.whole);
                    match toggled {
                        Some(Toggle::Case) => bar.case = !bar.case,
                        Some(Toggle::Word) => bar.whole = !bar.whole,
                        None => {}
                    }
                    bar.reseek |= toggled.is_some();
                    if toggled.is_some() {
                        // As in a code editor, typing goes on in the field.
                        ui.memory_mut(|m| m.request_focus(query_id));
                    }
                    if std::mem::take(&mut bar.focus_query) {
                        ui.memory_mut(|m| m.request_focus(query_id));
                        let end = CCursor::new(bar.query.chars().count());
                        crate::widgets::set_cursor_range(ui.ctx(), query_id, CCursorRange::two(CCursor::new(0), end));
                    }
                    if query.changed() {
                        bar.reseek = true;
                    }
                    let shift = ui.input(|i| i.modifiers.shift);
                    if query.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        // A one-line box lets go of the focus on Enter: keep it for the next.
                        ui.memory_mut(|m| m.request_focus(query_id));
                        todo = Some(if shift { Action::Previous } else { Action::Next });
                    }
                    let place = match (bar.query.is_empty(), bar.current) {
                        (true, _) => String::new(),
                        (false, _) if total == 0 => tl!("No results").to_string(),
                        (false, current) => {
                            let (n, all) = (current.map_or(0, |c| c + 1).to_string(), format!("{total}{}", if capped { "+" } else { "" }));
                            crate::i18n::fmt("{n} of {all}", &[("n", &n), ("all", &all)])
                        }
                    };
                    ui.add_sized(
                        [COUNT_WIDTH, crate::widgets::FLAT_BUTTON],
                        egui::Label::new(egui::RichText::new(place).small().weak()).truncate(),
                    );
                    let (prev, next) = (tl!("Previous match (Shift+Enter)").to_string(), tl!("Next match (Enter)").to_string());
                    if crate::widgets::flat_icon_button(ui, "up", &prev, &prev, total > 0).clicked() {
                        todo = Some(Action::Previous);
                    }
                    if crate::widgets::flat_icon_button(ui, "down", &next, &next, total > 0).clicked() {
                        todo = Some(Action::Next);
                    }
                    let close = tl!("Close (Esc)").to_string();
                    if crate::widgets::flat_icon_button(ui, "close", &close, &close, true).clicked() {
                        todo = Some(Action::Close);
                    }
                    // Escape in either field closes the bar (the field gives the focus up on it).
                    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if escaped && (query.has_focus() || query.lost_focus() || ui.memory(|m| m.has_focus(replace_id))) {
                        todo = Some(Action::Close);
                    }
                });
                if bar.replace_open {
                    ui.horizontal(|ui| {
                        let replace = ui.add(
                            egui::TextEdit::singleline(&mut bar.replace)
                                .id(replace_id)
                                .desired_width(field)
                                .margin(egui::Margin { left: 6, right: 6, top: 3, bottom: 3 })
                                .hint_text(tl!("Replace")),
                        );
                        // Under the count, the Replace buttons: one match, and all of them.
                        ui.add_space(COUNT_WIDTH + GAP);
                        let can = !running && bar.current.is_some();
                        let wait = tl!("Wait for the run to finish").to_string();
                        let (one_say, all_say) = (tl!("Replace (Enter)").to_string(), tl!("Replace All").to_string());
                        // Off, the hover says why: a run, or nothing to replace.
                        let nothing = tl!("Nothing to replace").to_string();
                        let one_hover = if running {
                            &wait
                        } else if !can {
                            &nothing
                        } else {
                            &one_say
                        };
                        let all_hover = if running {
                            wait.clone()
                        } else if capped {
                            crate::i18n::fmt("Too many matches to replace at once (more than {n})", &[("n", &MAX_MATCHES.to_string())])
                        } else {
                            tl!("Replace every match (one step of Undo)").to_string()
                        };
                        if crate::widgets::flat_icon_button(ui, "replace", &one_say, one_hover, can).clicked() {
                            todo = Some(Action::Replace);
                        } else if replace.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            // A one-line box lets go of the focus on Enter: keep it, for the next replace,
                            // which is made only when there is one to make.
                            ui.memory_mut(|m| m.request_focus(replace_id));
                            if can {
                                todo = Some(Action::Replace);
                            }
                        }
                        if crate::widgets::flat_icon_button(ui, "replace_all", &all_say, &all_hover, !running && total > 0 && !capped)
                            .clicked()
                        {
                            todo = Some(Action::ReplaceAll);
                        }
                        if (replace.has_focus() || replace.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            todo = Some(Action::Close);
                        }
                    });
                }
            });
        });
    });
    let found = bar.found.clone();
    // What was found is of the script as it was when the matches were worked out: a key typed in the
    // box since, in this frame, leaves them for the next.
    let current_text = bar.key.as_ref().is_some_and(|k| k.0 == egui::util::hash(&*script));
    match todo {
        Some(Action::Next) => bar.go(step(total, bar.current, false)),
        Some(Action::Previous) => bar.go(step(total, bar.current, true)),
        Some(Action::Replace) if current_text => {
            if let Some(r) = bar.current.and_then(|i| found.ranges.get(i)).filter(|_| !running) {
                crate::analysispane::keep_for_undo_of(ui.ctx(), box_id, script);
                *script = replace_one(script, r.clone(), &bar.replace);
                let after = r.start + bar.replace.chars().count();
                crate::widgets::set_cursor_range(ui.ctx(), box_id, CCursorRange::one(CCursor::new(after)));
                bar.anchor = after;
                bar.reseek = true;
            }
        }
        Some(Action::ReplaceAll) if current_text => {
            if !running && !found.capped && !found.ranges.is_empty() {
                crate::analysispane::keep_for_undo_of(ui.ctx(), box_id, script);
                *script = replace_all(script, &found, &bar.replace);
                crate::widgets::set_cursor_range(ui.ctx(), box_id, CCursorRange::one(CCursor::new(0)));
                bar.anchor = 0;
                bar.reseek = true;
            }
        }
        Some(Action::Close) => {
            bar.open = false;
            // What it found goes with it (up to 10,000 ranges), and is worked out again on opening.
            bar.found = Default::default();
            bar.key = None;
            // The cursor where the search was: the current match, selected, in the box.
            if let Some(r) = bar.current.and_then(|i| found.ranges.get(i)) {
                crate::widgets::set_cursor_range(ui.ctx(), box_id, CCursorRange::two(CCursor::new(r.start), CCursor::new(r.end)));
            }
            ui.memory_mut(|m| m.request_focus(box_id));
        }
        _ => {}
    }
    if todo.is_some() || bar.reseek || !current_text {
        // What was asked, or a key typed in the box this frame, shows in the box on the next frame.
        ui.ctx().request_repaint();
    }
    ui.data_mut(|d| d.insert_temp(id, bar));
}

/// The fill of a match, and of the current one: amber over the text, translucent, as the stop's
/// mark is, the current one stronger and outlined too, so it isn't told by its colour alone.
const MATCH: egui::Color32 = egui::Color32::from_rgba_premultiplied(90, 70, 0, 90);
const CURRENT: egui::Color32 = egui::Color32::from_rgba_premultiplied(160, 80, 0, 170);

/// Paints the matches over the rows of the laid-out box that show, and, for `scroll`, brings the
/// current one into view in the enclosing scroll.
pub fn paint(ui: &egui::Ui, out: &egui::text_edit::TextEditOutput, shown: &Shown) {
    use egui::text::{CCursor, CharIndex};
    let galley = &out.galley;
    let origin = crate::widgets::text_origin(out);
    let painter = ui.painter().with_clip_rect(out.text_clip_rect.intersect(ui.clip_rect()));
    let seen = painter.clip_rect();
    let ranges = &shown.found.ranges;
    let mut start = 0;
    for row in &galley.rows {
        let shown_chars = row.glyphs.len();
        let y = row.rect().translate(origin).y_range();
        if y.max >= seen.top() && y.min <= seen.bottom() {
            // The first match that doesn't end before this row, then each one that starts in it.
            let first = ranges.partition_point(|r| r.end <= start);
            for (i, r) in ranges.iter().enumerate().skip(first).take_while(|(_, r)| r.start < start + shown_chars) {
                let (a, b) = (r.start.max(start) - start, r.end.min(start + shown_chars) - start);
                let x = |c: usize| row.pos.x + row.x_offset(CharIndex(c)) + origin.x;
                let rect = egui::Rect::from_x_y_ranges(x(a)..=x(b), y);
                if shown.current == Some(i) {
                    painter.rect_filled(rect, 2.0, CURRENT);
                    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, CURRENT), egui::StrokeKind::Outside);
                } else {
                    painter.rect_filled(rect, 2.0, MATCH);
                }
            }
        }
        start += row.char_count_including_newline().0;
    }
    if shown.scroll
        && let Some(r) = shown.current.and_then(|i| ranges.get(i))
    {
        let at = egui::text::CCursor { prefer_next_row: true, ..CCursor::new(r.start) };
        ui.scroll_to_rect(galley.pos_from_cursor(at).translate(origin), Some(egui::Align::Center));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, query: &str, case: bool) -> Vec<(usize, usize)> {
        matches(text, query, case, false).ranges.into_iter().map(|r| (r.start, r.end)).collect()
    }

    #[test]
    fn an_empty_query_is_nowhere() {
        assert_eq!(at("abc", "", true), []);
        assert_eq!(at("", "a", true), []);
    }

    #[test]
    fn matches_come_in_order_and_do_not_overlap() {
        assert_eq!(at("map.zoomFull\nmap.zoomIn", "map.", true), [(0, 4), (13, 17)]);
        assert_eq!(at("aaaa", "aa", true), [(0, 2), (2, 4)]);
    }

    #[test]
    fn case_counts_only_when_asked() {
        assert_eq!(at("Cities cities CITIES", "cities", false), [(0, 6), (7, 13), (14, 20)]);
        assert_eq!(at("Cities cities CITIES", "cities", true), [(7, 13)]);
    }

    #[test]
    fn a_whole_word_has_no_letter_digit_or_underscore_against_it() {
        let whole = |text: &str, query: &str| -> Vec<(usize, usize)> {
            matches(text, query, true, true).ranges.into_iter().map(|r| (r.start, r.end)).collect()
        };
        // Punctuation, spaces and the ends of the text are boundaries; letters, digits and `_` are not.
        assert_eq!(whole("city, (city) city_x xcity city2 city", "city"), [(0, 4), (7, 11), (32, 36)]);
        // A word that is not ASCII, and a query that is not a word at all.
        assert_eq!(whole("école écoles", "école"), [(0, 5)]);
        assert_eq!(whole("a.b a.bc", "a.b"), [(0, 3)]);
        // Without it, every one counts.
        assert_eq!(matches("city cityx", "city", true, false).ranges.len(), 2);
        // A refused place doesn't hide the next one inside it.
        assert_eq!(whole("aaa a", "a"), [(4, 5)]);
    }

    #[test]
    fn positions_are_characters_not_bytes() {
        // "é" is two bytes, one character; the match after it is at 3, not 4.
        assert_eq!(at("é\"ab\"", "ab", true), [(2, 4)]);
        assert_eq!(at("ÉCOLE école", "école", false), [(0, 5), (6, 11)]);
    }

    #[test]
    fn a_letter_whose_lower_case_is_longer_keeps_the_positions() {
        // "İ" lower-cases to two characters; folding must not shift what comes after it.
        assert_eq!(at("İx x", "x", false), [(1, 2), (3, 4)]);
        // And it is not an `i`: it matches itself only.
        assert_eq!(at("İ i", "i", false), [(2, 3)]);
        assert_eq!(at("İ i", "İ", false), [(0, 1)]);
    }

    #[test]
    fn only_the_first_matches_are_kept() {
        let text = "a".repeat(MAX_MATCHES + 5);
        let found = matches(&text, "a", true, false);
        assert_eq!(found.ranges.len(), MAX_MATCHES);
        assert!(found.capped);
        assert!(!matches("aaa", "a", true, false).capped);
    }

    #[test]
    fn stepping_goes_round() {
        assert_eq!(step(0, None, false), None);
        assert_eq!(step(3, None, false), Some(0));
        assert_eq!(step(3, None, true), Some(2));
        assert_eq!(step(3, Some(2), false), Some(0));
        assert_eq!(step(3, Some(0), true), Some(2));
        assert_eq!(step(3, Some(1), false), Some(2));
        assert_eq!(step(3, Some(1), true), Some(0));
        // A current one past the end (the text got shorter) is taken as the last.
        assert_eq!(step(2, Some(9), false), Some(0));
    }

    #[test]
    fn the_first_match_from_a_cursor() {
        let found = matches("ab ab ab", "ab", true, false);
        assert_eq!(first_from(&found, 0), Some(0));
        assert_eq!(first_from(&found, 1), Some(1));
        assert_eq!(first_from(&found, 6), Some(2));
        // Past the last: round to the first.
        assert_eq!(first_from(&found, 7), Some(0));
        assert_eq!(first_from(&matches("x", "y", true, false), 0), None);
    }

    #[test]
    fn a_replace_leaves_the_rest_alone() {
        assert_eq!(replace_one("a Cities b", 2..8, "Towns"), "a Towns b");
        assert_eq!(replace_one("é Cities", 2..8, "Ü"), "é Ü");
        // An empty range is an insertion.
        assert_eq!(replace_one("abc", 1..1, "X"), "aXbc");
        // A range past the end is cut to it, not a panic.
        assert_eq!(replace_one("abc", 2..9, "X"), "abX");
        assert_eq!(replace_one("abc", 7..9, "X"), "abcX");
    }

    #[test]
    fn replace_all_takes_every_match_and_not_what_it_adds() {
        let text = "Cities, Cities\nCities";
        let found = matches(text, "Cities", true, false);
        assert_eq!(replace_all(text, &found, "Cities2"), "Cities2, Cities2\nCities2");
        assert_eq!(replace_all(text, &found, ""), ", \n");
        assert_eq!(replace_all("é a é", &matches("é a é", "é", true, false), "ee"), "ee a ee");
        assert_eq!(replace_all("abc", &Found::default(), "X"), "abc");
    }
}
