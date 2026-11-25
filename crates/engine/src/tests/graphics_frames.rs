//! A rectangle, circle, ellipse or curve dragged on the map is added as the drag previewed it
//! (#2032 C2f-6, `"frame": "view"`): built in the map's own coordinates as the view faces, on a
//! map in another projection and on one turned. A command that names no frame builds them as
//! every release did.

use crate::{Project, Session};
use rustgis_data::Geometry;
use rustgis_geom::Coord;
use serde_json::{Value, json};

/// A session on a map in Swiss LV95 (EPSG:2056), whose grid is turned from the meridians away
/// from its centre, looking at the corners used below.
fn on_lv95() -> Session {
    let mut s = Session::new(Project::default());
    s.run("map.projection", &json!({"type": "EPSG:2056"})).unwrap();
    s.run("map.goToXY", &json!({"lon": 9.0, "lat": 46.5, "scale": 2_000_000})).unwrap();
    s
}

/// The outer ring of the graphic `r` added, in longitude/latitude.
fn ring_of(s: &Session, r: &Value) -> Vec<Coord> {
    let (id, row) = (r["layer"].as_u64().unwrap() as u32, r["row"].as_u64().unwrap() as usize);
    match s.map().layer(id).unwrap().dataset.features[row].geometry.clone() {
        Some(Geometry::Polygon(p)) => p[0][0].clone(),
        other => panic!("{other:?}"),
    }
}

