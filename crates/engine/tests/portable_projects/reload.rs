use super::common::*;
use rustgis_engine::{
    Session,
    data::{
        Coord, Dataset, Field, FieldType, Geometry, GeometryType, Value, io,
        raster::{ColorHint, Crs, geotiff},
    },
    map::{RasterLayer, RasterRenderer, symbology::MovedLabel},
};
use serde_json::json;
use std::{path::Path, sync::Arc};

fn points(fields: &[&str], x: Option<f64>) -> Dataset {
    let mut ds = Dataset::new("Points", GeometryType::Point, fields.iter().map(|n| Field::new(*n, FieldType::Integer)).collect());
    if let Some(x) = x {
        ds.push(Some(Geometry::Point(Coord::new(x, 48.0))), fields.iter().map(|_| Value::Int(7)).collect());
    }
    ds
}

// Atomically publish a complete replacement rather than exposing a partially written source.
fn replace_points(path: &Path, ds: &Dataset) {
    let next = path.with_file_name("replacement.fgb");
    io::write_path(ds, &next).unwrap();
    std::fs::rename(next, path).unwrap();
}

fn loaded(s: &Session, id: u32) -> Dataset {
    s.map().layer(id).unwrap().load_all().unwrap()
}

fn first_point(ds: &Dataset) -> Coord {
    match ds.features[0].geometry.as_ref().unwrap() {
        Geometry::Point(at) => *at,
        other => panic!("expected fixture point, got {other:?}"),
    }
}

fn reject_reload(s: &mut Session, id: u32, expected: &str) {
    let source = s.map().layer(id).unwrap().source.clone();
    let before = serde_json::to_value(s.map().layer(id).unwrap()).unwrap();
    let mark = s.notice_mark();
    let error = s.run("layer.reloadData", &json!({"layer": id})).unwrap_err();
    assert!(error.to_lowercase().contains(&expected.to_lowercase()), "{error}");
    assert_eq!(serde_json::to_value(s.map().layer(id).unwrap()).unwrap(), before);
    if let Some(source) = source {
        assert!(s.map().layer(id).unwrap().source.as_ref().unwrap().same_opening(&source));
    }
    assert!(s.notices_since(mark).iter().any(|n| n.error && n.text.contains(&error)));
    assert!(s.notices_since(mark).iter().all(|n| n.error), "failed reload must not claim installation or lost moves");
}

