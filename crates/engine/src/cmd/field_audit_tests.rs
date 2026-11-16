//! Field capture round-trips (an audit of mobile offline capture and sync). The `#[ignore]`d tests
//! reproduce known bugs and fail today; run them with `--ignored`. The others pass.
//!
//! Field capture round-trips as a crew would run them: packages sent to phones, edited offline,
//! and brought back to the home project, sometimes by more than one phone, sometimes more than once.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::Session;

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("rustgis-audit-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn home(temp: &Temp) -> Session {
    let mut s = Session::new(crate::sample::world_project());
    s.path = Some(temp.0.join("home.rgis"));
    s
}

/// Opens the package on a "phone": unpacked into a folder of its own, as `project.open` does.
fn phone(temp: &Temp, bytes: &[u8], name: &str) -> Session {
    let dir = temp.0.join(name);
    let text = unpack_into(bytes, &dir).unwrap();
    let mut s = Session::new(project_in(&text, &dir).unwrap());
    s.opened_package = Some(temp.0.join(format!("{name}.rgisfield")));
    s
}

/// Sends the phone's map back and brings it back at home, returning the result and notices.
fn bring_back(temp: &Temp, field: &mut Session, home: &mut Session, name: &str) -> (Value, Vec<String>) {
    let (_, back) = package(field, &Value::Null).unwrap();
    let path = temp.0.join(format!("{name}-back.rgisfield"));
    std::fs::write(&path, &back).unwrap();
    let mark = home.notice_mark();
    let done = home.run("offline.bringBack", &json!({"path": path.display().to_string()})).unwrap();
    let said = home.notices_since(mark).iter().map(|n| n.text.clone()).collect();
    (done, said)
}

fn layer(s: &Session, name: &str) -> u32 {
    s.map().layers.iter().find(|l| l.name == name).map(|l| l.id).unwrap()
}

fn text_field(s: &Session, id: u32) -> String {
    s.map().layer(id).unwrap().dataset.fields.iter().find(|f| f.ty == rustgis_data::FieldType::Text).unwrap().name.clone()
}

fn value_of(s: &Session, id: u32, feature: u64, field: &str) -> Option<String> {
    let l = s.map().layer(id)?;
    let row = l.dataset.features.iter().position(|f| f.id == feature)?;
    Some(l.dataset.value(row, l.dataset.field_index(field)?).to_string())
}

fn count(s: &Session, id: u32) -> usize {
    s.map().layer(id).unwrap().dataset.features.len()
}

fn add_point(s: &mut Session, id: u32, field: &str, label: &str, at: [f64; 2]) -> u64 {
    s.run("edit.addFeature", &json!({"layer": id, "geometry": {"type": "Point", "coordinates": at}, "attributes": {field: label}}))
        .unwrap();
    s.map().layer(id).unwrap().dataset.features.last().unwrap().id
}

fn with_label(s: &Session, id: u32, field: &str, label: &str) -> usize {
    let l = s.map().layer(id).unwrap();
    let fi = l.dataset.field_index(field).unwrap();
    (0..l.dataset.features.len()).filter(|r| l.dataset.value(*r, fi).to_string() == label).count()
}

const AREA: [f64; 4] = [-20.0, -40.0, 60.0, 40.0];

/// One package, copied to two phones (emailed to a crew, AirDropped to a second device).
/// Phone A fixes a city's name; phone B never touches it. B is brought back after A.
#[test]
#[ignore = "repro of a known field-sync bug (mobile offline audit); run with --ignored"]
fn audit_second_phone_with_same_package_reverts_first_phones_edit() {
    let temp = Temp::new("two-phones-edit");
    let mut home = home(&temp);
    let (_, out) = package(&mut home, &json!({"name": "Crew", "extent": AREA})).unwrap();
    let mut a = phone(&temp, &out, "a");
    let mut b = phone(&temp, &out, "b");
    let cities = layer(&a, "Cities");
    let name = text_field(&a, cities);
    let fixed = a.map().layer(cities).unwrap().dataset.features[0].id;
    a.run("edit.setValue", &json!({"layer": cities, "row": 0, "field": name, "value": "Fixed by A"})).unwrap();
    // B edits a different city.
    b.run("edit.setValue", &json!({"layer": cities, "row": 1, "field": name, "value": "Fixed by B"})).unwrap();
    bring_back(&temp, &mut a, &mut home, "a");
    let hc = layer(&home, "Cities");
    assert_eq!(value_of(&home, hc, fixed, &name).as_deref(), Some("Fixed by A"));
    let (done, said) = bring_back(&temp, &mut b, &mut home, "b");
    eprintln!("B: {done}\nnotices: {said:#?}");
    assert_eq!(
        value_of(&home, hc, fixed, &name).as_deref(),
        Some("Fixed by A"),
        "A's edit was reverted by B's untouched copy; notices: {said:?}"
    );
}

