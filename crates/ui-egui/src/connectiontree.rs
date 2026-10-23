//! The datasets a saved connection offers, listed under it in Data Sources › Connections: one
//! level at a time as it is expanded (`connection.browse` names what each level reads, the app
//! fetches it off the frame and asks again with the answer), with the selection that
//! Add Selected (in a dataset's menu) adds. Each dataset added is its own undo step.

use std::collections::HashMap;
use std::sync::Arc;

use egui::collapsing_header::CollapsingState;
use egui::{Label, RichText, Sense, Ui};
use rustgis_engine::connections::{Connection, DATABASE_KINDS};
use rustgis_engine::render::rustgis_tiles::stac;
use rustgis_engine::sources::Source;
use serde_json::{Value, json};

use crate::serviceui::{self, ServiceSource};
use crate::theme::Tokens;
use crate::{RustGisApp, Slot, i18n};

type Key = (String, String);

#[derive(Default)]
pub struct State {
    /// What each level of each connection lists: (connection id, node; empty for the top).
    entries: HashMap<Key, Entry>,
    /// The definition each connection's entries were made for: an edit makes them stale.
    seen: HashMap<String, Connection>,
    pub picks: Vec<Pick>,
    /// Signed addresses of STAC assets waiting on a token: (token address, asset, name, answer).
    tokens: Vec<(String, String, String, Slot<Result<Vec<u8>, String>>)>,
}

pub enum Entry {
    Loading(Fetch),
    Ready(Arc<Listing>),
    Failed(String),
}

/// A level whose answer is on its way.
pub struct Fetch {
    node: String,
    /// The first answer, and the further documents a service's discovery asked for.
    first: Option<String>,
    documents: Vec<Value>,
    /// The address being fetched.
    pending: String,
    slot: Slot<Result<Vec<u8>, String>>,
}

pub struct Listing {
    pub children: Vec<Value>,
    pub truncated: bool,
    pub matched: Option<u64>,
    pub notes: Vec<String>,
    /// The service its layers belong to, to add them from the documents already read.
    pub source: Option<Arc<ServiceSource>>,
}

/// A dataset selected for Add Selected.
pub struct Pick {
    pub conn: String,
    pub key: String,
    pub name: String,
    pub child: Value,
    pub source: Option<Arc<ServiceSource>>,
}

fn params(id: &str, node: &str) -> Value {
    let mut p = json!({"id": id});
    if !node.is_empty() {
        p["node"] = json!(node);
    }
    p
}

/// Starts reading a level: what it needs fetched is asked for, and a level read without a fetch
/// (a database's tables, an S3 connection's own buckets) is read now. Errors go into the entry.
fn start(app: &mut RustGisApp, c: &Connection, node: &str) {
    let mut probe = params(&c.id, node);
    probe["probe"] = json!(true);
    let entry = match app.session.run("connection.browse", &probe) {
        Err(e) => Entry::Failed(e),
        Ok(v) => match v["fetch"].as_str() {
            None => match app.session.run("connection.browse", &params(&c.id, node)) {
                Ok(v) => Entry::Ready(Arc::new(listing(&v, None, Vec::new()))),
                Err(e) => Entry::Failed(e),
            },
            Some(url) => Entry::Loading(Fetch {
                node: node.to_string(),
                first: None,
                documents: Vec::new(),
                pending: url.to_string(),
                slot: crate::fetch_into(url),
            }),
        },
    };
    app.ui.connection_tree.entries.insert((c.id.clone(), node.to_string()), entry);
}