#[test]
fn reload_reopens_fgb_preserving_presentation_and_resets_feature_state_with_undo() {
    let f = Fixture::new("reload-fgb");
    let path = f.file("points.fgb");
    io::write_path(&points(&["rank"], Some(2.0)), &path).unwrap();
    let mut s = session();
    let id = add_stream(&mut s, &path, None);
    s.map_mut().layer_mut(id).unwrap().sync_source(&rustgis_engine::geom::Extent::new(-180.0, -80.0, 180.0, 80.0), true);
    assert_eq!(s.map().layer(id).unwrap().source.as_ref().unwrap().reads, 1);
    let other = s.map_mut().add_dataset(points(&[], Some(9.0)));
    s.run("group.add", &json!({"name": "Survey", "layers": [id]})).unwrap();
    s.run("layer.rename", &json!({"layer": id, "name": "Survey points"})).unwrap();
    s.run("layer.symbology", &json!({"layer": id, "type": "unique", "field": "rank"})).unwrap();
    let fid = s.map().layer(id).unwrap().dataset.features[0].id;
    let layer = s.map_mut().layer_mut(id).unwrap();
    layer.visible = false;
    layer.transparency = 37.0;
    layer.min_scale = Some(500_000.0);
    layer.max_scale = Some(100.0);
    layer.labels.enabled = true;
    layer.labels.default.field = "rank".into();
    layer.labels.default.size = 17.0;
    layer.labels.default.moved.push(MovedLabel { id: fid, at: [12.0, 34.0] });
    layer.selection = vec![0];
    let renderer = serde_json::to_value(&layer.renderer).unwrap();
    let mut labels = layer.labels.clone();
    labels.default.moved.clear();
    s.map_mut().camera.res = 1234.0;
    s.map_mut().label_lock = Some(vec![((id, fid, 0, 0), rustgis_engine::map::LockedPlace { place: 0, callout: None })]);
    let camera = serde_json::to_value(s.map().camera).unwrap();
    let groups = serde_json::to_value(&s.map().groups).unwrap();
    let order = s.map().layers.iter().map(|l| l.id).collect::<Vec<_>>();
    s.active_layer = Some(id);
    assert!(order.contains(&other));
    let signature = std::fs::metadata(&path).unwrap();
    replace_points(&path, &points(&["rank"], Some(8.0)));
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(signature.modified().unwrap()))
        .unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), signature.len());
    let mark = s.notice_mark();
    let result = s.run("layer.reloadData", &json!({})).unwrap();
    assert_eq!(result, json!({"layer": id, "path": path, "table": null}));
    assert_eq!(first_point(&loaded(&s, id)), Coord::new(8.0, 48.0));
    let layer = s.map().layer(id).unwrap();
    assert_eq!(layer.name, "Survey points");
    assert!(!layer.visible);
    assert_eq!(layer.transparency, 37.0);
    assert_eq!((layer.min_scale, layer.max_scale), (Some(500_000.0), Some(100.0)));
    assert_eq!(serde_json::to_value(&layer.renderer).unwrap(), renderer);
    assert_eq!(serde_json::to_value(&layer.labels).unwrap(), serde_json::to_value(labels).unwrap());
    assert!(layer.selection.is_empty());
    assert!(s.map().label_lock.is_none());
    assert_eq!(serde_json::to_value(s.map().camera).unwrap(), camera);
    assert_eq!(serde_json::to_value(&s.map().groups).unwrap(), groups);
    assert_eq!(s.map().layers.iter().map(|l| l.id).collect::<Vec<_>>(), order);
    assert_eq!(s.active_layer, Some(id));
    let notices = s.notices_since(mark);
    assert!(notices.iter().any(|n| n.text.contains("1 saved label move")));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.map().layer(id).unwrap().labels.default.moved[0].at, [12.0, 34.0]);
    assert_eq!(first_point(&io::read_path(&path).unwrap().0), Coord::new(8.0, 48.0));
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(first_point(&loaded(&s, id)), Coord::new(8.0, 48.0));
    let mark = s.notice_mark();
    s.run("layer.reloadData", &json!({"layer": "Survey points"})).unwrap();
    assert!(s.notices_since(mark).iter().all(|n| !n.text.contains("reset")), "no feature state means no loss notice");
}

#[test]
fn compatible_empty_fgb_is_a_successful_reload() {
    let f = Fixture::new("reload-empty");
    let path = f.file("polygons.fgb");
    sample_file(&path, "Countries");
    let mut s = session();
    let id = add_stream(&mut s, &path, None);
    let fields = s.map().layer(id).unwrap().dataset.fields.clone();
    let field = fields[0].name.clone();
    s.run("layer.symbology", &json!({"layer": id, "type": "unique", "field": field})).unwrap();
    // The built-in point writer uses Unknown to allow Point/MultiPoint mixtures. Polygon
    // output carries a declaration even without rows, so this is genuinely compatible empty data.
    replace_points(&path, &Dataset::new("Empty", GeometryType::Polygon, fields));
    s.run("layer.reloadData", &json!({"layer": id})).unwrap();
    assert!(loaded(&s, id).features.is_empty());
    assert_eq!(s.map().layer(id).unwrap().dataset.geometry_type, GeometryType::Polygon);
    assert_eq!(s.map().layer(id).unwrap().renderer.field(), Some(field.as_str()));
}

