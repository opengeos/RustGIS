//! Regression tests for policy, credential-resolution, reach, handoff, format-version, and
//! feature-limit guards in `rustgis-engine`.

use super::*;
use crate::policy::{AgentAccess, Call, Effects, Outcome, Pending, Revocation, Scope};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Live(AtomicBool);
impl Revocation for Live {
    fn allows(&self, _: Effects) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

fn assistant_scope(id: u64) -> Scope {
    Scope::assistant(id, Arc::new(Live(AtomicBool::new(true))))
}

fn agent_scope(access: AgentAccess, label: &str) -> Scope {
    Scope::agent(access, false, label, None, Arc::new(Live(AtomicBool::new(true))))
}

fn pending(s: &Session, scope: &Scope, call: &Call) -> Pending {
    match s.review(scope, call) {
        Outcome::NeedsApproval(p) => p,
        Outcome::Refused(e) => panic!("refused instead of a review: {e}"),
        Outcome::Allowed => panic!("expected approval requirement"),
    }
}

/// A counting secret store for verifying that non-reserved scopes never read saved credentials.
#[cfg(not(target_arch = "wasm32"))]
struct CountingSecrets {
    inner: rustgis_data::secrets::MemorySecrets,
    loads: AtomicUsize,
}

#[cfg(not(target_arch = "wasm32"))]
impl CountingSecrets {
    fn new() -> Self {
        Self { inner: rustgis_data::secrets::MemorySecrets::new(), loads: AtomicUsize::new(0) }
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl rustgis_data::secrets::SecretStore for CountingSecrets {
    fn save(&self, account: &str, value: &rustgis_data::secrets::Secret) -> Result<(), rustgis_data::secrets::SecretError> {
        self.inner.save(account, value)
    }

    fn load(&self, account: &str) -> Result<Option<rustgis_data::secrets::Secret>, rustgis_data::secrets::SecretError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.inner.load(account)
    }

    fn delete(&self, account: &str) -> Result<(), rustgis_data::secrets::SecretError> {
        self.inner.delete(account)
    }
}

/// Fix 1: `s3.list` with a `bucket` must not attach a signed session S3 connection when the active
/// scope does not allow reserved authority (Assistant or Agent).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn s3_list_by_bucket_does_not_attach_reserved_credentials_for_agent_or_assistant() {
    if isolate_process_test("tests::engine_guards::s3_list_by_bucket_does_not_attach_reserved_credentials_for_agent_or_assistant") {
        return;
    }
    let (base, seen) = serve_s3();
    let bucket = "s3-test-bucket";
    let mut s = Session::new(Project::default());
    s.run(
        "s3.connect",
        &json!({
            "name": "signed-local",
            "endpoint": base,
            "buckets": [bucket],
            "accessKeyId": "fixture-id-0001",
            "secretAccessKey": "fixture-secret-0001"
        }),
    )
    .unwrap();

    // Baseline: User scope attaches the signed connection and reaches the local endpoint.
    let listed = s.run("s3.list", &json!({"bucket": bucket})).unwrap();
    assert_eq!(listed["bucket"], bucket);
    assert!(!seen.lock().unwrap().is_empty(), "user listing should reach the signed endpoint");
    seen.lock().unwrap().clear();

    // Agent (Allow) scope has Network but not Reserved: must not attach the signed connection.
    let call = Call::command("s3.list", json!({"bucket": bucket}));
    let agent = agent_scope(AgentAccess::Allow, "s3-agent");
    assert!(s.run_scoped(agent, &call).is_err(), "agent must not list through the signed session connection");
    assert!(seen.lock().unwrap().is_empty(), "agent listing must not reach the signed endpoint");

    // Assistant approved for Network (not Reserved) must also not attach the signed connection.
    let assistant = assistant_scope(29001);
    let p = pending(&s, &assistant, &call);
    assert!(s.run_permitted(assistant.clone(), p.allow(), &call).is_err(), "assistant must not list through the signed session connection");
    assert!(seen.lock().unwrap().is_empty(), "assistant listing must not reach the signed endpoint");

