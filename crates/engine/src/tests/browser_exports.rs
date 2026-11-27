//! Exports by command where files are downloaded: the browser has no filesystem to write a path
//! in, so a script's, the Assistant's or a plugin's export is offered as a download instead.

use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value as Cell};
use serde_json::{Value, json};

use super::write_notices::Files;
use crate::{CmdResult, Project, Session};

/// A point layer, a standalone table and a raster, a layout and a report. A field name longer
/// than a Shapefile holds and a NaN, which GeoJSON can't, give exports something to say.
/// `browser` is whether files are downloaded.
fn session(files: &Files, browser: bool) -> Session {
    let mut s = Session::new(Project::default());
    let fields = vec![Field::new("long_attribute_name", FieldType::Text), Field::new("score", FieldType::Double)];
    let mut ds = Dataset::new("Places", GeometryType::Point, fields.clone());
    ds.crs = rustgis_data::raster::Crs::from_epsg(4326);
    ds.push(Some(Geometry::Point(rustgis_data::Coord::new(1.0, 2.0))), vec![Cell::Text("kept".into()), Cell::Float(f64::NAN)]);
    s.map_mut().add_dataset(ds);
    let mut totals = Dataset::new("Totals", GeometryType::None, fields);
    totals.push(None, vec![Cell::Text("a".into()), Cell::Float(1.5)]);
    s.project.add_table(totals);
    let grid = files.path().join("dem.asc");
    std::fs::write(&grid, "ncols 2\nnrows 2\nxllcorner 0\nyllcorner 0\ncellsize 1\n1 2\n3 4\n").unwrap();
    s.run("layer.addData", &json!({"path": grid, "name": "Dem"})).unwrap();
    s.run("layout.new", &json!({"name": "Page", "template": "blank"})).unwrap();
    s.run("report.new", &json!({"layer": "Places", "name": "List"})).unwrap();
    s.queue_gp_files = browser;
    s
}

/// `params` writing to `path`.
fn to(params: &Value, path: &str) -> Value {
    let mut p = params.clone();
    p["path"] = json!(path);
    p
}

/// The notices said since `mark`.
fn said(s: &Session, mark: u64) -> Vec<String> {
    s.notices_since(mark).iter().map(|n| n.text.clone()).collect()
}

/// Takes the one queued download and acknowledges it as the app does: its name, its bytes and
/// the acknowledgment's answer.
fn deliver(s: &mut Session, pending: &Value, error: Option<&str>) -> (String, Vec<u8>, CmdResult) {
    assert_eq!(pending["pending"], true, "{pending}");
    let mut files = s.take_gp_files();
    assert_eq!(files.len(), 1, "{pending}");
    let (id, name, bytes) = files.remove(0);
    assert_eq!(pending["download"].as_u64(), Some(id));
    let answer = s.acknowledge_gp_file(id, error);
    (name, bytes, answer)
}