#[test]
fn required_style_label_symbol_and_filter_fields_are_checked_before_reload_and_retry_works() {
    for binding in ["renderer", "label", "label expression", "symbol expression", "filter expression"] {
        let f = Fixture::new("reload-fields");
        let path = f.file("points.fgb");
        io::write_path(&points(&["required"], Some(2.0)), &path).unwrap();
        let mut s = session();
        let id = add_stream(&mut s, &path, None);
        if binding == "renderer" {
            s.run("layer.symbology", &json!({"layer": id, "type": "unique", "field": "required"})).unwrap();
        } else {
            let l = s.map_mut().layer_mut(id).unwrap();
            // Isolate each consumer: an incidental default label must not cause the rejection.
            l.labels.default.field.clear();
            match binding {
                "label" => {
                    l.labels.enabled = true;
                    l.labels.default.field = "required".into();
                }
                "label expression" => {
                    l.labels.enabled = true;
                    l.labels.default.by.text = Some("required + 1".into());
                }
                "symbol expression" => l.symbol_by.size = Some("required * 2".into()),
                "filter expression" => l.definition_query = Some("required > 0".into()),
                _ => unreachable!(),
            }
        }
        let name = s.map().layer(id).unwrap().name.clone();
        s.run("layer.rename", &json!({"layer": id, "name": "Before failed reload"})).unwrap();
        replace_points(&path, &points(&[], Some(8.0)));
        reject_reload(&mut s, id, "required");
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(s.map().layer(id).unwrap().name, name, "a failed reload must not add an undo step: {binding}");
        replace_points(&path, &points(&["required"], Some(8.0)));
        s.run("layer.reloadData", &json!({"layer": id})).unwrap();
        assert_eq!(first_point(&loaded(&s, id)), Coord::new(8.0, 48.0));
    }
}

#[test]
fn wrong_geometry_missing_file_and_missing_chosen_table_leave_the_layer_unchanged() {
    let f = Fixture::new("reload-source-errors");
    let path = f.file("points.fgb");
    io::write_path(&points(&[], Some(2.0)), &path).unwrap();
    let mut s = session();
    let id = add_stream(&mut s, &path, None);
    let next = f.file("replacement.fgb");
    sample_file(&next, "Countries");
    std::fs::rename(&next, &path).unwrap();
    reject_reload(&mut s, id, "Polygon");
    std::fs::remove_file(&path).unwrap();
    reject_reload(&mut s, id, "file");
    replace_points(&path, &points(&[], Some(8.0)));
    s.run("layer.reloadData", &json!({"layer": id})).unwrap();
    let gpkg = f.file("tables.gpkg");
    gpkg_file(&gpkg, &["a", "b"]);
    let id = add_stream(&mut s, &gpkg, Some("b"));
    let c = rusqlite::Connection::open(&gpkg).unwrap();
    c.execute_batch(
        "DROP TABLE b; DELETE FROM gpkg_contents WHERE table_name = 'b'; DELETE FROM gpkg_geometry_columns WHERE table_name = 'b';",
    )
    .unwrap();
    reject_reload(&mut s, id, "b");
    assert_eq!(s.map().layer(id).unwrap().linked_table(), Some("b"));
}

