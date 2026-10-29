//! Logins of saved web-service connections: sent only to the connection's origin, confirmed for
//! plain http://, remembered only on request and only for the origin they were saved for, never
//! shown, and asked for when a project's layers need one. Each test uses hosts of its own: the
//! logins sent are the process's.
use super::*;
use crate::render::rustgis_tiles::logins;
use rustgis_data::secrets::{MemorySecrets, Secret, SecretStore};
use std::sync::Arc;

fn session(store: Option<Arc<dyn SecretStore>>) -> Session {
    let mut s = Session::new(Project::default());
    s.database_logins.set_store(store);
    s
}

fn new(s: &mut Session, p: Value) -> String {
    s.run("connection.new", &p).unwrap()["id"].as_str().unwrap().to_string()
}

fn header(url: &str) -> Option<(String, String)> {
    logins::apply(url).and_then(|a| a.header)
}

/// Everything the user, a project file, the preferences or the assistant can see.
fn shown(s: &mut Session) -> String {
    let listed = s.run("connection.list", &Value::Null).unwrap().to_string();
    let notices: Vec<&str> = s.notices.iter().map(|n| n.text.as_str()).collect();
    format!("{listed}\n{}\n{}\n{notices:?}\n{:?}", s.connections.to_json(), serde_json::to_string(&s.project).unwrap(), s.database_logins)
}

#[test]
fn web_login_goes_only_to_its_connections_origin() {
    let mut s = session(None);
    let id = new(
        &mut s,
        json!({"kind":"wms","name":"Scoped","address":"https://scope757.example.org/wms?service=WMS","auth":"header","keyName":"X-Api-Key"}),
    );
    assert_eq!(s.run("connection.list", &Value::Null).unwrap()["connections"][0]["loggedIn"], false);
    assert!(header("https://scope757.example.org/wms").is_none());
    let r = s.run("connection.login", &json!({"id":id,"apiKey":"scope-sentinel-757"})).unwrap();
    assert_eq!(r["loggedIn"], true);
    assert_eq!(header("https://scope757.example.org/wms?request=GetMap").unwrap(), ("X-Api-Key".into(), "scope-sentinel-757".into()));
    assert_eq!(header("https://SCOPE757.example.org:443/tiles/1/2/3.png").unwrap().1, "scope-sentinel-757");
    // Another host, port or scheme gets nothing.
    for other in [
        "https://other757.example.org/wms",
        "https://scope757.example.org.evil.example/wms",
        "https://scope757.example.org:8443/wms",
        "http://scope757.example.org/wms",
    ] {
        assert!(logins::apply(other).is_none(), "{other}");
    }
    // A layer added from the connection records it; one added later from its origin, through its login.
    s.run("map.addTiles", &json!({"url":"https://scope757.example.org/tiles/{z}/{x}/{y}.png","name":"Named","connection":id})).unwrap();
    s.run("map.addTiles", &json!({"url":"https://scope757.example.org/other/{z}/{x}/{y}.png","name":"Through login"})).unwrap();
    s.run("map.addTiles", &json!({"url":"https://elsewhere757.example.org/{z}/{x}/{y}.png","name":"Elsewhere"})).unwrap();
    let tile = |s: &Session, name: &str| s.map().tiles.iter().find(|t| t.name == name).unwrap().connection.clone();
    assert_eq!((tile(&s, "Named"), tile(&s, "Through login"), tile(&s, "Elsewhere")), (id.clone(), id.clone(), String::new()));
    // A connection that isn't a saved web service can't be named.
    for bad in [json!(7), json!("c999757"), json!(["c1"])] {
        assert!(s.run("map.addTiles", &json!({"url":"https://scope757.example.org/x/{z}/{x}/{y}.png","connection":bad})).is_err());
    }
    // A query key goes in the request only, and its name is taken out of addresses kept.
    let q = new(
        &mut s,
        json!({"kind":"stac","address":"https://query757.example.org/api?apikey2=pasted-key&x=1","auth":"query","keyName":"apikey2"}),
    );
    let c = s.connections.find(&q).unwrap().clone();
    assert_eq!(c.address, "https://query757.example.org/api?x=1");
    s.run("connection.login", &json!({"id":q,"apiKey":"query sentinel/757"})).unwrap();
    let applied = logins::apply("https://query757.example.org/api/collections").unwrap();
    assert!(applied.url.ends_with("apikey2=query%20sentinel%2F757") && applied.header.is_none(), "{}", applied.url);
    assert_eq!(crate::sources::redact("https://query757.example.org/a?apikey2=leak&b=2"), "https://query757.example.org/a?b=2");
    // Logging out ends it; the address goes without it.
    s.run("connection.logout", &json!({"id":id})).unwrap();
    assert!(header("https://scope757.example.org/wms").is_none());
    s.run("connection.remove", &json!({"id":q})).unwrap();
    assert!(logins::apply("https://query757.example.org/api").is_none());
    let text = shown(&mut s);
    assert!(!text.contains("scope-sentinel-757") && !text.contains("query sentinel") && !text.contains("query%20sentinel"), "{text}");
}

