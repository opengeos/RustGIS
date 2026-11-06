use super::common::{Fixture, session};
use rustgis_engine::{
    Session,
    map::TileLayer,
    render::{
        self,
        rustgis_tiles::{cache, mbtiles, pmtiles, source},
    },
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};

fn picture(color: [u8; 4]) -> Vec<u8> {
    let mut p = tiny_skia::Pixmap::new(256, 256).unwrap();
    p.fill(tiny_skia::Color::from_rgba8(color[0], color[1], color[2], color[3]));
    p.encode_png().unwrap()
}

fn write_archive(path: &Path, color: [u8; 4], credit: &str) {
    let png = picture(color);
    if path.extension().unwrap() == "pmtiles" {
        let a = pmtiles::NewArchive {
            tile_type: pmtiles::TileType::Png,
            gzipped_tiles: false,
            bounds: [-180.0, -85.0, 180.0, 85.0],
            metadata: json!({"attribution": credit}),
        };
        std::fs::write(path, pmtiles::write(vec![(0, 0, 0, png)], &a).unwrap()).unwrap();
    } else {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS metadata (name TEXT, value TEXT); CREATE TABLE IF NOT EXISTS tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB); BEGIN IMMEDIATE; DELETE FROM metadata; DELETE FROM tiles;").unwrap();
        c.execute("INSERT INTO metadata VALUES ('format','png'),('minzoom','0'),('maxzoom','0'),('attribution',?1)", [credit]).unwrap();
        c.execute("INSERT INTO tiles VALUES (0,0,0,?1)", [png]).unwrap();
        c.execute_batch("COMMIT").unwrap();
    }
}

fn tile_url(t: &TileLayer) -> String {
    let template = if t.kind == rustgis_engine::map::TileKind::MbTiles { mbtiles::template(&t.url) } else { pmtiles::template(&t.url) };
    template.replace("{z}", "0").replace("{x}", "0").replace("{y}", "0")
}

fn decoded(t: &TileLayer) -> Arc<cache::Payload> {
    let url = tile_url(t);
    for _ in 0..100 {
        match cache::get_tile(&url, cache::Want::Image) {
            cache::State::Ready(p) => return p,
            cache::State::Failed(e) => panic!("{e}"),
            cache::State::Pending => cache::wait(Duration::from_millis(100)),
        }
    }
    panic!("tile did not arrive: {url}");
}

fn style(t: &TileLayer) -> Arc<source::Resolved> {
    for _ in 0..100 {
        if t.kind == rustgis_engine::map::TileKind::MbTiles {
            // MBTiles metadata workers have their own bounded wait, not cache::in_flight.
            let _ = mbtiles::info_ready(&t.url, Duration::from_millis(100)).unwrap();
        }
        match source::resolve(&render::tile_spec(t)) {
            source::Resolve::Ready(r) => return r,
            source::Resolve::Failed(e) => panic!("{e}"),
            source::Resolve::Pending => cache::wait(Duration::from_millis(100)),
        }
    }
    panic!("style did not resolve");
}

fn assert_color(p: &cache::Payload, expected: [u8; 4]) {
    let cache::Payload::Image(p) = p else { panic!("not an image") };
    assert_eq!(p.data().get(..4), Some(expected.as_slice()));
}

fn assert_render(s: &mut Session, expected: [u8; 4], credit: &str) {
    let v = render::View { width: 300.0, height: 200.0, pixels_per_point: 1.0, camera: s.map().camera, sky: false };
    for _ in 0..100 {
        let r = render::render(s.map_mut(), &v).unwrap();
        if !r.tiles_pending {
            assert_eq!(r.pixmap.pixel(150, 100).unwrap().red(), expected[0]);
            assert_eq!(r.pixmap.pixel(150, 100).unwrap().green(), expected[1]);
            assert_eq!(r.pixmap.pixel(150, 100).unwrap().blue(), expected[2]);
            assert!(r.attribution.iter().any(|a| a.contains(credit)), "{:?}", r.attribution);
            return;
        }
        cache::wait(Duration::from_millis(100));
    }
    panic!("render remained pending");
}