/// Each export command the browser can run downloads the file the desktop writes, under the
/// path's name, and says of it what the desktop says, once the download is taken.
#[test]
fn a_browser_export_by_path_downloads_the_desktops_file_and_says_the_same() {
    // Command, other parameters, file name, and whether two writes of it give the same bytes.
    let cases = [
        ("share.exportTable", json!({"table": "Totals"}), "totals.csv", true),
        ("share.exportTable", json!({"table": "Totals", "limit": 0}), "totals.geojson", true),
        ("share.exportLayer", json!({"layer": "Places"}), "z.geojson", true),
        ("share.exportLayer", json!({"layer": "Places"}), "z.csv", true),
        ("share.exportLayer", json!({"layer": "Places"}), "z.zip", true),
        ("share.exportLayer", json!({"layer": "Places", "crs": "EPSG:3857"}), "z.parquet", true),
        ("share.exportLayer", json!({"layer": "Places"}), "z.kml", true),
        ("share.exportLayer", json!({"layer": "Places"}), "z.kmz", true),
        ("share.exportLayer", json!({"layer": "Places"}), "z.gpx", true),
        ("data.convert", json!({"layer": "Places", "format": "wkt"}), "z.txt", true),
        ("data.convert", json!({"layer": "Places", "format": "csv"}), "z.csv", true),
        ("share.exportRaster", json!({"layer": "Dem"}), "dem.tif", true),
        ("share.exportRaster", json!({"layer": "Dem", "cog": true}), "cog.tif", true),
        ("share.exportMap", json!({"width": 64, "height": 64}), "map.png", true),
        ("share.exportMap", json!({"width": 64, "height": 64}), "map.tif", true),
        ("share.exportMap", json!({"width": 64, "height": 64, "cog": true}), "mapcog.tif", true),
        ("layout.export", json!({"layout": "Page", "dpi": 72}), "page.pdf", false),
        ("report.export", json!({"report": "List", "dpi": 72}), "list.pdf", false),
    ];
    // A new export is listed here, or among those refused.
    let refused = ["map.exportOrbit", "map.exportBlock", "source.publish", "offline.package"];
    for (command, ..) in crate::cmd::exports::IN_BACKGROUND {
        assert!(cases.iter().any(|c| c.0 == *command) || refused.contains(command), "{command}");
    }
    for (command, params, file, same_bytes) in cases {
        let files = Files::new();
        let mut desktop = session(&files, false);
        let path = files.path().join(file);
        let path = path.to_str().unwrap();
        let mark = desktop.notice_mark();
        let wrote = desktop.run(command, &to(&params, path)).unwrap_or_else(|e| panic!("{command} {file}: {e}"));
        let written = std::fs::read(path).unwrap();
        let folder = format!("{}{}", files.path().display(), std::path::MAIN_SEPARATOR);
        let desktop_said: Vec<String> = said(&desktop, mark).into_iter().map(|n| n.replace(&folder, "")).collect();
        // A layer's export says it was exported, and what its format couldn't hold.
        if command == "share.exportLayer" {
            assert!(desktop_said.contains(&format!("Exported {file}")), "{file}: {desktop_said:?}");
        }
        if file == "z.geojson" {
            assert!(desktop_said.iter().any(|n| n.contains("written as null")), "{desktop_said:?}");
        }
        if file == "z.zip" {
            assert!(desktop_said.iter().any(|n| n.contains("long_attribute_name")), "{desktop_said:?}");
        }

        let mut browser = session(&files, true);
        std::fs::remove_file(path).unwrap();
        let (mark, steps) = (browser.notice_mark(), browser.undo_depth());
        let pending = browser.run(command, &to(&params, file)).unwrap_or_else(|e| panic!("{command} {file}: {e}"));
        assert_eq!(said(&browser, mark), Vec::<String>::new(), "{command} {file}: nothing is said before the download");
        let (name, bytes, answer) = deliver(&mut browser, &pending, None);
        assert_eq!(name, file);
        if same_bytes {
            assert!(bytes == written, "{command} {file}: {} bytes downloaded, {} written", bytes.len(), written.len());
        } else {
            assert_eq!(bytes.get(..4), written.get(..4), "{command} {file}");
        }
        assert_eq!(said(&browser, mark), desktop_said, "{command} {file}");
        let answer = answer.unwrap();
        assert_eq!((&answer["delivered"], &answer["file"]), (&json!(true), &json!(file)), "{answer}");
        for (key, value) in wrote.as_object().unwrap() {
            if key != "path" && (same_bytes || key != "bytes") {
                assert_eq!(&answer[key], value, "{command} {file}: {key}");
            }
        }
        assert_eq!(answer["path"], file);
        assert!(!std::path::Path::new(file).exists(), "{command} {file}: no file is written");
        assert!(browser.project.gp_history.is_empty(), "an export isn't a tool run");
        assert_eq!(browser.undo_depth(), steps, "{command} {file}");
    }
}

/// The reported script: each line's export is downloaded, its losses told to a caller that reads
/// only the answer.
#[test]
fn a_scripts_browser_exports_are_pending_downloads() {
    let files = Files::new();
    let mut s = session(&files, true);
    let script = "share.exportTable {\"table\": \"Totals\", \"path\": \"totals.csv\"}\n\
                  share.exportLayer {\"layer\": \"Places\", \"path\": \"z.geojson\"}";
    let mark = s.notice_mark();
    let ran = s.run("script.run", &json!({"script": script})).unwrap();
    let results: Vec<&Value> = ran["ran"].as_array().unwrap().iter().map(|r| &r["result"]).collect();
    assert_eq!(results.iter().map(|r| &r["file"]).collect::<Vec<_>>(), ["totals.csv", "z.geojson"]);
    let messages = results[1]["messages"].to_string();
    assert!(messages.contains("written as null") && !messages.contains("Exported"), "{messages}");
    assert_eq!(s.take_gp_files().len(), 2);
    assert_eq!(said(&s, mark), Vec::<String>::new());
}