    // Implicit AWS connection (no custom endpoint): non-reserved scopes must wrap the list URL as
    // anonymous so transport `s3::prepare` / `signer_for` never signs it with the implicit bucket
    // credentials either.
    use crate::render::rustgis_tiles::s3;
    let aws_bucket = "aws-signed-guard-bucket";
    s.run(
        "s3.connect",
        &json!({
            "name": "signed-aws-implicit",
            "buckets": [aws_bucket],
            "accessKeyId": "fixture-aws-id",
            "secretAccessKey": "fixture-aws-secret"
        }),
    )
    .unwrap();
    let user_url = crate::cmd::s3::bucket_list_url(&s, aws_bucket, "", None, None).unwrap();
    let user_prep = s3::prepare(&user_url).unwrap();
    assert!(user_prep.signed, "user scope should sign implicit AWS bucket listing");

    let agent = agent_scope(AgentAccess::Allow, "s3-aws-agent");
    let agent_url = s.with_scope(agent, |s| crate::cmd::s3::bucket_list_url(s, aws_bucket, "", None, None)).unwrap().unwrap();
    assert!(s3::split_anonymous(&agent_url).1, "agent list URL must be marked anonymous: {agent_url}");
    let agent_prep = s3::prepare(&agent_url).unwrap();
    assert!(!agent_prep.signed && agent_prep.anonymous && agent_prep.headers.is_empty(), "agent list URL must not be signed");

    let asst_url = s.with_scope(assistant, |s| crate::cmd::s3::bucket_list_url(s, aws_bucket, "", None, None)).unwrap().unwrap();
    assert!(s3::split_anonymous(&asst_url).1, "assistant list URL must be marked anonymous: {asst_url}");
    let asst_prep = s3::prepare(&asst_url).unwrap();
    assert!(!asst_prep.signed && asst_prep.anonymous && asst_prep.headers.is_empty(), "assistant list URL must not be signed");

    let _ = s.run("s3.disconnect", &json!({"name": "signed-aws-implicit"}));
    let _ = s.run("s3.disconnect", &json!({"name": "signed-local"}));
}

/// Review fix: `open_peer_text(Peer::Embedder, ...)` runs under `Scope::peer()` and must never
/// load remembered database or S3 credentials from the secret store.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn peer_embedder_open_peer_text_never_loads_remembered_credentials() {
    if isolate_process_test("tests::engine_guards::peer_embedder_open_peer_text_never_loads_remembered_credentials") {
        return;
    }
    use rustgis_data::{Dataset, Field, FieldType, GeometryType};
    use rustgis_map::Layer;
    use rustgis_map::source::{PostgisOrigin, SourceLayer};

    let store = Arc::new(CountingSecrets::new());
    let mut setup = Session::new(Project::default());
    setup.database_logins.set_store(Some(store.clone()));

    let pg_addr = "postgresql://reader@127.0.0.1:1/embedder_db";
    let pg_id = setup.run("connection.new", &json!({"kind": "postgres", "name": "EmbedderPg", "address": pg_addr})).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    setup.run("connection.login", &json!({"id": pg_id, "password": "fixture-embedder-pw", "remember": true})).unwrap();

