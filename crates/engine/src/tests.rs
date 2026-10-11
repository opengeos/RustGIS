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
    assert!(done >= 241, "coverage fell to {done}");
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

#[test]
fn every_tool_survives_an_environment_extent() {
    // An extent holding some features, and one holding none, with every tool and junk params.
    for extent in [json!([0, 40, 20, 55]), json!([-179.9, -89, -179.8, -88.9])] {
        for t in rustgis_analysis::tools().filter(|t| !rustgis_analysis::is_external(t)) {
            let mut s = world();
            s.run("gp.env", &json!({"extent": extent})).unwrap();
            for params in [json!({"input": "Countries"}), json!({"input": "Cities", "distance": 1e9, "clip": "Countries"})] {
                let _ = s.run("gp.run", &json!({"tool": t.id, "params": params}));
            }
            let _ = s.run("info.project", &Value::Null);
        }
    }
    let mut s = world();
    s.run("gp.env", &json!({"extent": [-179.9, -89, -179.8, -88.9]})).unwrap();
    let e = s.run("gp.run", &json!({"tool": "buffer", "params": {"input": "Cities", "distance": 10}})).unwrap_err();
    assert!(e.contains("inside the environment"), "{e}");
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
    // A chart moved to a layer without its field charts that layer's first numeric field.
    let chart = s.project.layouts[0].elements.iter().find(|e| e.kind.code() == "chart").unwrap().id;
    s.run("layout.update", &json!({"element": chart, "layer": "Cities"})).unwrap();
    let Some(rustgis_map::layout::ElementKind::Chart { layer, field, .. }) = s.project.layouts[0].element(chart).map(|e| &e.kind) else {
        panic!()
    };
    let cities = &s.map().layer_by_name("Cities").unwrap().dataset;
    assert_eq!(layer, "Cities");
    assert!(cities.field_index(field).is_some_and(|i| cities.fields[i].ty.is_numeric()), "{field}");
    s.run("layout.update", &json!({"element": chart, "layer": "Countries", "field": "pop_est"})).unwrap();
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
    assert!(s.run("layout.update", &json!({"element": frame, "x": 99, "center": "Paris"})).is_err());
    assert!((s.project.layouts[0].element(frame).unwrap().x - 12.0).abs() < 1e-9, "a bad update changes nothing");
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
    let r = s.run("layout.rename", &json!({"name": "Europe 2", "newName": "europe"})).unwrap();
    assert_eq!(r["name"], "europe 2", "renaming keeps names unique");
    s.run("layout.rename", &json!({"name": "europe 2", "newName": "Europe 2"})).unwrap();
    s.run("layout.close", &Value::Null).unwrap();
    assert_eq!(s.active_layout, None);
    s.run("layout.remove", &json!({"name": "Europe 2"})).unwrap();
    assert_eq!(s.project.layouts.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn project_items_files_styles_packages_tasks_and_3d() {
    let dir = std::env::temp_dir().join(format!("rustgis-items-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |n: &str| dir.join(n).display().to_string();
    let mut s = world();
    // Folder connections.
    s.run("project.addFolder", &json!({"path": dir.display().to_string()})).unwrap();
    assert!(s.run("project.addFolder", &json!({"path": path("missing")})).is_err());
    assert_eq!(s.run("project.connections", &Value::Null).unwrap()["folders"][0]["available"], true);
    // Map and layer files.
    s.run("map.save", &json!({"path": path("world.rgismap")})).unwrap();
    s.run("map.import", &json!({"path": path("world.rgismap")})).unwrap();
    assert_eq!(s.project.maps.len(), 2);
    assert_eq!(s.project.maps[1].name, "World (2)");
    s.run("layer.symbology", &json!({"layer": "Cities", "type": "single", "color": "#AA00FF"})).ok();
    s.run("layer.save", &json!({"layer": "Cities", "path": path("cities.rgislyr")})).unwrap();
    let n = s.map().layers.len();
    s.run("layer.addData", &json!({"path": path("cities.rgislyr")})).unwrap();
    assert_eq!(s.map().layers.len(), n + 1);
    assert!(s.map().layer_by_name("Cities (2)").is_some());
    // Styles.
    s.run("style.new", &json!({"layer": "Countries", "name": "Choropleth"})).unwrap();
    s.run("style.save", &json!({"path": path("styles.rgisstyle")})).unwrap();
    s.run("style.remove", &json!({"name": "Choropleth"})).unwrap();
    s.run("style.add", &json!({"path": path("styles.rgisstyle")})).unwrap();
    assert!(s.run("style.apply", &json!({"name": "Choropleth", "layer": "Cities"})).is_err(), "polygon style on points");
    s.run("layer.symbology", &json!({"layer": "Countries", "type": "single"})).unwrap();
    s.run("style.apply", &json!({"name": "Choropleth", "layer": "Countries"})).unwrap();
    assert_eq!(s.map().layer_by_name("Countries").unwrap().renderer.kind_label(), "Graduated Colors");
    let r = s.run("style.import", &json!({"path": path("world.rgismap")})).unwrap();
    assert!(r["imported"].as_u64().unwrap() >= 4);
    // Templates keep the setup, not the features.
    s.run("project.saveTemplate", &json!({"path": path("atlas.rgistemplate")})).unwrap();
    let mut t = Session::new(Project::default());
    t.run("project.fromTemplate", &json!({"path": path("atlas.rgistemplate"), "name": "From template"})).unwrap();
    assert_eq!(t.project.name, "From template");
    assert!(t.map().layer_by_name("Countries").unwrap().dataset.features.is_empty());
    assert!(!t.project.styles.is_empty());
    // Packages open again.
    s.run("project.package", &json!({"path": path("atlas.rgispkg")})).unwrap();
    let mut o = Session::new(Project::default());
    o.run("project.open", &json!({"path": path("atlas.rgispkg")})).unwrap();
    assert_eq!(o.project.maps.len(), 2);
    assert!(o.path.is_none(), "a package opens unsaved");
    s.run("map.package", &json!({"path": path("world.rgismpk"), "mobile": true, "tolerance": 0.05})).unwrap();
    s.run("map.import", &json!({"path": path("world.rgismpk")})).unwrap();
    let full: usize = s.project.maps[0]
        .layer_by_name("Countries")
        .unwrap()
        .dataset
        .features
        .iter()
        .filter_map(|f| f.geometry.as_ref())
        .map(|g| g.coord_count())
        .sum();
    let mobile: usize = s
        .project
        .maps
        .last()
        .unwrap()
        .layer_by_name("Countries")
        .unwrap()
        .dataset
        .features
        .iter()
        .filter_map(|f| f.geometry.as_ref())
        .map(|g| g.coord_count())
        .sum();
    assert!(mobile < full, "mobile packages simplify: {mobile} < {full}");
    assert!(s.run("gp.package", &json!({"path": path("results.rgisgpk")})).is_err(), "no runs yet");
    s.run("map.activate", &json!({"name": "World"})).unwrap();
    s.run("gp.run", &json!({"tool": "buffer", "params": {"input": "Cities", "distance": 100}})).unwrap();
    s.run("gp.package", &json!({"path": path("results.rgisgpk")})).unwrap();
    let entries = rustgis_data::zip::read(&std::fs::read(path("results.rgisgpk")).unwrap()).unwrap();
    assert!(entries.iter().any(|(n, _)| n == "history.json") && entries.iter().any(|(n, _)| n.starts_with("results/")));
    // Layer templates.
    let id = s.run("layer.template", &json!({"template": "places"})).unwrap()["layer"].as_u64().unwrap() as u32;
    assert_eq!(s.map().layer(id).unwrap().dataset.fields.len(), 4);
    assert!(s.run("layer.template", &json!({"template": "unicorns"})).is_err());
    // Tasks.
    s.run("task.new", &json!({"name": "Explore", "steps": [{"title": "Zoom to Europe", "command": "map.goToXY", "params": {"lon": 10, "lat": 50, "scale": 20000000}}, {"title": "Select big countries", "command": "select.byAttributes", "params": {"layer": "Countries", "where": "pop_est > 50000000"}}]})).unwrap();
    assert!(s.run("task.new", &json!({"steps": [{"title": "x", "command": "no.such"}]})).is_err());
    let r = s.run("task.run", &json!({"task": "Explore"})).unwrap();
    assert_eq!(r["ran"].as_array().unwrap().len(), 2);
    assert!(s.map().selection_count() > 5);
    s.run("task.addStep", &json!({"task": "Explore", "title": "Clear", "command": "select.clear"})).unwrap();
    s.run("task.run", &json!({"task": "Explore", "step": 2})).unwrap();
    assert_eq!(s.map().selection_count(), 0);
    s.run("task.new", &json!({"name": "Replay", "fromHistory": true})).unwrap();
    assert!(s.trusted_tasks.contains("replay"), "history run in this session");
    // History that came with an opened project is someone else's.
    s.history_from_file = 1;
    s.run("task.new", &json!({"name": "Replay file", "fromHistory": true})).unwrap();
    assert!(!s.trusted_tasks.contains("replay file"));
    s.history_from_file = 0;
    assert_eq!(s.project.tasks.last().unwrap().steps.len(), 1);
    s.run("task.save", &json!({"task": "Explore", "path": path("explore.rgistask")})).unwrap();
    s.run("task.import", &json!({"path": path("explore.rgistask")})).unwrap();
    assert_eq!(s.project.tasks.last().unwrap().name, "Explore 2");
    // A task file may not touch files or run scripts.
    let sneaky =
        json!({"name": "Sneaky", "steps": [{"title": "Overwrite", "command": "share.exportMap", "params": {"path": path("x.png")}}]});
    assert!(s.run("task.import", &json!({"text": sneaky.to_string()})).is_err());
    let nested = json!({"name": "Nested", "steps": [{"title": "Run", "command": "script.run", "params": {"script": "map.zoomFull"}}]});
    assert!(s.run("task.import", &json!({"text": nested.to_string()})).is_err());
    // A task arriving inside a project file is vetted when it runs.
    s.project.tasks.push(crate::project::Task {
        name: "From a file".into(),
        description: String::new(),
        steps: vec![crate::project::TaskStep {
            title: "Export".into(),
            instructions: String::new(),
            command: "share.exportMap".into(),
            params: json!({"path": path("y.png")}),
        }],
    });
    assert!(s.run("task.run", &json!({"task": "From a file"})).is_err());
    assert!(!std::path::Path::new(&path("y.png")).exists());
    // A step naming a feature layer runs even when a raster is selected.
    s.run("task.new", &json!({"name": "Layered", "steps": [{"title": "Zoom", "command": "layer.zoomTo", "params": {"layer": "Lakes"}}]}))
        .unwrap();
    s.run("task.run", &json!({"task": "Layered"})).unwrap();
    // 3D export: polygons extruded by a field.
    s.run("share.export3d", &json!({"layer": "Countries", "path": path("countries.obj"), "heightField": "pop_est", "zScale": 0.00001}))
        .unwrap();
    let obj = std::fs::read_to_string(path("countries.obj")).unwrap();
    assert!(obj.lines().filter(|l| l.starts_with("v ")).count() > 1000 && obj.contains("\nf "));
    // Caps are triangles, and every face refers to a written vertex.
    let verts = obj.lines().filter(|l| l.starts_with("v ")).count();
    assert!(obj.lines().filter(|l| l.starts_with("f ")).all(|l| {
        let ids: Vec<usize> = l[2..].split_whitespace().map(|x| x.parse().unwrap()).collect();
        (3..=4).contains(&ids.len()) && ids.iter().all(|i| (1..=verts).contains(i))
    }));
    let (name, bytes) = crate::cmd::items::item_bytes(&mut s, "obj", &json!({"layer": "Cities"})).unwrap();
    assert!(name.ends_with(".obj") && !bytes.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn data_design_rules_hold_on_every_edit() {
    let mut s = world();
    s.run("layer.template", &json!({"template": "routes", "name": "Roads"})).unwrap();
    s.run("edit.addField", &json!({"layer": "Roads", "name": "kind", "type": "long"})).unwrap();
    s.run("edit.addField", &json!({"layer": "Roads", "name": "lanes", "type": "long"})).unwrap();
    // Domains.
    s.run("data.domain", &json!({"layer": "Roads", "field": "surface", "coded": [["paved", "Paved"], ["gravel", "Gravel"]]})).unwrap();
    s.run("data.domain", &json!({"layer": "Roads", "field": "lanes", "min": 1, "max": 8})).unwrap();
    assert!(s.run("data.domain", &json!({"layer": "Roads", "field": "name", "min": 1, "max": 2})).is_err(), "text takes coded values");
    let line = json!({"type": "LineString", "coordinates": [[0, 0], [1, 1]]});
    s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "attributes": {"surface": "paved", "lanes": 2}})).unwrap();
    assert!(s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "attributes": {"surface": "ice"}})).is_err());
    assert!(s.run("edit.setValue", &json!({"layer": "Roads", "row": 0, "field": "lanes", "value": 12})).is_err());
    assert_eq!(s.map().layer_by_name("Roads").unwrap().dataset.features.len(), 1, "the failed edits were undone");
    // Subtypes fill defaults on new features.
    s.run("data.subtypes", &json!({"layer": "Roads", "field": "kind", "subtypes": [{"code": 1, "name": "Highway", "defaults": {"lanes": 4}}, {"code": 2, "name": "Street", "defaults": {"lanes": 1}}]})).unwrap();
    assert!(s.run("data.subtypes", &json!({"layer": "Roads", "field": "name", "subtypes": [{"code": 1, "name": "x"}]})).is_err());
    let r = s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "attributes": {"kind": 1}})).unwrap();
    let row = r["row"].as_u64().unwrap() as usize;
    let lanes = |s: &Session, row: usize| {
        let l = s.map().layer_by_name("Roads").unwrap();
        l.dataset.value(row, l.dataset.field_index("lanes").unwrap()).clone()
    };
    assert_eq!(lanes(&s, row).as_f64(), Some(4.0));
    // Attribute rules: a calculation and a constraint.
    s.run(
        "data.attributeRule",
        &json!({"layer": "Roads", "name": "Length", "kind": "calculation", "field": "length_km", "expression": "lanes * 10"}),
    )
    .unwrap();
    s.run(
        "data.attributeRule",
        &json!({"layer": "Roads", "name": "Named highways", "kind": "constraint", "expression": "kind <> 1 OR name IS NOT NULL"}),
    )
    .unwrap();
    assert!(
        s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "attributes": {"kind": 1}})).is_err(),
        "highways need names"
    );
    let r = s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "attributes": {"kind": 1, "name": "A1"}})).unwrap();
    let row = r["row"].as_u64().unwrap() as usize;
    let l = s.map().layer_by_name("Roads").unwrap();
    assert_eq!(l.dataset.value(row, l.dataset.field_index("length_km").unwrap()).as_f64(), Some(40.0));
    assert!(s.run("data.attributeRule", &json!({"layer": "Roads", "name": "Bad", "kind": "constraint", "expression": "(("})).is_err());
    s.run("data.attributeRule", &json!({"layer": "Roads", "name": "Named highways", "remove": true})).unwrap();
    // Deleting a field a rule uses drops the rule, so editing isn't blocked.
    s.run("edit.addField", &json!({"layer": "Roads", "name": "temp", "type": "double"})).unwrap();
    s.run("data.attributeRule", &json!({"layer": "Roads", "name": "Temp", "kind": "constraint", "expression": "temp IS NULL OR temp > 0"}))
        .unwrap();
    let r = s.run("edit.deleteField", &json!({"layer": "Roads", "field": "temp"})).unwrap();
    assert_eq!(r["rulesRemoved"], 1);
    s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "attributes": {"kind": 2}})).unwrap();
    // Contingent values.
    s.run("data.contingentValues", &json!({"layer": "Roads", "name": "Surface by kind", "fields": ["kind", "surface"], "combos": [[1, "paved"], [2, "paved"], [2, "gravel"]]})).unwrap();
    assert!(
        s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "attributes": {"kind": 1, "surface": "gravel"}})).is_err()
    );
    s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "attributes": {"kind": 2, "surface": "gravel"}})).unwrap();
    // Templates.
    s.run("edit.template", &json!({"layer": "Roads", "name": "Gravel street", "defaults": {"kind": 2, "surface": "gravel"}})).unwrap();
    let r = s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "template": "Gravel street"})).unwrap();
    assert_eq!(lanes(&s, r["row"].as_u64().unwrap() as usize).as_f64(), Some(1.0), "template, then subtype defaults");
    // Archive and restore.
    s.run("data.archive", &json!({"layer": "Roads"})).unwrap();
    let t0 = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).unwrap().as_secs_f64() - 0.001;
    let n = s.map().layer_by_name("Roads").unwrap().dataset.features.len();
    s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line, "template": "Gravel street"})).unwrap();
    s.run("edit.setValue", &json!({"layer": "Roads", "row": 0, "field": "name", "value": "Main"})).unwrap();
    let h = s.run("data.archiveHistory", &json!({"layer": "Roads"})).unwrap();
    assert_eq!(h["edits"], 2);
    s.run("data.archiveRestore", &json!({"layer": "Roads", "time": t0})).unwrap();
    let l = s.map().layer_by_name("Roads").unwrap();
    assert_eq!(l.dataset.features.len(), n);
    assert!(l.dataset.value(0, l.dataset.field_index("name").unwrap()).is_null());
    // Edit status, and turning editing off.
    let st = s.run("edit.status", &Value::Null).unwrap();
    assert!(st["layers"].as_array().unwrap().iter().any(|l| l["layer"] == "Roads" && l["added"].as_u64().unwrap() >= 5));
    s.run("edit.enable", &json!({"on": false})).unwrap();
    assert!(s.run("edit.addFeature", &json!({"layer": "Roads", "geometry": line})).is_err());
    s.run("edit.enable", &json!({"on": true})).unwrap();
}

