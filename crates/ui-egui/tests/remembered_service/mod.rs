use super::*;
use egui::accesskit::Role;
use egui_kittest::kittest::NodeT;
use rustgis_engine::connections::{LoginReason, LoginRequest, WEB};
use rustgis_engine::data::secrets::{MemorySecrets, SecretError, SecretStore};
use rustgis_ui_egui::Services;
use std::sync::Arc;

type Harness = egui_kittest::Harness<'static, RustGisApp>;

fn with_store(store: Option<Arc<dyn SecretStore>>) -> Harness {
    with_app(RustGisApp::with_sample().with_services(Services { secrets: store, ..Default::default() }))
}

fn with_app(app: RustGisApp) -> Harness {
    egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
}
fn settle(h: &mut Harness) {
    h.run_steps(4);
}
fn type_field(h: &mut Harness, label: &str, text: &str, secret: bool) {
    h.get_by_role_and_label(if secret { Role::PasswordInput } else { Role::TextInput }, label).focus();
    h.step();
    h.event(egui::Event::Text(text.into()));
    settle(h);
}
fn header_form(h: &mut Harness, address: &str) {
    h.state_mut().run("ui.connection", json!({"kind":"wms"})).unwrap();
    settle(h);
    type_field(h, "Name", "Saved web login", false);
    type_field(h, "Address", address, false);
    h.get_by_role_and_label(Role::ComboBox, "Method").click();
    settle(h);
    h.get_by_label("API key in a header").click();
    settle(h);
    type_field(h, "Header name", "X-Api-Key", false);
    type_field(h, "Key", "web-widget-secret-sentinel", true);
    assert!(!h.state().ui.connection.remember);
}
fn assert_redacted(h: &Harness) {
    let app = h.state();
    let secret = "web-widget-secret-sentinel";
    assert!(!app.session.connections.to_json().to_string().contains(secret));
    assert!(!app.session.notices.iter().any(|n| n.text.contains(secret)));
    assert!(app.ui.connection.api_key.is_empty());
    assert!(app.ui.connection.password.is_empty());
    assert!(!app.ui.connection.remember);
    assert!(!app.ui.connection.allow_insecure);
    assert!(!app.dialogs.error.as_deref().unwrap_or("").contains(secret));
    assert!(!app.dialogs.info.as_deref().unwrap_or("").contains(secret));
    assert!(!app.ui.connection.warned.as_deref().unwrap_or("").contains(secret));
}

#[test]
fn remembered_service_key_save_redaction_status_forget_and_logout() {
    let store = Arc::new(MemorySecrets::new());
    let mut h = with_store(Some(store.clone()));
    header_form(&mut h, "https://saved-web-widget.example/wms");
    h.get_by_label("Remember on this computer").click();
    settle(&mut h);
    h.get_by_role_and_label(Role::Button, "Save").click();
    settle(&mut h);
    assert_eq!(h.state().dialogs.open, None);
    let c = h.state().session.connections.find("Saved web login").unwrap().clone();
    assert!(c.remembered);
    let saved = store.load(&format!("connection/{}", c.id)).unwrap().unwrap();
    assert!(saved.expose().contains("web-widget-secret-sentinel"));
    assert_redacted(&h);
    h.state_mut().run("ui.connection", json!({"id":c.id})).unwrap();
    settle(&mut h);
    h.get_by_label("Logged in");
    h.get_by_label("Saved on this computer");
    h.get_by_role_and_label(Role::Button, "Forget Saved Login").click();
    settle(&mut h);
    assert!(!h.state().session.connections.find(&c.id).unwrap().remembered);
    assert!(store.load(&format!("connection/{}", c.id)).unwrap().is_none());
    assert!(rustgis_engine::service_logins::logged_in(&h.state().session, &c));
    h.get_by_role_and_label(Role::Button, "Log Out").click();
    settle(&mut h);
    assert!(!rustgis_engine::service_logins::logged_in(&h.state().session, &c));
}

#[test]
fn remembered_service_failed_store_warns_without_retaining_secret() {
    let mut h = with_store(Some(Arc::new(MemorySecrets::failing(SecretError::Denied))));
    header_form(&mut h, "https://warning-web-widget.example/wms");
    h.get_by_label("Remember on this computer").click();
    settle(&mut h);
    h.get_by_role_and_label(Role::Button, "Save").click();
    settle(&mut h);
    assert_eq!(h.state().dialogs.open, Some("editConnection"));
    h.get_by_label("Logged in for this session");
    assert!(h.state().ui.connection.warned.is_some());
    assert!(!h.state().session.connections.find("Saved web login").unwrap().remembered);
    assert_redacted(&h);
    h.get_by_role_and_label(Role::Button, "Close").click();
    settle(&mut h);
    assert_eq!(h.state().dialogs.open, None);
}

