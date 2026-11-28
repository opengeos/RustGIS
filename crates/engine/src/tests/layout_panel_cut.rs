//! The cut of `layout.publication`'s panel sheet for an area much wider than tall (#3266): cut as
//! far as the panel allows, rather than not at all.

use super::*;
use rustgis_map::layout::{Element, Layout};

/// West, east and north of the areas; each test picks the south.
fn area(south: f64) -> Value {
    json!([-60.0, south, -30.0, -2.0])
}

/// Taller than any frame: never cut.
const TALL: [f64; 4] = [-46.8, -30.0, -38.5, -2.2];

/// A panel sheet with a title, a subtitle and sources, `extra`'s parameters over these.
fn panel(extra: Value) -> Result<Layout, String> {
    panel_of(world(), extra).map(|(l, _)| l)
}

/// The same over `s`'s map, with the command's warnings.
fn panel_of(mut s: Session, extra: Value) -> Result<(Layout, String), String> {
    let mut p = json!({"profile": "a4-portrait", "style": "panel", "extent": area(-3.0), "title": "Piauí",
        "subtitle": "Vegetation trend, 2001–2025", "sources": ["Boundaries: IBGE 2022"]});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    let r = s.run("layout.publication", &p)?;
    Ok((s.project.layouts.pop().unwrap(), r["warnings"].to_string()))
}

/// What a legend with more rows than its box takes is warned with.
const LEGEND_CUT: &str = "the legend lists more than fits its box";

/// A cut sheet whose legend is whole.
fn assert_cut_whole(extra: Value, uncut: f64, what: &str) -> Layout {
    let (l, warnings) = panel_of(world(), extra).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_cut(&l, uncut, what);
    assert!(!warnings.contains(LEGEND_CUT), "{what}: {warnings}");
    l
}

fn named<'a>(l: &'a Layout, name: &str) -> &'a Element {
    l.elements.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("no {name}"))
}

fn frame(l: &Layout) -> f64 {
    named(l, "Map Frame").h
}

/// The height left to the locator and the legend, from the first of them to the panel's foot.
fn column(l: &Layout) -> f64 {
    let panel = named(l, "Panel Box");
    let top = l
        .elements
        .iter()
        .filter(|e| e.name.starts_with("Locator") || e.name.starts_with("Legend"))
        .map(|e| e.y)
        .fold(f64::INFINITY, f64::min);
    panel.y + panel.h - top
}

/// Cut well short of the uncut frame, the locator and the legend still with their 30 mm (on A4).
fn assert_cut(l: &Layout, uncut: f64, what: &str) {
    assert!(frame(l) < 0.97 * uncut, "{what}: a frame of {} mm is not cut from {uncut}", frame(l));
    assert!(column(l) >= 30.0, "{what}: {} mm for the locator and the legend", column(l));
}

#[test]
fn a_panel_of_a_very_wide_area_is_cut_rather_than_left_whole() {
    let uncut = frame(&panel(json!({"extent": TALL})).unwrap());
    assert_cut(&panel(json!({"extent": area(-16.0)})).unwrap(), uncut, "the issue's area");
}

#[test]
fn a_wider_area_never_takes_a_taller_frame() {
    let uncut = frame(&panel(json!({"extent": TALL})).unwrap());
    let mut last = uncut;
    for south in [-20.0, -18.0, -16.0, -14.0, -10.0, -6.0, -3.0] {
        let l = panel(json!({"extent": area(south)})).unwrap();
        assert_cut(&l, uncut, &format!("south {south}"));
        assert!(frame(&l) <= last + 0.5, "south {south}: {} after {last}", frame(&l));
        last = frame(&l);
    }
}

#[test]
fn a_panel_with_no_title_is_cut_further() {
    let uncut = frame(&panel(json!({"extent": TALL})).unwrap());
    let titled = panel(json!({})).unwrap();
    let bare = panel(json!({"title": null, "subtitle": null})).unwrap();
    assert_cut(&titled, uncut, "titled");
    assert_cut(&bare, uncut, "no title");
    assert!(frame(&bare) < frame(&titled), "{} and {}", frame(&bare), frame(&titled));
}