/// The same two phones each capture a new point. A is brought back, then B.
#[test]
#[ignore = "repro of a known field-sync bug (mobile offline audit); run with --ignored"]
fn audit_second_phone_with_same_package_loses_first_phones_new_features() {
    let temp = Temp::new("two-phones-add");
    let mut home = home(&temp);
    let (_, out) = package(&mut home, &json!({"name": "Crew", "extent": AREA})).unwrap();
    let mut a = phone(&temp, &out, "a");
    let mut b = phone(&temp, &out, "b");
    let cities = layer(&a, "Cities");
    let name = text_field(&a, cities);
    let ida = add_point(&mut a, cities, &name, "Well from A", [10.0, 5.0]);
    let idb = add_point(&mut b, cities, &name, "Well from B", [11.0, 6.0]);
    eprintln!("phone ids: A {ida}, B {idb}");
    bring_back(&temp, &mut a, &mut home, "a");
    let hc = layer(&home, "Cities");
    let (done, said) = bring_back(&temp, &mut b, &mut home, "b");
    eprintln!("B: {done}\nnotices: {said:#?}");
    let a_kept = with_label(&home, hc, &name, "Well from A");
    let b_kept = with_label(&home, hc, &name, "Well from B");
    assert_eq!((a_kept, b_kept), (1, 1), "both crews' new wells should be at home; notices: {said:?}");
}

/// Phone B captures nothing new at all; it only brings back after A did.
#[test]
#[ignore = "repro of a known field-sync bug (mobile offline audit); run with --ignored"]
fn audit_idle_second_phone_deletes_first_phones_new_features() {
    let temp = Temp::new("two-phones-idle");
    let mut home = home(&temp);
    let (_, out) = package(&mut home, &json!({"name": "Crew", "extent": AREA})).unwrap();
    let mut a = phone(&temp, &out, "a");
    let mut b = phone(&temp, &out, "b");
    let cities = layer(&a, "Cities");
    let name = text_field(&a, cities);
    add_point(&mut a, cities, &name, "Well from A", [10.0, 5.0]);
    let row1 = 1;
    b.run("edit.setValue", &json!({"layer": cities, "row": row1, "field": name, "value": "Touched by B"})).unwrap();
    bring_back(&temp, &mut a, &mut home, "a");
    let hc = layer(&home, "Cities");
    let (done, said) = bring_back(&temp, &mut b, &mut home, "b");
    eprintln!("B: {done}\nnotices: {said:#?}");
    assert_eq!(with_label(&home, hc, &name, "Well from A"), 1, "A's new well was deleted by B; notices: {said:?}");
}

/// One phone, two trips: the crew brings back at lunch, keeps working on the same phone, brings
/// back again at the end of the day. In the morning they captured a point past the packaged extent.
#[test]
#[ignore = "repro of a known field-sync bug (mobile offline audit); run with --ignored"]
fn audit_incremental_return_duplicates_features_captured_outside_the_extent() {
    let temp = Temp::new("outside");
    let mut home = home(&temp);
    let (_, out) = package(&mut home, &json!({"name": "Day", "extent": AREA})).unwrap();
    let mut a = phone(&temp, &out, "a");
    let cities = layer(&a, "Cities");
    let name = text_field(&a, cities);
    // Walked a little past the edge of the area.
    add_point(&mut a, cities, &name, "Past the edge", [65.0, 5.0]);
    let (first, _) = bring_back(&temp, &mut a, &mut home, "lunch");
    eprintln!("lunch: {first}");
    // Afternoon: one more point, well inside.
    add_point(&mut a, cities, &name, "Afternoon", [10.0, 5.0]);
    let (second, said) = bring_back(&temp, &mut a, &mut home, "evening");
    eprintln!("evening: {second}\nnotices: {said:#?}");
    let hc = layer(&home, "Cities");
    assert_eq!(with_label(&home, hc, &name, "Past the edge"), 1, "captured once, at home twice; notices: {said:?}");
}