#[test]
fn service_secret_validation_precedes_saving_the_definition() {
    let mut h = with_store(None);
    header_form(&mut h, "https://refused-web-widget.example/wms");
    let invalid = "web-widget-secret-sentinel".repeat(200);
    h.state_mut().ui.connection.api_key = invalid.clone();
    h.get_by_role_and_label(Role::Button, "Save").click();
    settle(&mut h);
    assert_eq!(h.state().dialogs.open, Some("newConnection"));
    assert!(h.state().dialogs.error.is_some());
    assert!(h.state().ui.connection.id.is_none());
    assert!(h.state().session.connections.connections.is_empty());
    assert_eq!(h.state().ui.connection.api_key, invalid, "unsent input remains editable");
}

#[test]
fn service_http_needs_confirmation_and_unsaved_login_cannot_be_tested() {
    let store = Arc::new(MemorySecrets::new());
    let mut h = with_store(Some(store.clone()));
    header_form(&mut h, "http://insecure-web-widget.example/wms");
    assert!(h.get_by_role_and_label(Role::Button, "Test").accesskit_node().is_disabled());
    assert!(h.state().session.connections.connections.is_empty());
    h.get_by_role_and_label(Role::Button, "Save").click();
    settle(&mut h);
    assert!(h.state().session.connections.connections.is_empty());
    assert!(h.state().dialogs.error.as_deref().unwrap().contains("unencrypted"));
    h.get_by_label("Send the login over unencrypted http://").click();
    settle(&mut h);
    h.get_by_role_and_label(Role::Button, "Save").click();
    settle(&mut h);
    assert_eq!(h.state().dialogs.open, None);
    assert!(!h.state().session.connections.find("Saved web login").unwrap().remembered);
    assert!(store.load("connection/c1").unwrap().is_none());
    assert_redacted(&h);
}

#[test]
fn edited_service_test_button_is_disabled_and_retains_the_unsent_key() {
    let mut h = with_store(None);
    let c = h
        .state_mut()
        .run(
            "connection.new",
            json!({
                "kind":"wms", "address":"https://unchanged-widget.example/wms", "auth":"header", "keyName":"X-Api-Key"
            }),
        )
        .unwrap();
    h.state_mut().run("ui.connection", json!({"id":c["id"]})).unwrap();
    settle(&mut h);
    assert!(!h.get_by_role_and_label(Role::Button, "Test").accesskit_node().is_disabled());
    h.state_mut().ui.connection.address = "https://edited-widget.example/wms".into();
    h.state_mut().ui.connection.api_key = "unsent-widget-key".into();
    settle(&mut h);
    assert!(h.get_by_role_and_label(Role::Button, "Test").accesskit_node().is_disabled());
    assert_eq!(h.state().ui.connection.api_key, "unsent-widget-key");
    assert!(h.state().ui.connection.test.is_none());
    assert!(!h.state().session.database_logins.has_session_password(c["id"].as_str().unwrap()));
}