#[test]
fn a_panels_tall_sources_never_fail_a_wide_area() {
    let uncut = frame(&panel(json!({"extent": TALL})).unwrap());
    let long = "Boundaries and statistics of the municipalities, revised edition ".repeat(3);
    let mut cut = 0;
    for n in 1..=20 {
        let texts = json!({"sources": vec![long.clone(); n], "reference": long});
        // Sources an uncut sheet has no room for are the request's own failure.
        let mut tall = texts.clone();
        tall["extent"] = json!(TALL);
        if panel(tall).is_err() {
            continue;
        }
        let l = panel(texts).unwrap_or_else(|e| panic!("{n} sources: {e}"));
        assert!(column(&l) >= 30.0, "{n} sources: {}", column(&l));
        if frame(&l) < 0.97 * uncut {
            cut += 1;
        }
    }
    assert!(cut > 0);
}

#[test]
fn a_wide_area_is_cut_on_every_page_size() {
    for profile in crate::cmd::layout::PUBLICATION_PROFILES.iter().filter(|p| p.sheet) {
        let id = profile.id;
        let uncut = frame(&panel(json!({"profile": id, "extent": TALL})).unwrap());
        let l = panel(json!({"profile": id})).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(frame(&l) < 0.97 * uncut, "{id}: {} from {uncut}", frame(&l));
        // The sheet's sizes are A4's, scaled by its short side.
        let k = l.width.min(l.height) / 210.0;
        assert!(column(&l) >= 30.0 * k, "{id}: {} mm for the locator and the legend", column(&l));
    }
}

#[test]
fn a_cut_panel_keeps_its_legend_whole() {
    let uncut = frame(&panel(json!({"extent": TALL})).unwrap());
    // -20 was cut already, its legend short of its last layer; -3 is a band 1° tall.
    for south in [-20.0, -16.0, -3.0] {
        assert_cut_whole(json!({"extent": area(south)}), uncut, &format!("south {south}"));
    }
}

#[test]
fn the_cut_leaves_room_for_the_legend_asked_for() {
    let uncut = frame(&panel(json!({"extent": TALL})).unwrap());
    let all = frame(&assert_cut_whole(json!({}), uncut, "every layer"));
    let fewer = assert_cut_whole(json!({"legend": {"exclude": ["Countries"]}}), uncut, "a layer left out");
    assert!(frame(&fewer) < all, "{} and {all}", frame(&fewer));
    let (none, _) = panel_of(world(), json!({"legend": "none"})).unwrap();
    assert!(frame(&none) < frame(&fewer), "{} and {}", frame(&none), frame(&fewer));
    assert_cut_whole(json!({"legend": {"columns": 2}}), uncut, "two columns");
    assert_cut_whole(json!({"locator": ["inset", "globe"]}), uncut, "two locators");
    assert_cut_whole(json!({"locator": "none"}), uncut, "no locator");
}

#[test]
fn a_legend_no_cut_sheet_could_hold_leaves_the_sheet_whole() {
    let mut s = world();
    s.run("layer.symbology", &json!({"layer": "Countries", "type": "unique", "field": "name"})).unwrap();
    let uncut = frame(&panel(json!({"extent": TALL})).unwrap());
    let (l, warnings) = panel_of(s, json!({})).unwrap();
    assert!(frame(&l) >= 0.97 * uncut, "{} from {uncut}", frame(&l));
    assert!(warnings.contains(LEGEND_CUT), "{warnings}");
}

#[test]
fn a_tall_area_stays_whole() {
    let tall = panel(json!({"extent": TALL})).unwrap();
    // A4's margin and padding put an uncut panel's top at 15 mm.
    assert!((named(&tall, "Panel Box").y - 15.0).abs() < 1e-6);
}

#[test]
fn the_cut_counts_the_layers_the_final_frame_lists() {
    // One-colour layers running past the frame are left out of the legend as context: a frame the
    // legend makes taller can take them whole, and its legend lists them.
    for half in [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0] {
        let mut s = world();
        let (w, so, e, n) = (-55.0, -9.0 - half, -35.0, -9.0 + half);
        let ring = json!([[[w, so], [e, so], [e, n], [w, n], [w, so]]]);
        let fc = json!({"type": "FeatureCollection", "features": [{"type": "Feature", "properties": {}, "geometry": {"type": "Polygon", "coordinates": ring}}]});
        let path = std::env::temp_dir().join(format!("rustgis-3266-{}-{half}.geojson", std::process::id()));
        std::fs::write(&path, fc.to_string()).unwrap();
        for i in 0..3 {
            s.run("layer.addData", &json!({"path": path.display().to_string(), "name": format!("Block {i}")})).unwrap();
        }
        let _ = std::fs::remove_file(path);
        let (_, warnings) = panel_of(s, json!({"extent": area(-16.0)})).unwrap();
        assert!(!warnings.contains(LEGEND_CUT), "half {half}: {warnings}");
    }
}