/// The same, for an existing feature the crew moved out of the extent (a mislocated asset corrected).
#[test]
#[ignore = "repro of a known field-sync bug (mobile offline audit); run with --ignored"]
fn audit_incremental_return_duplicates_features_moved_outside_the_extent() {
    let temp = Temp::new("moved-out");
    let mut home = home(&temp);
    let (_, out) = package(&mut home, &json!({"name": "Day", "extent": AREA})).unwrap();
    let mut a = phone(&temp, &out, "a");
    let cities = layer(&a, "Cities");
    let name = text_field(&a, cities);
    let moved = a.map().layer(cities).unwrap().dataset.features[0].id;
    a.run("edit.setValue", &json!({"layer": cities, "row": 0, "field": name, "value": "Relocated"})).unwrap();
    a.map_mut().layer_mut(cities).unwrap().dataset.features[0].geometry =
        Some(rustgis_data::Geometry::Point(rustgis_geom::Coord { x: 70.0, y: 5.0 }));
    a.map_mut().layer_mut(cities).unwrap().touch_geometry();
    bring_back(&temp, &mut a, &mut home, "lunch");
    add_point(&mut a, cities, &name, "Afternoon", [10.0, 5.0]);
    let (second, said) = bring_back(&temp, &mut a, &mut home, "evening");
    eprintln!("evening: {second}\nnotices: {said:#?}");
    let hc = layer(&home, "Cities");
    let _ = moved;
    assert_eq!(with_label(&home, hc, &name, "Relocated"), 1, "moved once, at home twice; notices: {said:?}");
}

/// Two crews, each with their own package of overlapping areas (the documented way), edit the
/// same city differently. The second one back wins; is the first crew's value said?
#[test]
fn audit_two_packages_same_feature_last_back_wins_and_is_said() {
    let temp = Temp::new("two-packages");
    let mut home = home(&temp);
    let (_, out_a) = package(&mut home, &json!({"name": "North", "extent": AREA})).unwrap();
    let (_, out_b) = package(&mut home, &json!({"name": "South", "extent": AREA})).unwrap();
    let mut a = phone(&temp, &out_a, "a");
    let mut b = phone(&temp, &out_b, "b");
    let cities = layer(&a, "Cities");
    let name = text_field(&a, cities);
    let id = a.map().layer(cities).unwrap().dataset.features[0].id;
    a.run("edit.setValue", &json!({"layer": cities, "row": 0, "field": name, "value": "A says"})).unwrap();
    b.run("edit.setValue", &json!({"layer": cities, "row": 0, "field": name, "value": "B says"})).unwrap();
    let ida = add_point(&mut a, cities, &name, "New A", [10.0, 5.0]);
    let idb = add_point(&mut b, cities, &name, "New B", [11.0, 6.0]);
    eprintln!("phone ids: A {ida}, B {idb}");
    bring_back(&temp, &mut a, &mut home, "a");
    let (done, said) = bring_back(&temp, &mut b, &mut home, "b");
    eprintln!("B: {done}\nnotices: {said:#?}");
    let hc = layer(&home, "Cities");
    eprintln!("home value: {:?}", value_of(&home, hc, id, &name));
    assert_eq!((with_label(&home, hc, &name, "New A"), with_label(&home, hc, &name, "New B")), (1, 1));
    assert!(said.iter().any(|t| t.contains("Cities #")), "the overwritten edit should be named: {said:?}");
}

