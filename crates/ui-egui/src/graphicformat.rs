//! A text graphic in the Format pane (#516): the one text selected on the map, edited with the
//! form a layout's text has, each change one `graphics.edit` (one undo step). The pane shows a
//! layout's element instead while a layout is on screen.

use serde_json::json;

use rustgis_engine::map::graphics;

use crate::RustGisApp;
use crate::theme::Tokens;

/// A text graphic, by map (its index in the project), layer id and row: layer ids and rows are a
/// map's own.
type Key = (usize, u32, usize);

#[derive(Default)]
pub struct GraphicFormat {
    /// The text the pane last opened for: a new one opens it again, so closing the pane keeps the
    /// selection without the pane coming back by itself.
    formatted: Option<Key>,
    /// The text being typed: its graphic, the project it is for (`Session::opened`), and the text.
    draft: Option<(Key, u64, String)>,
    /// The graphic the pane drew the form of this frame: a draft for any other is saved at the
    /// end of the frame (`save_draft`).
    drawn: Option<Key>,
}

/// The text graphic the Format pane edits: the one selected on the map, when it is the only
/// thing selected there and a text.
pub(crate) fn selected(app: &RustGisApp) -> Option<(u32, usize)> {
    let mut rows = app.session.map().layers.iter().flat_map(|l| l.selection.iter().map(move |r| (l, *r)));
    let (l, row) = rows.next()?;
    if rows.next().is_some() || !l.graphics || graphics::kind(&l.dataset, row) != "text" {
        return None;
    }
    Some((l.id, row))
}

fn key(app: &RustGisApp, (layer, row): (u32, usize)) -> Key {
    (app.session.project.active_map, layer, row)
}

/// Opens the Format pane when another text is selected on the map (run each frame the map shows).
pub(crate) fn follow(app: &mut RustGisApp, ctx: &egui::Context) {
    let now = selected(app).map(|s| key(app, s));
    if now != app.graphic_format.formatted {
        app.graphic_format.formatted = now;
        if now.is_some() {
            app.ui.open_right(crate::RightPane::Format);
            ctx.request_repaint();
        }
    }
}

/// Selects a placed text and opens the Format pane on it (a double-click, Edit Text…, or
/// `ui.dialog` for a text by its layer and row), even closed over the same text.
pub(crate) fn edit(app: &mut RustGisApp, layer: u32, row: usize) -> Result<(), String> {
    let found = app.session.map().layer(layer).filter(|l| l.graphics).is_some_and(|l| graphics::kind(&l.dataset, row) == "text");
    if !found {
        return Err(tl!("no text graphic there").to_string());
    }
    app.run("select.rows", json!({"layer": layer, "rows": [row]}))?;
    app.graphic_format.formatted = Some(key(app, (layer, row)));
    app.ui.open_right(crate::RightPane::Format);
    Ok(())
}

/// The Format pane: a layout's element while a layout is on screen, else the text selected on
/// the map.
pub fn show(app: &mut RustGisApp, ui: &mut egui::Ui, t: &Tokens) {
    if app.session.active_layout.is_some() && !app.mb.shown() {
        crate::layoutview::format(app, ui, t);
        return;
    }
    let Some((layer, row)) = selected(app) else {
        ui.label(egui::RichText::new(tl!("Select one text on the map, or a layout element, to format it.")).color(t.text_dim));
        return;
    };
    let Some(l) = app.session.map().layer(layer) else { return };
    let ds = &l.dataset;
    let current = graphics::label(ds, row).unwrap_or_default().to_string();
    let (mut size, mut color, mut bold) =
        (f64::from(graphics::font_size(ds, row)), crate::theme::c32(graphics::text_color(ds, row)), graphics::bold(ds, row));
    let k = key(app, (layer, row));
    let opened = app.session.opened;
    app.graphic_format.drawn = Some(k);
    // A draft for another text is left for `save_draft`.
    let mut text = match app.graphic_format.draft.take() {
        Some((dk, p, t)) if dk == k && p == opened => t,
        other => {
            app.graphic_format.draft = other;
            current.clone()
        }
    };
    ui.label(egui::RichText::new(tl!("Text Graphic")).font(crate::theme::semibold(14.0)));
    ui.separator();
    let fields = crate::textform::Fields { text: &mut text, size: &mut size, color: Some(&mut color), bold: Some(&mut bold) };
    let settled = crate::textform::show(ui, ("graphic_format", k), &[graphics::TOKENS], fields, |_| {});
    let save = |app: &mut RustGisApp, change: serde_json::Value| {
        let mut p = json!({"layer": layer, "row": row});
        if let (Some(p), Some(c)) = (p.as_object_mut(), change.as_object()) {
            p.extend(c.clone());
        }
        let _ = app.run("graphics.edit", p);
    };
    if settled.typing {
        app.graphic_format.draft = Some((k, opened, text));
    } else if settled.text && text != current {
        save(app, json!({"text": text}));
    }
    if settled.size {
        save(app, json!({"fontSize": size}));
    }
    if settled.color {
        save(app, json!({"fill": crate::theme::rgba(color).hex8()}));
    }
    if settled.bold {
        save(app, json!({"bold": bold}));
    }
}