#[test]
fn web_login_methods_are_checked() {
    let mut s = session(None);
    for bad in [
        json!({"kind":"wms","address":"https://m757.example.org","auth":"digest"}),
        json!({"kind":"wms","address":"https://m757.example.org","auth":"basic"}),
        json!({"kind":"wms","address":"https://m757.example.org","auth":"basic","user":"a:b"}),
        json!({"kind":"wms","address":"https://m757.example.org","auth":"header","keyName":"Cookie"}),
        json!({"kind":"wms","address":"https://m757.example.org","auth":"header","keyName":"X Key"}),
        json!({"kind":"wms","address":"https://m757.example.org","auth":"query","keyName":""}),
        json!({"kind":"postgres","address":"postgres:host=127.0.0.1 port=1 user=u dbname=gis","auth":"basic","user":"u"}),
        json!({"kind":"s3","auth":"header","keyName":"X-Key"}),
    ] {
        assert!(s.run("connection.new", &bad).is_err(), "{bad}");
    }
    let id = new(&mut s, json!({"kind":"arcgis","address":"https://m757.example.org/server/rest/services","auth":"basic","user":"ana"}));
    for bad in [
        json!({"id":id,"apiKey":"basic-takes-a-password"}),
        json!({"id":id,"password":"bad\nline"}),
        json!({"id":id,"password":"x".repeat(5000)}),
        json!({"id":id,"password":"ok-secret","allowInsecure":"yes"}),
        json!({"id":id}),
    ] {
        let e = s.run("connection.login", &bad).unwrap_err();
        assert!(!e.contains("basic-takes-a-password") && !e.contains("bad\nline") && !e.contains("ok-secret"), "{e}");
    }
    assert!(header("https://m757.example.org/server/rest/services").is_none());
    s.run("connection.login", &json!({"id":id,"password":"pw-757"})).unwrap();
    assert_eq!(header("https://m757.example.org/server/rest/services").unwrap(), ("Authorization".into(), logins::basic("ana", "pw-757")));
    // A connection without a method doesn't log in with one; it can be chosen for its address.
    let plain = new(&mut s, json!({"kind":"wms","address":"https://plain757.example.org/wms"}));
    assert!(s.run("connection.login", &json!({"id":plain,"apiKey":"k"})).is_err());
    assert!(s.run("connection.login", &json!({"id":plain})).is_ok());
    assert!(
        s.run("connection.test", &json!({"kind":"wms","address":"https://u757.example.org","auth":"header","keyName":"X-K","apiKey":"k"}))
            .is_err()
    );
}

