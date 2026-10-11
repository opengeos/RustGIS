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
    assert!(done >= 133, "coverage fell to {done}");
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
fn globe_projection_toggles_renders_and_saves() {
    let mut s = with_raster();
    assert_eq!(s.run("map.projection", &Value::Null).unwrap()["projection"], "globe");
    assert_eq!(s.map().projection, rustgis_map::Projection::Globe);
    // The globe draws vectors, rasters and labels, from any side.
    for (lon, lat, res) in [(-81.0, 42.0, 30_000.0), (100.0, -60.0, 60_000.0), (-81.0, 42.0, 50.0), (0.0, 89.0, 160_000.0)] {
        s.map_mut().camera = rustgis_map::Camera { center: rustgis_geom::lonlat_to_merc(rustgis_geom::Coord::new(lon, lat)), res };
        let png = crate::cmd::share::map_png(&mut s, Some(400.0), Some(300.0), None).unwrap();
        assert!(png.len() > 1000);
    }
    assert!(s.run("map.projection", &json!({"type": "nope"})).is_err());
    s.run("map.projection", &json!({"type": "mercator"})).unwrap();
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.map().projection, rustgis_map::Projection::Globe, "undo restores the globe");
    let json = serde_json::to_string(&s.project).unwrap();
    assert!(Project::from_json(&json).unwrap().map().projection == rustgis_map::Projection::Globe);
}

