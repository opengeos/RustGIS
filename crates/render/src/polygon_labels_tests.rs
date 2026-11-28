//! A polygon's label trying other places (#3268): its pole of inaccessibility first, then where
//! it goes without the setting.

use super::*;
use rustgis_data::{Dataset, Field, FieldType, Value};
use rustgis_map::{Camera, LabelPlacement};

fn c(x: f64, y: f64) -> Coord {
    Coord::new(x, y)
}

fn ring(points: &[(f64, f64)]) -> Vec<Coord> {
    points.iter().chain(points.first()).map(|&(x, y)| c(x, y)).collect()
}

/// A U open to the north, in degrees: arms and base 0.1 wide. Its label point is in the widest
/// span, the base's, against its south edge; its pole is in a corner of the base, farther in.
fn u() -> Vec<Vec<Coord>> {
    vec![ring(&[(-0.25, -0.2), (0.25, -0.2), (0.25, 0.2), (0.15, 0.2), (0.15, -0.1), (-0.15, -0.1), (-0.15, 0.2), (-0.25, 0.2)])]
}

fn square(x: f64, y: f64, half: f64) -> Vec<Coord> {
    ring(&[(x - half, y - half), (x + half, y - half), (x + half, y + half), (x - half, y + half)])
}

/// A map with one polygon layer labelled "Lake", trying other places when `on`.
fn map(geometry: Geometry, on: bool) -> (Map, u32) {
    let mut m = Map::new("Map");
    let mut d = Dataset::new("areas", GeometryType::Polygon, vec![Field::new("name", FieldType::Text)]);
    d.push(Some(geometry), vec![Value::Text("Lake".into())]);
    let id = m.add_dataset(d);
    if let Some(l) = m.layer_mut(id) {
        l.labels.enabled = true;
        l.labels.default.other_positions = on;
    }
    (m, id)
}

fn view_at(lon: f64, lat: f64) -> View {
    let camera = Camera { center: lonlat_to_merc(c(lon, lat)), res: 100.0, bearing: 0.0, pitch: 0.0 };
    View { width: 800.0, height: 600.0, pixels_per_point: 1.0, camera, sky: false }
}

fn view() -> View {
    view_at(0.0, 0.0)
}

fn places(m: &mut Map, v: &View) -> Vec<(f32, f32)> {
    render(m, v).unwrap().labels.into_iter().filter(|l| l.text == "Lake").map(|l| (l.x, l.y)).collect()
}

fn one(m: &mut Map, v: &View) -> (f32, f32) {
    let p = places(m, v);
    assert_eq!(p.len(), 1, "{p:?}");
    p[0]
}

/// The longitude and latitude under a place on the screen.
fn at(v: &View, (x, y): (f32, f32)) -> Coord {
    rustgis_geom::merc_to_lonlat(v.camera.to_map(f64::from(x), f64::from(y), v.width, v.height))
}

