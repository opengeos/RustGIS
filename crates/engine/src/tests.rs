use serde_json::{Value, json};

use crate::{Project, Session, sample};

fn world() -> Session {
    let mut s = Session::new(Project::default());
    s.run("project.sample", &Value::Null).unwrap();
    s
}

#[test]
fn sample_project_loads() {
    let s = world();
    let names: Vec<&str> = s.map().layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Cities", "Rivers", "Lakes", "Countries"]);
    assert!(s.active_layer.is_some());
    assert!(!sample::world_project().map().bookmarks.is_empty());
}

#[test]
fn select_symbolize_undo_redo() {
    let mut s = world();
    let r = s.run("select.byAttributes", &json!({"layer": "Countries", "where": "continent = 'Africa'"})).unwrap();
    assert!(r["selected"].as_u64().unwrap() > 40);
    s.run("layer.symbology", &json!({"layer": "Countries", "type": "unique", "field": "continent"})).unwrap();
    assert_eq!(s.map().layer_by_name("Countries").unwrap().renderer.kind_label(), "Unique Values");
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.map().layer_by_name("Countries").unwrap().renderer.kind_label(), "Graduated Colors");
    s.run("edit.redo", &Value::Null).unwrap();
    assert_eq!(s.map().layer_by_name("Countries").unwrap().renderer.kind_label(), "Unique Values");
}

#[test]
fn geoprocessing_adds_layers_and_history() {
    let mut s = world();
    s.run("gp.run", &json!({"tool": "buffer", "params": {"input": "Cities", "distance": 200}})).unwrap();
    assert!(s.map().layer_by_name("Cities_Buffer").is_some());
    s.run("gp.run", &json!({"tool": "summaryStatistics", "params": {"input": "Countries", "field": "pop_est", "case": "continent"}}))
        .unwrap();
    assert_eq!(s.project.tables.len(), 1);
    assert!(s.run("gp.run", &json!({"tool": "buffer", "params": {"input": "Nope"}})).is_err());
    assert_eq!(s.project.gp_history.len(), 2, "a failed run restores the project");
}

#[test]
fn editing_features_and_fields() {
    let mut s = world();
    s.run(
        "edit.addFeature",
        &json!({"layer": "Cities", "geometry": {"type": "Point", "coordinates": [1.0, 2.0]}, "attributes": {"name": "Null Island"}}),
    )
    .unwrap();
    let n = s.map().layer_by_name("Cities").unwrap().dataset.features.len();
    assert!(
        s.run("edit.addFeature", &json!({"layer": "Cities", "geometry": {"type": "LineString", "coordinates": [[0,0],[1,1]]}})).is_err()
    );
    s.run("edit.addField", &json!({"layer": "Cities", "name": "score", "type": "double"})).unwrap();
    s.run("edit.calculateField", &json!({"layer": "Cities", "field": "score", "expression": "pop_max / 1000"})).unwrap();
    s.run("edit.delete", &Value::Null).unwrap();
    assert_eq!(s.map().layer_by_name("Cities").unwrap().dataset.features.len(), n - 1);
}

