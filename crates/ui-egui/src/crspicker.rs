//! The coordinate system box: type a code or words of a name (`2056`, `swiss lv95`, `ESRI:102033`)
//! and the matching systems of the catalog drop down under it, the recently used ones first.
//! ↑/↓ move the highlight, Enter takes it, Escape closes the list; clicking a row takes it too.
//! A typed text the list doesn't hold (a PROJ string, a preset) stays as typed.

use crate::theme::Tokens;
use egui::{Id, Key, Ui};
use rustgis_engine::data::raster::crs_search::{self, Found, Kind};

/// The most systems the list holds; more words narrow it.
const LIMIT: usize = 60;
/// The most recently used systems kept.
pub(crate) const RECENT_MAX: usize = 10;
/// The preference holding them, one code per line, the latest first.
pub(crate) const RECENT_PREF: &str = "recent_crs";
/// The list's height before it scrolls.
const LIST_HEIGHT: f32 = 260.0;

/// What a box keeps between frames.
#[derive(Clone, Default)]
struct State {
    open: bool,
    /// The highlighted row.
    at: usize,
    /// The list last drawn, so a click outside both closes it.
    drawn: Option<egui::Rect>,
}

/// One row of the list.
struct Row {
    code: String,
    name: String,
    /// What the row's tooltip says: the projection's method and the area of use.
    about: Option<String>,
    section: Section,
}

/// The parts of the list, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Recent,
    /// The map's own projections (Robinson, Equal Earth…), offered by a map's box only.
    Map,
    Catalog,
}

/// The rows for `text`: the recently used systems that match it (all of them for no text), for a
/// map its own projections whose names match, then the catalog's. `map` keeps the projected
/// systems only, a map being drawn in one.
fn rows(text: &str, recent: &[String], map: bool) -> Vec<Row> {
    let text = text.trim();
    let words: Vec<String> = text.to_lowercase().split_whitespace().map(str::to_string).collect();
    let matches = |hay: &str, digits: &str| words.iter().all(|w| hay.contains(w.as_str()) || digits.starts_with(w.as_str()));
    let mut out: Vec<Row> = Vec::new();
    for code in recent {
        let name = crs_search::name_of(code).unwrap_or_default().to_string();
        if map && rustgis_engine::map::Projection::from_code(code).is_err() {
            continue;
        }
        let hay = format!("{} {}", code.to_lowercase(), name.to_lowercase());
        if matches(&hay, code.rsplit(':').next().unwrap_or_default()) {
            out.push(Row { code: code.clone(), name, about: None, section: Section::Recent });
        }
    }
    if map && !words.is_empty() {
        let own = [("mercator", "Web Mercator"), ("globe", "Globe")]
            .into_iter()
            .chain(rustgis_engine::map::projection::PRESETS.iter().map(|(id, label, _)| (*id, *label)));
        for (id, label) in own {
            let hay = format!("{} {} {}", id.to_lowercase(), label.to_lowercase(), crate::i18n::t(label).to_lowercase());
            if matches(&hay, "") {
                out.push(Row { code: id.to_string(), name: crate::i18n::t(label).to_string(), about: None, section: Section::Map });
            }
        }
    }
    for Found { code, name, kind, area, method } in crs_search::search(text, map, LIMIT) {
        if map && kind != Kind::Projected {
            continue;
        }
        if !out.iter().any(|r| r.code.eq_ignore_ascii_case(&code)) {
            let about = match (method, area) {
                (Some(m), Some(a)) => Some(format!("{m}\n{a}")),
                (m, a) => m.or(a),
            };
            out.push(Row { code, name, about, section: Section::Catalog });
        }
    }
    out
}