/// `c` in the map's coordinates turned to the view: x right, y up as the camera faces.
fn in_view(s: &Session, c: Coord) -> Coord {
    let m = s.map().to_map(c).unwrap();
    let b = s.map().camera.bearing.to_radians();
    Coord::new(m.x * b.cos() - m.y * b.sin(), m.x * b.sin() + m.y * b.cos())
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

#[test]
fn a_dragged_rectangle_has_the_views_corners_on_any_map() {
    for bearing in [0.0, 30.0] {
        let mut s = on_lv95();
        s.run("map.setBearing", &json!({"bearing": bearing})).unwrap();
        let r = s.run("graphics.add", &json!({"shape": "rectangle", "points": [[8.0, 46.0], [10.0, 47.0]], "frame": "view"})).unwrap();
        let ring: Vec<Coord> = ring_of(&s, &r).into_iter().map(|c| in_view(&s, c)).collect();
        assert_eq!(ring.len(), 5);
        // Its sides run along the view's x and y.
        for w in ring.windows(2) {
            let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
            assert!(dx.abs() < 1e-4 || dy.abs() < 1e-4, "bearing {bearing}: side {dx} {dy}");
        }
        // And the corners dragged between are two of its corners.
        for c in [Coord::new(8.0, 46.0), Coord::new(10.0, 47.0)] {
            let v = in_view(&s, c);
            assert!(ring.iter().any(|r| close(r.x, v.x, 1e-9) && close(r.y, v.y, 1e-9)), "bearing {bearing}: {c:?}");
        }
    }
    // Named by no frame, it is a box of longitudes and latitudes, as before: on LV95 its sides
    // don't run along the map's grid.
    let mut s = on_lv95();
    let r = s.run("graphics.add", &json!({"shape": "rectangle", "points": [[8.0, 46.0], [10.0, 47.0]]})).unwrap();
    let ring = ring_of(&s, &r);
    assert_eq!(ring[..4], [Coord::new(8.0, 46.0), Coord::new(10.0, 46.0), Coord::new(10.0, 47.0), Coord::new(8.0, 47.0)]);
    let (a, b) = (in_view(&s, ring[0]), in_view(&s, ring[1]));
    assert!((b.y - a.y).abs() > 1_000.0, "a parallel isn't a grid line on LV95: {}", b.y - a.y);
}

#[test]
fn a_dragged_circle_and_ellipse_are_round_in_the_view() {
    let mut s = on_lv95();
    s.run("map.setBearing", &json!({"bearing": -20.0})).unwrap();
    let r = s.run("graphics.add", &json!({"shape": "circle", "points": [[9.0, 46.5], [9.5, 46.5]], "frame": "view"})).unwrap();
    let (c, e) = (in_view(&s, Coord::new(9.0, 46.5)), in_view(&s, Coord::new(9.5, 46.5)));
    let radius = ((e.x - c.x).powi(2) + (e.y - c.y).powi(2)).sqrt();
    for p in ring_of(&s, &r) {
        let v = in_view(&s, p);
        let d = ((v.x - c.x).powi(2) + (v.y - c.y).powi(2)).sqrt();
        // Within what LV95's round trip through longitude/latitude moves a place (a millimetre).
        assert!(close(d, radius, 1e-7), "{d} vs {radius}");
    }
    // An ellipse fills the view's box between its corners: its widest points touch the box.
    let r = s.run("graphics.add", &json!({"shape": "ellipse", "points": [[8.0, 46.0], [10.0, 47.0]], "frame": "view"})).unwrap();
    let (a, b) = (in_view(&s, Coord::new(8.0, 46.0)), in_view(&s, Coord::new(10.0, 47.0)));
    let ring: Vec<Coord> = ring_of(&s, &r).into_iter().map(|p| in_view(&s, p)).collect();
    let max_x = ring.iter().map(|p| p.x).fold(f64::MIN, f64::max);
    let min_y = ring.iter().map(|p| p.y).fold(f64::MAX, f64::min);
    assert!(close(max_x, a.x.max(b.x), 1e-7) && close(min_y, a.y.min(b.y), 1e-7), "{max_x} {min_y}");
    // The preview is the outline added.
    let preview = crate::cmd::graphics::dragged_outline(&s, "ellipse", &[Coord::new(8.0, 46.0), Coord::new(10.0, 47.0)]).unwrap();
    assert_eq!(preview, ring_of(&s, &r));
}

#[test]
fn a_frame_other_than_view_or_mercator_is_refused() {
    let mut s = on_lv95();
    let e = s.run("graphics.add", &json!({"shape": "rectangle", "points": [[8.0, 46.0], [10.0, 47.0]], "frame": "map"})).unwrap_err();
    assert!(e.contains("frame \"map\""), "{e}");
    let e = s.run("graphics.add", &json!({"shape": "rectangle", "points": [[8.0, 46.0], [10.0, 47.0]], "frame": true})).unwrap_err();
    assert!(e.contains("frame true"), "{e}");
    // A shape that leaves the map's projection is refused, not drawn folded.
    let e = s.run("graphics.add", &json!({"shape": "rectangle", "points": [[-170.0, -80.0], [170.0, 80.0]], "frame": "view"})).unwrap_err();
    assert!(e.contains("isn't on the map") || e.contains("past the map's projection"), "{e}");
}

/// On a map that shows a pole, a corner dragged past 85° is added where it was dragged, as the
/// preview shows it: only Web Mercator keeps points within 85° of the equator.
#[test]
fn a_dragged_corner_near_a_pole_stays_where_it_was_dragged() {
    let mut s = Session::new(Project::default());
    s.run("map.projection", &json!({"type": "EPSG:3413"})).unwrap();
    s.run("map.goToXY", &json!({"lon": -45.0, "lat": 88.0, "scale": 20_000_000})).unwrap();
    let corners = [Coord::new(-60.0, 87.0), Coord::new(-30.0, 88.5)];
    let r = s.run("graphics.add", &json!({"shape": "rectangle", "points": [[-60.0, 87.0], [-30.0, 88.5]], "frame": "view"})).unwrap();
    let ring = ring_of(&s, &r);
    for c in corners {
        assert!(ring.iter().any(|p| (p.x - c.x).abs() < 1e-6 && (p.y - c.y).abs() < 1e-6), "{c:?} in {ring:?}");
    }
    assert_eq!(crate::cmd::graphics::dragged_outline(&s, "rectangle", &corners).unwrap(), ring);
}