    let s3_id = setup
        .run(
            "connection.new",
            &json!({
                "kind": "s3",
                "name": "EmbedderS3",
                "address": "http://127.0.0.1:9",
                "options": {"credentials": "keys", "buckets": "embedder-bucket", "region": "us-east-1"}
            }),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    setup
        .run(
            "connection.login",
            &json!({
                "id": s3_id,
                "accessKeyId": "fixture-embedder-key",
                "secretAccessKey": "fixture-embedder-secret",
                "remember": true
            }),
        )
        .unwrap();
    let _ = setup.run("s3.disconnect", &json!({"name": "EmbedderS3"}));

    let definitions = setup.connections.to_json();
    crate::cmd::connections::clear_active();
    let mut s = Session::new(Project::default());
    s.database_logins.set_store(Some(store.clone()));
    s.connections = crate::connections::Connections::from_json(&definitions);

    let mut incoming = Project::default();
    let mut pg_layer = Layer::new(1, Dataset::new("EmbedderPgLayer", GeometryType::Point, vec![Field::new("id", FieldType::Integer)]));
    let mut pg_src = SourceLayer::default();
    pg_src.postgis = Some(PostgisOrigin {
        database: pg_addr.into(),
        table: "public.parcels".into(),
        crs: None,
        family: Some(rustgis_data::source::Family::Points),
    });
    pg_layer.source = Some(pg_src);
    incoming.map_mut().add_layer(pg_layer);

    let mut s3_layer = Layer::new(2, Dataset::new("EmbedderS3Layer", GeometryType::Point, vec![Field::new("id", FieldType::Integer)]));
    let mut s3_src = SourceLayer::default();
    s3_src.path = Some(rustgis_data::s3_address::bind("", "http://127.0.0.1:9", "s3://embedder-bucket/data.fgb").unwrap());
    s3_layer.source = Some(s3_src);
    incoming.map_mut().add_layer(s3_layer);

    let incoming_json = serde_json::to_string(&incoming).unwrap();
    let loads_before = store.loads();
    let report = s.open_peer_text(crate::Peer::Embedder, &incoming_json).unwrap();
    assert_eq!(report.reconnect, 1, "incoming database layer must require reconnect");
    assert_eq!(store.loads(), loads_before, "Peer::Embedder must not load remembered credentials");
    assert!(s.database_logins.activations().is_empty(), "Peer::Embedder must not activate DB logins");
    assert!(s.s3_binding(&s3_id).is_none(), "Peer::Embedder must not bind saved S3 logins");
}

/// Fix 2: `connection_logins::resolve` and `s3_connections::resolve_project_s3` must not resolve
/// saved keychain credentials when running under a non-reserved scope.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn non_reserved_scopes_do_not_auto_resolve_saved_keychain_logins() {
    if isolate_process_test("tests::engine_guards::non_reserved_scopes_do_not_auto_resolve_saved_keychain_logins") {
        return;
    }
    use crate::render::rustgis_tiles::s3;
    use rustgis_data::{Dataset, Field, FieldType, GeometryType};
    use rustgis_map::Layer;
    use rustgis_map::source::{PostgisOrigin, SourceLayer};

    let store = Arc::new(CountingSecrets::new());
    let mut s = Session::new(Project::default());
    s.database_logins.set_store(Some(store.clone()));

    // Set up a saved S3 connection with remembered credentials, then clear its runtime binding.
    let s3_id = s
        .run(
            "connection.new",
            &json!({
                "kind": "s3",
                "name": "SavedBucketS3",
                "address": "http://127.0.0.1:9",
                "options": {"credentials": "keys", "buckets": "guarded-bucket", "region": "us-east-1"}
            }),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.run(
        "connection.login",
        &json!({
            "id": s3_id,
            "accessKeyId": "fixture-s3-key",
            "secretAccessKey": "fixture-s3-secret",
            "remember": true
        }),
    )
    .unwrap();
    let _ = s.run("s3.disconnect", &json!({"name": "SavedBucketS3"}));

    // Set up a saved PostGIS connection with a remembered password.
    let pg_addr = "postgresql://reader@127.0.0.1:1/guarded_db";
    let pg_id = s.run("connection.new", &json!({"kind": "postgres", "name": "SavedPg", "address": pg_addr})).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.run("connection.login", &json!({"id": pg_id, "password": "fixture-pg-pw", "remember": true})).unwrap();

    // Open a cold session with the saved connection definitions and shared secret store so neither
    // connection has an in-memory session login yet.
    let definitions = s.connections.to_json();
    crate::cmd::connections::clear_active();
    let mut s = Session::new(Project::default());
    s.database_logins.set_store(Some(store.clone()));
    s.connections = crate::connections::Connections::from_json(&definitions);

    // Build a layer file referencing the saved S3 bucket.
    let mut s3_layer = Layer::new(1, Dataset::new("S3Guard", GeometryType::Point, vec![Field::new("id", FieldType::Integer)]));
    let mut s3_src = SourceLayer::default();
    s3_src.path = Some(rustgis_data::s3_address::bind("", "http://127.0.0.1:9", "s3://guarded-bucket/data.fgb").unwrap());
    s3_layer.source = Some(s3_src);
    let s3_lyr_text = serde_json::to_string(&s3_layer).unwrap();

    // Build a layer file referencing the saved PostGIS database.
    let mut pg_layer = Layer::new(2, Dataset::new("PgGuard", GeometryType::Point, vec![Field::new("id", FieldType::Integer)]));
    let mut pg_src = SourceLayer::default();
    pg_src.postgis = Some(PostgisOrigin {
        database: pg_addr.into(),
        table: "public.parcels".into(),
        crs: None,
        family: Some(rustgis_data::source::Family::Points),
    });
    pg_layer.source = Some(pg_src);
    let pg_lyr_text = serde_json::to_string(&pg_layer).unwrap();

    let loads_before = store.loads();
    let agent = agent_scope(AgentAccess::Allow, "import-agent");

    s.run_scoped(agent.clone(), &Call::command("layer.addLayerFile", json!({"text": s3_lyr_text}))).unwrap();
    assert_eq!(store.loads(), loads_before, "non-reserved scope must not load saved S3 credentials");
    assert!(s.s3_binding(&s3_id).is_none(), "non-reserved scope must not bind saved S3 credentials");

    s.run_scoped(agent, &Call::command("layer.addLayerFile", json!({"text": pg_lyr_text}))).unwrap();
    assert_eq!(store.loads(), loads_before, "non-reserved scope must not load saved DB credentials");
    let (_, pg_key) = crate::connections::database_key(pg_addr).unwrap();
    assert!(s.database_logins.activations().iter().all(|(k, _)| k != &pg_key), "DB credentials must not activate");

    // Sanity check: User scope does consult the saved credential store on both S3 and DB resolution.
    crate::s3_connections::resolve_project_s3(&mut s);
    let loads_after_s3 = store.loads();
    assert!(loads_after_s3 > loads_before, "user scope should resolve saved S3 credentials");
    crate::cmd::connections::resolve_project(&mut s);
    assert!(store.loads() > loads_after_s3, "user scope should resolve saved DB credentials");
    let _ = s3::disconnect("SavedBucketS3");
}

/// Fix 3: `Session::file_reach` returns `Reach::Public` whenever `plugin_held()` is true, even
/// when reading from a `path`, and plugin commands without `Effect::Network` remain held to public.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn held_plugin_file_reach_stays_public_even_for_path() {
    use crate::render::rustgis_tiles::{address::Reach, cache};
    use rustgis_data::{Dataset, Field, FieldType, GeometryType};
    use rustgis_map::Layer;
    use rustgis_map::source::SourceLayer;