#[test]
fn manual_archive_reload_reads_new_pixels_and_credits_without_touching_other_sources() {
    for ext in ["pmtiles", "mbtiles"] {
        let f = Fixture::new(&format!("reload-{ext}"));
        let path = f.file(&format!("linked.{ext}"));
        let other = f.file(&format!("unrelated.{ext}"));
        write_archive(&path, [255, 0, 0, 255], "Old credits");
        write_archive(&other, [0, 0, 255, 255], "Unrelated credits");
        let mut s = session();
        s.run("map.addTiles", &json!({"url": other, "name":"Other", "overlay":true})).unwrap();
        s.run("map.addTiles", &json!({"url": path, "name":"Linked", "overlay":true})).unwrap();
        s.run("map.goToXY", &json!({"lon":10,"lat":20,"scale":50_000_000})).unwrap();
        let i = s.map().tiles.iter().position(|t| t.name == "Linked").unwrap();
        s.map_mut().tiles[i].id = "linked-archive".into();
        s.map_mut().tiles[i].blend = rustgis_engine::map::effects::Blend::Normal;
        s.map_mut().tiles[i].expanded = true;
        let before = s.map().tiles.clone();
        let camera = s.map().camera;
        let other_tile = before.iter().find(|t| t.name == "Other").unwrap();
        let other_pixels = decoded(other_tile);
        let other_style = style(other_tile);
        let old_pixels = decoded(&before[i]);
        assert_color(&old_pixels, [255, 0, 0, 255]);
        assert!(style(&before[i]).attribution.iter().any(|a| a.contains("Old credits")));
        assert_render(&mut s, [255, 0, 0, 255], "Old credits");
        write_archive(&path, [0, 255, 0, 255], "New credits");
        // Existing retained data stays until the explicit command.
        assert!(Arc::ptr_eq(&decoded(&before[i]), &old_pixels));
        assert_eq!(s.run("map.reloadTiles", &json!({"id":"linked-archive"})).unwrap(), json!({"index":i,"path":path}));
        assert_eq!(s.map().tiles, before);
        assert_eq!(s.map().camera, camera);
        assert_color(&decoded(&before[i]), [0, 255, 0, 255]);
        let fresh = style(&before[i]);
        assert!(fresh.attribution.iter().any(|a| a.contains("New credits")));
        assert!(!fresh.attribution.iter().any(|a| a.contains("Old credits")));
        assert!(Arc::ptr_eq(&decoded(other_tile), &other_pixels));
        assert!(Arc::ptr_eq(&style(other_tile), &other_style));
        assert_render(&mut s, [0, 255, 0, 255], "New credits");
        assert_eq!(s.undo_label(), Some("Reload Data"));
        s.run("edit.undo", &Value::Null).unwrap();
        assert_eq!(s.map().tiles, before);
        assert_color(&decoded(&before[i]), [0, 255, 0, 255]); // Undo is not file recovery.
        s.run("edit.redo", &Value::Null).unwrap();
        assert_eq!(s.map().camera, camera);
        // All three selector forms, including case-insensitive names.
        s.run("map.reloadTiles", &json!({"name":"lInKeD"})).unwrap();
        s.run("map.reloadTiles", &json!({"index":i})).unwrap();
        rustgis_engine::data::mbtiles::close(&path);
        rustgis_engine::data::mbtiles::close(&other);
    }
}

fn rejected(s: &mut Session, params: Value, expected: &str) {
    let before = s.map().tiles.clone();
    let undo = s.undo_label();
    let mark = s.notice_mark();
    let error = s.run("map.reloadTiles", &params).unwrap_err();
    assert!(error.contains(expected), "{error}");
    assert_eq!(s.map().tiles, before);
    assert_eq!(s.undo_label(), undo);
    assert!(s.notices_since(mark).iter().any(|n| n.error && n.text.contains(expected)));
}

#[test]
fn failed_archive_reload_retains_tiles_configuration_and_allows_retry() {
    for ext in ["pmtiles", "mbtiles"] {
        let f = Fixture::new(&format!("reload-failure-{ext}"));
        let path = f.file(&format!("linked.{ext}"));
        write_archive(&path, [255, 0, 0, 255], "Retained credits");
        let mut s = session();
        s.run("map.addTiles", &json!({"url":path})).unwrap();
        let before = s.map().tiles[0].clone();
        let pixels = decoded(&before);
        let old_style = style(&before);
        // No live read is held by this test while changing the file on Windows.
        rustgis_engine::data::mbtiles::close(&path);
        std::fs::remove_file(&path).unwrap();
        rejected(&mut s, json!({"index":0}), "retry Reload Data");
        std::fs::write(&path, b"truncated").unwrap();
        rejected(&mut s, json!({"index":0}), "retry Reload Data");
        assert!(Arc::ptr_eq(&decoded(&before), &pixels));
        assert!(Arc::ptr_eq(&style(&before), &old_style));
        write_archive_after_truncation(&path, [0, 255, 0, 255], "Recovered credits");
        s.run("map.reloadTiles", &json!({"index":0})).unwrap();
        assert_color(&decoded(&before), [0, 255, 0, 255]);
        assert_eq!(s.map().tiles[0], before);
        rustgis_engine::data::mbtiles::close(&path);
    }
}

fn write_archive_after_truncation(path: &Path, color: [u8; 4], credit: &str) {
    std::fs::remove_file(path).unwrap();
    write_archive(path, color, credit);
}

#[test]
fn locked_mbtiles_reload_fails_without_evicting_retained_data_then_retries() {
    let f = Fixture::new("reload-locked");
    let path = f.file("locked.mbtiles");
    write_archive(&path, [255, 0, 0, 255], "Before lock");
    let mut s = session();
    s.run("map.addTiles", &json!({"url":path})).unwrap();
    let before = s.map().tiles[0].clone();
    let pixels = decoded(&before);
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("BEGIN EXCLUSIVE; UPDATE metadata SET value='After lock' WHERE name='attribution';").unwrap();
    rejected(&mut s, json!({"index":0}), "retry Reload Data");
    assert!(Arc::ptr_eq(&decoded(&before), &pixels));
    c.execute_batch("COMMIT").unwrap();
    s.run("map.reloadTiles", &json!({"index":0})).unwrap();
    assert!(style(&before).attribution.iter().any(|a| a.contains("After lock")));
    rustgis_engine::data::mbtiles::close(&path);
}