fn listing(v: &Value, first: Option<String>, documents: Vec<Value>) -> Listing {
    let source = v.get("service").filter(|s| s.is_object()).map(|s| {
        Arc::new(ServiceSource {
            url: s["url"].as_str().unwrap_or("").to_string(),
            kind: serviceui::kind_of(s["type"].as_str().unwrap_or("")),
            text: first.unwrap_or_default(),
            documents,
        })
    });
    Listing {
        children: v["children"].as_array().cloned().unwrap_or_default(),
        truncated: v["truncated"] == true,
        matched: v["matched"].as_u64(),
        notes: v["notes"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
        source,
    }
}

/// An answer arrived: asks again with it (and what came before). A service's discovery may need
/// further documents, which are fetched in turn.
fn advance(app: &mut RustGisApp, id: &str, mut f: Fetch, bytes: Vec<u8>) -> Entry {
    let Ok(text) = crate::service_text(bytes) else {
        return Entry::Failed(tl!("The answer isn't UTF-8 text").to_string());
    };
    if f.first.is_none() {
        f.first = Some(text);
    } else {
        f.documents.push(json!({"url": f.pending, "text": text}));
    }
    let mut p = params(id, &f.node);
    p["answer"] = json!(f.first);
    if !f.documents.is_empty() {
        p["documents"] = json!(f.documents);
    }
    match app.session.run("connection.browse", &p) {
        Err(e) => Entry::Failed(e),
        Ok(v) => match v["next"].as_str() {
            Some(next) => {
                f.pending = next.to_string();
                f.slot = crate::fetch_into(next);
                Entry::Loading(f)
            }
            None => Entry::Ready(Arc::new(listing(&v, f.first, f.documents))),
        },
    }
}

pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let tree = &mut app.ui.connection_tree;
    let mut landed = Vec::new();
    for (key, entry) in &tree.entries {
        if let Entry::Loading(f) = entry
            && let Some(r) = f.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
        {
            landed.push((key.clone(), r));
        }
    }
    for (key, r) in landed {
        let Some(Entry::Loading(f)) = app.ui.connection_tree.entries.remove(&key) else { continue };
        let entry = match r {
            Ok(bytes) => advance(app, &key.0, f, bytes),
            Err(e) => Entry::Failed(e),
        };
        app.ui.connection_tree.entries.insert(key, entry);
    }
    // STAC assets that needed a token: signed and added when it lands.
    for (token_url, href, name, slot) in std::mem::take(&mut app.ui.connection_tree.tokens) {
        let Some(r) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            app.ui.connection_tree.tokens.push((token_url, href, name, slot));
            continue;
        };
        match r.and_then(|b| stac::parse_sas(&b)) {
            Ok(token) => {
                stac::remember_token(&token_url, &token);
                if let Err(e) = app.run("ui.addUrl", json!({"url": stac::sign(&href, &token), "name": name, "zoom": true})) {
                    app.session.notify(e, true);
                }
                app.map.invalidate();
            }
            Err(e) => app.session.notify(e, true),
        }
    }
    let tree = &app.ui.connection_tree;
    if tree.entries.values().any(|e| matches!(e, Entry::Loading(_))) || !tree.tokens.is_empty() || !app.ui.service_steps.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Adds one dataset a listing offers. Each branch is one command, so each is one undo step.
pub fn add(app: &mut RustGisApp, child: &Value, source: Option<&ServiceSource>) -> Result<(), String> {
    let add = child.get("add").filter(|a| a.is_object()).ok_or_else(|| tl!("Nothing to add here").to_string())?;
    let command = add["command"].as_str().unwrap_or("");
    let params = add["params"].clone();
    match (source, child.get("layer").filter(|l| l.is_object())) {
        (Some(source), Some(layer)) => {
            serviceui::add_layer(app, source, layer, "", false)?;
        }
        _ => match command {
            "layer.addUrl" => {
                app.run("ui.addUrl", params)?;
            }
            "stac.addAsset" => add_asset(app, child["href"].as_str().unwrap_or(""), params["name"].as_str().unwrap_or(""))?,
            _ => {
                app.run(command, params)?;
            }
        },
    }
    app.map.invalidate();
    Ok(())
}