    let mut s = Session::new(Project::default());
    isolate_plugins();
    let plug_id = "eng-file-reach-held";
    plugin(plug_id, &["edit", "files"]);

    // User scope and Agent scope with a path remain Reach::Local; Agent with text is Reach::Public.
    let path_p = json!({"path": "/tmp/example.rgislyr"});
    let text_p = json!({"text": "{}"});
    assert_eq!(s.file_reach(&path_p), Reach::Local);
    let agent = agent_scope(AgentAccess::Allow, "reach-agent");
    assert_eq!(s.with_scope(agent.clone(), |s| s.file_reach(&path_p)).unwrap(), Reach::Local);
    assert_eq!(s.with_scope(agent, |s| s.file_reach(&text_p)).unwrap(), Reach::Public);

    // Held plugin scope must return Reach::Public even when given a "path".
    let loaded = rustgis_plugins::get(plug_id).unwrap();
    let plug_reach = s.as_session_plugin(loaded, |s| s.file_reach(&path_p)).unwrap();
    assert_eq!(plug_reach, Reach::Public, "held plugin file_reach must be Reach::Public for path");

    // End-to-end: a held plugin importing a .rgislyr from disk holds its local URL to Reach::Public
    // and never connects to a local listener during the Effect::Files-only command.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let local_url = format!("http://{}/plugin-path-guard/data.fgb", listener.local_addr().unwrap());
    cache::allow_named(&local_url);

    let mut lyr = Layer::new(1, Dataset::new("LocalFromPath", GeometryType::Point, vec![Field::new("id", FieldType::Integer)]));
    let mut src = SourceLayer::default();
    src.path = Some(local_url.clone());
    lyr.source = Some(src);