// Unix permits replacing an open archive's directory entry. Windows may refuse the rename
// while SQLite holds the original file; this test does not promise otherwise.
#[cfg(unix)]
#[test]
fn replacing_an_open_mbtiles_archive_reloads_the_new_file_not_its_pooled_inode() {
    let f = Fixture::new("reload-mbtiles-replaced-inode");
    let path = f.file("linked.mbtiles");
    let replacement = f.file("replacement.mbtiles");
    write_archive(&path, [255, 0, 0, 255], "Old inode");
    write_archive(&replacement, [0, 255, 0, 255], "Replacement inode");
    let mut s = session();
    s.run("map.addTiles", &json!({"url":path})).unwrap();
    let before = s.map().tiles[0].clone();
    assert_color(&decoded(&before), [255, 0, 0, 255]);
    assert!(style(&before).attribution.iter().any(|a| a.contains("Old inode")));
    std::fs::rename(&replacement, &path).unwrap();
    s.run("map.reloadTiles", &json!({"index":0})).unwrap();
    assert_eq!(s.map().tiles[0], before);
    assert_color(&decoded(&before), [0, 255, 0, 255]);
    assert!(style(&before).attribution.iter().any(|a| a.contains("Replacement inode")));
    rustgis_engine::data::mbtiles::close(&path);
}

#[test]
fn incompatible_reload_keeps_archive_presentation_and_undo_restores_prior_error() {
    for ext in ["pmtiles", "mbtiles"] {
        let f = Fixture::new(&format!("reload-incompatible-{ext}"));
        let path = f.file(&format!("vector.{ext}"));
        write_vector(&path);
        let mut s = session();
        s.run("map.addTiles", &json!({"url":path})).unwrap();
        s.map_mut().tiles[0].layers = vec!["places".into()];
        s.map_mut().tiles[0].style_layers =
            r##"[{"id":"places","type":"circle","source-layer":"places","paint":{"circle-color":"#2266aa"}}]"##.into();
        s.map_mut().tiles[0].hidden_layers = vec!["roads".into()];
        s.map_mut().tiles[0].opacity = 0.4;
        s.map_mut().tiles[0].visible = false;
        s.map_mut().tiles[0].blend = rustgis_engine::map::effects::Blend::Multiply;
        s.map_mut().tiles[0].id = "presentation".into();
        s.map_mut().tiles[0].error = Some("old unavailable archive".into());
        let before = s.map().tiles[0].clone();
        write_archive(&path, [0, 255, 0, 255], "Raster now");
        rejected(&mut s, json!({"index":0}), "raster tiles");
        write_vector(&path);
        s.run("map.reloadTiles", &json!({"index":0})).unwrap();
        let mut expected = before.clone();
        expected.error = None;
        assert_eq!(s.map().tiles[0], expected);
        s.run("edit.undo", &Value::Null).unwrap();
        assert_eq!(s.map().tiles[0], before);
        rustgis_engine::data::mbtiles::close(&path);
    }
}

fn write_vector(path: &Path) {
    if path.extension().unwrap() == "pmtiles" {
        super::common::archive_file(path);
    } else {
        write_archive(path, [255, 0, 0, 255], "Vector");
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute("UPDATE metadata SET value='pbf' WHERE name='format'", []).unwrap();
        let tile = rustgis_engine::render::rustgis_tiles::mvt::encode(&rustgis_engine::render::rustgis_tiles::mvt::Tile { layers: vec![] });
        c.execute("UPDATE tiles SET tile_data=?1", [tile]).unwrap();
    }
}

#[test]
fn archive_reload_rejects_remote_upload_and_invalid_selectors() {
    let mut s = session();
    s.run("map.addTiles", &json!({"url":"https://example.invalid/a.pmtiles"})).unwrap();
    rejected(&mut s, json!({"index":0}), "linked local PMTiles or MBTiles");
    // Uploaded archives are snapshots, not live paths.
    s.map_mut().tiles[0] = TileLayer::new("Snapshot", rustgis_engine::map::TileKind::MbTiles, "upload:local.mbtiles");
    rejected(&mut s, json!({"index":0}), "add snapshots/uploads again");
    for (p, expected) in [
        (json!({}), "required"),
        (json!({"index":-1}), "no tile layer"),
        (json!({"index":0.5}), "no tile layer"),
        (json!({"index":8}), "no tile layer"),
        (json!({"name":"absent"}), "no tile layer"),
        (json!({"id":"absent"}), "no tile layer"),
    ] {
        rejected(&mut s, p, expected);
    }
}