#[test]
fn project_save_and_open_roundtrip() {
    let mut s = world();
    let dir = std::env::temp_dir().join(format!("rustgis-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("atlas.rgisx");
    s.run("project.saveAs", &json!({"path": path.display().to_string()})).unwrap();
    let mut t = Session::new(Project::default());
    t.run("project.open", &json!({"path": path.display().to_string()})).unwrap();
    assert_eq!(t.map().layers.len(), 4);
    let png = dir.join("map.png");
    t.run("share.exportMap", &json!({"path": png.display().to_string(), "width": 320, "height": 200})).unwrap();
    assert!(std::fs::metadata(&png).unwrap().len() > 1000);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn save_clears_dirty_and_open_clears_history() {
    let mut s = world();
    assert!(!s.dirty && !s.can_undo(), "a fresh sample is clean");
    s.run("layer.color", &json!({"layer": "Countries", "color": "#336699"})).unwrap();
    assert!(s.dirty && s.can_undo());
    let dir = std::env::temp_dir().join(format!("rustgis-dirty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.rgisx");
    s.run("project.saveAs", &json!({"path": path.display().to_string()})).unwrap();
    assert!(!s.dirty, "saving clears the dirty marker");
    assert!(s.can_undo(), "saving keeps the edit history");
    s.run("project.open", &json!({"path": path.display().to_string()})).unwrap();
    assert!(!s.dirty && !s.can_undo() && !s.can_redo(), "opening starts a fresh history");
    assert!(s.run("edit.undo", &Value::Null).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn degenerate_geometry_and_deleted_fields() {
    let mut s = world();
    for g in [
        json!({"type": "LineString", "coordinates": [[0, 0]]}),
        json!({"type": "MultiPoint", "coordinates": []}),
        json!({"type": "MultiPolygon", "coordinates": []}),
    ] {
        let layer = if g["type"] == "LineString" {
            "Rivers"
        } else if g["type"] == "MultiPoint" {
            "Cities"
        } else {
            "Countries"
        };
        assert!(s.run("edit.addFeature", &json!({"layer": layer, "geometry": g})).is_err(), "{g}");
    }
    s.run("layer.definitionQuery", &json!({"layer": "Countries", "where": "pop_est > 1000"})).unwrap();
    s.run("edit.deleteField", &json!({"layer": "Countries", "field": "pop_est"})).unwrap();
    let l = s.map().layer_by_name("Countries").unwrap();
    assert_eq!(l.renderer.kind_label(), "Single Symbol");
    assert!(l.definition_query.is_none());
    s.run("edit.deleteField", &json!({"layer": "Countries", "field": "name"})).unwrap();
    assert!(!s.map().layer_by_name("Countries").unwrap().labels.enabled);
}

#[test]
fn open_from_text() {
    let mut s = world();
    let text = serde_json::to_string(&s.project).unwrap();
    s.run("layer.addDataText", &json!({"fileName": "pts.csv", "text": "name,lon,lat\nA,1,2\n"})).unwrap();
    assert!(s.map().layer_by_name("pts").is_some());
    assert!(s.run("layer.addDataText", &json!({"fileName": "x.shp", "text": ""})).is_err());
    assert!(s.run("layer.addDataText", &json!({"fileName": "x.geojson", "text": "{"})).is_err());
    assert!(s.run("project.openText", &json!({"text": "not json"})).is_err());
    s.run("project.openText", &json!({"text": text})).unwrap();
    assert!(s.map().layer_by_name("pts").is_none() && !s.dirty && !s.can_undo());
}

#[test]
fn catalog_ids_exist() {
    let s = Session::new(Project::default());
    for f in crate::catalog::FEATURES {
        if let Some(id) = f.command {
            assert!(id.starts_with("ui.") || s.registry.get(id).is_some(), "catalog names unknown command {id}");
        }
    }
    let (done, total) = crate::catalog::coverage();
    assert!(total > 250);
    // The floor only rises.
    assert!(done >= 68, "coverage fell to {done}");
}

#[test]
fn command_ids_are_unique() {
    let s = Session::new(Project::default());
    let mut ids: Vec<&str> = s.registry.commands.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    let n = ids.len();
    ids.dedup();
    assert_eq!(n, ids.len());
}

/// Every command with junk parameters returns Ok or Err, never panics, and leaves a usable session.
/// A 3-band 8×6 UTM 17N GeoTIFF near 81°W, 42°N, as base64.
fn utm_tiff_b64() -> String {
    use base64::Engine;
    use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster, geotiff};
    let gt = GeoTransform { origin_x: 497_000.0, origin_y: 4_651_000.0, pixel_width: 500.0, pixel_height: -500.0 };
    let bands = (0..3).map(|b| (0..48).map(|i| ((i * 5 + b * 40) % 256) as f32).collect()).collect();
    let r = Raster::new("utm", 8, 6, bands, gt, Crs::from_epsg(32617).unwrap(), None, ColorHint::Rgb { alpha: false }, "UInt8").unwrap();
    base64::engine::general_purpose::STANDARD.encode(geotiff::write(&r, 0).unwrap())
}

fn with_raster() -> Session {
    let mut s = world();
    s.run("layer.addDataBytes", &json!({"fileName": "scene.tif", "data": utm_tiff_b64()})).unwrap();
    s
}

#[test]
fn raster_layers_open_style_identify_and_round_trip() {
    let mut s = with_raster();
    let l = s.map().layers.last().unwrap();
    assert!(l.is_raster(), "rasters go under the vector layers");
    assert_eq!(l.name, "scene");
    assert_eq!(s.active_layer, Some(l.id));
    let e = l.extent();
    assert!((e.center().x + 81.0).abs() < 0.1 && (e.center().y - 42.0).abs() < 0.1, "{e:?}");
    // Rendering, stretches and enhancement.
    for p in [
        json!({"type": "stretch", "band": 2, "ramp": "Terrain"}),
        json!({"type": "classify", "classes": 4, "method": "quantile"}),
        json!({"type": "unique"}),
        json!({"type": "shadedRelief", "azimuth": 270}),
        json!({"type": "rgb"}),
    ] {
        s.run("raster.symbology", &p).unwrap_or_else(|e| panic!("{p}: {e}"));
    }
    assert!(s.run("raster.symbology", &json!({"type": "colormap"})).is_err());
    assert!(s.run("raster.symbology", &json!({"band": 9})).is_err());
    s.run("raster.bands", &json!({"red": 3, "green": 2, "blue": 1})).unwrap();
    assert!(s.run("raster.bands", &json!({"preset": "colorInfrared"})).is_err(), "no band 4");
    s.run("raster.stretch", &json!({"type": "standardDeviation", "n": 2.5})).unwrap();
    s.run("raster.dra", &json!({"on": true})).unwrap();
    s.run("raster.resampling", &json!({"type": "bilinear"})).unwrap();
    s.run("raster.enhance", &json!({"brightness": 20, "contrast": -10, "gamma": 1.4})).unwrap();
    s.run("layer.ramp", &json!({"ramp": "Aurora"})).unwrap();
    let st = s.map().layers.last().unwrap().raster.as_ref().unwrap().style.clone();
    assert_eq!(st.renderer, rustgis_map::RasterRenderer::Rgb { red: 2, green: 1, blue: 0 });
    assert!(st.dra && st.gamma == 1.4);
    // Feature-only commands are off for rasters.
    assert!(s.run("table.open", &Value::Null).is_err());
    // Identify reports features first (Lake Erie is here), then pixel values.
    let v = s.run("identify", &json!({"lon": -81.0, "lat": 42.0})).unwrap();
    assert_eq!(v["layer"], "Lakes", "{v}");
    for l in ["Cities", "Rivers", "Lakes", "Countries"] {
        s.run("layer.visibility", &json!({"layer": l, "visible": false})).unwrap();
    }
    let v = s.run("identify", &json!({"lon": -81.0, "lat": 42.0})).unwrap();
    assert_eq!(v["raster"], true, "{v}");
    assert!(v["attributes"]["Band 1"].is_string());
    let info = s.run("raster.info", &Value::Null).unwrap();
    assert_eq!(info["crs"], "EPSG:32617");
    assert_eq!(info["bands"].as_array().map(Vec::len), Some(3));
    // Export, then reopen from the file.
    let dir = std::env::temp_dir().join(format!("rustgis-raster-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tif = dir.join("out.tif");
    s.run("share.exportRaster", &json!({"path": tif.display().to_string()})).unwrap();
    s.run("layer.addData", &json!({"path": tif.display().to_string()})).unwrap();
    assert!(s.map().layer_by_name("out").is_some_and(|l| l.raster.as_ref().is_some_and(|r| r.path.is_some())));
    // Save and reload the project: the upload is embedded, the file is reopened by path.
    let proj = dir.join("p.rgisx");
    s.run("project.saveAs", &json!({"path": proj.display().to_string()})).unwrap();
    s.run("project.open", &json!({"path": proj.display().to_string()})).unwrap();
    for name in ["scene", "out"] {
        let l = s.map().layer_by_name(name).unwrap();
        assert!(l.raster.as_ref().unwrap().data.is_some(), "{name} reloaded");
    }
    assert!(s.map().layer_by_name("scene").unwrap().raster.as_ref().unwrap().style.dra, "style kept");
    // Undo after a style change restores the previous style.
    s.active_layer = s.map().layer_by_name("scene").map(|l| l.id);
    s.run("raster.dra", &json!({"on": false})).unwrap();
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.map().layer_by_name("scene").unwrap().raster.as_ref().unwrap().style.dra);
    let _ = std::fs::remove_dir_all(&dir);
    // The map renders with rasters.
    assert!(crate::cmd::share::map_png(&mut s, Some(320.0), Some(200.0), None).is_ok());
}

#[test]
fn hostile_params_never_panic() {
    let junk = [
        Value::Null,
        json!({}),
        json!({"layer": 99999, "path": "/nonexistent/x.geojson", "name": "", "where": "((", "tool": "buffer", "params": 7}),
        json!({"layer": "Countries", "row": 1e30, "field": "nope", "value": [1], "scale": -1, "lon": 1e300, "lat": "x", "classes": 1e9, "type": "graduated"}),
        json!({"layer": "Cities", "rows": [1, 99999999], "geometry": {"type": "Polygon", "coordinates": [[]]}, "width": 1e9, "height": -3, "expression": "'"}),
        json!({"band": -1, "red": 1e12, "green": "x", "blue": f64::MAX, "preset": "", "ramp": "nope", "classes": -5, "n": 1e308, "low": -50, "high": 900, "gamma": 0, "zFactor": -1, "fileName": "a.tif", "data": "!!!"}),
        json!({"fileName": "a.tif", "data": "AAAA", "type": "unique", "on": "maybe"}),
    ];
    let ids: Vec<&str> = Session::new(Project::default()).registry.commands.iter().map(|c| c.id).collect();
    for id in ids {
        if matches!(
            id,
            "project.save" | "project.saveAs" | "share.exportMap" | "share.exportLayer" | "share.exportTable" | "share.exportRaster"
        ) {
            continue;
        }
        for p in &junk {
            let mut s = world();
            let _ = s.run(id, p);
            let _ = s.run("info.project", &Value::Null);
            // Again with a raster layer active.
            let mut s = with_raster();
            let _ = s.run(id, p);
            let _ = s.run("info.project", &Value::Null);
        }
    }
}
