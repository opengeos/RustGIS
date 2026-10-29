//! Logins of saved web-service connections (WMS, WMTS, WFS, WCS, OGC API, ArcGIS REST, CSW,
//! STAC): HTTP basic authentication, an API key in a header, or an API key in a query parameter.
//!
//! The connection keeps the method, the user name and the header or parameter name, never the
//! password or key. That is given to `connection.login` and kept for the session (in
//! [`crate::connections::DatabaseLogins`], by connection id), or on this computer when the user
//! asks to remember it: one opaque secret at the account `connection/<id>`, the JSON
//! `{"origin", "auth", "user", "keyName", "secret"}`. The origin and method travel with the secret,
//! so a connection whose address or method changed outside the app can never send it elsewhere.
//!
//! A login is sent only to its connection's origin (scheme, host and port), through the tile
//! cache's origin-scoped hook ([`rustgis_tiles::logins`](crate::render::rustgis_tiles::logins)),
//! and only for one connection per origin at a time: which one is never picked silently. Layers
//! added from a connection record its id ([`tag`]); a project's layers are matched to the
//! connections of their origin when it opens ([`resolve`]), and asked about when that isn't
//! clear or the login is missing. Nothing here prints, returns or logs a secret: every failure is
//! a fixed sentence.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rustgis_data::secrets::{Secret, SecretStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Session;
use crate::connections::{Connection, LoginReason, LoginRequest, SERVICE_KINDS, WEB, account, login_origin};
use crate::render::rustgis_tiles::{cache, logins};

/// A stored value that can't be used (damaged, or saved for another origin or method).
pub const UNUSABLE: &str = "The saved login could not be used. Enter it again.";
/// Nothing is saved for the connection.
pub const NONE_SAVED: &str = "No saved login for this connection. Enter it again.";
/// There is no credential store, or it refused.
pub const UNAVAILABLE: &str = "The saved login is unavailable. Enter it for this session.";
/// The login worked, the secret wasn't kept.
pub const NOT_REMEMBERED: &str = "The login is available for this session but was not remembered on this computer.";
/// A saved login couldn't be deleted, so what needed it gone didn't happen.
pub const NOT_FORGOTTEN: &str = "The saved login could not be forgotten. The connection was not changed.";
/// A login typed for a plain http:// address without the confirmation.
pub const INSECURE: &str = "This connection's address uses plain http://, so its login would be sent unencrypted. Confirm with allowInsecure: true to send it anyway.";
/// A password or key that can't be sent.
pub const BAD_SECRET: &str = "The password or key must be at most 4096 characters, without control characters.";
/// A login asked of a connection with nothing to log in with this session.
pub const REQUIRED: &str = "This connection's login is required: give its password (basic) or apiKey (header, query).";
/// A login for a definition that isn't saved.
pub const UNSAVED: &str = "Save the connection first: a login belongs to a saved connection.";
/// An address a login can't go to.
pub const NO_ORIGIN: &str = "The connection's address has no http:// or https:// origin a login can go to.";

/// The most bytes a stored value may have.
const MAX_STORED: usize = 8192;

/// The commands that add layers from a service, and so may record the connection they came from
/// (their `connection` parameter, or the connection whose login their address is read with).
pub const ADD_COMMANDS: &[&str] =
    &["service.add", "service.import", "service.refresh", "stac.addAsset", "layer.addUrl", "layer.addDownload", "map.addTiles"];

#[derive(Serialize)]
struct Stored<'a> {
    origin: &'a str,
    auth: &'a str,
    user: &'a str,
    #[serde(rename = "keyName")]
    key_name: &'a str,
    secret: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Loaded {
    origin: String,
    auth: String,
    user: String,
    #[serde(rename = "keyName")]
    key_name: String,
    secret: String,
}

fn encode(c: &Connection, origin: &str, secret: &str) -> Result<Secret, &'static str> {
    let stored = Stored { origin, auth: &c.auth, user: &c.user, key_name: &c.key_name, secret };
    let text = serde_json::to_string(&stored).map_err(|_| UNUSABLE)?;
    if text.len() > MAX_STORED {
        return Err(UNUSABLE);
    }
    Ok(Secret::new(text))
}

