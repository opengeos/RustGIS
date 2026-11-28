//! Profiles beside a layout's map frame (#2863 part 2): the commands, what they say, and how a
//! profile agrees with `profile.create`.

use crate::{Session, sample};
use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};
use rustgis_geom::Coord;
use rustgis_map::layout::{ElementKind, Profile, ProfileSide};
use serde_json::{Value, json};

/// A layout with a frame of the sample's map round (10, 50), a raster "DEM" of one degree's hundred
/// metres of latitude over it, and the frame's id.
fn with_dem() -> (Session, u64) {
    let mut s = Session::new(sample::world_project());
    let (n, cell) = (400usize, 0.05);
    let data = (0..n * n).map(|i| ((60.0 - ((i / n) as f64 + 0.5) * cell) * 100.0) as f32).collect();
    let gt = GeoTransform { origin_x: 0.0, origin_y: 60.0, pixel_width: cell, pixel_height: -cell };
    let r = Raster::new("dem", n as u32, n as u32, vec![data], gt, Crs::LonLat, None, ColorHint::Gray, "Float32").unwrap();
    s.map_mut().add_raster("DEM", rustgis_map::RasterLayer::from_path(r, "dem.tif"));
    s.run("layout.new", &json!({"page": "a4", "template": "map"})).unwrap();
    let frame = u64::from(s.project.layouts[0].first_frame().unwrap());
    s.run("layout.update", &json!({"element": frame, "center": [10, 50], "scale": 1e7})).unwrap();
    (s, frame)
}

fn profile(s: &Session, id: u64) -> (Profile, [f64; 4]) {
    let e = s.project.layouts[0].element(id as u32).expect("the element");
    match &e.kind {
        ElementKind::Profile(p) => (p.clone(), [e.x, e.y, e.w, e.h]),
        _ => panic!("a profile"),
    }
}

fn frame_box(s: &Session, frame: u64) -> [f64; 4] {
    let e = s.project.layouts[0].element(frame as u32).expect("the frame");
    [e.x, e.y, e.w, e.h]
}

fn said(s: &Session, mark: u64) -> Vec<String> {
    s.notices_since(mark).iter().map(|n| n.text.clone()).collect()
}

/// A profile is added along a section line of its frame (its first unless named), of the top
/// raster shown unless named, beside the frame on the side the line runs along; it follows the
/// frame when moved or resized, its section line and layer when renamed, and goes with the frame;
/// a box asked of it is its gap and thickness; and all of it is undone.
#[test]
fn a_profile_is_added_follows_its_frame_and_is_undone() {
    let (mut s, frame) = with_dem();
    let err = s.run("layout.addElement", &json!({"kind": "profile"})).unwrap_err();
    assert!(err.contains("the frame has no section line"), "{err}");
    s.run("layout.addElement", &json!({"kind": "section", "at": [10, 50]})).unwrap();
    let mark = s.notice_mark();
    let id = s.run("layout.addElement", &json!({"kind": "profile", "unit": "m", "layout": 0, "text": null})).unwrap()["element"]
        .as_u64()
        .unwrap();
    assert!(said(&s, mark).is_empty(), "{:?}", said(&s, mark));
    let (p, at) = profile(&s, id);
    assert_eq!(
        (p.section.as_str(), p.layer.as_str(), p.side, p.title.as_str(), p.unit.as_str()),
        ("A", "DEM", ProfileSide::Bottom, "DEM", "m")
    );
    let [fx, fy, fw, fh] = frame_box(&s, frame);
    assert_eq!(at, [fx, fy + fh + p.gap, fw, p.thickness]);
    // Refused, nothing changed: a side across its line, a key it doesn't read, a box of its own.
    for (bad, says) in [
        (json!({"kind": "profile", "side": "left"}), "goes bottom or top of its frame"),
        (json!({"kind": "profile", "rows": 5}), "a profile takes no \"rows\""),
        (json!({"kind": "profile", "x": 5}), "a profile's box is its frame's side"),
        (json!({"kind": "profile", "layer": "Countries"}), "no raster layer 'Countries'"),
        (json!({"kind": "profile", "band": 2}), "'DEM' has 1 band(s)"),
        (json!({"kind": "profile", "min": 9, "max": 1}), "min (9) is below max (1)"),
    ] {
        let err = s.run("layout.addElement", &bad).unwrap_err();
        assert!(err.contains(says), "{bad}: {err}");
    }
    assert_eq!(s.project.layouts[0].elements.iter().filter(|e| e.kind.is_profile()).count(), 1);
    // The frame moved and resized: the profile with it.
    s.run("layout.update", &json!({"element": frame, "x": 20, "y": 30, "w": 120})).unwrap();
    let (p, at) = profile(&s, id);
    assert_eq!(at, [20.0, 30.0 + fh + p.gap, 120.0, p.thickness]);
    // Dragged along the frame and away from it: only its gap follows; held to its limits, said.
    s.run("layout.update", &json!({"element": id, "x": 60, "y": at[1] + 4.0})).unwrap();
    let (p, at) = profile(&s, id);
    assert_eq!((at[0], p.gap), (20.0, 7.0));
    let mark = s.notice_mark();
    s.run("layout.update", &json!({"element": id, "thickness": 1000, "fill": null, "lineColor": "#1F5C84"})).unwrap();
    assert!(said(&s, mark).iter().any(|n| n.contains("thickness 1000 held to 300")), "{:?}", said(&s, mark));
    let (p, _) = profile(&s, id);
    assert_eq!((p.thickness, p.fill, p.line_color.hex()), (300.0, None, "#1F5C84".to_string()));
    let err = s.run("layout.update", &json!({"element": id, "gap": 2, "y": 9})).unwrap_err();
    assert!(err.contains("not both"), "{err}");
    let err = s.run("layout.update", &json!({"element": id, "rows": 3})).unwrap_err();
    assert!(err.contains("a profile takes no \"rows\""), "{err}");
    // Its section line and its layer renamed: it follows them.
    s.run("layout.update", &json!({"element": frame, "section": {"name": "a", "newName": "Serra"}})).unwrap();
    s.run("layer.rename", &json!({"layer": "DEM", "name": "Relevo"})).unwrap();
    let (p, _) = profile(&s, id);
    assert_eq!((p.section.as_str(), p.layer.as_str()), ("Serra", "Relevo"));
    // Its frame removed, it goes with it; undone, both are back.
    s.run("layout.removeElement", &json!({"element": frame})).unwrap();
    assert!(s.project.layouts[0].element(id as u32).is_none());
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.project.layouts[0].element(id as u32).is_some());
}

