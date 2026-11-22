//! The project's coordinate system (#2032 C3a): saved only when set, with `project-crs`; new maps
//! drawn in it and new empty layers held in it; taken from the first layer added when asked.

use rustgis_data::raster::Crs;
use serde_json::{Value, json};

use crate::{Project, Session};

/// Swiss LV95, a projected system a map can be drawn in.
const LV95: Crs = Crs::Epsg(2056);

fn saved(s: &Session) -> Value {
    serde_json::to_value(&s.project).unwrap()
}

fn requires(v: &Value) -> Vec<String> {
    v.get("requires")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

#[test]
fn a_project_without_a_coordinate_system_saves_as_before() {
    let s = Session::new(Project::default());
    let v = saved(&s);
    assert!(v.get("coordinate_system").is_none() && v.get("crs_from_first_layer").is_none(), "{v}");
    assert!(!requires(&v).contains(&"project-crs".to_string()));
    let info = Session::new(Project::default()).run("info.project", &json!({})).unwrap();
    assert_eq!(info["coordinateSystem"], json!({"crs": null, "label": null, "mapsCanUse": null, "fromFirstLayer": false}));
}

#[test]
fn the_coordinate_system_is_set_saved_reopened_and_undone() {
    let mut s = Session::new(Project::default());
    let out = s.run("project.coordinateSystem", &json!({"crs": "2056"})).unwrap();
    assert_eq!(out, json!({"crs": "EPSG:2056", "label": "EPSG:2056", "mapsCanUse": true, "fromFirstLayer": false}));
    let v = saved(&s);
    assert_eq!(requires(&v), ["project-crs"]);
    assert!(v.get("crs_from_first_layer").is_none());
    let mut t = Session::new(Project::default());
    t.run("project.openText", &json!({"text": v.to_string()})).unwrap();
    assert_eq!(t.project.coordinate_system, Some(LV95));
    // The option alone needs the capability too: an older reader would save the project without it.
    s.run("project.coordinateSystem", &json!({"crs": null, "fromFirstLayer": true})).unwrap();
    assert_eq!((s.project.coordinate_system, s.project.crs_from_first_layer), (None, true));
    assert_eq!(requires(&saved(&s)), ["project-crs"]);
    // Omitted keys are kept; Undo takes each step back.
    s.run("project.coordinateSystem", &json!({"crs": "EPSG:32632"})).unwrap();
    assert_eq!((s.project.coordinate_system, s.project.crs_from_first_layer), (Some(Crs::Epsg(32632)), true));
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!((s.project.coordinate_system, s.project.crs_from_first_layer), (None, true));
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!((s.project.coordinate_system, s.project.crs_from_first_layer), (Some(LV95), false));
}

#[test]
fn the_command_refuses_what_it_cant_read() {
    let mut s = Session::new(Project::default());
    for p in
        [json!({}), json!({"crs": 7}), json!({"crs": "EPSG:999999999"}), json!({"crs": "not a system"}), json!({"fromFirstLayer": "yes"})]
    {
        assert!(s.run("project.coordinateSystem", &p).is_err(), "{p}");
    }
    assert_eq!((s.project.coordinate_system, s.project.crs_from_first_layer), (None, false));
    // An empty code clears it, as null does.
    s.run("project.coordinateSystem", &json!({"crs": "2056"})).unwrap();
    s.run("project.coordinateSystem", &json!({"crs": " "})).unwrap();
    assert_eq!(s.project.coordinate_system, None);
}

#[test]
fn a_new_map_is_drawn_in_the_project_system_where_the_active_map_looks() {
    let mut s = Session::new(Project::default());
    s.run("map.new", &json!({"name": "Before"})).unwrap();
    assert_eq!(s.map().projection, rustgis_map::Projection::WebMercator, "without one, as before");
    s.run("map.zoomToExtent", &json!({"extent": [6.0, 45.8, 10.5, 47.8]})).unwrap();
    let centre = s.map().center_lonlat();
    s.run("project.coordinateSystem", &json!({"crs": "EPSG:2056"})).unwrap();
    // The maps already there keep theirs.
    assert_eq!(s.map().projection, rustgis_map::Projection::WebMercator);
    s.run("map.new", &json!({"name": "Swiss"})).unwrap();
    assert_eq!(s.map().projection.code(), "EPSG:2056");
    let at = s.map().center_lonlat();
    assert!((at.x - centre.x).abs() < 1e-6 && (at.y - centre.y).abs() < 1e-6, "{at:?} vs {centre:?}");
}

#[test]
fn a_new_map_in_a_system_a_map_cant_use_says_why() {
    let mut s = Session::new(Project::default());
    // ETRS89, in degrees.
    s.run("project.coordinateSystem", &json!({"crs": "EPSG:4258"})).unwrap();
    assert_eq!(s.run("info.project", &json!({})).unwrap()["coordinateSystem"]["mapsCanUse"], json!(false));
    let mark = s.notice_mark();
    s.run("map.new", &json!({"name": "Degrees"})).unwrap();
    assert_eq!(s.map().projection, rustgis_map::Projection::WebMercator);
    let said: Vec<String> = s.notices_since(mark).iter().map(|n| n.text.clone()).collect();
    assert!(said.iter().any(|t| t.contains("Web Mercator") && t.contains("EPSG:4258")), "{said:?}");
}

#[test]
fn a_new_template_layer_is_held_in_the_project_system_and_edited_there() {
    let mut s = Session::new(Project::default());
    s.run("layer.template", &json!({"template": "points", "name": "Plain"})).unwrap();
    assert_eq!(s.map().layer_by_name("Plain").unwrap().dataset.coord_crs(), Crs::LonLat, "without one, as before");
    s.run("project.coordinateSystem", &json!({"crs": "EPSG:2056"})).unwrap();
    s.run("layer.template", &json!({"template": "points", "name": "Swiss"})).unwrap();
    let l = s.map().layer_by_name("Swiss").unwrap();
    assert_eq!((l.dataset.coord_crs(), l.dataset.crs), (LV95, Some(LV95)));
    // A point drawn at Bern's place is held in LV95 metres.
    s.map_mut().editing.settings.enabled = true;
    s.run("edit.addFeature", &json!({"layer": "Swiss", "geometry": {"type": "Point", "coordinates": [7.43863, 46.95108]}})).unwrap();
    let l = s.map().layer_by_name("Swiss").unwrap();
    let Some(rustgis_data::Geometry::Point(c)) = l.dataset.features.first().and_then(|f| f.geometry.clone()) else { panic!("no point") };
    assert!((c.x - 2_600_000.0).abs() < 5.0 && (c.y - 1_200_000.0).abs() < 5.0, "{c:?}");
    // Saved, it needs the native coordinates as any held layer does.
    let r = requires(&saved(&s));
    assert!(r.contains(&"native-coordinates".to_string()) && r.contains(&"project-crs".to_string()), "{r:?}");
}

/// A GeoPackage with a point at Bern in LV95, made in a project of that system.
fn lv95_file(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("rustgis-project-crs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bern.gpkg");
    let mut w = Session::new(Project::default());
    w.run("project.coordinateSystem", &json!({"crs": "EPSG:2056"})).unwrap();
    w.run("layer.template", &json!({"template": "points", "name": "Bern"})).unwrap();
    w.map_mut().editing.settings.enabled = true;
    w.run("edit.addFeature", &json!({"layer": "Bern", "geometry": {"type": "Point", "coordinates": [7.43863, 46.95108]}})).unwrap();
    w.run("share.exportLayer", &json!({"layer": "Bern", "path": path})).unwrap();
    (dir, path)
}

#[test]
fn the_first_layer_added_gives_its_system_when_asked() {
    let (dir, path) = lv95_file("first");
    // Off (the default): nothing is taken.
    let mut s = Session::new(Project::default());
    s.run("layer.addData", &json!({"path": path, "name": "Off"})).unwrap();
    assert_eq!(s.project.coordinate_system, None);
    // On: the next layer added with a system of its own gives it, said, and Undo takes both back.
    s.run("project.coordinateSystem", &json!({"fromFirstLayer": true})).unwrap();
    let mark = s.notice_mark();
    s.run("layer.addData", &json!({"path": path, "name": "On"})).unwrap();
    assert_eq!(s.project.coordinate_system, Some(LV95));
    assert!(s.notices_since(mark).iter().any(|n| n.text.contains("EPSG:2056") && n.text.contains("'On'")), "{:?}", s.notices);
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.project.coordinate_system, None);
    assert!(s.map().layer_by_name("On").is_none());
    // Redo brings back the layer and the system the step set.
    s.run("edit.redo", &Value::Null).unwrap();
    assert_eq!(s.project.coordinate_system, Some(LV95));
    // Once set, a later layer doesn't change it.
    s.run("project.coordinateSystem", &json!({"crs": "EPSG:32632"})).unwrap();
    s.run("layer.addData", &json!({"path": path, "name": "Later"})).unwrap();
    assert_eq!(s.project.coordinate_system, Some(Crs::Epsg(32632)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_layer_without_a_system_of_its_own_gives_none() {
    let mut s = Session::new(Project::default());
    s.run("project.coordinateSystem", &json!({"fromFirstLayer": true})).unwrap();
    // A template's layer is made in longitude/latitude with no system named.
    s.run("layer.template", &json!({"template": "lines"})).unwrap();
    assert_eq!(s.project.coordinate_system, None);
}
