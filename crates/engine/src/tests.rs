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
    assert!(done >= 69, "coverage fell to {done}");
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

#[cfg(feature = "geolibre")]
#[test]
fn geolibre_tools_run_on_layers_and_add_their_outputs() {
    let mut s = with_raster();
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