/// The office adds a column to the layer while the crew is out. The crew's edits come back.
#[test]
#[ignore = "repro of a known field-sync bug (mobile offline audit); run with --ignored"]
fn audit_office_adds_a_field_while_crew_is_out() {
    let temp = Temp::new("schema");
    let mut home = home(&temp);
    let (_, out) = package(&mut home, &json!({"name": "Day", "extent": AREA})).unwrap();
    let mut a = phone(&temp, &out, "a");
    let cities = layer(&a, "Cities");
    let name = text_field(&a, cities);
    add_point(&mut a, cities, &name, "Captured", [10.0, 5.0]);
    let hc = layer(&home, "Cities");
    home.run("edit.addField", &json!({"layer": hc, "name": "inspected", "type": "text"})).unwrap();
    let (_, back) = package(&mut a, &Value::Null).unwrap();
    let path = temp.0.join("back.rgisfield");
    std::fs::write(&path, &back).unwrap();
    let r = home.run("offline.bringBack", &json!({"path": path.display().to_string()}));
    eprintln!("bring back after office schema change: {r:?}");
    assert!(r.is_ok(), "the whole day's edits were refused: {r:?}");
}

/// The crew deletes a point on the phone that the office edited meanwhile, and the office
/// deletes a point the crew edited.
#[test]
fn audit_delete_vs_edit_both_ways() {
    let temp = Temp::new("delete-edit");
    let mut home = home(&temp);
    let (_, out) = package(&mut home, &json!({"name": "Day", "extent": AREA})).unwrap();
    let mut a = phone(&temp, &out, "a");
    let cities = layer(&a, "Cities");
    let name = text_field(&a, cities);
    let f0 = a.map().layer(cities).unwrap().dataset.features[0].id;
    let f1 = a.map().layer(cities).unwrap().dataset.features[1].id;
    // Phone: delete f0, edit f1.
    a.run("edit.setValue", &json!({"layer": cities, "row": 1, "field": name, "value": "Crew edit"})).unwrap();
    a.run("select.rows", &json!({"layer": cities, "rows": [0]})).unwrap();
    a.run("edit.delete", &Value::Null).unwrap();
    // Office: edit f0, delete f1.
    let hc = layer(&home, "Cities");
    let r0 = home.map().layer(hc).unwrap().dataset.features.iter().position(|f| f.id == f0).unwrap();
    home.run("edit.setValue", &json!({"layer": hc, "row": r0, "field": name, "value": "Office edit"})).unwrap();
    let r1 = home.map().layer(hc).unwrap().dataset.features.iter().position(|f| f.id == f1).unwrap();
    home.run("select.rows", &json!({"layer": hc, "rows": [r1]})).unwrap();
    home.run("edit.delete", &Value::Null).unwrap();
    let (done, said) = bring_back(&temp, &mut a, &mut home, "a");
    eprintln!("{done}\nnotices: {said:#?}");
    eprintln!("f0 at home: {:?}; f1 at home: {:?}", value_of(&home, hc, f0, &name), value_of(&home, hc, f1, &name));
}

#[allow(dead_code)]
fn _unused(_: &Path) {}