    let dir = std::env::temp_dir().join(format!("rustgis-plugin-reach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lyr_path = dir.join("local.rgislyr");
    std::fs::write(&lyr_path, serde_json::to_vec(&lyr).unwrap()).unwrap();

    forward(&mut s, plug_id, "layer.addLayerFile", json!({"path": lyr_path.display().to_string()})).unwrap();
    assert_eq!(cache::ceiling_for(&local_url), Reach::Public, "URL named by plugin-opened path must be held to public");
    assert_eq!(
        listener.accept().map(|_| ()).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "local listener must not be contacted by plugin file import"
    );

    let _ = std::fs::remove_dir_all(&dir);
    rustgis_plugins::unload(plug_id);
}

/// Fix 4: `Kind::Peer` denies `Effect::Files`, `project.openText` skips identical-save secret
/// restore for non-user scopes, and `project.fromTemplate` declares its `text` document role.
#[test]
fn peer_scope_refuses_files_and_open_text_skips_restore_for_non_user_and_template_marks_text_role() {
    use rustgis_data::{Dataset, Field, FieldType, GeometryType};
    use rustgis_map::Layer;

    let mut s = Session::new(Project::default());

    // 1. Kind::Peer denies Effect::Files commands (project.open, project.fromTemplate),
    // while still permitting Effect::Replaces without Files (project.openText).
    assert!(matches!(s.review(&Scope::peer(), &Call::command("project.open", json!({"path": "never.rgis"}))), Outcome::Refused(_)));
    assert!(matches!(
        s.review(&Scope::peer(), &Call::command("project.fromTemplate", json!({"path": "never.rgistemplate"}))),
        Outcome::Refused(_)
    ));
    assert!(s.run_scoped(Scope::peer(), &Call::command("project.open", json!({"path": "never.rgis"}))).is_err());
    assert!(s.run_scoped(Scope::peer(), &Call::command("project.fromTemplate", json!({"path": "never.rgistemplate"}))).is_err());
    assert!(matches!(s.review(&Scope::peer(), &Call::command("project.openText", json!({"text": "{}"}))), Outcome::Allowed));

    // 2. project.openText must never trigger durable::restore for non-user scopes.
    let secret = "RGSYNTH_open_text_guard_secret";
    let mut ds = Dataset::new("SecretLayer", GeometryType::Point, vec![Field::new("id", FieldType::Integer)]);
    ds.source = Some(format!("https://guard.invalid/data.geojson?token={secret}"));
    let mut orig = Session::new(Project::default());
    orig.map_mut().add_layer(Layer::new(1, ds));
    let (saved, report) = orig.project.saved_text(None).unwrap();
    assert!(report.layer_addresses() > 0);
    assert!(!saved.contains(secret));

    // User scope restores from the identical-save cache.
    let mut by_user = Session::new(Project::default());
    by_user.run("project.openText", &json!({"text": saved})).unwrap();
    assert!(serde_json::to_string(&by_user.project).unwrap().contains(secret), "user scope baseline should restore live secret");

    // Non-user scopes (Agent and Peer) must never restore the live secret.
    let mut by_agent = Session::new(Project::default());
    let agent = agent_scope(AgentAccess::Allow, "open-text-agent");
    by_agent.run_scoped(agent, &Call::command("project.openText", json!({"text": saved}))).unwrap();
    assert!(!serde_json::to_string(&by_agent.project).unwrap().contains(secret), "agent scope project.openText must not restore secrets");

    let mut by_peer = Session::new(Project::default());
    by_peer.run_scoped(Scope::peer(), &Call::command("project.openText", json!({"text": saved}))).unwrap();
    assert!(!serde_json::to_string(&by_peer.project).unwrap().contains(secret), "peer scope project.openText must not restore secrets");

    // 3. project.fromTemplate declares ParamRole for "text" as Document::Project.
    let spec = s.registry.get("project.fromTemplate").unwrap();
    assert!(
        spec.roles.unwrap().contains(&crate::ParamRole { key: "text", role: crate::Role::Document(crate::Document::Project) }),
        "project.fromTemplate must declare text as Document::Project"
    );
}

/// Fix 5: Non-user scopes (Agent, Plugin) cannot consume internal `"ran"` handoffs from
/// `s.downloaded`, `s.trend_results`, or `s.sql_results`, leaving pending entries intact.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn non_user_scopes_cannot_consume_internal_ran_handoffs() {
    let mut s = Session::new(Project::default());
    isolate_plugins();
    let plug_id = "eng-ran-guard";
    plugin(plug_id, &["edit", "network", "files"]);

    let geojson = br##"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]},"properties":{"color":"#ff0000"}}]}"##.to_vec();
    let emb_id = 9101u64;
    let zarr_id = 9102u64;
    s.downloaded.push((emb_id, geojson));
    s.downloaded.push((zarr_id, vec![1, 2, 3, 4]));
    let trend_id = crate::cmd::cube::land(&mut s, Vec::new(), 4);
    let sql_id = 9103u64;
    let sql_out = rustgis_data::source::duckdb::Output::default();
    s.sql_results.push((sql_id, "SELECT 1".into(), None, sql_out));

    let agent = agent_scope(AgentAccess::Allow, "ran-agent");

    // embeddings.add with "ran"
    let emb_p = json!({"dataset": "earth-index", "ran": emb_id});
    let err = s.run_scoped(agent.clone(), &Call::command("embeddings.add", emb_p.clone())).unwrap_err();
    assert!(err.contains("reserved for the app"), "embeddings.add agent: {err}");
    let err = forward(&mut s, plug_id, "embeddings.add", emb_p).unwrap_err();
    assert!(err.contains("reserved for the app"), "embeddings.add plugin: {err}");
    assert!(s.downloaded.iter().any(|(d, _)| *d == emb_id), "embeddings downloaded entry must remain intact");

    // layer.addZarr with "ran"
    let zarr_p = json!({"path": "store.zarr", "ran": zarr_id});
    let err = s.run_scoped(agent.clone(), &Call::command("layer.addZarr", zarr_p.clone())).unwrap_err();
    assert!(err.contains("reserved for the app"), "layer.addZarr agent: {err}");
    let err = forward(&mut s, plug_id, "layer.addZarr", zarr_p).unwrap_err();
    assert!(err.contains("reserved for the app"), "layer.addZarr plugin: {err}");
    assert!(s.downloaded.iter().any(|(d, _)| *d == zarr_id), "zarr downloaded entry must remain intact");

    // cube.trend with "ran"
    let trend_p = json!({"ran": trend_id});
    let err = s.run_scoped(agent.clone(), &Call::command("cube.trend", trend_p.clone())).unwrap_err();
    assert!(err.contains("reserved for the app"), "cube.trend agent: {err}");
    let err = forward(&mut s, plug_id, "cube.trend", trend_p).unwrap_err();
    assert!(err.contains("reserved for the app"), "cube.trend plugin: {err}");
    assert!(s.trend_results.iter().any(|(k, ..)| *k == trend_id), "trend_results entry must remain intact");

    // layer.addSql with "ran"
    let sql_p = json!({"sql": "SELECT 1", "ran": sql_id});
    let err = s.run_scoped(agent, &Call::command("layer.addSql", sql_p.clone())).unwrap_err();
    assert!(err.contains("reserved for the app"), "layer.addSql agent: {err}");
    let err = forward(&mut s, plug_id, "layer.addSql", sql_p).unwrap_err();
    assert!(err.contains("reserved for the app"), "layer.addSql plugin: {err}");
    assert!(s.sql_results.iter().any(|(k, ..)| *k == sql_id), "sql_results entry must remain intact");

    rustgis_plugins::unload(plug_id);
}

/// Fix 6: `share.webMap` and `share.locator` emit a counted notice when features are omitted past
/// the 200,000-feature limit, and emit no limit notice for clean inputs below the limit.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn web_map_and_locator_notify_when_feature_limit_is_reached() {
    use rustgis_data::{Dataset, Feature, Field, FieldType, Geometry, GeometryType, Value as AttrValue};
    use rustgis_geom::Coord;
    use rustgis_map::Layer;

