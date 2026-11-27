//! The wizard's map step.
use super::*;

/// Which area the map frame shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Area {
    /// The view, left to `layout.publication`.
    View,
    /// A layer's features, as Zoom To Layer frames them (hidden or not): the layer's id, in the
    /// map named.
    Layer { map: String, id: u32 },
    /// The selected features of the map named.
    Selection { map: String },
}

/// The name `layout.publication` gives when none is: data, not translated.
pub(super) const DEFAULT_NAME: &str = "Publication";

/// A side sent for an extent with none along it (a point, a straight line), in degrees.
const MIN_SIDE: f64 = 0.01;

/// The map step: the area the map frame shows and the layout's name.
pub(super) fn map_step(ui: &mut egui::Ui, s: &Session, d: &mut Draft) {
    let map = s.map();
    // Top first, as Contents lists them. A layer's kept extent here; the one it shows is read at
    // Create, as it walks the rows under a definition query.
    let framable = || map.layers.iter().rev().filter(|l| !l.extent().is_empty());
    egui::Grid::new("publication_wizard_map").num_columns(2).spacing(egui::vec2(8.0, 8.0)).show(ui, |ui| {
        // A row for each choice, so the label sits beside the first one.
        ui.label(tl!("Map extent"));
        if ui.radio(d.area == Area::View, tl!("View")).on_hover_text(tl!("What the map view shows.")).clicked() {
            d.area = Area::View;
        }
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| {
            let picked = match &d.area {
                Area::Layer { map: name, id } if *name == map.name => Some(*id),
                _ => None,
            };
            let top = framable().next().map(|l| l.id);
            if ui.add_enabled(top.is_some(), egui::RadioButton::new(matches!(d.area, Area::Layer { .. }), tl!("Layer"))).clicked()
                && let Some(id) = picked.or(top)
            {
                d.area = Area::Layer { map: map.name.clone(), id };
            }
            if let Some(id) = picked {
                let shown = framable().find(|l| l.id == id).map_or_else(String::new, |l| l.name.clone());
                egui::ComboBox::from_id_salt("publication_wizard_map_layer")
                    .selected_text(shown)
                    .width(ui.available_width().min(180.0))
                    .show_ui(ui, |ui| {
                        for l in framable() {
                            if ui.selectable_label(l.id == id, l.name.as_str()).clicked() {
                                d.area = Area::Layer { map: map.name.clone(), id: l.id };
                            }
                        }
                    });
            }
        });
        ui.end_row();
        ui.label("");
        let selection = egui::RadioButton::new(matches!(d.area, Area::Selection { .. }), tl!("Selection"));
        if ui
            .add_enabled(map.selection_count() > 0, selection)
            .on_hover_text(tl!("The selected features."))
            .on_disabled_hover_text(tl!("Nothing is selected in the map."))
            .clicked()
        {
            d.area = Area::Selection { map: map.name.clone() };
        }
        ui.end_row();
        let label = ui.label(tl!("Name"));
        ui.add(egui::TextEdit::singleline(&mut d.name).hint_text(DEFAULT_NAME).desired_width(field_width(ui))).labelled_by(label.id);
        ui.end_row();
    });
}

/// How the review names the area.
pub(super) fn area_label(s: &Session, area: &Area) -> String {
    match area {
        Area::View => tl!("The view").to_string(),
        Area::Layer { map, id } => s
            .map()
            .layers
            .iter()
            .find(|l| l.id == *id && s.map().name == *map)
            .map_or_else(|| tl!("(none)").to_string(), |l| i18n::fmt("Layer: {layer}", &[("layer", &l.name)])),
        Area::Selection { .. } => tl!("Selected features").to_string(),
    }
}

/// `e` as `layout.publication` takes it: cut to ±180 and ±90, a side of no length widened to
/// [`MIN_SIDE`]; none when nothing is left to frame.
fn framed(e: Extent) -> Option<[f64; 4]> {
    if e.is_empty() {
        return None;
    }
    let side = |min: f64, max: f64, limit: f64| {
        let pad = if max > min { 0.0 } else { MIN_SIDE / 2.0 };
        let (min, max) = ((min - pad).max(-limit), (max + pad).min(limit));
        (min < max).then_some((min, max))
    };
    let ((west, east), (south, north)) = (side(e.min_x, e.max_x, 180.0)?, side(e.min_y, e.max_y, 90.0)?);
    Some([west, south, east, north])
}