#[test]
fn plain_http_needs_confirmation_before_a_login_is_sent() {
    let mut s = session(Some(Arc::new(MemorySecrets::new())));
    let id = new(&mut s, json!({"kind":"wfs","address":"http://insecure757.example.org/wfs","auth":"basic","user":"ana"}));
    assert_eq!(s.run("connection.list", &Value::Null).unwrap()["connections"][0]["insecure"], true);
    let e = s.run("connection.login", &json!({"id":id,"password":"http-sentinel-757","remember":true})).unwrap_err();
    assert_eq!(e, crate::service_logins::INSECURE);
    assert!(header("http://insecure757.example.org/wfs").is_none());
    assert!(!s.connections.find(&id).unwrap().remembered && !s.database_logins.has_session_password(&id));
    s.run("connection.login", &json!({"id":id,"password":"http-sentinel-757","allowInsecure":true,"remember":true})).unwrap();
    assert!(header("http://insecure757.example.org/wfs?request=GetFeature").is_some());
    assert!(header("https://insecure757.example.org/wfs").is_none());
    // Confirmed when typed: logging in again with the saved login needs no new confirmation.
    s.run("connection.logout", &json!({"id":id})).unwrap();
    assert!(header("http://insecure757.example.org/wfs").is_none());
    s.run("connection.login", &json!({"id":id})).unwrap();
    assert!(header("http://insecure757.example.org/wfs").is_some());
    s.run("connection.remove", &json!({"id":id})).unwrap();
}