#[test]
fn custom_projections_switch_keep_scale_query_and_render() {
    let mut s = with_raster();
    s.run("map.goToXY", &json!({"lon": 10.0, "lat": 50.0, "scale": 5_000_000})).unwrap();
    let before = s.scale();
    let r = s.run("map.projection", &json!({"type": "europeEqualArea"})).unwrap();
    assert_eq!(r["projection"], "europeEqualArea");
    assert!((s.scale() / before - 1.0).abs() < 0.02, "scale kept: {before} vs {}", s.scale());
    let c = s.map().center_lonlat();
    assert!((c.x - 10.0).abs() < 1e-6 && (c.y - 50.0).abs() < 1e-6, "{c:?}");
    // Identify and box select work in the projection's coordinates.
    let v = s.run("identify", &json!({"lon": 2.35, "lat": 48.85, "tolerancePx": 5})).unwrap();
    assert!(!v.is_null(), "something is under Paris");
    let sel = s.run("select.byRect", &json!({"minLon": 5.0, "minLat": 45.0, "maxLon": 15.0, "maxLat": 55.0})).unwrap();
    assert!(sel["selected"].as_u64().unwrap() > 0);
    assert!(crate::cmd::share::map_png(&mut s, Some(400.0), Some(300.0), None).unwrap().len() > 1000);
    // An EPSG code; the raster (UTM 17N) in the map's own system draws without reprojection.
    s.run("map.projection", &json!({"type": "EPSG:32617"})).unwrap();
    s.run("layer.zoomTo", &json!({"layer": "scene"})).unwrap();
    assert!(crate::cmd::share::map_png(&mut s, Some(400.0), Some(300.0), None).unwrap().len() > 1000);
    assert!(s.run("map.projection", &json!({"type": "EPSG:4326"})).is_err(), "degrees are refused");
    assert!(s.run("map.projection", &json!({"type": "nope"})).is_err());
    assert!(s.run("map.goToXY", &json!({"lon": 100.0, "lat": 0.0})).is_err(), "folded far side of the UTM zone");
    // Undo and save/reload keep the projection.
    s.run("map.projection", &json!({"type": "mollweide"})).unwrap();
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.map().projection.code(), "EPSG:32617");
    let json = serde_json::to_string(&s.project).unwrap();
    assert_eq!(Project::from_json(&json).unwrap().map().projection.code(), "EPSG:32617");
    assert!(s.run("map.projections", &Value::Null).unwrap().as_array().unwrap().len() >= 10);
    // A project naming a projection this version can't use opens in Web Mercator.
    let broken = json.replace("EPSG:32617", "EPSG:9");
    let reopened = Project::from_json(&broken).unwrap();
    assert_eq!(reopened.map().projection, rustgis_map::Projection::WebMercator);
    assert_eq!(reopened.map().camera, rustgis_map::Camera::world(), "UTM metres aren't read as Mercator ones");
    // Bookmarks follow a projection switch, and the extent history doesn't replay old cameras.
    s.run("map.projection", &json!({"type": "mercator"})).unwrap();
    s.run("map.goToXY", &json!({"lon": 10.0, "lat": 50.0, "scale": 5_000_000})).unwrap();
    s.run("bookmark.add", &json!({"name": "Europe"})).unwrap();
    s.run("map.goToXY", &json!({"lon": -100.0, "lat": 40.0})).unwrap();
    assert!(s.can_extent_back());
    s.run("map.projection", &json!({"type": "europeEqualArea"})).unwrap();
    assert!(!s.can_extent_back(), "history from Web Mercator is dropped");
    s.run("bookmark.go", &json!({"name": "Europe"})).unwrap();
    let c = s.map().center_lonlat();
    assert!((c.x - 10.0).abs() < 1e-6 && (c.y - 50.0).abs() < 1e-6, "{c:?}");
    assert!((s.scale() / 5_000_000.0 - 1.0).abs() < 0.02, "{}", s.scale());
    // An export at another size measures its scale bar with its own camera.
    let wide = s.map().ground_res_at(&rustgis_map::Camera { res: s.map().camera.res * 3.0, ..s.map().camera });
    assert!((wide / s.map().ground_res() - 3.0).abs() < 1e-9);
    // The built-in world projections (and Equal Earth by its EPSG code) draw the whole world.
    for p in ["robinson", "equalEarth", "naturalEarth", "winkelTripel", "eckertIV", "EPSG:8857"] {
        s.run("map.projection", &json!({"type": p})).unwrap_or_else(|e| panic!("{p}: {e}"));
        s.run("map.zoomFull", &Value::Null).unwrap();
        let c = s.map().center_lonlat();
        assert!(c.x.abs() < 1.0 && c.y.abs() < 30.0, "{p}: {c:?}");
        let v = s.run("identify", &json!({"lon": 2.35, "lat": 48.85, "tolerancePx": 5})).unwrap();
        assert!(!v.is_null(), "{p}: something is under Paris");
        assert!(crate::cmd::share::map_png(&mut s, Some(400.0), Some(300.0), None).unwrap().len() > 1000, "{p}");
    }
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
    s.run("raster.symbology", &json!({"type": "rgb"})).unwrap(); // keeps the combination
    assert!(s.run("raster.bands", &json!({"preset": "colorInfrared"})).is_err(), "no band 4");
    s.run("raster.stretch", &json!({"type": "standardDeviation", "n": 2.5})).unwrap();
    s.run("raster.dra", &json!({"on": true})).unwrap();
    s.run("raster.resampling", &json!({"type": "bilinear"})).unwrap();
    s.run("raster.enhance", &json!({"brightness": 20, "contrast": -10, "gamma": 1.4})).unwrap();
    s.run("layer.ramp", &json!({"ramp": "Aurora"})).unwrap();
    let st = s.map().layers.last().unwrap().raster.as_ref().unwrap().style.clone();
    assert_eq!(st.renderer, rustgis_map::RasterRenderer::Rgb { red: 2, green: 1, blue: 0 });
    assert!(st.dra && st.gamma == 1.4);
    // Feature-only commands are off for rasters, and layer conditions follow a named layer.
    assert!(s.run("table.open", &Value::Null).is_err());
    assert!(s.run("labels.enable", &json!({"layer": "scene"})).is_err());
    s.run("layer.symbology", &json!({"layer": "Countries", "type": "single"})).unwrap();
    s.active_layer = s.map().layer_by_name("Countries").map(|l| l.id);
    s.run("raster.info", &json!({"layer": "scene"})).unwrap();
    s.active_layer = s.map().layer_by_name("scene").map(|l| l.id);
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