#[test]
fn modify_tools_snapping_topology_relationships_and_elevation() {
    let mut s = with_raster();
    s.run("layer.template", &json!({"template": "lines", "name": "Paths"})).unwrap();
    let add = |s: &mut Session, coords: Value| {
        s.run("edit.addFeature", &json!({"layer": "Paths", "geometry": {"type": "LineString", "coordinates": coords}})).unwrap()["row"]
            .as_u64()
            .unwrap() as usize
    };
    let a = add(&mut s, json!([[0, 0], [1, 0]]));
    // Move one degree east (about 111 km at the equator), then rotate and scale.
    s.run("select.rows", &json!({"layer": "Paths", "rows": [a]})).unwrap();
    s.run("edit.move", &json!({"dx": 111_319.5, "dy": 0})).unwrap();
    let first = |s: &Session, row: usize| match s.map().layer_by_name("Paths").unwrap().dataset.features[row].geometry.as_ref().unwrap() {
        rustgis_data::Geometry::Line(p) => p[0][0],
        _ => panic!("not a line"),
    };
    assert!((first(&s, a).x - 1.0).abs() < 1e-3, "{:?}", first(&s, a));
    s.run("edit.rotate", &json!({"angle": 90})).unwrap();
    s.run("edit.scale", &json!({"factor": 2})).unwrap();
    s.run("edit.flip", &Value::Null).unwrap();
    // Split, then merge back, then explode.
    let b = add(&mut s, json!([[10, 10], [12, 10]]));
    s.run("select.rows", &json!({"layer": "Paths", "rows": [b]})).unwrap();
    s.run("edit.split", &json!({"lon": 11, "lat": 10.2})).unwrap();
    let n = s.map().layer_by_name("Paths").unwrap().dataset.features.len();
    s.run("select.rows", &json!({"layer": "Paths", "rows": [b, n - 1]})).unwrap();
    // A merge the rules reject leaves the layer as it was.
    s.run("data.attributeRule", &json!({"layer": "Paths", "name": "Never", "kind": "constraint", "expression": "name = 'never'"})).unwrap();
    let before = s.map().layer_by_name("Paths").unwrap().dataset.features.clone();
    assert!(s.run("edit.merge", &Value::Null).is_err());
    assert_eq!(s.map().layer_by_name("Paths").unwrap().dataset.features, before);
    s.run("data.attributeRule", &json!({"layer": "Paths", "name": "Never", "remove": true})).unwrap();
    s.run("select.rows", &json!({"layer": "Paths", "rows": [b, n - 1]})).unwrap();
    s.run("edit.merge", &Value::Null).unwrap();
    assert_eq!(s.map().layer_by_name("Paths").unwrap().dataset.features.len(), n - 1);
    s.run("edit.explode", &Value::Null).unwrap();
    assert_eq!(s.map().layer_by_name("Paths").unwrap().dataset.features.len(), n);
    assert!(s.run("edit.setGeometry", &json!({"layer": "Paths", "row": 0, "geometry": {"type": "Point", "coordinates": [0, 0]}})).is_err());
    s.run("edit.setGeometry", &json!({"layer": "Paths", "row": 0, "geometry": {"type": "LineString", "coordinates": [[0, 0], [0, 1]]}}))
        .unwrap();
    // Traverse: two legs from a start, ground to grid off and on.
    s.run("layer.select", &json!({"layer": "Paths"})).unwrap();
    let r = s
        .run("edit.traverse", &json!({"start": [20, 0], "legs": [{"distance": 1000, "bearing": 90}, {"distance": 1000, "bearing": 0}]}))
        .unwrap();
    let row = r["row"].as_u64().unwrap() as usize;
    let end = |s: &Session, row: usize| match s.map().layer_by_name("Paths").unwrap().dataset.features[row].geometry.as_ref().unwrap() {
        rustgis_data::Geometry::Line(p) => *p[0].last().unwrap(),
        _ => panic!("not a line"),
    };
    let e = end(&s, row);
    assert!((e.x - 20.008983).abs() < 1e-5 && (e.y - 0.009043).abs() < 1e-4, "{e:?}");
    s.run("edit.groundToGrid", &json!({"scale": 1.5, "rotation": 0})).unwrap();
    let r = s.run("edit.traverse", &json!({"start": [20, 0], "legs": [{"distance": 1000, "bearing": 90}]})).unwrap();
    assert!((end(&s, r["row"].as_u64().unwrap() as usize).x - 20.013475).abs() < 1e-5);
    assert!(s.run("edit.groundToGrid", &json!({"scale": 9})).is_err());
    s.run("edit.groundToGrid", &json!({"on": false})).unwrap();
    // Snapping to a vertex near the pointer.
    s.run("map.goToXY", &json!({"lon": 20, "lat": 0, "scale": 50_000})).unwrap();
    let off = s.run("edit.snap", &json!({"lon": 20.0001, "lat": 0.0001})).unwrap();
    assert_eq!(off["snapped"], false, "snapping is off");
    s.run("edit.snapping", &json!({"on": true, "tolerancePx": 20})).unwrap();
    let on = s.run("edit.snap", &json!({"lon": 20.0001, "lat": 0.0001})).unwrap();
    assert_eq!(on["snapped"], true);
    assert!((on["lon"].as_f64().unwrap() - 20.0).abs() < 1e-9 && on["lat"].as_f64().unwrap().abs() < 1e-9, "{on}");
    // Topology: overlapping countries? Lakes inside countries? Dangling paths.
    s.run("topology.rules", &json!({"add": {"rule": "mustNotHaveDangles", "layer": "Paths"}})).unwrap();
    s.run("topology.rules", &json!({"add": {"rule": "mustBeCoveredBy", "layer": "Cities", "by": "Countries"}})).unwrap();
    assert!(s.run("topology.rules", &json!({"add": {"rule": "mustNotOverlap", "layer": "Paths"}})).is_err(), "overlap is for polygons");
    let v = s.run("topology.validate", &Value::Null).unwrap();
    assert!(v["errors"].as_u64().unwrap() > 0);
    assert_eq!(s.topology_errors.len() as u64, v["errors"].as_u64().unwrap());
    // Relationships: cities to their country by ISO code.
    s.run("data.relate", &json!({"name": "Country of city", "origin": "Countries", "originKey": "name", "destination": "Cities", "destinationKey": "adm0name"})).unwrap();
    assert!(
        s.run(
            "data.relate",
            &json!({"name": "x", "origin": "Countries", "originKey": "nope", "destination": "Cities", "destinationKey": "adm0name"})
        )
        .is_err()
    );
    s.run("select.byAttributes", &json!({"layer": "Countries", "where": "name = 'France'"})).unwrap();
    let rel = s.run("data.related", &Value::Null).unwrap();
    assert!(rel["related"].as_u64().unwrap() >= 1, "{rel}");
    // Elevation from the raster surface goes in z.
    s.run("edit.elevation", &json!({"mode": "constant", "z": 125})).unwrap();
    s.run("layer.template", &json!({"template": "points", "name": "Spots"})).unwrap();
    s.run("edit.addFeature", &json!({"layer": "Spots", "geometry": {"type": "Point", "coordinates": [0, 0]}})).unwrap();
    let l = s.map().layer_by_name("Spots").unwrap();
    assert_eq!(l.dataset.value(0, l.dataset.field_index("z").unwrap()).as_f64(), Some(125.0));
    assert!(s.run("edit.elevation", &json!({"mode": "surface", "surface": "Cities"})).is_err(), "not a raster");
    let tools = s.run("edit.tools", &Value::Null).unwrap();
    assert!(tools.as_array().unwrap().len() >= 9);
}

