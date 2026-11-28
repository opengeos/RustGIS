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
    let mut s = world();
    let mut p = json!({"profile": "a4-portrait", "style": "panel", "extent": area(-3.0), "title": "Piauí",
        "subtitle": "Vegetation trend, 2001–2025", "sources": ["Boundaries: IBGE 2022"]});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    s.run("layout.publication", &p)?;
    Ok(s.project.layouts.pop().unwrap())
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
    }
}

#[test]
fn an_area_already_cut_keeps_its_frame_and_a_tall_one_stays_whole() {
    let h = frame(&panel(json!({"extent": area(-20.0)})).unwrap());
    assert!((h - 86.0).abs() < 1.0, "{h}");
    let tall = panel(json!({"extent": TALL})).unwrap();
    // A4's margin and padding put an uncut panel's top at 15 mm.
    assert!((named(&tall, "Panel Box").y - 15.0).abs() < 1e-6);
}