/// Every command with junk parameters returns Ok or Err, never panics, and leaves a usable session.
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
            "project.save"
                | "project.saveAs"
                | "share.exportMap"
                | "share.exportLayer"
                | "share.exportTable"
                | "share.exportRaster"
                // Downloads the runner from the network.
                | "gp.installGeolibre"
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

#[cfg(feature = "geolibre")]
#[test]
fn geolibre_tools_run_on_layers_and_add_their_outputs() {
    let mut s = with_raster();
    let st = s.run("gp.geolibre", &Value::Null).unwrap();
    assert_eq!(st["supported"], true);
    assert!(s.run("gp.setGeolibre", &json!({"path": "/no/such/geolibre"})).is_err(), "not a runner");
    let undo = s.undo_label();
    s.run("gp.setGeolibre", &json!({"path": ""})).unwrap();
    assert_eq!(s.undo_label(), undo, "choosing a runner isn't an edit");
    if st["available"] != true {
        eprintln!("skipped: no GeoLibre runner (set RUSTGIS_GEOLIBRE)");
        return;
    }
    let tools = s.run("gp.tools", &Value::Null).unwrap();
    assert!(tools.as_array().unwrap().iter().any(|t| t["id"] == "geolibre:slope"));
    let before = s.map().layers.len();
    let r = s.run("gp.run", &json!({"tool": "geolibre:slope", "params": {"input": "scene"}})).unwrap();
    assert_eq!(r["layers"].as_array().map(Vec::len), Some(1), "{r}");
    assert_eq!(s.map().layers.len(), before + 1);
    let slope = s.map().layer_by_name("Slope").unwrap();
    assert!(slope.is_raster() && slope.raster.as_ref().is_some_and(|r| r.embedded.is_some()), "kept in the project");
    // It saves with the project, and undo takes it away.
    let json = serde_json::to_string(&s.project).unwrap();
    assert!(
        Project::from_json(&json)
            .unwrap()
            .map()
            .layer_by_name("Slope")
            .is_some_and(|l| l.raster.as_ref().is_some_and(|r| r.data.is_some()))
    );
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.map().layer_by_name("Slope").is_none());
    // A vector where a raster belongs is an error, not a crash.
    let err = s.run("gp.run", &json!({"tool": "geolibre:slope", "params": {"input": "Cities"}})).unwrap_err();
    assert!(err.starts_with("Slope:"), "{err}");
}

/// The browser's way: prepare a run, let a runner (here, pretend) produce the files, then
/// apply it with `gp.finish`, which keeps undo and history.
#[cfg(feature = "geolibre")]
#[test]
fn geolibre_runs_can_finish_asynchronously() {
    use crate::cmd::gp::{external_complete, external_start};
    let mut s = with_raster();
    assert!(s.run("gp.finish", &json!({"id": 99})).is_err(), "nothing pending");
    assert!(external_complete(&mut s, 99, Ok((vec![], vec![]))).is_err(), "nothing started");
    if s.run("gp.geolibre", &Value::Null).unwrap()["available"] != true {
        eprintln!("skipped: no GeoLibre runner (set RUSTGIS_GEOLIBRE)");
        return;
    }
    assert!(external_start(&mut s, "geolibre:slope", &json!({})).unwrap_err().contains("required"));
    assert!(external_start(&mut s, "buffer", &json!({})).is_err(), "only GeoLibre tools");
    let run = external_start(&mut s, "geolibre:slope", &json!({"input": "scene"})).unwrap();
    assert_eq!((run.tool.as_str(), run.files.len()), ("slope", 1), "the embedded raster is staged");
    assert!(run.argv.iter().any(|a| a.starts_with("--output=/work/")));
    // The "runner" writes the input back as the output.
    let out = run.files[0].1.clone();
    external_complete(&mut s, run.id, Ok((vec![("output.tif".into(), out)], vec![]))).unwrap();
    let before = s.map().layers.len();
    let r = s.run("gp.finish", &json!({"id": run.id})).unwrap();
    assert_eq!(r["layers"].as_array().map(Vec::len), Some(1), "{r}");
    assert_eq!(s.map().layers.len(), before + 1);
    assert!(s.project.gp_history.last().is_some_and(|h| h.ok && h.tool == "Slope"));
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.map().layers.len(), before, "undo removes it");
    assert!(s.run("gp.finish", &json!({"id": run.id})).is_err(), "applied once");
    // A runner error is reported like a failed gp.run.
    let run = external_start(&mut s, "geolibre:slope", &json!({"input": "scene"})).unwrap();
    external_complete(&mut s, run.id, Err("trap".into())).unwrap();
    assert_eq!(s.run("gp.finish", &json!({"id": run.id})).unwrap_err(), "Slope: trap");
    // A run of a project that was closed meanwhile doesn't land in the new one.
    let run = external_start(&mut s, "geolibre:slope", &json!({"input": "scene"})).unwrap();
    s.run("project.new", &Value::Null).unwrap();
    assert!(external_complete(&mut s, run.id, Ok((vec![], vec![]))).unwrap_err().contains("project was closed"));
}