#[test]
fn editable_gpkg_reload_observes_wal_refuses_every_dirty_kind_and_discards_only_in_memory() {
    for kind in ["insert", "update", "delete"] {
        let f = Fixture::new("reload-gpkg-edits");
        let path = f.file("tables.gpkg");
        gpkg_file(&path, &["a", "b"]);
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            UPDATE gpkg_contents SET last_change = '2026-01-01T00:00:00.000Z';
            ALTER TABLE b ADD COLUMN name TEXT; UPDATE b SET name = 'original';
            INSERT INTO b (geom, name) SELECT geom, 'second' FROM b WHERE fid = 1;",
        )
        .unwrap();
        let mut s = session();
        let id = s.run("gpkg.addLayer", &json!({"path": path, "table": "b"})).unwrap()["layer"].as_u64().unwrap() as u32;
        match kind {
            "insert" => {
                s.run("edit.addFeature", &json!({"layer": id, "geometry": {"type": "Point", "coordinates": [3.0, 49.0]}, "attributes": {"name": "local insert"}})).unwrap();
            }
            "update" => {
                s.run("edit.setValue", &json!({"layer": id, "row": 0, "field": "name", "value": "local update"})).unwrap();
            }
            "delete" => {
                s.run("select.rows", &json!({"layer": id, "rows": [1]})).unwrap();
                s.run("edit.delete", &json!({})).unwrap();
            }
            _ => unreachable!(),
        }
        let dirty = s.map().layer(id).unwrap().dataset.clone();
        let pending = s.run("gpkg.pendingEdits", &json!({"layer": id})).unwrap();
        assert_eq!(pending[format!("{kind}s")], 1);
        c.execute_batch(
            "UPDATE b SET name = 'external' WHERE fid = 1;
            UPDATE gpkg_contents SET last_change = '2026-01-02T00:00:00.000Z' WHERE table_name = 'b';",
        )
        .unwrap();
        reject_reload(&mut s, id, "discardEdits");
        assert_eq!(s.map().layer(id).unwrap().dataset, dirty);
        let result = s.run("layer.reloadData", &json!({"layer": id, "discardEdits": true})).unwrap();
        assert_eq!(result, json!({"layer": id, "path": path, "table": "b"}));
        let fresh = &s.map().layer(id).unwrap().dataset;
        assert_eq!(fresh.features.len(), 2);
        assert_eq!(fresh.value(0, fresh.field_index("name").unwrap()), &Value::Text("external".into()));
        let pending = s.run("gpkg.pendingEdits", &json!({"layer": id})).unwrap();
        assert_eq!((pending["inserts"].as_u64(), pending["updates"].as_u64(), pending["deletes"].as_u64()), (Some(0), Some(0), Some(0)));
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(s.map().layer(id).unwrap().dataset, dirty);
        assert_eq!(s.run("gpkg.pendingEdits", &json!({"layer": id})).unwrap()[format!("{kind}s")], 1);
        let disk: String = c.query_row("SELECT name FROM b WHERE fid = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(disk, "external", "reload and undo must not write the GeoPackage");
        let count: i64 = c.query_row("SELECT count(*) FROM b", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 2);
        s.run("edit.redo", &json!({})).unwrap();
        s.run("edit.setValue", &json!({"layer": id, "row": 0, "field": "name", "value": "saved after reload"})).unwrap();
        s.run("gpkg.saveEdits", &json!({"layer": id})).unwrap();
        let disk: String = c.query_row("SELECT name FROM b WHERE fid = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(disk, "saved after reload", "saving must use the refreshed optimistic baseline");
        let untouched: i64 = c.query_row("SELECT count(*) FROM a", [], |r| r.get(0)).unwrap();
        assert_eq!(untouched, 1, "the selected table must not drift to the first table");
    }
}

#[test]
fn raster_reload_refreshes_pixels_preserves_style_and_rejects_missing_bands() {
    let f = Fixture::new("reload-raster");
    let path = f.file("rgb.tif");
    raster_file(&path, 3);
    let mut s = session();
    s.run("layer.addData", &json!({"path": path})).unwrap();
    let id = s.active_layer.unwrap();
    let raster = s.map_mut().layer_mut(id).unwrap().raster.as_mut().unwrap();
    raster.style.renderer = RasterRenderer::Rgb { red: 2, green: 0, blue: 1 };
    raster.style.gamma = 1.8;
    raster.style.brightness = 17.0;
    raster.style.mask.values = vec![2.0];
    let style = serde_json::to_value(&raster.style).unwrap();
    let mut changed = rustgis_engine::data::raster::read_path(&path).unwrap();
    changed.bands[2].data[0] = 99.0;
    std::fs::write(&path, geotiff::write(&changed, 0).unwrap()).unwrap();
    s.run("layer.reloadData", &json!({"layer": id})).unwrap();
    let raster = s.map().layer(id).unwrap().raster.as_ref().unwrap();
    assert_eq!(raster.data.as_ref().unwrap().value(2, 0, 0), Some(99.0));
    assert_eq!(serde_json::to_value(&raster.style).unwrap(), style);
    let data = Arc::clone(raster.data.as_ref().unwrap());
    raster_file(&path, 1);
    reject_reload(&mut s, id, "band");
    assert!(Arc::ptr_eq(s.map().layer(id).unwrap().raster.as_ref().unwrap().data.as_ref().unwrap(), &data));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.map().layer(id).unwrap().raster.as_ref().unwrap().data.as_ref().unwrap().value(2, 0, 0), Some(40.0));
}