/// The `extent` to send for `area`, none for the view; an area that is gone since it was picked is
/// an error rather than another one framed.
pub(super) fn area_extent(s: &Session, area: &Area) -> Result<Option<[f64; 4]>, String> {
    let map = s.map();
    let extent = match area {
        Area::View => return Ok(None),
        Area::Layer { map: name, .. } | Area::Selection { map: name } if *name != map.name => {
            return Err(tl!("Another map is open since the map extent was picked: pick it again.").to_string());
        }
        Area::Layer { id, .. } => map
            .layers
            .iter()
            .find(|l| l.id == *id)
            .ok_or_else(|| tl!("The layer picked is no longer in the map: pick the map extent again.").to_string())?
            .shown_extent(),
        Area::Selection { .. } => {
            if map.selection_count() == 0 {
                return Err(tl!("Nothing is selected: pick the map extent again.").to_string());
            }
            map.selection_extent()
        }
    };
    framed(extent).map(Some).ok_or_else(|| tl!("There is nothing to frame there: pick another map extent.").to_string())
}

#[cfg(test)]
mod tests {
    use super::harness::*;
    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};

    fn close(got: Option<[f64; 4]>, want: [f64; 4]) {
        let got = got.expect("an extent");
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-9), "{got:?} is not {want:?}");
    }

    /// A side of no length (a point, a straight line) is widened to 0.01°, the rest is left as it
    /// is; what lies past ±180 and ±90 is cut; nothing left to frame is none.
    #[test]
    fn an_extent_is_widened_and_cut_to_what_the_command_takes() {
        close(framed(Extent::new(10.0, 20.0, 10.0, 20.0)), [9.995, 19.995, 10.005, 20.005]);
        close(framed(Extent::new(10.0, 20.0, 12.0, 20.0)), [10.0, 19.995, 12.0, 20.005]);
        close(framed(Extent::new(10.0, 20.0, 10.0, 22.0)), [9.995, 20.0, 10.005, 22.0]);
        close(framed(Extent::new(180.0, 90.0, 180.0, 90.0)), [179.995, 89.995, 180.0, 90.0]);
        close(framed(Extent::new(-190.0, -95.0, 200.0, 91.0)), [-180.0, -90.0, 180.0, 90.0]);
        assert_eq!(framed(Extent::EMPTY), None);
        assert_eq!(framed(Extent::new(190.0, 0.0, 200.0, 1.0)), None);
    }

    /// The view sends no extent; a layer sends the extent of the features it shows; the
    /// selection, that of the selected features.
    #[test]
    fn each_area_sends_its_own_extent() {
        let mut app = app();
        let map = app.session.map().name.clone();
        assert_eq!(area_extent(&app.session, &Area::View), Ok(None));
        app.run("layer.definitionQuery", json!({"layer": "Cities", "where": "pop_max > 10000000"})).unwrap();
        let (id, shown, all) = {
            let l = app.session.map().layers.iter().find(|l| l.name == "Cities").unwrap();
            (l.id, l.shown_extent(), l.extent())
        };
        assert_ne!(shown, all, "the query narrows it");
        assert_eq!(area_extent(&app.session, &Area::Layer { map: map.clone(), id }), Ok(framed(shown)));
        app.run("select.byAttributes", json!({"layer": "Cities", "where": "name = 'Tokyo'"})).unwrap();
        let selected = app.session.map().selection_extent();
        let got = area_extent(&app.session, &Area::Selection { map }).unwrap();
        assert_eq!(got, framed(selected));
        assert!(got.is_some_and(|[w, s, e, n]| w < e && s < n), "one point, framed: {got:?}");
    }

    /// A layer removed, a selection cleared or another map since the area was picked is refused,
    /// saying so, rather than framing something else.
    #[test]
    fn an_area_gone_is_refused() {
        let mut app = app();
        let map = app.session.map().name.clone();
        let id = app.session.map().layers.iter().find(|l| l.name == "Rivers").unwrap().id;
        let layer = Area::Layer { map: map.clone(), id };
        let selection = Area::Selection { map };
        assert_eq!(area_extent(&app.session, &selection), Err("Nothing is selected: pick the map extent again.".to_string()));
        app.run("layer.remove", json!({"layer": "Rivers"})).unwrap();
        assert_eq!(
            area_extent(&app.session, &layer),
            Err("The layer picked is no longer in the map: pick the map extent again.".to_string())
        );
        app.run("map.new", json!({})).unwrap();
        let other = Err("Another map is open since the map extent was picked: pick it again.".to_string());
        assert_eq!(area_extent(&app.session, &layer), other);
        assert_eq!(area_extent(&app.session, &selection), other);
    }

    /// The step starts on the view; Layer picks one from the list; Selection is off with nothing
    /// selected.
    #[test]
    fn the_step_picks_the_view_a_layer_or_the_selection() {
        let mut app = app();
        open(&mut app);
        app.dialogs.publication.as_mut().unwrap().step = Step::Map;
        let mut h = harness(app);
        let area = |h: &egui_kittest::Harness<'static, RustGisApp>| h.state().dialogs.publication.as_ref().unwrap().area.clone();
        assert_eq!(area(&h), Area::View);
        assert!(h.get_by_role_and_label(Role::RadioButton, "Selection").accesskit_node().is_disabled());
        h.get_by_role_and_label(Role::RadioButton, "Layer").click();
        h.run_steps(4);
        let top = h.state().session.map().layers.last().unwrap().id;
        assert!(matches!(area(&h), Area::Layer { id, .. } if id == top), "{:?}", area(&h));
        h.state_mut().run("select.byAttributes", json!({"layer": "Cities", "where": "name = 'Tokyo'"})).unwrap();
        h.run_steps(2);
        h.get_by_role_and_label(Role::RadioButton, "Selection").click();
        h.run_steps(4);
        assert!(matches!(area(&h), Area::Selection { .. }), "{:?}", area(&h));
    }

    /// Create frames the layer picked and names the layout as typed; a figure takes both too.
    #[test]
    fn create_frames_the_area_and_names_the_layout() {
        for size in ["a4", "figure-1col"] {
            let mut app = app();
            open(&mut app);
            let map = app.session.map().name.clone();
            app.run("select.byAttributes", json!({"layer": "Cities", "where": "name = 'Tokyo'"})).unwrap();
            let [w, s, e, n] = framed(app.session.map().selection_extent()).unwrap();
            {
                let d = app.dialogs.publication.as_mut().unwrap();
                d.size = size.into();
                d.area = Area::Selection { map };
                d.name = "  Tokyo sheet ".into();
                d.step = Step::Review;
            }
            let mut h = harness(app);
            click(&mut h, "Create");
            let l = h.state().session.project.layouts.last().unwrap();
            assert_eq!(l.name, "Tokyo sheet", "{size}");
            let center = l.elements.iter().find_map(|el| match &el.kind {
                rustgis_engine::map::layout::ElementKind::MapFrame { center, .. } if el.name == "Map Frame" => Some(*center),
                _ => None,
            });
            let c = center.expect("the map frame");
            assert!(c.x > w - 0.01 && c.x < e + 0.01 && c.y > s - 0.01 && c.y < n + 0.01, "{size}: {c:?}");
        }
    }

    /// An area gone at Create is said, and the wizard stays with nothing made.
    #[test]
    fn create_with_the_area_gone_is_said_and_makes_nothing() {
        let mut app = app();
        open(&mut app);
        let map = app.session.map().name.clone();
        {
            let d = app.dialogs.publication.as_mut().unwrap();
            d.area = Area::Selection { map };
            d.step = Step::Review;
        }
        let before = app.session.project.layouts.len();
        let mut h = harness(app);
        let said = h.state().session.notice_mark();
        click(&mut h, "Create");
        let app = h.state();
        assert_eq!(app.session.project.layouts.len(), before);
        assert_eq!(app.session.notice_mark(), said + 1);
        assert_eq!(app.dialogs.open, Some("publicationWizard"));
    }

    /// The name goes only when typed, for a sheet and for a figure.
    #[test]
    fn the_name_is_sent_only_when_typed() {
        let app = app();
        let mut d = Draft::new(&app.session);
        d.name = "   ".into();
        assert!(d.params().get("name").is_none());
        d.name = "Rivers".into();
        assert_eq!(d.params()["name"], "Rivers");
        d.size = "figure-1col".into();
        assert_eq!(d.params(), json!({"profile": "figure-1col", "name": "Rivers"}));
    }
}