/// A coordinate system box `width` wide, editing `text`. `recent` are the recently used codes,
/// the latest first. Returns the box's response, and whether a row of the list was taken this
/// frame (its code is then in `text`).
pub(crate) fn crs_box(
    ui: &mut Ui,
    salt: &str,
    text: &mut String,
    recent: &[String],
    map: bool,
    width: f32,
    hint: &str,
    t: &Tokens,
) -> (egui::Response, bool) {
    let id = Id::unique(("crs_box", salt));
    let mut state: State = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let r = ui.add(egui::TextEdit::singleline(text).id(id.with("edit")).hint_text(hint).desired_width(width));
    if r.gained_focus() || r.clicked() || r.changed() {
        state.open = true;
    }
    // Focus selects what the box holds, so typing searches anew rather than adding to a code.
    if r.gained_focus()
        && let Some(mut edit) = egui::text_edit::TextEditState::load(ui.ctx(), r.id)
    {
        let end = text.chars().count();
        edit.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(end))));
        edit.store(ui.ctx(), r.id);
    }
    if r.changed() {
        state.at = 0;
    }
    let rows = if state.open { rows(text, recent, map) } else { Vec::new() };
    let mut taken = None;
    // Enter ends a single-line edit, so the box has just lost its focus when Enter takes a row.
    if state.open && (r.has_focus() || r.lost_focus()) {
        let (up, down, enter, escape) = ui
            .input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if down && !rows.is_empty() {
            state.at = (state.at + 1).min(rows.len() - 1);
        }
        if up {
            state.at = state.at.saturating_sub(1);
        }
        // The list's keys are its own: Escape doesn't close the dialog, Enter doesn't submit it.
        if escape {
            state.open = false;
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
        }
        if enter && let Some(row) = rows.get(state.at) {
            taken = Some(row.code.clone());
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
        }
    }
    // Enter ends a single-line edit, which loses focus: that is the take, not a close.
    if state.open && r.lost_focus() && taken.is_none() && !ui.input(|i| i.key_pressed(Key::Enter)) {
        // A click on the list loses the box's focus first: the list closes on a click elsewhere.
        let on_list = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| state.drawn.is_some_and(|d| d.contains(p)));
        if !on_list {
            state.open = false;
        }
    }
    if state.open
        && ui.input(|i| i.pointer.any_pressed())
        && let Some(p) = ui.input(|i| i.pointer.interact_pos())
        && !r.rect.contains(p)
        && !state.drawn.is_some_and(|d| d.contains(p))
    {
        state.open = false;
    }
    state.at = state.at.min(rows.len().saturating_sub(1));
    if state.open && (!rows.is_empty() || !text.trim().is_empty()) {
        let shown = egui::Popup::from_response(&r)
            .id(id.with("list"))
            .open(true)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .width(width.max(360.0))
            .show(|ui| {
                // As wide as asked, and as tall as its rows up to its height: the popup keeps its
                // first frame's size otherwise, a row or two when the list opened on one code.
                ui.set_min_width(width.max(360.0));
                list(ui, &rows, &mut state.at, t)
            });
        if let Some(shown) = shown {
            state.drawn = Some(shown.response.rect);
            if let Some(i) = shown.inner {
                taken = rows.get(i).map(|row| row.code.clone());
            }
        }
    } else {
        state.drawn = None;
    }
    if let Some(code) = &taken {
        *text = code.clone();
        state.open = false;
    }
    ui.data_mut(|d| d.insert_temp(id, state));
    (r, taken.is_some())
}