/// A STAC asset by its address, signed first when it needs a token (one in hand is used; one to
/// fetch is fetched off the frame and the asset added when it lands).
fn add_asset(app: &mut RustGisApp, href: &str, name: &str) -> Result<(), String> {
    if !stac::needs_signing(href) {
        app.run("ui.addUrl", json!({"url": href, "name": name, "zoom": true}))?;
        return Ok(());
    }
    let token_url = stac::sas_token_url(href)?;
    if let Some(token) = stac::cached_token(&token_url) {
        app.run("ui.addUrl", json!({"url": stac::sign(href, &token), "name": name, "zoom": true}))?;
    } else {
        let slot = crate::fetch_into(&token_url);
        app.ui.connection_tree.tokens.push((token_url, href.to_string(), name.to_string(), slot));
    }
    Ok(())
}

/// Opens the kind's own add dialog on a dataset: a table in the database dialog, a service layer
/// in the service dialog, a STAC collection in the STAC dialog.
fn add_with_options(app: &mut RustGisApp, c: &Connection, child: &Value, source: Option<&ServiceSource>) -> Result<(), String> {
    match child["what"].as_str().unwrap_or("") {
        "table" => {
            crate::sourcespane::open(
                app,
                &Source {
                    kind: "database".into(),
                    name: c.name.clone(),
                    address: c.address.clone(),
                    extra: json!({"layer": child["key"]}),
                },
            )?;
            app.dialogs.connection = Some(c.id.clone());
            Ok(())
        }
        "collection" => {
            crate::sourcespane::open(
                app,
                &Source { kind: "stac".into(), name: c.name.clone(), address: c.address.clone(), extra: json!({"api": "true"}) },
            )?;
            app.ui.stac.collection = child["detail"].as_str().unwrap_or("").to_string();
            app.ui.stac.connection = Some(c.id.clone());
            Ok(())
        }
        _ => match (source, child.get("layer").filter(|l| l.is_object())) {
            (Some(source), Some(layer)) => {
                app.run("ui.addService", json!({"url": source.url, "type": source.kind}))?;
                app.dialogs.connection = Some(c.id.clone());
                app.ui.service_browse.filter = layer["name"].as_str().unwrap_or("").to_string();
                Ok(())
            }
            _ => Err(tl!("Nothing to add here").to_string()),
        },
    }
}

/// What a listed child is, in words.
fn what_label(what: &str) -> &'static str {
    match what {
        "schema" => tl!("schema"),
        "table" => tl!("table"),
        "folder" => tl!("folder"),
        "service" => tl!("service"),
        "map" => tl!("map"),
        "vectorTiles" => tl!("vector tiles"),
        "features" => tl!("features"),
        "coverage" => tl!("coverage"),
        "style" => tl!("style"),
        "record" => tl!("record"),
        "download" => tl!("download"),
        "collection" => tl!("collection"),
        "catalog" => tl!("catalog"),
        "item" => tl!("item"),
        "asset" => tl!("asset"),
        "bucket" => tl!("bucket"),
        "prefix" => tl!("folder"),
        "object" => tl!("file"),
        _ => "",
    }
}

fn picked(app: &RustGisApp, conn: &str, key: &str) -> bool {
    app.ui.connection_tree.picks.iter().any(|p| p.conn == conn && p.key == key)
}

fn toggle(app: &mut RustGisApp, c: &Connection, child: &Value, source: Option<&Arc<ServiceSource>>) {
    let key = child["key"].as_str().unwrap_or("");
    let picks = &mut app.ui.connection_tree.picks;
    if let Some(i) = picks.iter().position(|p| p.conn == c.id && p.key == key) {
        picks.remove(i);
    } else {
        picks.push(Pick {
            conn: c.id.clone(),
            key: key.to_string(),
            name: child["name"].as_str().unwrap_or("").to_string(),
            child: child.clone(),
            source: source.cloned(),
        });
    }
}