/// Saves the text typed for a graphic the pane didn't draw this frame (the pane was closed or
/// hidden, or the selection changed) before its field could say it lost focus. A draft for
/// another project, or another map than the one shown, is dropped: its layer ids and rows aren't
/// this one's. Runs once a frame, after the panes and the view.
pub(crate) fn save_draft(app: &mut RustGisApp) {
    let drawn = app.graphic_format.drawn.take();
    let Some((k, opened, text)) = app.graphic_format.draft.take() else { return };
    if Some(k) == drawn && opened == app.session.opened {
        app.graphic_format.draft = Some((k, opened, text));
        return;
    }
    let (map, layer, row) = k;
    let here = opened == app.session.opened && map == app.session.project.active_map;
    let current = app.session.map().layer(layer).and_then(|l| graphics::label(&l.dataset, row)).map(str::to_string);
    if here && current.is_some_and(|c| c != text) && !text.trim().is_empty() {
        let _ = app.run("graphics.edit", json!({"layer": layer, "row": row, "text": text}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RightPane::Format;
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;

    fn harness(app: RustGisApp) -> egui_kittest::Harness<'static, RustGisApp> {
        let mut fonts = false;
        egui_kittest::Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                follow(app, &ui.ctx().clone());
                let t = Tokens::get(ui.ctx());
                if app.ui.right == Some(Format) {
                    show(app, ui, &t);
                }
                save_draft(app);
            },
            app,
        )
    }

    /// Selecting one text on the map opens the Format pane on it; its form saves each change as
    /// one undo step, and text being typed when the pane closes is saved.
    #[test]
    fn a_selected_text_is_formatted_in_the_pane() {
        let mut app = RustGisApp::with_sample();
        let added = app.run("graphics.add", json!({"shape": "text", "points": [[10, 50]], "text": "Old"})).unwrap();
        let (layer, row) = (added["layer"].as_u64().unwrap() as u32, added["row"].as_u64().unwrap() as usize);
        let mut h = harness(app);
        h.run_steps(2);
        assert_eq!(h.state().ui.right, None, "nothing selected, nothing opened");
        h.state_mut().run("select.rows", json!({"layer": layer, "rows": [row]})).unwrap();
        h.run_steps(3);
        assert_eq!(h.state().ui.right, Some(Format));
        let ds = |h: &egui_kittest::Harness<'_, RustGisApp>| h.state().session.map().layer(layer).unwrap().dataset.clone();
        // Bold, at once and undoable.
        h.get_by_role_and_label(Role::CheckBox, "Bold").click();
        h.run_steps(2);
        assert!(graphics::bold(&ds(&h), row));
        assert_eq!(h.state().session.undo_label(), Some("Edit Graphic"));
        // Typed, then the pane closed: the text is saved, the text still selected.
        h.query_all_by_role(Role::MultilineTextInput).find(|n| n.value().as_deref() == Some("Old")).unwrap().focus();
        h.run_steps(2);
        h.query_all_by_role(Role::MultilineTextInput).find(|n| n.is_focused()).unwrap().type_text("New");
        h.run_steps(2);
        assert_eq!(graphics::label(&ds(&h), row), Some("Old"), "still being typed");
        h.state_mut().run("ui.pane.format", json!({})).unwrap();
        h.run_steps(2);
        assert!(graphics::label(&ds(&h), row).is_some_and(|t| t.contains("New")), "{:?}", graphics::label(&ds(&h), row));
        assert_eq!(selected(h.state()), Some((layer, row)));
        assert_eq!(h.state().ui.right, None, "closed, it stays closed");
        // Editing it again (a double-click, Edit Text…) brings it back.
        edit(h.state_mut(), layer, row).unwrap();
        assert_eq!(h.state().ui.right, Some(Format));
        assert!(edit(h.state_mut(), layer, row + 99).is_err());
    }

    /// Only one text is formatted: two graphics selected, or one that isn't a text, leave the
    /// pane waiting.
    #[test]
    fn only_one_selected_text_is_formatted() {
        let mut app = RustGisApp::with_sample();
        let a = app.run("graphics.add", json!({"shape": "text", "points": [[10, 50]], "text": "A"})).unwrap();
        let layer = a["layer"].as_u64().unwrap();
        let b = app.run("graphics.add", json!({"shape": "text", "points": [[20, 40]], "text": "B", "layer": layer})).unwrap();
        let point = app.run("graphics.add", json!({"shape": "point", "points": [[30, 30]], "layer": layer})).unwrap();
        app.run("select.rows", json!({"layer": layer, "rows": [a["row"], b["row"]]})).unwrap();
        assert_eq!(selected(&app), None, "two texts");
        app.run("select.rows", json!({"layer": layer, "rows": [point["row"]]})).unwrap();
        assert_eq!(selected(&app), None, "a point");
        app.run("select.rows", json!({"layer": layer, "rows": [b["row"]]})).unwrap();
        assert_eq!(selected(&app), Some((layer as u32, b["row"].as_u64().unwrap() as usize)));
    }
}