#[test]
fn remembered_web_login_lifetime_and_origin_binding() {
    let store = Arc::new(MemorySecrets::new());
    let mut s = session(Some(store.clone()));
    let id = new(
        &mut s,
        json!({"kind":"ogcapi","name":"Kept","address":"https://kept757.example.org/ogc","auth":"header","keyName":"X-Api-Key"}),
    );
    let account = format!("connection/{id}");
    // Session-only by default.
    s.run("connection.login", &json!({"id":id,"apiKey":"kept-sentinel-757"})).unwrap();
    assert_eq!(store.load(&account).unwrap(), None);
    assert!(!s.connections.find(&id).unwrap().remembered);
    let r = s.run("connection.login", &json!({"id":id,"apiKey":"kept-sentinel-757","remember":true})).unwrap();
    assert_eq!((r["remembered"].as_bool(), r.get("warning")), (Some(true), None));
    let envelope: Value = serde_json::from_str(store.load(&account).unwrap().unwrap().expose()).unwrap();
    assert_eq!(
        envelope,
        json!({"origin":"https://kept757.example.org","auth":"header","user":"","keyName":"X-Api-Key","secret":"kept-sentinel-757"})
    );
    assert!(!s.connections.to_json().contains("kept-sentinel"));
    // A later session (the preferences reloaded) logs in with it, without typing.
    let prefs = s.connections.to_json();
    let mut later = session(Some(store.clone()));
    later.connections = crate::connections::Connections::from_json(&prefs);
    s.run("connection.logout", &json!({"id":id})).unwrap();
    assert!(header("https://kept757.example.org/ogc").is_none());
    later.run("connection.login", &json!({"id":id})).unwrap();
    assert_eq!(header("https://kept757.example.org/ogc").unwrap().1, "kept-sentinel-757");
    // Forgetting it keeps this session's login.
    later.run("connection.forgetLogin", &json!({"id":id})).unwrap();
    assert_eq!(store.load(&account).unwrap(), None);
    assert!(!later.connections.find(&id).unwrap().remembered);
    assert!(header("https://kept757.example.org/ogc").is_some());
    // A renamed connection keeps it; another origin or key name ends it and deletes the saved one.
    later.run("connection.login", &json!({"id":id,"apiKey":"kept-sentinel-757","remember":true})).unwrap();
    later.run("connection.edit", &json!({"id":id,"name":"Renamed","address":"https://kept757.example.org/ogc/v2"})).unwrap();
    assert!(later.connections.find(&id).unwrap().remembered && header("https://kept757.example.org/x").is_some());
    later.run("connection.edit", &json!({"id":id,"address":"https://kept757.example.org:8443/ogc"})).unwrap();
    assert!(!later.connections.find(&id).unwrap().remembered);
    assert_eq!(store.load(&account).unwrap(), None);
    assert!(header("https://kept757.example.org/x").is_none() && header("https://kept757.example.org:8443/x").is_none());
    assert_eq!(later.run("connection.login", &json!({"id":id})).unwrap_err(), crate::service_logins::REQUIRED);

    // An envelope saved for another origin (an address changed outside the app) is refused.
    let moved = r#"{"version":1,"next":5,"connections":[{"id":"c4","name":"Moved","kind":"wms","address":"https://moved757.example.org/wms","auth":"header","keyName":"X-Api-Key","remembered":true}]}"#;
    store
        .save(
            "connection/c4",
            &Secret::new(r#"{"origin":"https://attacker757.example.org","auth":"header","user":"","keyName":"X-Api-Key","secret":"moved-sentinel-757"}"#),
        )
        .unwrap();
    let mut third = session(Some(store.clone()));
    third.connections = crate::connections::Connections::from_json(moved);
    let e = third.run("connection.login", &json!({"id":"c4"})).unwrap_err();
    assert_eq!(e, crate::service_logins::UNUSABLE);
    assert!(header("https://moved757.example.org/wms").is_none() && header("https://attacker757.example.org/wms").is_none());
    // Nothing saved at all: the marker goes.
    store.delete("connection/c4").unwrap();
    assert_eq!(third.run("connection.login", &json!({"id":"c4"})).unwrap_err(), crate::service_logins::NONE_SAVED);
    assert!(!third.connections.find("c4").unwrap().remembered);
    // Without a store (the browser), a login lasts for the session and says it wasn't kept.
    let mut web = session(None);
    let w = new(&mut web, json!({"kind":"wms","address":"https://session757.example.org/wms","auth":"query","keyName":"key"}));
    let r = web.run("connection.login", &json!({"id":w,"apiKey":"session-sentinel-757","remember":true})).unwrap();
    assert_eq!(r["warning"], crate::service_logins::NOT_REMEMBERED);
    assert!(logins::apply("https://session757.example.org/wms").is_some());
    web.run("connection.remove", &json!({"id":w})).unwrap();
    let mut all = shown(&mut s);
    all.push_str(&shown(&mut later));
    all.push_str(&shown(&mut third));
    all.push_str(&shown(&mut web));
    for secret in ["kept-sentinel-757", "moved-sentinel-757", "session-sentinel-757"] {
        assert!(!all.contains(secret), "{secret}");
    }
    // What a recording or the History would keep is refused: the parameters hold a secret.
    assert!(crate::sources::holds_secret(&json!({"id":id,"apiKey":"kept-sentinel-757"})));
    assert!(crate::sources::holds_secret(&json!({"id":id,"password":"kept-sentinel-757"})));
    later.run("connection.remove", &json!({"id":id})).unwrap();
}

/// A project whose tile layer came from connection `id` of `origin`.
fn project(origin: &str, id: &str) -> String {
    let mut p = Project::default();
    let mut t = rustgis_map::TileLayer::new("Secured", rustgis_map::TileKind::Raster, format!("{origin}/tiles/{{z}}/{{x}}/{{y}}.png"));
    t.connection = id.into();
    p.map_mut().tiles.push(t);
    serde_json::to_string(&p).unwrap()
}

fn requests(s: &mut Session) -> Value {
    s.run("connection.list", &Value::Null).unwrap()["loginRequests"].clone()
}

#[test]
fn a_project_opened_without_its_connection_or_login_asks_for_one() {
    let origin = "https://open757.example.org";
    let text = project(origin, "c7");
    assert!(text.contains(r#""connection":"c7""#) && !text.contains("open-sentinel"));
    // No such connection on this computer: asked to make one, not failing silently.
    let mut s = session(None);
    s.run("project.openText", &json!({"text": text})).unwrap();
    let r = requests(&mut s);
    assert_eq!((r[0]["reason"].as_str(), r[0]["origin"].as_str(), r[0]["layers"].as_u64()), (Some("connection"), Some(origin), Some(1)));
    assert!(s.notices.iter().any(|n| n.text.contains(origin)));
    // The connection made, but no login yet: asked for its password.
    let id = new(&mut s, json!({"kind":"wmts","address":format!("{origin}/wmts"),"auth":"header","keyName":"X-Key"}));
    let r = requests(&mut s);
    assert_eq!((r[0]["reason"].as_str(), &r[0]["connections"]), (Some("password"), &json!([id])));
    s.run("connection.login", &json!({"id":id,"apiKey":"open-sentinel-757"})).unwrap();
    assert_eq!(requests(&mut s), json!([]));
    assert!(header(&format!("{origin}/tiles/1/0/0.png")).is_some());
    // Reopened, the project's recorded connection (another id here) still matches the only one.
    s.run("project.openText", &json!({"text": project(origin, &id)})).unwrap();
    assert_eq!(requests(&mut s), json!([]));
    s.run("connection.remove", &json!({"id":id})).unwrap();
}
#[test]
fn a_sessions_logins_are_its_own_and_end_with_it() {
    let origin = "https://sessions757.example.org";
    let mut a = session(None);
    let mut b = session(None);
    // Both sessions' first connection is c1, for the same address.
    let id = new(&mut a, json!({"kind":"wms","address":format!("{origin}/wms"),"auth":"header","keyName":"X-Key"}));
    assert_eq!(new(&mut b, json!({"kind":"wms","address":format!("{origin}/wms"),"auth":"header","keyName":"X-Key"})), id);
    a.run("connection.login", &json!({"id":id,"apiKey":"session-a-757"})).unwrap();
    // B doesn't see A's login as its own, and can't end it.
    assert_eq!(b.run("connection.list", &Value::Null).unwrap()["connections"][0]["loggedIn"], false);
    b.run("connection.logout", &json!({"id":id})).unwrap();
    b.run("connection.remove", &json!({"id":id})).unwrap();
    assert_eq!(header(&format!("{origin}/wms")).unwrap().1, "session-a-757");
    drop(b);
    assert!(header(&format!("{origin}/wms")).is_some());
    assert_eq!(a.run("connection.list", &Value::Null).unwrap()["connections"][0]["loggedIn"], true);
    // The session that made it ends it.
    drop(a);
    assert!(header(&format!("{origin}/wms")).is_none());
}

#[test]
fn an_imported_group_reopened_then_refreshed_logs_in_with_its_connection() {
    let origin = "https://group757.example.org";
    let mut s = session(None);
    let id = new(&mut s, json!({"kind":"ogcapi","address":format!("{origin}/ogc"),"auth":"basic","user":"ana"}));
    let mut p = Project::default();
    let mut l = rustgis_map::Layer::new(41, rustgis_data::Dataset::new("Roads", rustgis_data::GeometryType::None, Vec::new()));
    l.connection = id.clone();
    p.map_mut().layers.push(l);
    p.map_mut().service_groups.push(rustgis_map::ServiceGroup {
        id: 5,
        name: "Imported".into(),
        url: format!("{origin}/ogc/collections"),
        kind: "ogcapi".into(),
        expanded: true,
        children: vec![rustgis_map::ServiceChild {
            layer: 41,
            collection: "roads".into(),
            source_url: format!("{origin}/ogc/collections/roads"),
        }],
    });
    let text = serde_json::to_string(&p).unwrap();
    // Reopened without a login: its connection's is asked for.
    s.run("project.openText", &json!({"text": text})).unwrap();
    let r = requests(&mut s);
    assert_eq!((r[0]["reason"].as_str(), &r[0]["connections"], r[0]["layers"].as_u64()), (Some("password"), &json!([id]), Some(1)));
    s.run("connection.login", &json!({"id":id,"password":"group-sentinel-757"})).unwrap();
    assert!(header(&format!("{origin}/ogc/collections/roads/items")).is_some());
    // A later refresh (its answer given, so nothing is fetched here) logs in again first.
    s.run("connection.logout", &json!({"id":id})).unwrap();
    s.database_logins.set_session_password(&id, "group-sentinel-757");
    let group = s.map().service_groups[0].id;
    let _ = s.run("service.refresh", &json!({"group": group, "text": "not a document"}));
    assert_eq!(header(&format!("{origin}/ogc/collections/roads/items")).unwrap().1, logins::basic("ana", "group-sentinel-757"));
    s.run("connection.remove", &json!({"id":id})).unwrap();
}

#[test]
fn several_connections_for_an_address_are_never_picked_silently() {
    let origin = "https://two757.example.org";
    let mut s = session(None);
    let a = new(&mut s, json!({"kind":"wms","name":"A","address":format!("{origin}/wms"),"auth":"header","keyName":"X-Key"}));
    let b = new(&mut s, json!({"kind":"wms","name":"B","address":format!("{origin}/wms?map=b")}));
    s.run("connection.login", &json!({"id":a,"apiKey":"a-sentinel-757"})).unwrap();
    // A project recording neither: several match, so the user picks (B has no login, still a match).
    s.run("connection.logout", &json!({"id":a})).unwrap();
    s.database_logins.set_session_password(&a, "a-sentinel-757");
    let mut p = Project::default();
    p.map_mut().tiles.push(rustgis_map::TileLayer::new("Plain", rustgis_map::TileKind::Raster, format!("{origin}/{{z}}/{{x}}/{{y}}.png")));
    s.run("project.openText", &json!({"text": serde_json::to_string(&p).unwrap()})).unwrap();
    let r = requests(&mut s);
    assert_eq!((r[0]["reason"].as_str(), &r[0]["connections"]), (Some("choose"), &json!([a, b])));
    assert!(header(&format!("{origin}/1/0/0.png")).is_none());
    // Recording one of them is a choice: it logs in with that one's session login.
    s.run("project.openText", &json!({"text": project(origin, &a)})).unwrap();
    assert_eq!(requests(&mut s), json!([]));
    assert_eq!(header(&format!("{origin}/tiles/1/0/0.png")).unwrap().1, "a-sentinel-757");
    // Choosing B (no login) for the address sends none.
    s.run("connection.login", &json!({"id":b})).unwrap();
    assert!(header(&format!("{origin}/tiles/1/0/0.png")).is_none());
    s.run("connection.remove", &json!({"id":a})).unwrap();
}

#[test]
fn junk_login_parameters_never_panic() {
    let mut s = session(Some(Arc::new(MemorySecrets::new())));
    let id = new(&mut s, json!({"kind":"csw","address":"https://junk757.example.org/csw","auth":"query","keyName":"k"}));
    let junk = [
        json!(null),
        json!([]),
        json!({"id": id, "apiKey": 7, "password": {"a": 1}, "remember": [], "allowInsecure": null}),
        json!({"id": id, "auth": "\u{0}", "user": "\u{feff}", "keyName": "x".repeat(10_000)}),
        json!({"id": id, "apiKey": "\u{7f}", "allowInsecure": true}),
        json!({"id": [id], "connection": {"id": id}}),
    ];
    for cmd in [
        "connection.new",
        "connection.edit",
        "connection.login",
        "connection.logout",
        "connection.forgetLogin",
        "connection.test",
        "map.addTiles",
        "service.add",
    ] {
        for p in &junk {
            let _ = s.run(cmd, p);
        }
    }
    s.run("connection.remove", &json!({"id":id})).unwrap();
}