/// A 40×40 lon/lat DEM near (10°E, 45°N): a hill in the middle, with a wall to its east.
fn dem_b64() -> String {
    use base64::Engine;
    use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster, geotiff};
    let gt = GeoTransform { origin_x: 10.0, origin_y: 45.04, pixel_width: 0.001, pixel_height: -0.001 };
    let z: Vec<f32> = (0..1600)
        .map(|i| {
            let (c, r) = ((i % 40) as f32 - 20.0, (i / 40) as f32 - 20.0);
            let hill = 100.0 * (-(c * c + r * r) / 60.0).exp();
            if i % 40 == 30 { 400.0 } else { 200.0 + hill }
        })
        .collect();
    let r = Raster::new("dem", 40, 40, vec![z], gt, Crs::LonLat, None, ColorHint::Gray, "Float32").unwrap();
    base64::engine::general_purpose::STANDARD.encode(geotiff::write(&r, 0).unwrap())
}

#[test]
fn analysis_workflows_models_scripts_and_raster_functions() {
    let mut s = world();
    s.run("layer.addDataBytes", &json!({"fileName": "dem.tif", "data": dem_b64()})).unwrap();
    let dem = s.map().layers.iter().find(|l| l.is_raster()).unwrap().name.clone();
    // Raster functions and a chain.
    for (func, extra) in [
        ("slope", json!({})),
        ("aspect", json!({})),
        ("hillshade", json!({"azimuth": 300})),
        ("threshold", json!({"value": 250})),
        ("bandMath", json!({"expression": "b1 * 2 - 100"})),
    ] {
        let mut p = json!({"layer": dem, "function": func});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        s.run("raster.function", &p).unwrap_or_else(|e| panic!("{func}: {e}"));
    }
    assert!(s.run("raster.function", &json!({"layer": dem, "function": "warp"})).is_err());
    assert!(s.run("raster.function", &json!({"layer": dem, "function": "bandMath", "expression": "b2"})).is_err());
    let r = s
        .run(
            "raster.functionChain",
            &json!({"layer": dem, "steps": [{"function": "slope"}, {"function": "threshold", "value": 10}], "name": "Steep"}),
        )
        .unwrap();
    let steep = s.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap();
    assert_eq!(steep.name, "Steep");
    let data = steep.raster.as_ref().unwrap().data.as_ref().unwrap();
    assert!(data.bands[0].stats.max == 1.0 && data.bands[0].stats.min == 0.0);
    // Rasters made by functions save with the project.
    let text = serde_json::to_string(&s.project).unwrap();
    let back = crate::Project::from_json(&text).unwrap();
    assert!(back.map().layer_by_name("Steep").unwrap().raster.as_ref().unwrap().data.is_some());
    // Suitability: high and flat.
    let slope_name = format!("{dem}_slope");
    let r = s
        .run(
            "analysis.suitability",
            &json!({"criteria": [
        {"layer": dem, "ranges": [[0, 250, 1], [250, 1000, 10]], "weight": 2},
        {"layer": slope_name, "ranges": [[0, 5, 10], [5, 90, 1]], "weight": 1}]}),
        )
        .unwrap();
    assert!(r["layer"].as_u64().is_some());
    // Viewshed and line of sight: the wall at column 30 hides what's beyond it.
    // On the flat northern edge: everything west of the wall is in view.
    let v = s.run("analysis.viewshed", &json!({"layer": dem, "lon": 10.0055, "lat": 45.0395, "height": 2})).unwrap();
    assert!(v["visibleCells"].as_u64().unwrap() > 100, "{v}");
    let los = s.run("analysis.lineOfSight", &json!({"layer": dem, "from": [10.0205, 45.0395], "to": [10.0355, 45.0395]})).unwrap();
    assert_eq!(los["visible"], false);
    let los = s.run("analysis.lineOfSight", &json!({"layer": dem, "from": [10.0205, 45.0395], "to": [10.0055, 45.0395]})).unwrap();
    assert_eq!(los["visible"], true);
    // IDW of city populations, with an environment extent and cell size.
    s.run("gp.env", &json!({"extent": [-10, 35, 30, 60], "cellSize": 0.5})).unwrap();
    let r = s.run("analysis.interpolate", &json!({"layer": "Cities", "field": "pop_max"})).unwrap();
    let idw = s.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().raster.as_ref().unwrap().data.clone().unwrap();
    assert_eq!((idw.width, idw.height), (80, 50));
    // The environment extent limits tools to the features meeting it.
    s.run("gp.run", &json!({"tool": "buffer", "params": {"input": "Cities", "distance": 10, "output": "Europe buffers"}})).unwrap();
    let europe = s.map().layer_by_name("Europe buffers").unwrap().dataset.features.len();
    let all = s.map().layer_by_name("Cities").unwrap().dataset.features.len();
    assert!(europe > 10 && europe < all / 2, "{europe} of {all}");
    s.run("gp.env", &json!({"extent": null, "cellSize": null})).unwrap();
    // Network route along the rivers: two points on the same river.
    let rivers = s.map().layer_by_name("Rivers").unwrap();
    let (a, b2) = match rivers.dataset.features.iter().filter_map(|f| f.geometry.as_ref()).find_map(|g| match g {
        rustgis_data::Geometry::Line(p) if p[0].len() > 10 => Some((p[0][0], p[0][p[0].len() - 1])),
        _ => None,
    }) {
        Some(x) => x,
        None => panic!("no river"),
    };
    let r = s.run("analysis.route", &json!({"layer": "Rivers", "from": [a.x, a.y], "to": [b2.x, b2.y]})).unwrap();
    assert!(r["lengthKm"].as_f64().unwrap() > 10.0, "{r}");
    // Neighbourhood, simulation, profile, exploration, similarity, conversion.
    let n = s
        .run("analysis.neighborhood", &json!({"lon": 2.35, "lat": 48.85, "radius": 300000, "layers": ["Cities"], "select": true}))
        .unwrap();
    assert!(n["layers"][0]["count"].as_u64().unwrap() >= 2);
    s.run("select.byAttributes", &json!({"layer": "Countries", "where": "name = 'France'"})).unwrap();
    let sim = s.run("analysis.simulate", &json!({"layer": "Countries", "count": 20, "seed": 3})).unwrap();
    assert_eq!(sim["points"], 20);
    s.run("select.clear", &Value::Null).unwrap();
    let prof = s.run("data.profile", &json!({"layer": "Countries"})).unwrap();
    assert!(prof["fields"].as_array().unwrap().iter().any(|f| f["field"] == "pop_est" && f["min"].as_f64().is_some()));
    let ex = s.run("data.explore", &json!({"layer": "Countries", "fields": ["pop_est", "gdp_md"]})).unwrap();
    assert!(ex["correlation"][0][1].as_f64().unwrap() > 0.3, "population and GDP correlate");
    let fr = s.map().layer_by_name("Countries").unwrap().dataset.features.iter().position(|_| true).unwrap();
    let sm = s.run("data.similar", &json!({"layer": "Countries", "row": fr, "fields": ["pop_est", "gdp_md"], "count": 3})).unwrap();
    assert_eq!(sm["similar"].as_array().unwrap().len(), 3);
    for fmt in ["kml", "gpx", "wkt", "geojson", "csv"] {
        let t = s.run("data.convert", &json!({"layer": "Cities", "format": fmt})).unwrap();
        assert!(t["text"].as_str().unwrap().len() > 1000, "{fmt}");
    }
    // Models chain tools; scripts run commands.
    s.run(
        "model.new",
        &json!({"name": "Buffer then dissolve", "steps": [
        {"tool": "buffer", "params": {"input": "Cities", "distance": 50}},
        {"tool": "dissolve", "params": {"input": "%1"}}]}),
    )
    .unwrap();
    let r = s.run("model.run", &json!({"name": "Buffer then dissolve"})).unwrap();
    assert_eq!(r["ran"].as_array().unwrap().len(), 2);
    assert!(s.run("model.new", &json!({"steps": [{"tool": "nope", "params": {}}]})).is_err());
    // A model carried in a file can't touch files.
    s.project.models.push(crate::project::Model {
        name: "From a file".into(),
        steps: vec![crate::project::ModelStep {
            tool: "buffer".into(),
            params: json!({"input": "Cities", "distance": 5, "output": "/tmp/x.shp"}),
        }],
    });
    let e = s.run("model.run", &json!({"name": "From a file"})).unwrap_err();
    assert!(e.contains("from a file"), "{e}");
    let m = s.run("model.new", &json!({"name": "Replay", "fromHistory": true})).unwrap();
    assert!(m["steps"].as_u64().unwrap() >= 2);
    let dissolve = s.project.models.last().unwrap().steps.iter().find(|st| st.tool == "dissolve").unwrap();
    assert!(dissolve.params["input"].as_str().unwrap().starts_with('%'), "history inputs wire to earlier steps");
    let r = s.run("script.run", &json!({"script": "# zoom then select\nmap.goToXY {\"lon\": 2, \"lat\": 48, \"scale\": 5000000}\nselect.byAttributes {\"layer\": \"Cities\", \"where\": \"pop_max > 1000000\"}"})).unwrap();
    assert_eq!(r["ran"].as_array().unwrap().len(), 2);
    assert!(s.run("script.run", &json!({"script": "no.such {}"})).is_err());
    let g = s.run("gp.gallery", &json!({"kind": "raster"})).unwrap();
    assert!(g.as_array().unwrap().iter().any(|t| t["id"] == "raster.function"));
}