#[test]
fn remembered_service_recovers_through_fresh_shell_services_and_login_form() {
    let store = Arc::new(MemorySecrets::new());
    let directory = std::env::temp_dir().join(format!("rustgis-web-login-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let services = Services {
        secrets: Some(store.clone()),
        recovery: Some(Box::new(rustgis_ui_egui::FolderStore::open(&directory).unwrap())),
        ..Default::default()
    };
    let mut app = RustGisApp::with_sample().with_services(services);
    app.start(&Default::default()).unwrap();
    let mut first = with_app(app);
    header_form(&mut first, "https://fresh-widget.example/wms");
    first.get_by_label("Remember on this computer").click();
    settle(&mut first);
    first.get_by_role_and_label(Role::Button, "Save").click();
    settle(&mut first);
    let saved = first.state().session.connections.find("Saved web login").unwrap().clone();
    let definitions = first.state().session.connections.to_json().to_string();
    first.state_mut().run("layer.template", json!({"template":"points", "name":"Recovered web work"})).unwrap();
    settle(&mut first);
    first.state().services.recovery.as_ref().unwrap().flush();
    drop(first);
    let mut app = RustGisApp::with_sample().with_services(Services {
        secrets: Some(store),
        load_pref: Some(Box::new(move |key| (key == rustgis_engine::connections::PREF).then(|| definitions.clone()))),
        recovery: Some(Box::new(rustgis_ui_egui::FolderStore::open(&directory).unwrap())),
        ..Default::default()
    });
    app.start(&Default::default()).unwrap();
    let mut fresh = with_app(app);
    settle(&mut fresh);
    fresh.get_by_role_and_label(Role::Button, "Recover").click();
    settle(&mut fresh);
    assert!(fresh.state().session.map().layer_by_name("Recovered web work").is_some());
    assert!(!rustgis_engine::service_logins::logged_in(&fresh.state().session, &saved));
    fresh.state_mut().run("ui.connection", json!({"id":saved.id})).unwrap();
    fresh.state_mut().ui.connection.login = true;
    settle(&mut fresh);
    fresh.get_by_label("Saved on this computer");
    fresh.get_by_role_and_label(Role::Button, "Save").click();
    settle(&mut fresh);
    assert!(rustgis_engine::service_logins::logged_in(&fresh.state().session, &saved));
    assert_redacted(&fresh);
    drop(fresh);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn remembered_service_without_store_renders_unavailable_without_reading_keychain() {
    let mut h = with_store(None);
    let c = h
        .state_mut()
        .run(
            "connection.new",
            json!({
                "kind":"wms", "address":"https://unavailable-widget.example/wms", "auth":"header", "keyName":"X-Api-Key"
            }),
        )
        .unwrap();
    let id = c["id"].as_str().unwrap();
    h.state_mut().session.connections.set_remembered(id, true);
    h.state_mut().run("ui.connection", json!({"id":id})).unwrap();
    settle(&mut h);
    h.get_by_label("Saved login unavailable");
    assert!(h.query_by_label("Saved on this computer").is_none());
    assert!(h.get_by_role_and_label(Role::Button, "Forget Saved Login").accesskit_node().is_disabled());
}

fn request(h: &mut Harness, reason: LoginReason, key: &str, connections: Vec<String>) {
    let opened = h.state().session.opened;
    h.state_mut().session.database_logins.request(LoginRequest {
        reason,
        kind: WEB,
        key: key.into(),
        connections,
        layers: Vec::new(),
        tiles: 1,
        opened,
        prompted: false,
    });
    settle(h);
}

#[test]
fn service_missing_connection_prompt_prefills_origin_and_cancel_stays_closed() {
    let mut h = with_store(None);
    let origin = "https://missing-web-widget.example";
    request(&mut h, LoginReason::Connection, origin, Vec::new());
    assert_eq!(h.state().dialogs.open, Some("chooseConnection"));
    h.get_by_label("New Connection…").click();
    settle(&mut h);
    assert_eq!(h.state().dialogs.open, Some("newConnection"));
    assert_eq!(h.state().ui.connection.kind, "service");
    assert_eq!(h.state().ui.connection.address, origin);
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    settle(&mut h);
    assert_eq!(h.state().dialogs.open, None);
}

#[test]
fn service_chooser_accepts_auth_none_and_password_request_opens_login() {
    let mut h = with_store(None);
    let origin = "https://choose-web-widget.example";
    let public = h.state_mut().run("connection.new", json!({"kind":"wms","name":"Public web service","address":origin})).unwrap();
    let private = h
        .state_mut()
        .run("connection.new", json!({"kind":"wms","name":"Private web service","address":origin,"auth":"basic","user":"reader"}))
        .unwrap();
    let public = public["id"].as_str().unwrap().to_string();
    let private = private["id"].as_str().unwrap().to_string();
    request(&mut h, LoginReason::Choose, origin, vec![public.clone(), private.clone()]);
    assert_eq!(h.state().dialogs.open, Some("chooseConnection"));
    h.query_all_by_label("Public web service").next().unwrap().click();
    settle(&mut h);
    h.get_by_role_and_label(Role::Button, "Use Connection").click();
    settle(&mut h);
    assert_eq!(h.state().dialogs.open, None);
    assert_eq!(h.state().session.database_logins.selection(WEB, origin), Some(public.as_str()));
    request(&mut h, LoginReason::Password, origin, vec![private.clone()]);
    assert_eq!(h.state().dialogs.open, Some("editConnection"));
    assert!(h.state().ui.connection.login);
    assert_eq!(h.state().ui.connection.id.as_deref(), Some(private.as_str()));
    h.get_by_role_and_label(Role::PasswordInput, "Password");
    assert!(h.query_by_label("Remember on this computer").is_none(), "no store: session-only");
}