/// Forgets what a connection (or, with `None`, every connection) listed and what was selected
/// from it: the next expansion reads it again.
pub fn refresh(app: &mut RustGisApp, id: Option<&str>) {
    let tree = &mut app.ui.connection_tree;
    match id {
        Some(id) => {
            tree.entries.retain(|(c, _), _| c != id);
            tree.picks.retain(|p| p.conn != id);
        }
        None => {
            tree.entries.clear();
            tree.picks.clear();
        }
    }
}

/// Drops the picks of a connection that was edited or removed since (even while its row is
/// folded, when its list isn't refreshed): they would add from the old definition.
pub fn prune(app: &mut RustGisApp) {
    let (connections, tree) = (&app.session.connections, &mut app.ui.connection_tree);
    let seen = &tree.seen;
    tree.picks.retain(|p| connections.find(&p.conn).is_some_and(|c| seen.get(&p.conn) == Some(c)));
}

/// Adds every selected dataset, each as its own undo step, and clears the selection.
fn add_picks(app: &mut RustGisApp) {
    prune(app);
    for pick in std::mem::take(&mut app.ui.connection_tree.picks) {
        if let Err(e) = add(app, &pick.child, pick.source.as_deref()) {
            app.session.notify(e, true);
        }
    }
}

/// A connection's expanded body: its top level, listed (or its error).
pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, c: &Connection) {
    // An edited connection lists what it now offers.
    if app.ui.connection_tree.seen.get(&c.id) != Some(c) {
        refresh(app, Some(&c.id));
        app.ui.connection_tree.seen.insert(c.id.clone(), c.clone());
    }
    level(app, ui, t, c, "");
}

/// One level: loading, its error with the ways to go on, or its children.
fn level(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, c: &Connection, node: &str) {
    let key = (c.id.clone(), node.to_string());
    let shown = match app.ui.connection_tree.entries.get(&key) {
        None => None,
        Some(Entry::Loading(_)) => Some(Err(None)),
        Some(Entry::Failed(e)) => Some(Err(Some(e.clone()))),
        Some(Entry::Ready(l)) => Some(Ok(l.clone())),
    };
    match shown {
        None => {
            start(app, c, node);
            ui.ctx().request_repaint();
        }
        Some(Err(None)) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tl!("Loading…"));
            });
        }
        Some(Err(Some(e))) => {
            ui.label(RichText::new(e).small().color(t.red));
            ui.horizontal(|ui| {
                if ui.small_button(tl!("Retry")).clicked() {
                    app.ui.connection_tree.entries.remove(&key);
                }
                let keys = c.kind == "s3" && c.options.get("credentials").is_some_and(|v| v == "keys");
                if (DATABASE_KINDS.contains(&c.kind.as_str()) || keys) && ui.small_button(tl!("Log In…")).clicked() {
                    crate::connectionui::log_in(app, &c.id);
                }
            });
        }
        Some(Ok(l)) => {
            children(app, ui, t, c, &l, &l.children);
            if l.children.is_empty() {
                ui.label(RichText::new(tl!("Nothing here")).small().weak());
            }
            if l.truncated {
                let n = l.children.len().to_string();
                let text = match l.matched {
                    Some(m) => i18n::fmt("{n} of {m} are listed.", &[("n", &n), ("m", &m.to_string())]),
                    None => i18n::fmt("The first {n} are listed.", &[("n", &n)]),
                };
                ui.label(RichText::new(text).small().weak());
            }
            for note in &l.notes {
                ui.label(RichText::new(note).small().weak());
            }
        }
    }
}