/// What the download's name can't keep of the path is said, once the download is taken; a plain
/// file name says neither.
#[test]
fn a_browser_export_says_when_its_download_is_named_otherwise() {
    let files = Files::new();
    let mut s = session(&files, true);
    for (command, path, file, zipped, folder) in [
        ("share.exportLayer", "z.geojson", "z.geojson", false, false),
        ("share.exportLayer", "z.zip", "z.zip", false, false),
        ("share.exportLayer", "Roads.SHP", "Roads.zip", true, false),
        ("share.exportLayer", "/home/me/out/rows.shp", "rows.zip", true, true),
        ("share.exportTable", "out\\rows.csv", "rows.csv", false, true),
    ] {
        let params = json!({"layer": "Places", "table": "Totals", "path": path});
        let mark = s.notice_mark();
        let pending = s.run(command, &params).unwrap();
        let (name, bytes, _) = deliver(&mut s, &pending, None);
        assert_eq!(name, file, "{path}");
        assert_eq!(bytes.starts_with(b"PK"), file.ends_with(".zip"), "{path}");
        let notices = said(&s, mark);
        let zip_note = format!("{file}: a Shapefile is several files, so the browser downloads them in one .zip");
        let folder_note = format!("{file}: the browser chooses where downloads go, so the folder in '{path}' wasn't used");
        assert_eq!(notices.contains(&zip_note), zipped, "{path}: {notices:?}");
        assert_eq!(notices.contains(&folder_note), folder, "{path}: {notices:?}");
    }
}

/// What the browser can't write is refused before anything is made, saying how to export instead.
#[test]
fn what_the_browser_cant_write_is_refused_up_front() {
    let files = Files::new();
    let mut s = session(&files, true);
    for (command, path, why) in [
        ("share.exportLayer", "z.gpkg", "can't write GeoPackage or FlatGeobuf files; export to .geojson"),
        ("share.exportLayer", "z.FGB", "can't write GeoPackage or FlatGeobuf files; export to .geojson"),
        ("share.exportTable", "rows.gpkg", "can't write GeoPackage or FlatGeobuf files; export to .geojson"),
        ("share.exportLayer", "out/", "this one has none"),
        ("share.exportMap", "..", "this one has none"),
        ("offline.package", "site.rgisfield", "use Package for the Field on the ribbon"),
    ] {
        let areas = s.project.offline_areas.len();
        let e = s.run(command, &json!({"layer": "Places", "table": "Totals", "name": "Site", "path": path})).unwrap_err();
        assert!(e.contains(why), "{command} {path}: {e}");
        assert_eq!(s.project.offline_areas.len(), areas);
        assert!(s.take_gp_files().is_empty());
    }
}

/// A download the browser refuses is an error, and nothing says the file was exported.
#[test]
fn a_browser_export_not_taken_is_not_said_to_be_exported() {
    let files = Files::new();
    let mut s = session(&files, true);
    let pending = s.run("share.exportLayer", &json!({"layer": "Places", "path": "z.geojson"})).unwrap();
    let mark = s.notice_mark();
    let (_, _, answer) = deliver(&mut s, &pending, Some("download blocked"));
    assert_eq!(answer.unwrap()["delivered"], false);
    let notices = s.notices_since(mark);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].error && notices[0].text == "download blocked", "{notices:?}");
    assert!(s.project.gp_history.is_empty());
}

/// A queued download dropped by a later failure, or by another project replacing its own, is
/// said: the desktop would have written the file by then. A run that keeps it says nothing.
#[test]
fn a_browser_export_dropped_before_its_download_is_said() {
    let export = "share.exportTable {\"table\": \"Totals\", \"path\": \"totals.csv\"}";
    for (then, why) in [
        ("map.zoomFull", None),
        ("no.suchCommand", Some("as the command that made them failed")),
        ("project.new", Some("as another project replaced theirs first")),
    ] {
        let files = Files::new();
        let mut s = session(&files, true);
        let mark = s.notice_mark();
        let ran = s.run("script.run", &json!({"script": format!("{export}\n{then}")}));
        assert_eq!(ran.is_ok(), then != "no.suchCommand", "{ran:?}");
        let dropped: Vec<String> = said(&s, mark).into_iter().filter(|n| n.contains("not downloaded")).collect();
        let expected = why.map(|why| format!("1 export(s) not downloaded, {why}: totals.csv"));
        assert_eq!(dropped, expected.into_iter().collect::<Vec<_>>(), "{then}");
        assert_eq!(s.take_gp_files().len(), usize::from(why.is_none()), "{then}");
    }
    // Replaced by itself, between the export and the frame that would offer it.
    let files = Files::new();
    let mut s = session(&files, true);
    s.run("share.exportTable", &json!({"table": "Totals", "path": "totals.csv"})).unwrap();
    let mark = s.notice_mark();
    s.run("project.new", &json!({})).unwrap();
    assert!(said(&s, mark).iter().any(|n| n == "1 export(s) not downloaded, as another project replaced theirs first: totals.csv"));
}