#[cfg(not(target_arch = "wasm32"))]
fn roads_gpkg(dir: &Path) -> PathBuf {
    let path = dir.join("roads.gpkg");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE gpkg_spatial_ref_sys (srs_name TEXT, srs_id INTEGER PRIMARY KEY, organization TEXT,
             organization_coordsys_id INTEGER, definition TEXT, description TEXT);
         INSERT INTO gpkg_spatial_ref_sys VALUES ('WGS 84', 4326, 'EPSG', 4326, '', '');
         CREATE TABLE gpkg_contents (table_name TEXT PRIMARY KEY, data_type TEXT, identifier TEXT,
             description TEXT, last_change TEXT, min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE, srs_id INTEGER);
         INSERT INTO gpkg_contents (table_name, data_type, srs_id, last_change) VALUES ('roads', 'features', 4326, '2000-01-01T00:00:00.000Z');
         CREATE TABLE gpkg_geometry_columns (table_name TEXT, column_name TEXT, geometry_type_name TEXT,
             srs_id INTEGER, z TINYINT, m TINYINT);
         INSERT INTO gpkg_geometry_columns VALUES ('roads', 'geom', 'POINT', 4326, 0, 0);
         CREATE TABLE roads (fid INTEGER PRIMARY KEY, geom BLOB, name TEXT, lanes INTEGER DEFAULT 2);",
    )
    .unwrap();
    let blob = |x: f64, y: f64| {
        let mut b = b"GP\x00\x03".to_vec();
        b.extend_from_slice(&4326i32.to_le_bytes());
        for o in [x, x, y, y] {
            b.extend_from_slice(&o.to_le_bytes());
        }
        b.push(1);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
        b
    };
    // Ten points, x = 1..=10: a packaged area of x 1..3 holds three.
    for k in 1..=10i64 {
        db.execute("INSERT INTO roads (fid, geom, name) VALUES (?1, ?2, ?3)", rusqlite::params![k, blob(k as f64, 20.0), format!("r{k}")])
            .unwrap();
    }
    path
}

/// A GeoPackage layer loaded for editing goes to the field. On the field copy (here opened on the
/// same computer, so its absolute path still names the file; a laptop crew, or a phone with the
/// file pushed to the same path) the crew edits one road and presses Save to GeoPackage.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "repro of a known field-sync bug (mobile offline audit); run with --ignored"]
fn audit_field_copy_of_a_geopackage_layer_saves_deletions_of_everything_outside_the_area() {
    let temp = Temp::new("gpkg");
    let gpkg = roads_gpkg(&temp.0);
    let mut home = Session::new(crate::project::Project::default());
    home.path = Some(temp.0.join("home.rgis"));
    home.run("gpkg.addLayer", &json!({"path": gpkg, "table": "roads", "name": "R"})).unwrap();
    let (_, out) = package(&mut home, &json!({"name": "Roads", "extent": [0.5, 19.0, 3.5, 21.0]})).unwrap();
    let mut a = phone(&temp, &out, "a");
    let r = layer(&a, "R");
    eprintln!("field copy has {} roads; gpkg binding kept: {}", count(&a, r), a.map().layer(r).unwrap().gpkg.is_some());
    a.map_mut().editing.settings.enabled = true;
    a.run("edit.setValue", &json!({"layer": r, "row": 0, "field": "name", "value": "fixed"})).unwrap();
    let saved = a.run("gpkg.saveEdits", &json!({"layer": "R"}));
    eprintln!("save from field copy: {saved:?}");
    let db = rusqlite::Connection::open(&gpkg).unwrap();
    let left: i64 = db.query_row("SELECT count(*) FROM roads", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 10, "the GeoPackage lost {} of its 10 roads", 10 - left);
}

/// No phone at all: Download Map, Open Offline Map, edit, Save to GeoPackage, on the desktop.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "repro of a known field-sync bug (mobile offline audit); run with --ignored"]
fn audit_offline_map_on_desktop_saves_deletions_of_everything_outside_the_area() {
    let temp = Temp::new("gpkg-desktop");
    let gpkg = roads_gpkg(&temp.0);
    let mut s = Session::new(crate::project::Project::default());
    s.run("gpkg.addLayer", &json!({"path": gpkg, "table": "roads", "name": "R"})).unwrap();
    s.run("offline.download", &json!({"name": "Site", "extent": [0.5, 19.0, 3.5, 21.0]})).unwrap();
    s.run("offline.open", &json!({"name": "Site"})).unwrap();
    eprintln!("active map: {}", s.map().name);
    s.map_mut().editing.settings.enabled = true;
    let r = layer(&s, "R");
    s.run("edit.setValue", &json!({"layer": r, "row": 0, "field": "name", "value": "fixed"})).unwrap();
    let saved = s.run("gpkg.saveEdits", &json!({"layer": "R"}));
    eprintln!("save from offline map: {saved:?}");
    let db = rusqlite::Connection::open(&gpkg).unwrap();
    let left: i64 = db.query_row("SELECT count(*) FROM roads", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 10, "the GeoPackage lost {} of its 10 roads", 10 - left);
}