/// A folder in the tree: a header that opens on a click, and a body drawn when it is open.
/// `node` is set when the body is a level of its own (read when first opened, refreshed from
/// the header's menu).
fn branch(
    app: &mut RustGisApp,
    ui: &mut Ui,
    c: &Connection,
    child: &Value,
    node: Option<&str>,
    body: impl FnOnce(&mut RustGisApp, &mut Ui),
) {
    let salt = node.or(child["key"].as_str()).unwrap_or("");
    let id = ui.make_persistent_id(("conntree", &c.id, salt));
    let state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let open = state.is_open();
    let (mut clicked, mut refreshed) = (false, false);
    let mut header = state.show_header(ui, |ui| {
        ui.label(RichText::new(what_label(child["what"].as_str().unwrap_or(""))).small().weak());
        let r = ui.add(Label::new(child["name"].as_str().unwrap_or("")).sense(Sense::click()));
        clicked = r.clicked();
        if node.is_some() {
            r.context_menu(|ui| {
                if ui.button(tl!("Refresh")).clicked() {
                    refreshed = true;
                    ui.close();
                }
            });
        }
    });
    if clicked {
        header.set_open(!open);
    }
    header.body(|ui| body(app, ui));
    if refreshed && let Some(node) = node {
        app.ui.connection_tree.entries.remove(&(c.id.clone(), node.to_string()));
    }
}

fn children(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, c: &Connection, listing: &Listing, list: &[Value]) {
    for child in list {
        if let Some(node) = child["node"].as_str() {
            branch(app, ui, c, child, Some(node), |app, ui| level(app, ui, t, c, node));
            continue;
        }
        if let Some(inline) = child["children"].as_array() {
            branch(app, ui, c, child, None, |app, ui| {
                children(app, ui, t, c, listing, inline);
                if inline.is_empty() {
                    ui.label(RichText::new(tl!("Nothing here")).small().weak());
                }
            });
            continue;
        }
        dataset(app, ui, c, listing, child);
    }
}

/// A dataset: click selects it, double-click adds it, the menu adds it (with options).
fn dataset(app: &mut RustGisApp, ui: &mut Ui, c: &Connection, listing: &Listing, child: &Value) {
    let name = child["name"].as_str().unwrap_or("");
    let is_picked = picked(app, &c.id, child["key"].as_str().unwrap_or(""));
    let r = ui
        .horizontal(|ui| {
            let r = ui.selectable_label(is_picked, name);
            ui.label(RichText::new(what_label(child["what"].as_str().unwrap_or(""))).small().weak());
            r
        })
        .inner;
    // A dataset without an add (an OGC API feature collection, imported together) is added
    // with options only.
    let addable = child.get("add").is_some_and(Value::is_object);
    let hint = if addable { tl!("Click to select, double-click to add, right-click for more") } else { tl!("Right-click for more") };
    let mut hover = hint.to_string();
    if let Some(detail) = child["detail"].as_str().filter(|d| !d.is_empty()) {
        hover = format!("{hover}\n{detail}");
    }
    let r = r.on_hover_text(hover);
    let (mut add_now, mut with_options, mut add_selected, mut clear) = (false, false, false, false);
    let selected = app.ui.connection_tree.picks.len();
    r.context_menu(|ui| {
        if addable && ui.button(tl!("Add")).clicked() {
            add_now = true;
            ui.close();
        }
        if child["options"] == true && ui.button(tl!("Add With Options…")).clicked() {
            with_options = true;
            ui.close();
        }
        if selected > 0 {
            ui.separator();
            if ui.button(i18n::fmt("Add Selected ({n})", &[("n", &selected.to_string())])).clicked() {
                add_selected = true;
                ui.close();
            }
            if ui.button(tl!("Clear Selection")).clicked() {
                clear = true;
                ui.close();
            }
        }
    });
    if addable && r.double_clicked() {
        // The first click selected it: a double-click adds it alone.
        let key = child["key"].as_str().unwrap_or("");
        app.ui.connection_tree.picks.retain(|p| !(p.conn == c.id && p.key == key));
        add_now = true;
    } else if addable && r.clicked() {
        toggle(app, c, child, listing.source.as_ref());
    }
    if clear {
        app.ui.connection_tree.picks.clear();
    }
    if add_selected {
        add_picks(app);
    }
    let result = if with_options {
        add_with_options(app, c, child, listing.source.as_deref())
    } else if add_now {
        add(app, child, listing.source.as_deref())
    } else {
        Ok(())
    };
    if let Err(e) = result {
        app.session.notify(e, true);
    }
}