#[test]
fn clipboard_copies_cuts_and_pastes_features() {
    let mut s = world();
    assert!(s.run("edit.copy", &Value::Null).is_err(), "nothing selected");
    s.run("select.byAttributes", &json!({"layer": "Cities", "where": "name = 'Paris'"})).unwrap();
    let r = s.run("edit.copy", &Value::Null).unwrap();
    assert_eq!(r["copied"], 1);
    let text = r["text"].as_str().unwrap().to_string();
    assert!(text.contains("Paris"));
    // Into the same layer, then as a new layer, then from GeoJSON text.
    let n = s.map().layer_by_name("Cities").unwrap().dataset.features.len();
    s.run("edit.paste", &json!({"layer": "Cities"})).unwrap();
    assert_eq!(s.map().layer_by_name("Cities").unwrap().dataset.features.len(), n + 1);
    assert!(s.run("edit.paste", &json!({"layer": "Countries"})).is_err(), "points don't go in polygons");
    let r = s.run("edit.pasteSpecial", &json!({"name": "Copied"})).unwrap();
    assert_eq!(r["pasted"], 1);
    assert_eq!(s.map().layer_by_name("Copied").unwrap().dataset.features.len(), 1);
    s.run("edit.pasteSpecial", &json!({"text": text, "name": "FromText"})).unwrap();
    assert!(s.map().layer_by_name("FromText").is_some());
    assert!(s.run("edit.paste", &json!({"layer": "Cities", "text": "not json"})).is_err());
    // Cut removes the features.
    s.run("select.byAttributes", &json!({"layer": "Cities", "where": "name = 'Paris'"})).unwrap();
    let cut = s.run("edit.cut", &Value::Null).unwrap();
    assert_eq!(cut["copied"], 2);
    assert_eq!(s.map().layer_by_name("Cities").unwrap().dataset.features.len(), n - 1);
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.map().layer_by_name("Cities").unwrap().dataset.features.len(), n + 1);
    // The sample layers live in the project, not in files.
    assert!(s.run("layer.copyPath", &json!({"layer": "Cities"})).is_err());
}