#[test]
fn raster_reload_requires_the_palette_used_by_a_saved_colormap() {
    let f = Fixture::new("reload-palette");
    let path = f.file("palette.tif");
    raster_file(&path, 1);
    let mut s = session();
    s.run("layer.addData", &json!({"path": path})).unwrap();
    let id = s.active_layer.unwrap();
    // The writer does not export palettes; retain a decoded palette on a real linked raster.
    let mut pixels = rustgis_engine::data::raster::read_path(&path).unwrap();
    pixels.color = ColorHint::Palette(vec![[0, 0, 0, 255], [30, 120, 240, 255]]);
    let raster = RasterLayer::from_path(pixels, path.to_str().unwrap());
    assert!(matches!(raster.style.renderer, RasterRenderer::Colormap));
    let data = Arc::clone(raster.data.as_ref().unwrap());
    s.map_mut().layer_mut(id).unwrap().raster = Some(raster);
    reject_reload(&mut s, id, "palette");
    assert!(Arc::ptr_eq(s.map().layer(id).unwrap().raster.as_ref().unwrap().data.as_ref().unwrap(), &data));
}

#[test]
fn detached_snapshot_and_remote_source_cannot_be_reloaded_as_local_files() {
    let f = Fixture::new("reload-unsupported");
    let path = f.file("points.fgb");
    io::write_path(&points(&[], Some(2.0)), &path).unwrap();
    let mut s = session();
    let id = add_stream(&mut s, &path, None);
    s.run("layer.loadAll", &json!({"layer": id})).unwrap();
    assert!(s.map().layer(id).unwrap().source.is_none());
    reject_reload(&mut s, id, "local");
    let id = add_stream(&mut s, &path, None);
    // Supply remote provenance without issuing a network request to construct the fixture.
    s.map_mut().layer_mut(id).unwrap().source.as_mut().unwrap().path = Some("https://example.invalid/points.fgb".into());
    reject_reload(&mut s, id, "local");
}

#[test]
fn shapefile_reload_reads_changed_dbf_and_prj_sidecars() {
    rustgis_engine::data::source::shp::use_temporary_index_dir();
    let f = Fixture::new("reload-shapefile");
    let path = f.file("points.shp");
    let original = points(&["rank"], Some(2.0));
    for (ext, bytes) in io::shapefile_parts_in(&original, Crs::WebMercator).unwrap().0 {
        std::fs::write(path.with_extension(ext), bytes).unwrap();
    }
    let mut s = session();
    let id = add_stream(&mut s, &path, None);
    assert!(first_point(&loaded(&s, id)).x.abs() < 0.001);
    let mut replacement = original;
    replacement.features[0].attrs[0] = Value::Int(12345);
    // Leave .shp untouched: both changed observations must come from sidecars.
    for (ext, bytes) in io::shapefile_parts_in(&replacement, Crs::LonLat).unwrap().0 {
        if matches!(ext, "dbf" | "prj") {
            std::fs::write(path.with_extension(ext), bytes).unwrap();
        }
    }
    s.run("layer.reloadData", &json!({"layer": id})).unwrap();
    let ds = loaded(&s, id);
    assert_eq!(first_point(&ds), Coord::new(2.0, 48.0));
    assert_eq!(ds.value(0, ds.field_index("rank").unwrap()).as_f64(), Some(12345.0));
}