#[test]
fn layer_effects_and_imagery_tools() {
    use crate::render::View;
    let mut s = world();
    s.run("layer.addDataBytes", &json!({"fileName": "dem.tif", "data": dem_b64()})).unwrap();
    let dem = s.map().layers.iter().find(|l| l.is_raster()).unwrap().name.clone();
    s.run("map.goToXY", &json!({"lon": 10.0, "lat": 48.0, "scale": 20_000_000})).unwrap();
    let view = |s: &Session| View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: s.map().camera, dark: false };
    let png = |s: &mut Session| {
        let v = view(s);
        crate::render::render(s.map_mut(), &v).unwrap().pixmap
    };
    // Every effect renders.
    s.run("layer.blend", &json!({"layer": "Countries", "mode": "multiply"})).unwrap();
    assert!(s.run("layer.blend", &json!({"layer": "Countries", "mode": "dodge"})).is_err());
    s.run("layer.mask", &json!({"layer": "Cities", "mask": "Countries"})).unwrap();
    assert!(s.run("layer.mask", &json!({"layer": "Countries", "mask": "Countries"})).is_err());
    assert!(s.run("layer.mask", &json!({"layer": "Countries", "mask": "Cities"})).is_err(), "masks are polygons");
    s.run("layer.swipe", &json!({"layer": "Countries", "position": 0.5})).unwrap();
    let swiped = png(&mut s);
    // Right of the divider the countries are gone: only water and basemap land remain.
    s.run("layer.swipe", &json!({"layer": "Countries", "off": true})).unwrap();
    let full = png(&mut s);
    assert_ne!(swiped.data(), full.data());
    s.run("layer.displayFilters", &json!({"layer": "Cities", "filters": [{"name": "Big at small scales", "minScale": 1e12, "maxScale": 5_000_000, "where": "pop_max > 5000000"}]})).unwrap();
    assert!(s.run("layer.displayFilters", &json!({"layer": "Cities", "filters": [{"where": "(("}]})).is_err());
    s.run("layer.aggregation", &json!({"layer": "Cities", "shape": "hexagon", "size": 30})).unwrap();
    assert!(s.run("layer.aggregation", &json!({"layer": "Countries", "shape": "hexagon"})).is_err());
    png(&mut s);
    s.run("layer.aggregation", &json!({"layer": "Cities", "shape": "square"})).unwrap();
    png(&mut s);
    s.run("layer.extrusion", &json!({"layer": "Countries", "type": "field", "field": "pop_est", "factor": 0.002})).unwrap();
    s.run("layer.faces", &json!({"layer": "Countries", "culling": false, "lighting": true})).unwrap();
    s.run("layer.lightShadow", &json!({"layer": "Countries", "shadows": true})).unwrap();
    let extruded = png(&mut s);
    assert_ne!(extruded.data(), full.data());
    assert!(s.run("layer.faces", &json!({"layer": "Lakes", "culling": true})).is_err(), "no extrusion yet");
    s.run("raster.rotation", &json!({"layer": dem, "angle": 30})).unwrap();
    s.run("map.goToXY", &json!({"lon": 10.02, "lat": 45.02, "scale": 50_000})).unwrap();
    png(&mut s);
    assert!(s.run("raster.rotation", &json!({"layer": "Countries", "angle": 30})).is_err());
    s.run("raster.rotation", &json!({"layer": dem, "angle": 0})).unwrap();
    // Imagery.
    let r = s.run("raster.georeference", &json!({"layer": dem, "points": [[0, 0, 20.0, 50.0], [40, 40, 20.4, 49.6]]})).unwrap();
    let e = r["extent"].as_array().unwrap();
    assert!((e[0].as_f64().unwrap() - 20.0).abs() < 1e-6 && (e[3].as_f64().unwrap() - 50.0).abs() < 1e-6, "{r}");
    assert!(
        s.run("raster.georeference", &json!({"layer": dem, "points": [[0, 0, 20.0, 50.0], [40, 40, 19.0, 51.0]]})).is_err(),
        "north up only"
    );
    s.run("raster.georeference", &json!({"layer": dem, "extent": [10.0, 45.0, 10.04, 45.04]})).unwrap();
    let sp = s.run("raster.spectral", &json!({"layer": dem, "lon": 10.0205, "lat": 45.0195})).unwrap();
    assert!(sp["values"][0].as_f64().unwrap() > 290.0);
    s.run("raster.index", &json!({"layer": dem, "index": "ndvi", "nir": 1, "red": 1})).unwrap();
    let r = s.run("raster.classify", &json!({"layer": dem, "classes": 3})).unwrap();
    // Forms send numbers as floats; whole ones are accepted.
    s.run("raster.classify", &json!({"layer": dem, "classes": 3.0, "name": "float_classes"})).unwrap();
    assert!(s.run("raster.classify", &json!({"layer": dem, "classes": 2.5})).is_err());
    assert!(r["layer"].as_u64().is_some());
    s.run("raster.function", &json!({"layer": dem, "function": "bandMath", "expression": "b1 + 10", "name": "dem_later"})).unwrap();
    let ch = s.run("raster.change", &json!({"before": dem, "after": "dem_later", "method": "difference"})).unwrap();
    let ch_layer = s.map().layer(ch["layer"].as_u64().unwrap() as u32).unwrap().raster.as_ref().unwrap().data.clone().unwrap();
    assert!((ch_layer.bands[0].stats.mean - 10.0).abs() < 0.01);
    s.run("raster.process", &json!({"layer": dem, "operation": "clip", "extent": [10.005, 45.005, 10.02, 45.02]})).unwrap();
    let r = s.run("raster.process", &json!({"layer": dem, "operation": "resample", "factor": 4})).unwrap();
    let small = s.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().raster.as_ref().unwrap().data.clone().unwrap();
    assert_eq!((small.width, small.height), (10, 10));
    let r = s
        .run(
            "raster.editPixels",
            &json!({"layer": dem, "polygon": [[10.0, 45.0], [10.01, 45.0], [10.01, 45.01], [10.0, 45.01]], "value": 0}),
        )
        .unwrap();
    assert_eq!(r["pixels"], 100);
    // Supervised classification from two training squares.
    s.run("layer.template", &json!({"template": "polygons", "name": "Training"})).unwrap();
    for (name, x) in [("low", 10.002), ("high", 10.018)] {
        s.run("edit.addFeature", &json!({"layer": "Training", "geometry": {"type": "Polygon", "coordinates": [[[x, 45.018], [x + 0.004, 45.018], [x + 0.004, 45.022], [x, 45.022], [x, 45.018]]]}, "attributes": {"name": name}})).unwrap();
    }
    let r = s.run("raster.classifySupervised", &json!({"layer": dem, "training": "Training", "classField": "name"})).unwrap();
    assert_eq!(r["classes"].as_array().unwrap().len(), 2, "{r}");
    // Mensuration.
    let m = s.run("raster.measure", &json!({"layer": dem, "kind": "height", "points": [[10.0015, 45.0385], [10.0205, 45.0195]]})).unwrap();
    assert!(m["metres"].as_f64().unwrap() > 50.0, "{m}");
    s.run(
        "raster.measure",
        &json!({"layer": dem, "kind": "volume", "points": [[10.015, 45.015], [10.025, 45.015], [10.025, 45.025], [10.015, 45.025]]}),
    )
    .unwrap();
    s.run("raster.measure", &json!({"kind": "area", "points": [[0, 0], [1, 0], [1, 1]]})).unwrap();
    assert_eq!(s.run("raster.measurements", &Value::Null).unwrap().as_array().unwrap().len(), 3);
    // Workspaces: an orthomosaic of two rasters, a DSM mosaic and its mesh.
    s.run("imagery.workspace", &json!({"kind": "ortho", "name": "Valley", "images": [dem, "dem_later"]})).unwrap();
    s.run("imagery.workspaceOptions", &json!({"name": "Valley", "cellSize": 0.002})).unwrap();
    let r = s.run("imagery.mosaic", &json!({"name": "Valley"})).unwrap();
    let mo = s.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().raster.as_ref().unwrap().data.clone().unwrap();
    assert_eq!((mo.width, mo.height), (20, 20));
    s.run("imagery.workspace", &json!({"kind": "reality", "images": ["dem_later"]})).unwrap();
    let r = s.run("imagery.mosaic", &json!({"name": "Reality Mapping"})).unwrap();
    let dsm = s.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().name.clone();
    let mesh = s.run("imagery.mesh", &json!({"layer": dsm})).unwrap();
    assert!(mesh["faces"].as_u64().unwrap() > 100);
    // Meshes go to .obj files; workspace methods are first or mean, checked on creation too.
    assert!(s.run("imagery.mesh", &json!({"layer": dsm, "path": "/tmp/dsm.png"})).is_err());
    assert!(s.run("imagery.workspace", &json!({"kind": "ortho", "images": [dem], "method": "median"})).is_err());
    assert!(s.run("imagery.workspace", &json!({"kind": "ortho", "images": [dem], "cellSize": 20})).is_err());
    // A mean mosaic keeps fractional values.
    s.run("imagery.workspaceOptions", &json!({"name": "Valley", "method": "mean"})).unwrap();
    let r = s.run("imagery.mosaic", &json!({"name": "Valley"})).unwrap();
    let mo = s.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().raster.as_ref().unwrap().data.clone().unwrap();
    assert_eq!(mo.data_type, "Float32");
    assert!(s.run("imagery.workspace", &json!({"kind": "lidar", "images": [dem]})).is_err());
    // Effects and workspaces save with the project.
    let p = crate::Project::from_json(&serde_json::to_string(&s.project).unwrap()).unwrap();
    assert!(p.map().layer_by_name("Countries").unwrap().extrusion.is_some() && p.workspaces.len() == 2);
}