fn decode(value: &Secret, c: &Connection, origin: &str) -> Result<Secret, &'static str> {
    let text = value.expose();
    if text.len() > MAX_STORED {
        return Err(UNUSABLE);
    }
    // A decoding error would quote the text: its details are dropped.
    let l: Loaded = serde_json::from_str(text).map_err(|_| UNUSABLE)?;
    if l.origin != origin || l.auth != c.auth || l.user != c.user || l.key_name != c.key_name || !logins::valid_value(&l.secret) {
        return Err(UNUSABLE);
    }
    Ok(Secret::new(l.secret))
}

/// What reading a connection's saved login gave.
pub enum Loading {
    Found(Secret),
    /// The store has none for it.
    Missing,
    /// There is no store, or it refused.
    Unavailable,
    /// What it holds can't be used (another origin or method, or damaged).
    Unusable,
}

type Store<'a> = Option<&'a Arc<dyn SecretStore>>;

/// Reads the login saved for `c` (a user action: this may ask the system to unlock its store).
pub fn load(store: Store<'_>, c: &Connection) -> Loading {
    let Some(store) = store else { return Loading::Unavailable };
    let value = match store.load(&account(&c.id)) {
        Ok(Some(v)) => v,
        Ok(None) => return Loading::Missing,
        Err(_) => return Loading::Unavailable,
    };
    let Some(origin) = login_origin(c) else { return Loading::Unusable };
    match decode(&value, c, &origin) {
        Ok(secret) => Loading::Found(secret),
        Err(_) => Loading::Unusable,
    }
}

/// Keeps the login of `c`, replacing an earlier one; whether it was kept.
pub fn save(store: Store<'_>, c: &Connection, secret: &str) -> bool {
    let Some(store) = store else { return false };
    let Some(origin) = login_origin(c) else { return false };
    let Ok(value) = encode(c, &origin, secret) else { return false };
    store.save(&account(&c.id), &value).is_ok()
}

/// Whether `c` is a web-service connection (one that can log in with a method).
pub fn is_service(c: &Connection) -> bool {
    SERVICE_KINDS.contains(&c.kind.as_str())
}

/// The secret a login of `c` sends, as its method sends it: a header or a query parameter.
fn method(c: &Connection, secret: &str) -> Option<(logins::Method, String)> {
    match c.auth.as_str() {
        "basic" => Some((logins::Method::Header("Authorization".into()), logins::basic(&c.user, secret))),
        "header" => Some((logins::Method::Header(c.key_name.clone()), secret.to_string())),
        "query" => Some((logins::Method::Query(c.key_name.clone()), secret.to_string())),
        _ => None,
    }
}

/// Checks a typed password or key.
pub fn check_secret(secret: &str) -> Result<(), &'static str> {
    if logins::valid_value(secret) { Ok(()) } else { Err(BAD_SECRET) }
}

/// The web-service logins a session made the ones sent to an origin. The tile cache's fetches are
/// the process's, so a login is owned by the session and the connection (`<session>/<id>`): a
/// session never takes over, ends or reports as its own another session's login, and the logins
/// a session made end when it does (a script's or a Python session that is dropped).
pub struct SessionLogins {
    session: u64,
    /// The origins this session's logins were made for.
    origins: BTreeSet<String>,
}

impl SessionLogins {
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        SessionLogins { session: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), origins: BTreeSet::new() }
    }

    /// The registry's owner of connection `id`'s login in this session.
    fn owner(&self, id: &str) -> String {
        format!("{}/{id}", self.session)
    }

    /// The connection whose login this session sends to `origin`, if it is this session's.
    fn connection(&self, origin: &str) -> Option<String> {
        let owner = logins::owner(origin)?;
        owner.strip_prefix(&format!("{}/", self.session)).map(str::to_string)
    }
}