#[test]
fn reload_reconverts_original_gdal_folder_and_accepts_its_declared_empty_table() {
    use rustgis_engine::{
        data::source::gdal,
        map::source::{GdalOrigin, SourceLayer},
    };
    if !gdal::available() {
        return;
    }
    let f = Fixture::new("reload-gdal-folder");
    let folder = f.file("survey.gdb");
    let input = f.file("survey.geojson");
    let publish = |x: Option<f64>| {
        io::write_path(&points(&["rank"], x), &input).unwrap();
        let output = std::process::Command::new(gdal::program("ogr2ogr").unwrap())
            .args(["-f", "OpenFileGDB", "-overwrite", "-nln", "survey", "-nlt", "POINT"])
            .arg(&folder)
            .arg(&input)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    };
    publish(Some(2.0));
    let original = gdal::convert_one(&folder, "survey", None, None).unwrap().unwrap();
    let handle = rustgis_engine::data::source::open_path(&original, None).unwrap();
    let mut source = SourceLayer::from_path(original.to_str().unwrap(), None, handle);
    source.gdal = Some(GdalOrigin { path: folder.to_str().unwrap().into(), layer: "survey".into(), family: None, crs: None });
    let mut s = session();
    let id = s.map_mut().add_streamed("Folder survey", source);
    publish(Some(8.0));
    s.run("layer.reloadData", &json!({"layer": id})).unwrap();
    assert_eq!(first_point(&loaded(&s, id)), Coord::new(8.0, 48.0));
    assert_ne!(s.map().layer(id).unwrap().source.as_ref().unwrap().path.as_deref(), original.to_str());
    assert_eq!(s.map().layer(id).unwrap().linked_file(), folder.to_str());
    assert_eq!(s.map().layers.len(), 1);
    publish(None);
    s.run("layer.reloadData", &json!({"layer": id})).unwrap();
    assert!(loaded(&s, id).features.is_empty());
    assert_eq!(s.map().layer(id).unwrap().dataset.geometry_type, GeometryType::Point);
}

#[test]
fn reload_reconverts_duckdb_table_and_keeps_only_its_original_geometry_branch() {
    use rustgis_engine::data::source::duckdb;
    if !duckdb::available() {
        return;
    }
    let f = Fixture::new("reload-duckdb");
    let path = f.file("survey.duckdb");
    let sql = |statement: &str| {
        let output = std::process::Command::new(duckdb::program().unwrap())
            .arg(&path)
            .arg("-c")
            .arg(format!("LOAD spatial; {statement}"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    };
    sql("CREATE TABLE survey AS SELECT 7 AS rank, ST_Point(2,48) AS geom");
    let mut s = session();
    s.run("layer.addDatabase", &json!({"database": path, "table": "survey", "stream": true})).unwrap();
    let id = s.active_layer.unwrap();
    assert_eq!(first_point(&loaded(&s, id)), Coord::new(2.0, 48.0));
    sql("UPDATE survey SET geom=ST_Point(8,48); INSERT INTO survey VALUES(9,ST_GeomFromText('LINESTRING(0 0,1 1)'))");
    s.run("layer.reloadData", &json!({"layer": id})).unwrap();
    let fresh = loaded(&s, id);
    assert_eq!(first_point(&fresh), Coord::new(8.0, 48.0));
    assert_eq!(fresh.features.len(), 1);
    assert_eq!(s.map().layers.len(), 1);
    sql("DELETE FROM survey");
    s.run("layer.reloadData", &json!({"layer": id})).unwrap();
    assert!(loaded(&s, id).features.is_empty());
    sql("DROP TABLE survey");
    reject_reload(&mut s, id, "survey");
}