    let make_session = |count: usize| -> Session {
        let mut ds = Dataset::new("Places", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        ds.features.reserve(count);
        for i in 0..count {
            ds.features.push(Feature {
                id: i as u64 + 1,
                geometry: Some(Geometry::Point(Coord::new(0.0, 0.0))),
                attrs: vec![AttrValue::Text("p".into())],
            });
        }
        let mut s = Session::new(Project::default());
        s.map_mut().add_layer(Layer::new(1, ds));
        s
    };

    let dir = std::env::temp_dir().join(format!("rustgis-share-limit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Clean input below limit: no limit notice.
    let mut clean = make_session(3);
    let mark = clean.notice_mark();
    clean.run("share.webMap", &json!({"path": dir.join("clean.html").display().to_string()})).unwrap();
    assert!(
        clean.notices_since(mark).iter().all(|n| !n.text.contains("feature limit")),
        "clean webMap must not emit limit notice: {:?}",
        clean.notices_since(mark)
    );
    let mark = clean.notice_mark();
    clean.run("share.locator", &json!({"layer": "Places", "path": dir.join("clean.rgisloc").display().to_string()})).unwrap();
    assert!(
        clean.notices_since(mark).iter().all(|n| !n.text.contains("feature limit")),
        "clean locator must not emit limit notice: {:?}",
        clean.notices_since(mark)
    );

    // Input past MAX_FEATURES (200,000): emits one counted notice for each.
    let mut big = make_session(200_002);
    let mark = big.notice_mark();
    big.run("share.webMap", &json!({"path": dir.join("big.html").display().to_string()})).unwrap();
    let web_notices: Vec<String> = big.notices_since(mark).iter().map(|n| n.text.clone()).collect();
    assert!(
        web_notices.iter().any(|n| n.contains("2 feature(s) left out of the web map") && n.contains("200000")),
        "webMap must emit counted limit notice: {web_notices:?}"
    );

    let mark = big.notice_mark();
    big.run("share.locator", &json!({"layer": "Places", "path": dir.join("big.rgisloc").display().to_string()})).unwrap();
    let loc_notices: Vec<String> = big.notices_since(mark).iter().map(|n| n.text.clone()).collect();
    assert!(
        loc_notices.iter().any(|n| n.contains("2 feature(s) left out of the locator") && n.contains("200000")),
        "locator must emit counted limit notice: {loc_notices:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Fix 7: All five document readers refuse future `format` / unknown `requires`, and
/// `notebook.open` refuses future Jupyter `nbformat` major versions.
#[test]
fn future_format_and_nbformat_files_are_refused() {
    use base64::Engine;

    let mut s = Session::new(Project::default());
    let future_format = json!({"format": 99, "requires": ["future-feature"]}).to_string();
    let future_requires = json!({"format": 1, "requires": ["future-feature"]}).to_string();

    for payload in [&future_format, &future_requires] {
        for cmd in ["presentation.import", "report.open", "notebook.open", "bookmark.import"] {
            let err = s.run(cmd, &json!({"text": payload})).unwrap_err();
            assert!(
                err.contains("newer RustGIS") || err.contains("future-feature"),
                "{cmd} should refuse future format/requires, got: {err}"
            );
        }
        let dl_zip = rustgis_data::zip::write(&[("model.json".into(), payload.as_bytes().to_vec())]).unwrap();
        let dl_b64 = base64::engine::general_purpose::STANDARD.encode(dl_zip);
        let err = s.run("dl.import", &json!({"data": dl_b64})).unwrap_err();
        assert!(
            err.contains("newer RustGIS") || err.contains("future-feature"),
            "dl.import should refuse future format/requires, got: {err}"
        );
    }

    // Jupyter nbformat > 4 must be refused rather than silently downgraded to 4.
    let nb5 = json!({"nbformat": 5, "cells": []}).to_string();
    let err = s.run("notebook.open", &json!({"text": nb5})).unwrap_err();
    assert!(err.contains("nbformat 5"), "notebook.open must refuse nbformat 5, got: {err}");

    // Valid nbformat 4 still opens cleanly.
    let nb4 = json!({"nbformat": 4, "cells": []}).to_string();
    s.run("notebook.open", &json!({"text": nb4})).unwrap();
}

/// Fix 8: Generated-import refusal for `map.import` names "a map file", not "a GeoLibre project".
#[test]
fn map_import_generated_refusal_mentions_map_file() {
    let s = Session::new(Project::default());
    let assistant = assistant_scope(29008);

    match s.review(&assistant, &Call::command("map.import", json!({"path": "example.rgismap"}))) {
        Outcome::Refused(msg) => {
            assert!(msg.contains("a map file"), "expected map file message, got: {msg}");
            assert!(!msg.contains("GeoLibre"), "must not falsely mention GeoLibre: {msg}");
        }
        other => panic!("expected Refused for map.import, got: {}", outcome_name(&other)),
    }
    match s.review(&assistant, &Call::command("project.importQgis", json!({"path": "example.qgs"}))) {
        Outcome::Refused(msg) => assert!(msg.contains("QGIS"), "{msg}"),
        other => panic!("expected Refused for project.importQgis, got: {}", outcome_name(&other)),
    }
    match s.review(&assistant, &Call::command("project.importGeolibre", json!({"path": "example.geolibre.json"}))) {
        Outcome::Refused(msg) => assert!(msg.contains("GeoLibre"), "{msg}"),
        other => panic!("expected Refused for project.importGeolibre, got: {}", outcome_name(&other)),
    }
}

fn outcome_name(o: &Outcome) -> &'static str {
    match o {
        Outcome::Allowed => "Allowed",
        Outcome::NeedsApproval(_) => "NeedsApproval",
        Outcome::Refused(_) => "Refused",
    }
}