impl Default for SessionLogins {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SessionLogins {
    fn drop(&mut self) {
        let prefix = format!("{}/", self.session);
        for origin in &self.origins {
            if let Some(owner) = logins::owner(origin).filter(|o| o.starts_with(&prefix)) {
                logins::deactivate(origin, &owner);
            }
        }
    }
}

/// Makes `c` the connection whose login goes to its origin, with `secret` (none: a connection
/// without a login method, chosen to send none). `insecure` confirms a plain http:// origin. The
/// tiles and documents cached for the origin are asked for again.
pub(crate) fn activate(s: &mut Session, c: &Connection, secret: Option<&Secret>, insecure: bool) -> Result<(), String> {
    let origin = login_origin(c).ok_or(NO_ORIGIN)?;
    match secret.and_then(|secret| method(c, secret.expose())) {
        Some((m, value)) => {
            logins::activate(&origin, &s.web_logins.owner(&c.id), m, &value, insecure).map_err(str::to_string)?;
            s.web_logins.origins.insert(origin.clone());
        }
        None => {
            // This session's own login there ends; another session's isn't this one's to end.
            if let Some(id) = s.web_logins.connection(&origin) {
                logins::deactivate(&origin, &s.web_logins.owner(&id));
            }
        }
    }
    s.database_logins.select(WEB, &origin, &c.id);
    s.database_logins.clear_requests_for(WEB, &origin);
    cache::clear();
    Ok(())
}

/// Ends `c`'s login where it is the one sent to its origin (or `origin`, the one it had before an
/// edit), and its choice for that origin.
pub(crate) fn deactivate(s: &mut Session, c: &Connection, origin: Option<String>) {
    let Some(origin) = origin.or_else(|| login_origin(c)) else { return };
    if logins::deactivate(&origin, &s.web_logins.owner(&c.id)) {
        cache::clear();
    }
    if s.database_logins.selection(WEB, &origin) == Some(c.id.as_str()) {
        s.database_logins.drop_selection(WEB, &origin);
    }
}

/// Whether `c`'s login, made in session `s`, is the one sent to its origin now.
pub fn logged_in(s: &Session, c: &Connection) -> bool {
    login_origin(c).and_then(|o| s.web_logins.connection(&o)).is_some_and(|id| id == c.id)
}

/// The secret `c` logs in with, with what needs no typing: the one given this session, else the
/// one saved on this computer (read once, then kept for the session). Errors are fixed sentences.
pub(crate) fn known_secret(s: &mut Session, c: &Connection) -> Result<Secret, String> {
    if let Some(secret) = s.database_logins.session_password(&c.id) {
        return Ok(secret.clone());
    }
    if !c.remembered {
        return Err(REQUIRED.into());
    }
    match load(s.database_logins.secret_store(), c) {
        Loading::Found(secret) => {
            s.database_logins.set_session_password(&c.id, secret.expose());
            Ok(secret)
        }
        Loading::Missing => {
            s.connections.set_remembered(&c.id, false);
            Err(NONE_SAVED.into())
        }
        Loading::Unavailable => Err(UNAVAILABLE.into()),
        Loading::Unusable => Err(UNUSABLE.into()),
    }
}

/// The web address a tile layer reads (a tile template's archive for a PMTiles or COG template).
fn tile_address(url: &str) -> &str {
    crate::sources::template_archive(url).map_or(url, |(_, archive)| archive)
}

/// The web address a layer reads again later (a streamed source or a raster read in place).
fn layer_address(l: &rustgis_map::Layer) -> Option<&str> {
    l.source.as_ref().and_then(|s| s.path.as_deref()).or_else(|| l.raster.as_ref().and_then(|r| r.path.as_deref()))
}

/// What a project reads from one origin again later: its layers and tile layers, and the saved
/// connections they record.
#[derive(Default)]
struct Reads {
    recorded: BTreeSet<String>,
    layers: Vec<u32>,
    tiles: usize,
}

/// The origins the project reads from, with what reads each.
fn reads(s: &Session) -> BTreeMap<String, Reads> {
    let mut out = BTreeMap::<String, Reads>::new();
    for m in &s.project.maps {
        for t in &m.tiles {
            let Some(origin) = logins::origin(tile_address(&t.url)) else { continue };
            let r = out.entry(origin).or_default();
            r.tiles += 1;
            if !t.connection.is_empty() {
                r.recorded.insert(t.connection.clone());
            }
        }
        for l in &m.layers {
            let Some(origin) = layer_address(l).and_then(logins::origin) else { continue };
            let r = out.entry(origin).or_default();
            r.layers.push(l.id);
            if !l.connection.is_empty() {
                r.recorded.insert(l.connection.clone());
            }
        }
        // An imported service's group is read again by a refresh, from its address: it logs in
        // with the connection its layers record.
        for g in &m.service_groups {
            let Some(origin) = logins::origin(&g.url) else { continue };
            let r = out.entry(origin).or_default();
            for child in &g.children {
                r.layers.push(child.layer);
                if let Some(l) = m.layers.iter().find(|l| l.id == child.layer && !l.connection.is_empty()) {
                    r.recorded.insert(l.connection.clone());
                }
            }
        }
    }
    out
}

/// Matches what the project reads from each origin (every origin, or those `only` names) to the
/// saved connections of that origin, and logs in with the one that is clear: the connection the
/// layers record, the one chosen this session, or the only one. Several, a missing login, or a
/// recorded connection this computer doesn't have are asked about ([`LoginRequest`]), never
/// guessed.
pub(crate) fn resolve(s: &mut Session, only: Option<&BTreeSet<String>>) {
    for (origin, r) in reads(s) {
        if only.is_some_and(|only| !only.contains(&origin)) {
            continue;
        }
        let candidates: Vec<Connection> = s
            .connections
            .connections
            .iter()
            .filter(|c| is_service(c) && login_origin(c).as_deref() == Some(origin.as_str()))
            .cloned()
            .collect();
        // The recorded ids that are connections of this origin (looked up by id only: a name in a
        // project is no one's choice).
        let recorded: Vec<&Connection> = candidates.iter().filter(|c| r.recorded.contains(&c.id)).collect();
        let selected = s.database_logins.selection(WEB, &origin).and_then(|id| candidates.iter().find(|c| c.id == id));
        let chosen = match (recorded.as_slice(), selected) {
            ([only], None) => Some(*only),
            ([only], Some(sel)) if sel.id == only.id => Some(*only),
            ([_, ..], Some(sel)) if recorded.iter().any(|c| c.id == sel.id) && recorded.len() > 1 => Some(sel),
            ([], Some(sel)) => Some(sel),
            ([], None) if candidates.len() == 1 => candidates.first(),
            _ => None,
        };
        let chosen = chosen.cloned();
        let opened = s.opened;
        let request = |reason, connections: Vec<String>| LoginRequest {
            reason,
            kind: WEB,
            key: origin.clone(),
            connections,
            layers: r.layers.clone(),
            tiles: r.tiles,
            opened,
            prompted: false,
        };
        let Some(c) = chosen else {
            if candidates.iter().any(|c| !c.auth.is_empty()) && (candidates.len() > 1 || recorded.len() > 1) {
                // Several connections for the address, and none of them clear: the user picks.
                let ids: Vec<String> = if recorded.len() > 1 {
                    recorded.iter().map(|c| c.id.clone()).collect()
                } else {
                    candidates.iter().map(|c| c.id.clone()).collect()
                };
                ask(s, request(LoginReason::Choose, ids));
            } else if candidates.is_empty() && r.recorded.iter().any(|id| !s.connections.connections.iter().any(|c| &c.id == id)) {
                // Layers came from a connection this computer doesn't have.
                ask(s, request(LoginReason::Connection, Vec::new()));
            } else {
                s.database_logins.clear_requests_for(WEB, &origin);
            }
            continue;
        };
        let active = s.web_logins.connection(&origin);
        if c.auth.is_empty() || active.as_deref() == Some(c.id.as_str()) {
            // Nothing to send, or it is sent already.
            if c.auth.is_empty() && active.is_some() {
                let _ = activate(s, &c, None, false);
            }
            s.database_logins.clear_requests_for(WEB, &origin);
            continue;
        }
        // A secret given this session or saved was confirmed for this origin when it was typed.
        match known_secret(s, &c).and_then(|secret| activate(s, &c, Some(&secret), true)) {
            Ok(()) => {}
            Err(e) => {
                if e != REQUIRED {
                    s.notify(format!("{}: {e}", c.name), true);
                }
                ask(s, request(LoginReason::Password, vec![c.id.clone()]));
            }
        }
    }
}

/// Adds a request for the app to ask, saying it once (for the command line and scripts too).
fn ask(s: &mut Session, request: LoginRequest) {
    let known = s
        .database_logins
        .requests(s.opened)
        .any(|r| r.kind == request.kind && r.key == request.key && r.reason == request.reason && r.connections == request.connections);
    if !known {
        let n = request.layers.len() + request.tiles;
        let names: Vec<String> = request
            .connections
            .iter()
            .filter_map(|id| s.connections.connections.iter().find(|c| &c.id == id))
            .map(|c| format!("'{}' ({})", c.name, c.id))
            .collect();
        let text = match request.reason {
            LoginReason::Choose => format!(
                "{n} layer(s) read {} and several saved connections match it: {}. Choose one with connection.login {{id}}.",
                request.key,
                names.join(", ")
            ),
            LoginReason::Password => {
                format!(
                    "{n} layer(s) read {} and need the login of the connection {}: connection.login {{id, …}}.",
                    request.key,
                    names.join(", ")
                )
            }
            LoginReason::Connection => format!(
                "{n} layer(s) read {} with a saved connection this computer doesn't have: add a connection for it (connection.new) and log in.",
                request.key
            ),
        };
        s.notify(text, false);
    }
    s.database_logins.request(request);
}

/// What the project held before an add command: its layers by id and its tile layers, so the
/// ones the command added can be told apart.
pub(crate) struct Before {
    layers: Vec<BTreeSet<u32>>,
    tiles: Vec<Vec<(String, String)>>,
}

/// The connection an add command names (its `connection`), checked before it runs: a saved
/// web-service connection, by id.
fn named(s: &Session, params: &Value) -> Result<Option<Connection>, String> {
    match params.get("connection") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) => {
            let c = s
                .connections
                .connections
                .iter()
                .find(|c| &c.id == id)
                .filter(|c| is_service(c))
                .ok_or("connection is the id of a saved web-service connection (connection.list)")?;
            Ok(Some(c.clone()))
        }
        Some(_) => Err("connection is the id of a saved web-service connection (connection.list)".into()),
    }
}