#[test]
fn view_effects_scene_and_navigation() {
    use crate::render::View;
    let mut s = world();
    s.run("map.goToXY", &json!({"lon": 10.0, "lat": 48.0, "scale": 20_000_000})).unwrap();
    let png = |s: &mut Session| {
        let v = View { width: 300.0, height: 200.0, pixels_per_point: 1.0, camera: s.map().camera, dark: false };
        crate::render::render(s.map_mut(), &v).unwrap().pixmap.data().to_vec()
    };
    let plain = png(&mut s);
    for p in [
        json!({"sunTime": 1_710_979_200.0}),
        json!({"weather": "fog"}),
        json!({"weather": "snow", "vignette": true, "edges": true}),
        json!({"tiltShift": true, "grain": true}),
        json!({"brightness": 20, "contrast": 10, "saturation": -50, "sepia": 30}),
    ] {
        s.run("map.effects", &p).unwrap_or_else(|e| panic!("{p}: {e}"));
        assert!(png(&mut s) != plain, "{p}");
    }
    assert!(s.run("map.effects", &json!({"weather": "hail"})).is_err());
    s.run("map.effects", &json!({"reset": true})).unwrap();
    assert!(png(&mut s) == plain, "reset restores the map");
    s.run("map.colorVision", &json!({"mode": "deuteranopia"})).unwrap();
    assert!(png(&mut s) != plain);
    // Exports aren't simulated.
    let export = crate::cmd::share::map_png(&mut s, Some(300.0), Some(200.0), None).unwrap();
    s.run("map.colorVision", &json!({"mode": "normal"})).unwrap();
    assert!(export == crate::cmd::share::map_png(&mut s, Some(300.0), Some(200.0), None).unwrap());
    s.run("map.drawingMode", &json!({"mode": "wireframe"})).unwrap();
    assert!(png(&mut s) != plain);
    s.run("map.drawingMode", &json!({"mode": "standard"})).unwrap();
    s.run("layer.priority", &json!({"layer": "Countries", "value": 5})).unwrap();
    assert!(png(&mut s) != plain, "countries now draw over the cities");
    s.run("layer.priority", &json!({"layer": "Countries", "value": 0})).unwrap();
    s.run("map.clipping", &json!({"mode": "distance", "distance": 500_000})).unwrap();
    assert!(png(&mut s) != plain);
    s.run("map.clipping", &json!({"mode": "none"})).unwrap();
    s.run("map.displayType", &json!({"mode": "speed"})).unwrap();
    assert!(png(&mut s) != plain, "no antialiasing");
    s.run("map.displayType", &json!({"mode": "quality"})).unwrap();
    assert!(png(&mut s) == plain);
    // Views and navigation.
    s.run("map.viewMode", &json!({"mode": "local"})).unwrap();
    assert!(s.map().projection.code().starts_with("local:"));
    png(&mut s);
    s.run("map.viewMode", &json!({"mode": "global"})).unwrap();
    assert!(s.map().projection.is_globe());
    s.run("map.navigation", &json!({"mode": "planar"})).unwrap();
    assert!(!s.map().projection.is_globe(), "planar navigation is flat");
    s.run("map.navigation", &json!({"mode": "firstPerson"})).unwrap();
    let before = s.map().center_lonlat();
    s.run("map.walk", &json!({"forward": 1})).unwrap();
    assert!(s.map().center_lonlat().y > before.y, "walking forward goes north");
    let cam = s.run("map.camera", &json!({"lon": 2.35, "lat": 48.85, "scale": 1_000_000})).unwrap();
    assert!((cam["lon"].as_f64().unwrap() - 2.35).abs() < 1e-6);
    assert!(s.run("map.navigator", &Value::Null).unwrap()["navigator"].as_bool().unwrap());
}
