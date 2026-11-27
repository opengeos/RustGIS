//! Import QGIS Project run off the window (#2476): in the desktop app the import runs on a copy of
//! the session, on a thread of its own, and the map it made is taken when it lands. The map taken is
//! the one the import makes when it runs here; a saved web login is the session's, borrowed and
//! left open; a cancelled import leaves nothing behind (its sweep is in `cancellation.rs`).

use super::{Project, Session, ogc_server};
use ogc_server::{Route, Server};
use rustgis_analysis::control::Control;
use serde_json::{Value, json};

/// A folder of its own for one test, removed when it ends.
struct Folder(std::path::PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rustgis-qgis-ahead-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A WFS of three roads.
fn roads_server() -> Server {
    let features: Vec<_> = [1, 2, 3]
        .iter()
        .map(|id| json!({"type": "Feature", "id": id, "properties": {"n": id}, "geometry": {"type": "Point", "coordinates": [*id, 1]}}))
        .collect();
    let page = json!({"type": "FeatureCollection", "features": features}).to_string();
    let query = "SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=roads";
    let format = "OUTPUTFORMAT=application%2Fjson&SRSNAME=EPSG:4326";
    let server = Server::new(vec![
        Route::new("/wfs?SERVICE=WFS&REQUEST=GetCapabilities", ""),
        Route::new(&format!("/wfs?{query}&{format}&COUNT=50000"), &page),
        Route::new(&format!("/wfs?{query}&SRSNAME=EPSG:4326&RESULTTYPE=hits"), r#"<FeatureCollection numberMatched="3"/>"#),
    ]);
    let base = server.base.clone();
    server.set(
        "/wfs?SERVICE=WFS&REQUEST=GetCapabilities",
        &format!(
            r#"<WFS_Capabilities version="2.0.0"><OperationsMetadata><Operation name="GetFeature"><DCP><HTTP><Get href="{base}/wfs?"/></HTTP></DCP></Operation></OperationsMetadata><FeatureTypeList><FeatureType><Name>roads</Name><Title>Roads</Title></FeatureType></FeatureTypeList></WFS_Capabilities>"#
        ),
    );
    server
}

/// A project in `dir` with a WFS layer at `base`, a GeoJSON file beside it, a CSV without geometry
/// (a table), a PostGIS table (a saved connection made for it), a group holding the file's layer,
/// and a bookmark.
fn project(dir: &Folder, base: &str) -> std::path::PathBuf {
    let towns = json!({"type": "FeatureCollection", "features": [
        {"type": "Feature", "properties": {"name": "A"}, "geometry": {"type": "Point", "coordinates": [8.5, 47.4]}},
        {"type": "Feature", "properties": {"name": "B"}, "geometry": {"type": "Point", "coordinates": [7.4, 46.9]}}]});
    std::fs::write(dir.0.join("towns.geojson"), towns.to_string()).unwrap();
    // Without a geometry field: a table of its own.
    std::fs::write(dir.0.join("notes.csv"), "id,note\n1,first\n2,second\n").unwrap();
    let layer = |id: &str, kind: &str, provider: &str, source: &str| {
        format!(
            r#"<maplayer type="{kind}"><id>{id}</id><datasource>{source}</datasource><layername>{id}</layername><provider>{provider}</provider></maplayer>"#
        )
    };
    let qgs = format!(
        r#"<qgis version="4.2.3" projectname="Ahead"><title>Ahead</title><layer-tree-group>
            <layer-tree-group name="Places" checked="Qt::Checked" expanded="1"><layer-tree-layer id="Towns" checked="Qt::Checked"/></layer-tree-group>
            <layer-tree-layer id="Roads" checked="Qt::Checked"/><layer-tree-layer id="Parcels" checked="Qt::Checked"/>
            <layer-tree-layer id="Notes" checked="Qt::Checked"/>
        </layer-tree-group><projectlayers>{}{}{}{}</projectlayers>
        <Bookmarks><Bookmark id="b" name="Zurich" group="" extent="POLYGON((8.4 47.3, 8.6 47.3, 8.6 47.5, 8.4 47.5, 8.4 47.3))"><spatialrefsys><authid>EPSG:4326</authid></spatialrefsys></Bookmark></Bookmarks></qgis>"#,
        layer("Towns", "vector", "ogr", "./towns.geojson"),
        layer("Roads", "vector", "WFS", &format!("typename='roads' url='{base}/wfs' version='2.0.0'")),
        layer("Notes", "vector", "delimitedtext", "file:./notes.csv?type=csv&amp;geomType=none"),
        layer(
            "Parcels",
            "vector",
            "postgres",
            "dbname='gis' host=db.example.invalid port=5432 user='reader' table=\"public\".\"parcels\" (geom)"
        ),
    );
    let path = dir.0.join("ahead.qgs");
    std::fs::write(&path, qgs).unwrap();
    path
}

/// `project.importQgis` with `p`, run as the desktop app runs it: on a copy of the session, on a
/// thread of its own (here, this one), and taken when it lands.
fn import_ahead(s: &mut Session, p: &Value, control: &std::sync::Arc<Control>) -> crate::CmdResult {
    let read = crate::cmd::reads::read_ahead(s, "project.importQgis", p)?;
    assert!(read.reads(), "the import runs on a copy");
    let done = read.run(control)?;
    crate::cmd::reads::read_land(s, done)
}

#[test]
fn an_import_run_on_a_copy_makes_the_map_an_import_here_makes() {
    let dir = Folder::new("same");
    let server = roads_server();
    let path = project(&dir, &server.base);
    let p = json!({"path": path.display().to_string()});
    let mut here = Session::new(Project::default());
    let answer_here = here.run("project.importQgis", &p).unwrap();
    let mut ahead = Session::new(Project::default());
    let answer_ahead = import_ahead(&mut ahead, &p, &Control::new()).unwrap();
    assert_eq!(answer_ahead, answer_here);
    assert_eq!(answer_ahead["layers"].as_array().map(Vec::len), Some(2), "{answer_ahead}");
    // The CSV without geometry, a table here as when the import runs here (which notes its layer as
    // not added).
    assert_eq!(ahead.project.tables.len(), 1);
    let tables = |s: &Session| s.project.tables.iter().map(|t| (t.id, t.dataset.name.clone())).collect::<Vec<_>>();
    assert_eq!(tables(&ahead), tables(&here));
    // The whole project as saved, its maps, layers, groups and bookmarks among it.
    assert!(super::cancellation::contents(&ahead.project) == super::cancellation::contents(&here.project));
    assert_eq!(ahead.project.active_map, here.project.active_map);
    assert_eq!(ahead.active_layer, here.active_layer);
    // The PostGIS table's saved connection, made here once the copy has landed, without a password.
    assert_eq!(ahead.connections.connections, here.connections.connections);
    assert_eq!(ahead.connections.connections.len(), 1);
    // What it said is said here, as when it runs here.
    let said = |s: &Session| s.notices.iter().map(|n| n.text.clone()).collect::<Vec<_>>();
    assert_eq!(said(&ahead), said(&here));
    // One Undo step takes the map away again.
    let maps = ahead.project.maps.len();
    ahead.run("edit.undo", &json!({})).unwrap();
    assert_eq!(ahead.project.maps.len(), maps - 1);
}

/// A map named as the project is named again when a map took the name while it was read, as a new
/// map is; saved connections changed meanwhile refuse it, to be imported again.
#[test]
fn an_import_on_a_copy_lands_on_the_session_as_it_is_then() {
    let dir = Folder::new("then");
    let server = roads_server();
    let path = project(&dir, &server.base);
    let p = json!({"path": path.display().to_string()});
    let mut s = Session::new(Project::default());
    let read = crate::cmd::reads::read_ahead(&mut s, "project.importQgis", &p).unwrap();
    let done = read.run(&Control::new()).unwrap();
    // (Renamed meanwhile: a map made or opened meanwhile is a change of map, which the read refuses.)
    s.map_mut().name = "Ahead".into();
    let answer = crate::cmd::reads::read_land(&mut s, done).unwrap();
    assert_eq!(answer["map"], "Ahead2");
    assert_eq!(s.map().name, "Ahead2");
    assert!(s.map().layer_by_name("Towns").is_some());
    let mut s = Session::new(Project::default());
    let read = crate::cmd::reads::read_ahead(&mut s, "project.importQgis", &p).unwrap();
    let done = read.run(&Control::new()).unwrap();
    s.run("connection.new", &json!({"kind": "postgres", "address": "postgres:host=db2.example.invalid dbname=other"})).unwrap();
    let maps = s.project.maps.len();
    let e = crate::cmd::reads::read_land(&mut s, done).unwrap_err();
    // (Refused when it lands: by the read, which sees the account changed, or by the import.)
    assert!(e.contains("changed"), "{e}");
    assert_eq!(s.project.maps.len(), maps);
    assert_eq!(s.connections.connections.len(), 1);
}

/// A service's saved login: the copy sends it as this session would, borrowing this session's
/// login, which stays open after the copy is gone; a login this session couldn't open isn't
/// opened by the copy either.
#[test]
fn an_import_on_a_copy_borrows_the_sessions_web_login() {
    let dir = Folder::new("login");
    let server = roads_server();
    let path = project(&dir, &server.base);
    let p = json!({"path": path.display().to_string()});
    let mut s = Session::new(Project::default());
    let id = s
        .run(
            "connection.new",
            &json!({"kind": "wfs", "name": "Roads", "address": format!("{}/wfs", server.base), "auth": "query", "keyName": "key"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.run("connection.login", &json!({"id": id, "apiKey": "RGSYNTH_ahead_key", "allowInsecure": true})).unwrap();
    let keyed =
        |server: &Server| server.requests().iter().filter(|r| r.query.iter().any(|(k, v)| k == "key" && v == "RGSYNTH_ahead_key")).count();
    // (The fixture answers only the addresses it was given, so the keyed ones go unanswered and the
    // layer is left out: what matters is what was sent.)
    import_ahead(&mut s, &p, &Control::new()).unwrap();
    let sent = keyed(&server);
    assert!(sent > 0, "the copy sent the session's login");
    // Still open here: a request now carries it, with no login asked for again.
    let c = s.connections.connections.iter().find(|c| c.id == id).unwrap().clone();
    assert!(crate::service_logins::logged_in(&s, &c));
    // Logged out, the session can't open it, and neither does the copy: the layer is left out,
    // saying why, and nothing is sent with the key.
    s.run("connection.logout", &json!({"id": id})).unwrap();
    let answer = import_ahead(&mut s, &p, &Control::new()).unwrap();
    let notes = serde_json::to_string(&answer["notes"]).unwrap();
    assert!(notes.contains(crate::service_logins::LOGGED_OUT), "{notes}");
    assert_eq!(keyed(&server), sent);
}

/// A login saved on this computer and not yet opened this session: opened for the import before
/// it starts, so the import lands (its accounts as they were when it started) and the copy sends
/// it.
#[test]
fn an_import_on_a_copy_opens_a_saved_login_before_it_starts() {
    use rustgis_data::secrets::MemorySecrets;
    let dir = Folder::new("saved");
    let server = roads_server();
    let path = project(&dir, &server.base);
    let p = json!({"path": path.display().to_string()});
    let mut s = Session::new(Project::default());
    s.database_logins.set_store(Some(std::sync::Arc::new(MemorySecrets::new())));
    let id = s
        .run(
            "connection.new",
            &json!({"kind": "wfs", "name": "Roads", "address": format!("{}/wfs", server.base), "auth": "query", "keyName": "key"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.run("connection.login", &json!({"id": id, "apiKey": "RGSYNTH_saved_key", "allowInsecure": true, "remember": true})).unwrap();
    // Saved, but not open this session.
    let c = s.connections.connections.iter().find(|c| c.id == id).unwrap().clone();
    crate::service_logins::deactivate(&mut s, &c, None);
    s.database_logins.clear_session_password(&id);
    assert!(!crate::service_logins::logged_in(&s, &c));
    import_ahead(&mut s, &p, &Control::new()).unwrap();
    assert!(server.requests().iter().any(|r| r.query.iter().any(|(k, v)| k == "key" && v == "RGSYNTH_saved_key")));
    assert!(crate::service_logins::logged_in(&s, &c), "opened here, and still open");
}

/// A borrowing copy opens no login, even one saved where it could read it: it sends only what its
/// session opened for it, and refuses the rest, saying why.
#[test]
fn a_borrowing_copy_opens_no_login_of_its_own() {
    use rustgis_data::secrets::MemorySecrets;
    let store = std::sync::Arc::new(MemorySecrets::new());
    let mut s = Session::new(Project::default());
    s.database_logins.set_store(Some(store.clone()));
    let origin = "https://borrowed.example.invalid";
    let id =
        s.run("connection.new", &json!({"kind": "wfs", "name": "B", "address": origin, "auth": "query", "keyName": "key"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
    s.run("connection.login", &json!({"id": id, "apiKey": "RGSYNTH_borrowed", "remember": true})).unwrap();
    let c = s.connections.connections.iter().find(|c| c.id == id).unwrap().clone();
    crate::service_logins::deactivate(&mut s, &c, None);
    s.database_logins.clear_session_password(&id);
    // Borrowed for no origin: the copy has none open, though the store it reaches holds one.
    let mut copy = Session::new(Project::default());
    copy.database_logins.set_store(Some(store));
    copy.connections = s.connections.clone();
    copy.web_logins = s.web_logins.borrowing(&Default::default(), [(origin.to_string(), "why".to_string())].into());
    let url = format!("{origin}/wfs");
    assert_eq!(crate::service_logins::request(&mut copy, None, &url).unwrap_err(), "why");
    assert!(!crate::service_logins::logged_in(&copy, &c));
}