#[test]
fn popup_modes_choose_what_identify_reports() {
    let mut s = world();
    let at = json!({"lon": 2.35, "lat": 48.85, "tolerancePx": 5});
    let top = s.run("identify", &at).unwrap();
    assert_eq!(top["layer"], "Cities");
    s.run("map.popupMode", &json!({"mode": "visible"})).unwrap();
    let all = s.run("identify", &at).unwrap();
    let layers: Vec<&str> = all["results"].as_array().unwrap().iter().map(|r| r["layer"].as_str().unwrap()).collect();
    assert_eq!(layers, ["Cities", "Countries"]);
    s.run("map.popupMode", &json!({"mode": "selectedInContents"})).unwrap();
    s.run("layer.select", &json!({"layer": "Countries"})).unwrap();
    assert_eq!(s.run("identify", &at).unwrap()["layer"], "Countries");
    s.run("map.popupMode", &json!({"mode": "none"})).unwrap();
    assert!(s.run("identify", &at).unwrap().is_null());
    assert!(s.run("map.popupMode", &json!({"mode": "bogus"})).is_err());
    // The mode is saved with the project.
    let text = serde_json::to_string(&s.project).unwrap();
    assert!(text.contains("\"popup_mode\":\"None\""));
}

#[test]
fn label_range_placement_lock_and_convert() {
    use crate::render::View;
    let mut s = world();
    s.run("labels.enable", &json!({"layer": "Countries", "on": true})).unwrap();
    s.run("map.goToXY", &json!({"lon": 10.0, "lat": 50.0, "scale": 20_000_000})).unwrap();
    let view = |s: &Session| View { width: 800.0, height: 600.0, pixels_per_point: 1.0, camera: s.map().camera, dark: false };
    let count = |s: &mut Session| {
        let v = view(s);
        crate::render::render(s.map_mut(), &v).unwrap().labels.len()
    };
    let placed = count(&mut s);
    assert!(placed > 5);
    // Out beyond 1:5,000,000: no labels at 1:20,000,000.
    s.run("labels.scaleRange", &json!({"layer": "Countries", "min": 5_000_000})).unwrap();
    let n = count(&mut s);
    assert!(n < placed, "{n} < {placed}");
    s.run("labels.scaleRange", &json!({"layer": "Countries", "clear": true})).unwrap();
    assert_eq!(count(&mut s), placed);
    s.run("labels.distanceRange", &json!({"layer": "Countries", "farthest": 1000})).unwrap();
    assert!(count(&mut s) < placed);
    s.run("labels.distanceRange", &json!({"layer": "Countries", "farthest": null})).unwrap();
    assert!(s.run("labels.scaleRange", &json!({"layer": "Countries", "min": -4})).is_err());
    // Pause hides every label; unplaced labels are reported when asked for.
    s.run("labels.pause", &json!({"on": true})).unwrap();
    assert_eq!(count(&mut s), 0);
    s.run("labels.pause", &json!({"on": false})).unwrap();
    s.run("labels.viewUnplaced", &json!({"on": true})).unwrap();
    let v = view(&s);
    let r = crate::render::render(s.map_mut(), &v).unwrap();
    assert!(!r.unplaced.is_empty() && r.unplaced.iter().all(|l| l.color == crate::render::UNPLACED));
    s.run("labels.viewUnplaced", &json!({"on": false})).unwrap();
    // Placement moves labels.
    s.run("labels.placement", &json!({"layer": "Cities", "position": "right"})).unwrap();
    assert_eq!(s.map().layer_by_name("Cities").unwrap().labels.placement, crate::map::LabelPlacement::Right);
    assert!(s.run("labels.placement", &json!({"position": "sideways"})).is_err());
    // Lock keeps the labelled set as the map moves.
    s.run("labels.lock", &json!({"on": true})).unwrap();
    let locked = count(&mut s);
    assert!(s.map().label_lock.is_some());
    s.run("map.zoomOut", &Value::Null).unwrap();
    assert!(count(&mut s) <= locked);
    s.run("labels.lock", &json!({"on": false})).unwrap();
    count(&mut s);
    assert!(s.map().label_lock.is_none());
    // Convert makes an annotation layer and turns the source's labels off.
    let r = s.run("labels.convert", &json!({"layer": "Countries"})).unwrap();
    assert!(r["annotations"].as_u64().unwrap() > 100);
    let a = s.map().layer_by_name("Countries Annotation").unwrap();
    assert!(a.labels.enabled && a.labels.field == "TEXT");
    assert!(!s.map().layer_by_name("Countries").unwrap().labels.enabled);
}