/// What to compare after command `id` runs, if it adds layers; an error for a `connection` that
/// isn't a saved web-service connection's id.
pub(crate) fn before(s: &Session, id: &str, params: &Value) -> Result<Option<Before>, String> {
    if !ADD_COMMANDS.contains(&id) {
        return Ok(None);
    }
    named(s, params)?;
    Ok(Some(Before {
        layers: s.project.maps.iter().map(|m| m.layers.iter().map(|l| l.id).collect()).collect(),
        tiles: s.project.maps.iter().map(|m| m.tiles.iter().map(|t| (t.url.clone(), t.name.clone())).collect()).collect(),
    }))
}

/// Records on the layers an add command made the connection they came from: the one it names
/// (`connection`) for those read from its origin or holding no address, else the one whose login
/// their origin is read with. Then logs in for their origins as a project's layers do.
pub(crate) fn tag(s: &mut Session, params: &Value, before: Before) -> Result<(), String> {
    let named = named(s, params)?;
    let named_origin = named.as_ref().and_then(login_origin);
    let mut origins = BTreeSet::new();
    // The connection a layer of `origin` (none: it holds no address) records.
    let pick = |origin: Option<&String>| -> Option<String> {
        match (&named, origin) {
            (Some(c), None) => Some(c.id.clone()),
            (Some(c), Some(o)) if named_origin.as_ref() == Some(o) => Some(c.id.clone()),
            (_, Some(o)) => s.web_logins.connection(o),
            (None, None) => None,
        }
    };
    for (i, m) in s.project.maps.iter_mut().enumerate() {
        let mut old = before.tiles.get(i).cloned().unwrap_or_default();
        for t in &mut m.tiles {
            // Tile layers have no id: those not there before (each old one matched once).
            if let Some(at) = old.iter().position(|(url, name)| *url == t.url && *name == t.name) {
                old.swap_remove(at);
                continue;
            }
            let origin = logins::origin(tile_address(&t.url));
            if let Some(id) = pick(origin.as_ref()) {
                t.connection = id;
            }
            origins.extend(origin);
        }
        let old_layers = before.layers.get(i);
        let mut added = BTreeSet::new();
        for l in m.layers.iter_mut().filter(|l| old_layers.is_none_or(|old| !old.contains(&l.id))) {
            added.insert(l.id);
            let origin = layer_address(l).or(l.dataset.source.as_deref()).and_then(logins::origin);
            if let Some(id) = pick(origin.as_ref()) {
                l.connection = id;
            }
            if layer_address(l).is_some() {
                origins.extend(origin);
            }
        }
        // An imported service's group is refreshed from its address later.
        for g in m.service_groups.iter().filter(|g| g.children.iter().any(|c| added.contains(&c.layer))) {
            origins.extend(logins::origin(&g.url));
        }
    }
    if !origins.is_empty() {
        resolve(s, Some(&origins));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        Connection {
            id: "c4".into(),
            kind: "wms".into(),
            address: "https://maps.example.org/wms".into(),
            auth: "header".into(),
            key_name: "X-Api-Key".into(),
            ..Connection::default()
        }
    }

    #[test]
    fn saved_login_is_bound_to_its_origin_and_method_and_never_echoed() {
        let c = conn();
        let v = encode(&c, "https://maps.example.org", "very-secret-value").unwrap();
        assert_eq!(decode(&v, &c, "https://maps.example.org").unwrap().expose(), "very-secret-value");
        assert_eq!(decode(&v, &c, "https://maps.example.org:8443"), Err(UNUSABLE));
        assert_eq!(decode(&v, &c, "http://maps.example.org"), Err(UNUSABLE));
        assert_eq!(decode(&v, &c, "https://other.example.org"), Err(UNUSABLE));
        let mut renamed = c.clone();
        renamed.key_name = "X-Other".into();
        assert_eq!(decode(&v, &renamed, "https://maps.example.org"), Err(UNUSABLE));
        let mut basic = c.clone();
        basic.auth = "basic".into();
        assert_eq!(decode(&v, &basic, "https://maps.example.org"), Err(UNUSABLE));
        for bad in [
            "not json very-secret-value",
            r#"{"origin":"https://maps.example.org","auth":"header","user":"","keyName":"X-Api-Key"}"#,
            r#"{"origin":"https://maps.example.org","auth":"header","user":"","keyName":"X-Api-Key","secret":"s","extra":1}"#,
            r#"{"origin":"https://maps.example.org","auth":"header","user":"","keyName":"X-Api-Key","secret":""}"#,
            r#"{"origin":"https://maps.example.org","auth":"header","user":"","keyName":"X-Api-Key","secret":"a\nb"}"#,
            "[]",
        ] {
            let e = decode(&Secret::new(bad), &c, "https://maps.example.org").err();
            assert_eq!(e, Some(UNUSABLE), "{bad}");
        }
        for message in [UNUSABLE, NONE_SAVED, UNAVAILABLE, NOT_REMEMBERED, NOT_FORGOTTEN, INSECURE, BAD_SECRET, REQUIRED] {
            assert!(!message.contains("very-secret-value"));
        }
    }
}