/// How far, in degrees, the place is from the nearest edge, negative outside the polygon.
fn depth(rings: &[Vec<Coord>], p: Coord) -> f64 {
    let mut inside = false;
    let mut best = f64::INFINITY;
    for r in rings {
        for w in r.windows(2) {
            let (a, b) = (w[0], w[1]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            best = best.min((p.x - a.x - t * dx).hypot(p.y - a.y - t * dy));
        }
    }
    if inside { best } else { -best }
}

/// A point layer above the polygon with a heavier label centred at `places`, which take them.
fn block(m: &mut Map, v: &View, places: &[(f32, f32)]) {
    let mut d = Dataset::new("blockers", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
    for &p in places {
        d.push(Some(Geometry::Point(at(v, p))), vec![Value::Text("XX".into())]);
    }
    let id = m.add_dataset(d);
    if let Some(l) = m.layer_mut(id) {
        l.renderer = rustgis_map::Renderer::Single(rustgis_map::Symbol { size: 1.0, ..l.symbol(0) });
        l.labels.enabled = true;
        l.labels.default.placement = LabelPlacement::Center;
        l.labels.default.weight = 10;
    }
}

/// The polygon's label point on the screen.
fn today(v: &View, g: &Geometry) -> (f32, f32) {
    let (x, y) = v.to_screen(lonlat_to_merc(g.label_point().unwrap()));
    (x as f32, y as f32)
}

fn near(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).hypot(a.1 - b.1) < 1.0
}

#[test]
fn off_a_polygons_label_stays_at_its_label_point() {
    let g = Geometry::Polygon(vec![u()]);
    let (mut m, _) = map(g.clone(), false);
    let v = view();
    assert!(near(one(&mut m, &v), today(&v, &g)));
}

#[test]
fn on_it_goes_to_the_point_farthest_from_the_edges() {
    let g = Geometry::Polygon(vec![u()]);
    let (mut m, _) = map(g.clone(), true);
    let v = view();
    let placed = one(&mut m, &v);
    assert!(!near(placed, today(&v, &g)), "{placed:?}");
    // The widest circle fits a corner of the base, radius 0.0586; an arm's is 0.05.
    let d = depth(&u(), at(&v, placed));
    assert!(d > 0.054, "{d}");
}

#[test]
fn with_its_pole_taken_it_goes_to_its_label_point() {
    let g = Geometry::Polygon(vec![u()]);
    let (mut m, _) = map(g.clone(), true);
    let v = view();
    let pole = one(&mut m, &v);
    block(&mut m, &v, &[pole]);
    assert!(near(one(&mut m, &v), today(&v, &g)));
}

#[test]
fn with_both_taken_it_is_unplaced_once_at_its_pole() {
    let g = Geometry::Polygon(vec![u()]);
    let (mut m, _) = map(g.clone(), true);
    let v = view();
    let pole = one(&mut m, &v);
    block(&mut m, &v, &[pole, today(&v, &g)]);
    m.label_display.show_unplaced = true;
    let r = render(&mut m, &v).unwrap();
    assert!(r.labels.iter().all(|l| l.text != "Lake"));
    let unplaced: Vec<_> = r.unplaced.iter().filter(|l| l.text == "Lake").map(|l| (l.x, l.y)).collect();
    assert_eq!(unplaced.len(), 1);
    assert!(near(unplaced[0], pole), "{unplaced:?} {pole:?}");
}

#[test]
fn a_polygons_pole_keeps_out_of_its_hole() {
    // A ring wider on the west: the hole is off centre.
    let rings = vec![square(0.0, 0.0, 0.2), square(0.05, 0.0, 0.1)];
    let (mut m, _) = map(Geometry::Polygon(vec![rings.clone()]), true);
    let v = view();
    let d = depth(&rings, at(&v, one(&mut m, &v)));
    assert!(d > 0.065, "in the wide side, away from the hole: {d}");
}

#[test]
fn a_multipolygons_pole_is_in_the_part_its_label_point_picks() {
    let g = Geometry::Polygon(vec![vec![square(0.33, 0.2, 0.03)], u()]);
    let (mut m, _) = map(g, true);
    let v = view();
    let d = depth(&u(), at(&v, one(&mut m, &v)));
    assert!(d > 0.054, "{d}");
}

#[test]
fn a_polygon_without_area_keeps_its_label_point() {
    let g = Geometry::Polygon(vec![vec![ring(&[(-0.2, 0.0), (0.0, 0.0), (0.2, 0.0)])]]);
    let v = view();
    let off = places(&mut map(g.clone(), false).0, &v);
    assert_eq!(places(&mut map(g, true).0, &v), off);
}

#[test]
fn a_label_moved_by_hand_stays_where_it_was_put() {
    let (mut m, id) = map(Geometry::Polygon(vec![u()]), true);
    let fid = m.layer(id).unwrap().dataset.features.get(0).unwrap().id;
    m.layer_mut(id).unwrap().labels.default.moved = vec![rustgis_map::symbology::MovedLabel { id: fid, at: [0.0, 0.1] }];
    let v = view();
    let placed = at(&v, one(&mut m, &v));
    assert!((placed.x - 0.0).abs() < 2e-3 && (placed.y - 0.1).abs() < 2e-3, "{placed:?}");
}

#[test]
fn a_curved_label_has_no_pole() {
    let g = Geometry::Polygon(vec![u()]);
    let v = view();
    let render_curved = |on: bool| {
        let (mut m, id) = map(g.clone(), on);
        // A name too long for the centre line falls back to a straight label.
        let l = m.layer_mut(id).unwrap();
        l.labels.default.placement = LabelPlacement::Curved;
        render(&mut m, &v).unwrap().labels.into_iter().map(|l| (l.x, l.y)).collect::<Vec<_>>()
    };
    assert_eq!(render_curved(true), render_curved(false));
}

#[test]
fn a_lock_keeps_the_place_each_polygon_label_took() {
    let g = Geometry::Polygon(vec![u()]);
    for taken in [false, true] {
        let (mut m, _) = map(g.clone(), true);
        let v = view();
        let pole = one(&mut m, &v);
        if taken {
            block(&mut m, &v, &[pole]);
        }
        m.label_display.locked = true;
        let before = at(&v, one(&mut m, &v));
        let kept = m.label_lock.as_ref().unwrap().iter().find(|(_, place)| place.place == u8::from(taken));
        assert!(kept.is_some(), "{:?}", m.label_lock);
        let moved = view_at(0.03, 0.02);
        let after = at(&moved, one(&mut m, &moved));
        assert!((after.x - before.x).abs() < 2e-3 && (after.y - before.y).abs() < 2e-3, "{taken}: {before:?} {after:?}");
    }
}

/// With its pole out of view a label is at its label point, as without the setting, and a lock
/// taken then keeps it there when the pole comes into view.
#[test]
fn a_pole_out_of_view_leaves_the_label_at_its_label_point() {
    let g = Geometry::Polygon(vec![u()]);
    // The pole is above this view; the label point is inside it.
    let low = view_at(0.0, -0.435);
    let (mut m, _) = map(g.clone(), true);
    assert!(near(one(&mut m, &low), today(&low, &g)));
    m.label_display.locked = true;
    assert!(near(one(&mut m, &low), today(&low, &g)));
    let v = view();
    assert!(near(one(&mut m, &v), today(&v, &g)), "locked, it stays");
}

#[test]
fn turned_and_tilted_views_place_the_pole() {
    let g = Geometry::Polygon(vec![u()]);
    for (bearing, pitch) in [(30.0, 0.0), (0.0, 30.0)] {
        let mut v = view();
        (v.camera.bearing, v.camera.pitch) = (bearing, pitch);
        let off = one(&mut map(g.clone(), false).0, &v);
        let on = one(&mut map(g.clone(), true).0, &v);
        assert!(!near(on, off), "turned {bearing}, tilted {pitch}: {on:?}");
    }
}