/// Draws the rows; the one clicked, if any.
fn list(ui: &mut Ui, rows: &[Row], at: &mut usize, t: &Tokens) -> Option<usize> {
    if rows.is_empty() {
        ui.label(egui::RichText::new(tl!("No coordinate system matches; a code or PROJ string is used as typed.")).weak());
        return None;
    }
    let mut clicked = None;
    let tall = (rows.len() as f32 * 30.0 + 24.0).min(LIST_HEIGHT);
    egui::ScrollArea::vertical().max_height(LIST_HEIGHT).min_scrolled_height(tall).show(ui, |ui| {
        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
            let mut heading = None;
            for (i, row) in rows.iter().enumerate() {
                let section = match row.section {
                    Section::Recent => tl!("Recently used"),
                    Section::Map => tl!("Map projections"),
                    Section::Catalog => tl!("Coordinate systems"),
                };
                if heading != Some(section) {
                    if heading.is_some() {
                        ui.separator();
                    }
                    ui.label(egui::RichText::new(section).size(11.0).color(t.text_dim));
                    heading = Some(section);
                }
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    &row.code,
                    0.0,
                    egui::TextFormat { font_id: egui::FontId::monospace(11.5), color: t.text_dim, ..Default::default() },
                );
                if row.section == Section::Map {
                    job = egui::text::LayoutJob::default();
                }
                if !row.name.is_empty() {
                    job.append(
                        &row.name,
                        if job.is_empty() { 0.0 } else { 10.0 },
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(13.0),
                            color: ui.visuals().text_color(),
                            ..Default::default()
                        },
                    );
                }
                let r = ui.selectable_label(*at == i, job);
                let r = match &row.about {
                    Some(about) => r.on_hover_text(about),
                    None => r,
                };
                if *at == i && ui.input(|inp| inp.key_pressed(Key::ArrowDown) || inp.key_pressed(Key::ArrowUp)) {
                    r.scroll_to_me(None);
                }
                if r.clicked() {
                    *at = i;
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

/// Notes `code` as the latest coordinate system used, keeping [`RECENT_MAX`]; blank is ignored.
pub(crate) fn remember(recent: &mut Vec<String>, code: &str) -> bool {
    let code = code.trim();
    if code.is_empty() {
        return false;
    }
    recent.retain(|c| !c.eq_ignore_ascii_case(code));
    recent.insert(0, code.to_string());
    recent.truncate(RECENT_MAX);
    true
}

/// The recently used codes saved as [`RECENT_PREF`].
pub(crate) fn from_pref(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in value.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !out.iter().any(|c: &String| c.eq_ignore_ascii_case(line)) && out.len() < RECENT_MAX {
            out.push(line.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recently_used_systems_come_first_and_only_once() {
        let recent = vec!["EPSG:2056".to_string(), "EPSG:4326".to_string()];
        let found = rows("lv95", &recent, false);
        assert_eq!(found[0].code, "EPSG:2056");
        assert_eq!(found[0].section, Section::Recent);
        assert_eq!(found.iter().filter(|r| r.code == "EPSG:2056").count(), 1);
        // No text: the recent ones alone.
        let all = rows("", &recent, false);
        assert_eq!(all.iter().map(|r| r.code.as_str()).collect::<Vec<_>>(), ["EPSG:2056", "EPSG:4326"]);
        assert_eq!(all[1].name, "WGS 84");
        // A map can't be drawn in degrees.
        assert!(!rows("", &recent, true).iter().any(|r| r.code == "EPSG:4326"));
        // A map's own projections are found by name, before the catalog's systems.
        let robinson = rows("robinson", &[], true);
        assert_eq!((robinson[0].code.as_str(), robinson[0].section), ("robinson", Section::Map));
        assert!(robinson.iter().any(|r| r.code == "ESRI:54030"));
        assert!(!rows("robinson", &[], false).iter().any(|r| r.section == Section::Map), "data is never in a preset");
    }

    #[test]
    fn the_recent_list_keeps_the_latest_first_without_repeats() {
        let mut recent = from_pref("EPSG:2056\n\nepsg:2056\nEPSG:3857\n");
        assert_eq!(recent, ["EPSG:2056", "EPSG:3857"]);
        assert!(remember(&mut recent, "EPSG:3857"));
        assert_eq!(recent, ["EPSG:3857", "EPSG:2056"]);
        assert!(!remember(&mut recent, "  "));
        for n in 0..20 {
            remember(&mut recent, &format!("EPSG:{}", 32600 + n));
        }
        assert_eq!(recent.len(), RECENT_MAX);
        assert_eq!(recent[0], "EPSG:32619");
    }
}