#[test]
fn graphics_draw_shapes_text_and_pictures() {
    use crate::render::View;
    use base64::Engine;
    let mut s = world();
    s.run("map.goToXY", &json!({"lon": 10.0, "lat": 50.0, "scale": 20_000_000})).unwrap();
    // The first graphic makes a graphics layer.
    let r = s.run("graphics.add", &json!({"shape": "rectangle", "points": [[0, 45], [10, 55]], "fill": "#FF000040"})).unwrap();
    let id = r["layer"].as_u64().unwrap() as u32;
    assert!(s.map().layer(id).unwrap().graphics);
    for (shape, pts) in [
        ("circle", json!([[5, 50], [8, 50]])),
        ("ellipse", json!([[0, 40], [20, 48]])),
        ("line", json!([[0, 40], [5, 45], [10, 40]])),
        ("curve", json!([[0, 40], [5, 45], [10, 40]])),
        ("freehand", json!([[0, 40], [1, 41], [2, 40]])),
        ("polygon", json!([[0, 40], [5, 45], [10, 40]])),
        ("lasso", json!([[0, 40], [5, 45], [10, 40], [5, 38]])),
        ("point", json!([[2, 48]])),
    ] {
        s.run("graphics.add", &json!({"shape": shape, "points": pts})).unwrap_or_else(|e| panic!("{shape}: {e}"));
    }
    assert!(s.run("graphics.add", &json!({"shape": "polygon", "points": [[0, 0], [1, 1]]})).is_err());
    assert!(s.run("graphics.add", &json!({"shape": "rectangle", "points": [[0, 0], [0, 1]]})).is_err());
    assert!(s.run("graphics.add", &json!({"shape": "text", "points": [[0, 0]]})).is_err(), "text needs text");
    assert!(s.run("graphics.add", &json!({"shape": "blob", "points": [[0, 0]]})).is_err());
    assert!(s.run("graphics.add", &json!({"shape": "point", "points": [[0, 0]], "layer": "Cities"})).is_err(), "not a graphics layer");
    // Dynamic text expands when drawn.
    s.run("graphics.add", &json!({"shape": "text", "points": [[10, 50]], "text": "{map} {scale}", "fontSize": 20})).unwrap();
    let view = View { width: 800.0, height: 600.0, pixels_per_point: 1.0, camera: s.map().camera, dark: false };
    let r = crate::render::render(s.map_mut(), &view).unwrap();
    assert!(
        r.labels.iter().any(|l| l.text.starts_with("World 1:") && l.size == 20.0),
        "{:?}",
        r.labels.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
    // A picture: a 2×2 PNG.
    let mut png = tiny_skia::Pixmap::new(2, 2).unwrap();
    png.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
    let b64 = base64::engine::general_purpose::STANDARD.encode(png.encode_png().unwrap());
    s.run("graphics.add", &json!({"shape": "picture", "points": [[0, 45], [10, 55]], "image": b64})).unwrap();
    assert!(s.run("graphics.add", &json!({"shape": "picture", "points": [[0, 45], [10, 55]], "image": "bm90IGFuIGltYWdl"})).is_err());
    let v = View { width: 800.0, height: 600.0, pixels_per_point: 1.0, camera: s.map().camera, dark: false };
    let r = crate::render::render(s.map_mut(), &v).unwrap();
    let (x, y) = v.to_screen(rustgis_geom::lonlat_to_merc(rustgis_geom::Coord::new(5.0, 50.0)));
    let px = r.pixmap.pixel(x as u32, y as u32).unwrap();
    assert!(px.red() > 200 && px.green() < 60, "the picture covers its rectangle: {px:?}");
    // Restyle the selection; keep the tool.
    s.run("select.rows", &json!({"layer": id, "rows": [0]})).unwrap();
    s.run("graphics.style", &json!({"fill": "#00FF00", "width": 3})).unwrap();
    assert_eq!(rustgis_map::graphics::symbol(&s.map().layer(id).unwrap().dataset, 0).fill, rustgis_map::Rgba::rgb(0, 255, 0));
    assert!(s.run("graphics.style", &json!({})).is_err(), "nothing to change");
    s.run("graphics.style", &json!({"text": "not on a rectangle"})).unwrap();
    assert!(rustgis_map::graphics::label(&s.map().layer(id).unwrap().dataset, 0).is_none(), "text only goes on text graphics");
    assert_eq!(rustgis_render::picture_size(&b64), Some((2, 2)));
    assert!(s.run("graphics.keepTool", &Value::Null).unwrap()["keepTool"].as_bool().unwrap());
    s.run("tool.set", &json!({"tool": "graphicRectangle"})).unwrap();
    assert_eq!(s.tool.code(), "graphicRectangle");
    // Graphics round-trip through a project file.
    let text = serde_json::to_string(&s.project).unwrap();
    let p: crate::Project = serde_json::from_str(&text).unwrap();
    assert!(p.map().layer(id).unwrap().graphics);
    s.run("graphics.newLayer", &json!({"name": "Notes"})).unwrap();
    assert!(s.map().layer_by_name("Notes").unwrap().graphics);
}

#[test]
fn layouts_compose_export_and_round_trip() {
    use base64::Engine;
    let dir = std::env::temp_dir().join(format!("rustgis-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = world();
    s.run("map.goToXY", &json!({"lon": 10.0, "lat": 50.0, "scale": 20_000_000})).unwrap();
    let r = s.run("layout.new", &json!({"name": "Europe", "page": "a4", "orientation": "landscape"})).unwrap();
    assert_eq!(r["layout"], 0);
    assert_eq!(s.active_layout, Some(0));
    let l = &s.project.layouts[0];
    assert!((l.width - 297.0).abs() < 1e-9 && l.elements.len() == 6, "the map template adds a frame and five surrounds");
    let frame = l.first_frame().unwrap();
    // Frames, surrounds and the rest.
    for shape in ["circle", "ellipse", "polygon", "lasso"] {
        s.run("layout.addFrame", &json!({"shape": shape, "points": [[0, 0], [1, 0], [0.5, 1]]})).unwrap();
    }
    assert!(s.run("layout.addFrame", &json!({"shape": "polygon", "points": [[0, 0], [1, 0]]})).is_err());
    let mut png = tiny_skia::Pixmap::new(4, 4).unwrap();
    png.fill(tiny_skia::Color::from_rgba8(0, 128, 255, 255));
    let b64 = base64::engine::general_purpose::STANDARD.encode(png.encode_png().unwrap());
    for p in [
        json!({"kind": "table", "layer": "Countries", "rows": 5}),
        json!({"kind": "chart", "layer": "Countries", "field": "pop_est", "label": "name"}),
        json!({"kind": "rectangle"}),
        json!({"kind": "dynamicText", "text": "Page {page} of {pages} · {scale}"}),
        json!({"kind": "picture", "image": b64}),
        json!({"kind": "grid", "step": 10}),
    ] {
        s.run("layout.addElement", &p).unwrap_or_else(|e| panic!("{p}: {e}"));
    }
    let inset = s.run("layout.addFrame", &json!({"shape": "rectangle", "x": 200, "y": 150, "w": 60, "h": 40})).unwrap()["element"]
        .as_u64()
        .unwrap();
    s.run("layout.update", &json!({"element": inset, "scale": 200_000_000})).unwrap();
    s.run("layout.addElement", &json!({"kind": "extentIndicator", "frame": inset, "of": frame})).unwrap();
    assert!(s.run("layout.addElement", &json!({"kind": "extentIndicator", "frame": inset, "of": inset})).is_err());
    assert!(s.run("layout.addElement", &json!({"kind": "sparkles"})).is_err());
    // Frame zooms and edits.
    s.run("layout.frameZoom", &json!({"element": frame, "to": "layer", "layer": "Lakes"})).unwrap();
    s.run("layout.update", &json!({"element": frame, "x": 12, "w": 180, "center": [10, 50]})).unwrap();
    assert!(s.run("layout.update", &json!({"element": 9999, "x": 1})).is_err());
    s.run("layout.update", &json!({"element": frame, "name": "Main map"})).unwrap();
    let chart = s.project.layouts[0].elements.iter().find(|e| matches!(e.kind, rustgis_map::layout::ElementKind::Chart { .. })).unwrap().id;
    assert!(s.run("layout.update", &json!({"element": chart, "field": "no_such_field"})).is_err());
    assert!(s.run("layout.update", &json!({"element": chart, "layer": "Atlantis"})).is_err());
    s.run("layout.update", &json!({"element": chart, "field": "gdp_md"})).unwrap();
    assert_eq!(s.project.layouts[0].element(frame).unwrap().name, "Main map");
    // A map series over the lakes: one page each.
    let r = s.run("layout.series", &json!({"layer": "Lakes", "nameField": "name"})).unwrap();
    let pages = r["pages"].as_u64().unwrap();
    assert!(pages > 5);
    let next = s.run("layout.seriesPage", &json!({"page": "next"})).unwrap();
    assert_eq!(next["page"], 1);
    s.run("layout.seriesRefresh", &Value::Null).unwrap();
    // A manual zoom of the series frame survives exports.
    s.run("layout.update", &json!({"element": frame, "scale": 3_000_000})).unwrap();
    let scale_of = |s: &Session| match &s.project.layouts[0].element(frame).unwrap().kind {
        rustgis_map::layout::ElementKind::MapFrame { scale, .. } => *scale,
        _ => 0.0,
    };
    // Export PNG and a PDF of every page.
    let png_path = dir.join("europe.png");
    s.run("layout.export", &json!({"path": png_path.display().to_string(), "dpi": 72})).unwrap();
    let bytes = std::fs::read(&png_path).unwrap();
    assert!(bytes.starts_with(b"\x89PNG"));
    let pdf_path = dir.join("europe.pdf");
    s.run("layout.export", &json!({"path": pdf_path.display().to_string(), "dpi": 36})).unwrap();
    let pdf = std::fs::read(&pdf_path).unwrap();
    let count = format!("/Count {pages}");
    assert!(pdf.windows(count.len()).any(|w| w == count.as_bytes()), "one PDF page per series page");
    assert_eq!(s.project.layouts[0].series.as_ref().unwrap().page, 1, "the shown page is restored");
    assert_eq!(scale_of(&s), 3_000_000.0, "and so is the frame's zoom");
    assert!(s.run("layout.export", &json!({"path": dir.join("x.svg").display().to_string()})).is_err());
    // Layout files.
    let file = dir.join("europe.rgislayout");
    s.run("layout.save", &json!({"path": file.display().to_string()})).unwrap();
    s.run("layout.import", &json!({"path": file.display().to_string()})).unwrap();
    assert_eq!(s.project.layouts.len(), 2);
    assert_eq!(s.project.layouts[1].name, "Europe 2");
    assert!(s.run("layout.import", &json!({"text": "{}"})).is_err());
    // Removing the frame removes its surrounds; undo brings them back.
    let before = s.project.layouts[1].elements.len();
    let first = s.project.layouts[1].first_frame().unwrap();
    s.run("layout.removeElement", &json!({"layout": 1, "element": first})).unwrap();
    assert!(s.project.layouts[1].elements.len() < before - 3);
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.project.layouts[1].elements.len(), before);
    // Layouts are saved with the project.
    let text = serde_json::to_string(&s.project).unwrap();
    let p = crate::Project::from_json(&text).unwrap();
    assert_eq!(p.layouts.len(), 2);
    s.run("layout.close", &Value::Null).unwrap();
    assert_eq!(s.active_layout, None);
    s.run("layout.remove", &json!({"name": "Europe 2"})).unwrap();
    assert_eq!(s.project.layouts.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