/// A profile its page can't draw says so when it is added and when the page is exported: beside a
/// tilted frame, one notice naming it and why.
#[test]
fn a_profile_not_drawn_is_said() {
    let (mut s, frame) = with_dem();
    s.run("layout.addElement", &json!({"kind": "section", "at": [10, 50], "direction": "vertical"})).unwrap();
    s.run("layout.update", &json!({"element": frame, "pitch": 40})).unwrap();
    let mark = s.notice_mark();
    let id = s.run("layout.addElement", &json!({"kind": "profile"})).unwrap()["element"].as_u64().unwrap();
    assert_eq!(profile(&s, id).0.side, ProfileSide::Right, "a vertical line's beside the frame");
    let notes = said(&s, mark);
    assert!(notes.len() == 1 && notes[0].contains("1 profile(s) not drawn") && notes[0].contains("tilted"), "{notes:?}");
    // Each way the page is drawn says it once: a PNG, a PDF, the layout view's picture.
    let drawn: Vec<(&str, Box<dyn Fn(&mut Session)>)> = vec![
        ("png", Box::new(|s| drop(crate::cmd::layout::layout_bytes(s, 0, false, 48.0, false).unwrap()))),
        ("pdf", Box::new(|s| drop(crate::cmd::layout::layout_bytes(s, 0, true, 48.0, true).unwrap()))),
        ("view", Box::new(|s| drop(crate::cmd::layout::layout_png(s, 0, 48.0, None).unwrap()))),
    ];
    let profile_notes = |s: &Session, mark: u64| said(s, mark).into_iter().filter(|n| n.contains("profile(s)")).collect::<Vec<_>>();
    for (how, export) in &drawn {
        let mark = s.notice_mark();
        export(&mut s);
        assert_eq!(profile_notes(&s, mark), notes, "{how}");
    }
    // Flat again, it is drawn, and nothing is said.
    s.run("layout.update", &json!({"element": frame, "pitch": 0})).unwrap();
    for (how, export) in &drawn {
        let mark = s.notice_mark();
        export(&mut s);
        assert!(profile_notes(&s, mark).is_empty(), "{how}: {:?}", said(&s, mark));
    }
}

