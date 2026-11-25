//! A layout made from New Layout is titled with the map's name as `{map}`, so renaming the map
//! changes the title.

use super::*;
use rustgis_map::layout::ElementKind;

/// What the layout `li`'s element `name` says as the page draws it: its text with the tokens
/// filled from the map in the layout's first map frame.
fn drawn(s: &Session, li: usize, name: &str) -> String {
    let l = &s.project.layouts[li];
    let ElementKind::Text { text, .. } = &l.elements.iter().find(|e| e.name == name).unwrap().kind else { panic!("{name} isn't text") };
    let ElementKind::MapFrame { map, .. } = &l.element(l.first_frame().unwrap()).unwrap().kind else { panic!() };
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)).unwrap();
    let cam = rustgis_map::layout::frame_camera(m, rustgis_geom::Coord::new(0.0, 0.0), 1.0e7, 0.0, 0.0);
    let info = crate::render::layout::PageInfo { page: 1, pages: 1, page_name: String::new() };
    crate::render::layout::expand(text, l, &info, Some((m, cam)))
}

/// The text stored in the layout `li`'s element `name`.
fn stored(s: &Session, li: usize, name: &str) -> String {
    match &s.project.layouts[li].elements.iter().find(|e| e.name == name).unwrap().kind {
        ElementKind::Text { text, .. } => text.clone(),
        k => panic!("{name}: {k:?}"),
    }
}

#[test]
fn the_new_layout_title_follows_a_rename_of_the_map() {
    let mut s = world();
    s.run("layout.new", &json!({"page": "a4", "template": "map"})).unwrap();
    let before = s.map().name.clone();
    assert_eq!(drawn(&s, 0, "Title"), before);
    s.run("map.rename", &json!({"name": "Location map"})).unwrap();
    assert_eq!(drawn(&s, 0, "Title"), "Location map");
    assert_eq!(stored(&s, 0, "Title"), "{map}");
}

#[test]
fn a_publication_sheet_titled_with_the_map_token_follows_a_rename() {
    let mut s = world();
    s.run("layout.publication", &json!({"profile": "a4", "extent": [-46.8, -11.6, -38.5, -2.2], "title": "{map}"})).unwrap();
    s.run("map.rename", &json!({"name": "Location map"})).unwrap();
    assert_eq!(drawn(&s, 0, "Title"), "Location map");
}

/// The layout `li`'s elements as (name, x, y, w, h), so two layouts compare by their boxes.
fn boxes(s: &Session, li: usize) -> Vec<(String, f64, f64, f64, f64)> {
    s.project.layouts[li].elements.iter().map(|e| (e.name.clone(), e.x, e.y, e.w, e.h)).collect()
}

#[test]
fn a_sheet_titled_with_the_map_token_is_laid_out_as_one_titled_with_the_name() {
    let mut s = world();
    let name = s.map().name.clone();
    let extent = json!([-46.8, -11.6, -38.5, -2.2]);
    s.run("layout.publication", &json!({"profile": "a4", "extent": extent, "title": name})).unwrap();
    s.run("layout.publication", &json!({"profile": "a4", "extent": extent, "title": "{map}"})).unwrap();
    assert_eq!(boxes(&s, 0), boxes(&s, 1));
    assert_eq!(drawn(&s, 0, "Title"), drawn(&s, 1, "Title"));
}

#[test]
fn a_title_typed_over_the_token_stays_as_typed_after_a_rename() {
    let mut s = world();
    s.run("layout.new", &json!({"page": "a4", "template": "map"})).unwrap();
    let title = s.project.layouts[0].elements.iter().find(|e| e.name == "Title").unwrap().id;
    s.run("layout.update", &json!({"layout": 0, "element": title, "text": "Flood risk"})).unwrap();
    s.run("map.rename", &json!({"name": "Location map"})).unwrap();
    assert_eq!(drawn(&s, 0, "Title"), "Flood risk");
    assert_eq!(stored(&s, 0, "Title"), "Flood risk");
}

/// A name wider than the title's box can't be a `{map}` that its lines break: the sheet writes it
/// out, wrapped as it was before, and says so.
#[test]
fn a_map_name_wider_than_the_title_is_written_out_and_said() {
    let mut s = world();
    let long = "A very long map name that goes on and on past the width of the title box on a sheet of paper";
    s.run("map.rename", &json!({"name": long})).unwrap();
    let extent = json!([-46.8, -11.6, -38.5, -2.2]);
    let r = s.run("layout.publication", &json!({"profile": "a4", "extent": extent, "title": "{map}"})).unwrap();
    let text = stored(&s, 0, "Title");
    assert_eq!(text.replace('\n', " "), long);
    assert!(text.contains('\n'), "wrapped to the box: {text}");
    let warnings: Vec<&str> = r["warnings"].as_array().unwrap().iter().filter_map(|w| w.as_str()).collect();
    assert_eq!(warnings.iter().filter(|w| w.contains("{map}") && w.contains("written out")).count(), 1, "{warnings:?}");
    // A name that fits stays the token, and nothing is said.
    s.run("map.rename", &json!({"name": "Location map"})).unwrap();
    let r = s.run("layout.publication", &json!({"profile": "a4", "extent": extent, "title": "{map}"})).unwrap();
    assert_eq!(stored(&s, 1, "Title"), "{map}");
    assert!(r["warnings"].as_array().unwrap().iter().all(|w| !w.as_str().unwrap_or("").contains("written out")), "{r}");
}