/// The Mantiqueira of #2863: a vertical section line of a UTM 23S frame through the Pedra da Mina,
/// whose profile `profile.create` gave again within 0.11 m over 1,600 samples of the Copernicus DEM.
/// On a relief of that frame, the profile's samples and `profile.create`'s between the line's ends,
/// as many, agree as closely: a vertical line of a UTM frame strays little from the straight line
/// of longitude and latitude between its ends.
#[test]
fn the_mantiqueira_profile_agrees_with_profile_create() {
    let mut s = Session::new(sample::world_project());
    s.run("map.projection", &json!({"type": "EPSG:31983"})).unwrap();
    let crs = Crs::from_epsg(31983).unwrap();
    let to = crs.transformer().unwrap();
    let mid = to.from_lonlat(Coord::new(-44.715, -22.412)).unwrap();
    let (cols, rows, cell) = (1600usize, 1000usize, 30.0);
    let (x0, y0) = (((mid.x - 24_000.0) / cell).round() * cell, ((mid.y + 15_000.0) / cell).round() * cell);
    // Ridges and valleys a few kilometres across, some 2,000 m of relief, as the Mantiqueira's.
    let z = |e: f64, n: f64| 1500.0 + 600.0 * (e / 2_300.0).sin() * (n / 1_700.0).cos() + 300.0 * ((e + n) / 900.0).sin();
    let data = (0..rows * cols).map(|i| z(x0 + ((i % cols) as f64 + 0.5) * cell, y0 - ((i / cols) as f64 + 0.5) * cell) as f32).collect();
    let gt = GeoTransform { origin_x: x0, origin_y: y0, pixel_width: cell, pixel_height: -cell };
    let r = Raster::new("dem", cols as u32, rows as u32, vec![data], gt, crs, None, ColorHint::Gray, "Float32").unwrap();
    s.map_mut().add_raster("DEM", rustgis_map::RasterLayer::from_path(r, "dem.tif"));
    s.run("layout.new", &json!({"page": "a4", "orientation": "landscape", "template": "blank"})).unwrap();
    s.run("layout.addFrame", &json!({})).unwrap();
    let frame = u64::from(s.project.layouts[0].first_frame().unwrap());
    s.run(
        "layout.update",
        &json!({"element": frame, "x": 30, "y": 34, "w": 186, "h": 112, "center": [-44.715, -22.412], "scale": 200_000}),
    )
    .unwrap();
    s.run("layout.addElement", &json!({"kind": "section", "direction": "vertical", "at": [-44.8429, -22.4282]})).unwrap();
    let id = s.run("layout.addElement", &json!({"kind": "profile", "unit": "m"})).unwrap()["element"].as_u64().unwrap();
    let (p, _) = profile(&s, id);
    let (samples, _) = rustgis_render::layout::profile_samples(&s.project.layouts[0], &s.project.maps, &p).unwrap();
    // The line's ends on the ground, as the frame shows them.
    let map = &s.project.maps[0];
    let ElementKind::MapFrame { center, scale, .. } = &s.project.layouts[0].element(frame as u32).unwrap().kind else { panic!("a frame") };
    let cam = rustgis_map::layout::frame_camera(map, *center, *scale, 0.0, 0.0);
    let (fw, fh) = (186.0 * rustgis_map::layout::PT_PER_MM, 112.0 * rustgis_map::layout::PT_PER_MM);
    let (x, _) = cam.try_to_screen(map.to_map(Coord::new(-44.8429, -22.4282)).unwrap(), fw, fh).unwrap();
    let ends: Vec<[f64; 2]> =
        [0.0, fh].iter().map(|y| map.to_lonlat(cam.try_to_map(x, *y, fw, fh).unwrap()).unwrap()).map(|c| [c.x, c.y]).collect();
    let n = samples.values.len();
    assert!(n > 700, "a sample a cell: {n}");
    let got = s.run("profile.create", &json!({"layer": "DEM", "points": ends, "samples": n})).unwrap();
    let theirs: Vec<f64> = got["samples"].as_array().unwrap().iter().map(|r| r["elevation"].as_f64().unwrap()).collect();
    assert_eq!(theirs.len(), n);
    let off = samples.values.iter().zip(&theirs).map(|(a, b)| (a.unwrap() - b).abs()).fold(0.0, f64::max);
    assert!(off < 0.5, "{off} m apart");
    // Its length is the frame's height on the ground (112 mm at 1:200,000, the UTM scale factor
    // aside) on the ellipsoid; profile.create's, on the sphere, a few tenths of a percent longer.
    let (ours, theirs) = (*samples.metres.last().unwrap(), got["length"].as_f64().unwrap());
    assert!((ours / 22_400.0 - 1.0).abs() < 0.002, "{ours} m");
    assert!((theirs / ours - 1.0).abs() < 0.006, "{theirs} m against {ours} m");
}
