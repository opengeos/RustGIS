//! Logins of saved web-service connections (WMS, WMTS, WFS, WCS, OGC API, ArcGIS REST, CSW,
//! STAC): HTTP basic authentication or an API key in a header.
//!
//! The connection keeps the method, user name and header name, never the password or key.
//! A secret given to `connection.login` lasts only for this session, held by connection id in
//! [`crate::connections::DatabaseLogins`]. Its runtime identity also binds it to the canonical
//! origin, method, user and header, so changing a definition cannot send an old secret elsewhere.
//!
//! Each session/connection login has an opaque request binding. Only requests carrying that
//! binding use its credential, and only at the connection's exact origin. Bindings distinguish
//! authenticated cache entries and are revoked on replacement, logout and session drop. Layers
//! record their saved connection id, never a trusted persisted runtime binding. A project opened
//! without its connection or login asks through the existing login-request queue.
//! Nothing here prints, returns or logs a secret: every failure is
//! a fixed sentence.

use rustgis_data::secrets::Secret;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use crate::Session;
use crate::connections::{Connection, LoginReason, LoginRequest, SERVICE_KINDS, WEB, login_origin};
use crate::render::rustgis_tiles::{cache, logins};

/// A transient secret whose connection identity changed.
pub const UNUSABLE: &str = "The session login could not be used. Enter it again.";
/// Persistence is a separate feature; a service login never accesses the credential store.
pub const SESSION_ONLY: &str = "Web-service logins last for this session only.";
/// A login typed for a plain http:// address without the confirmation.
pub const INSECURE: &str = "This connection's address uses plain http://, so its login would be sent unencrypted. Confirm with allowInsecure: true to send it anyway.";
/// A password or key that can't be sent.
pub const BAD_SECRET: &str = "The password or key must be at most 4096 bytes, without control characters.";
/// A login asked of a connection with nothing to log in with this session.
pub const REQUIRED: &str = "This connection's login is required: give its password (basic) or apiKey (header).";
/// A login for a definition that isn't saved.
pub const UNSAVED: &str = "Save the connection first: a login belongs to a saved connection.";
/// An address a login can't go to.
pub const NO_ORIGIN: &str = "The connection's address has no http:// or https:// origin a login can go to.";
/// A Test must not send the edited form's credential to the saved endpoint.
pub const SAVE_BEFORE_TEST: &str = "Save changes before testing this connection.";
/// The commands that add layers from a service, and so may record the connection they came from
/// (their `connection` parameter, or the connection whose login their address is read with).
pub const ADD_COMMANDS: &[&str] = &[
    "service.add",
    "service.import",
    "service.refresh",
    "stac.search",
    "stac.addAsset",
    "stac.addCube",
    "layer.addUrl",
    "layer.addDownload",
    "map.addTiles",
];

/// Whether `c` is a web-service connection (one that can log in with a method).
pub fn is_service(c: &Connection) -> bool {
    SERVICE_KINDS.contains(&c.kind.as_str())
}

/// The secret a login of `c` sends, as its header method sends it.
fn method(c: &Connection, secret: &str) -> Option<(logins::Method, String)> {
    match c.auth.as_str() {
        "basic" => Some((logins::Method::Header("Authorization".into()), logins::basic(&c.user, secret))),
        "header" => Some((logins::Method::Header(c.key_name.clone()), secret.to_string())),
        _ => None,
    }
}

/// Checks a typed password or key.
pub fn check_secret(secret: &str) -> Result<(), &'static str> {
    if logins::valid_value(secret) { Ok(()) } else { Err(BAD_SECRET) }
}

/// Checks the complete wire credential before a form edit can invalidate an earlier login.
pub fn check_login_secret(c: &Connection, secret: &str) -> Result<(), String> {
    check_secret(secret).map_err(str::to_string)?;
    if c.auth == "basic" && !logins::valid_basic(&c.user, secret) {
        return Err("The encoded Basic login exceeds the supported header size or contains invalid characters.".into());
    }
    Ok(())
}

/// An active login belongs to one connection in one session; the token is not a secret, but
/// saved projects must never treat it as authority to use another session's login.
struct ActiveLogin {
    origin: String,
    binding: String,
    auth: String,
    user: String,
    key_name: String,
}

impl ActiveLogin {
    fn matches(&self, c: &Connection) -> bool {
        login_origin(c).as_deref() == Some(self.origin.as_str())
            && self.auth == c.auth
            && self.user == c.user
            && self.key_name == c.key_name
    }
}

pub struct SessionLogins {
    session: u64,
    active: BTreeMap<String, ActiveLogin>,
}

impl SessionLogins {
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self { session: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), active: BTreeMap::new() }
    }

    fn owner(&self, id: &str) -> String {
        format!("{}/{id}", self.session)
    }
}

impl Default for SessionLogins {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SessionLogins {
    fn drop(&mut self) {
        let mut ended = false;
        for active in self.active.values() {
            ended |= logins::deactivate(&active.binding);
        }
        if ended {
            cache::clear();
        }
    }
}

/// Replaces only this connection's binding, after the complete new credential has validated.
/// Other sessions and other connections at this origin retain their own bindings.
pub(crate) fn activate(s: &mut Session, c: &Connection, secret: Option<&Secret>, insecure: bool) -> Result<(), String> {
    let origin = login_origin(c).ok_or(NO_ORIGIN)?;
    let next = match secret.and_then(|secret| method(c, secret.expose())) {
        Some((method, value)) => {
            let binding = logins::activate(&origin, &s.web_logins.owner(&c.id), method, &value, insecure).map_err(str::to_string)?;
            Some(ActiveLogin { origin: origin.clone(), binding, auth: c.auth.clone(), user: c.user.clone(), key_name: c.key_name.clone() })
        }
        None => None,
    };
    if let Some(old) = s.web_logins.active.remove(&c.id) {
        logins::deactivate(&old.binding);
    }
    if let Some(next) = next {
        s.web_logins.active.insert(c.id.clone(), next);
    }
    s.database_logins.select(WEB, &origin, &c.id);
    s.database_logins.clear_requests_for(WEB, &origin);
    rebind_connection(s, c);
    cache::clear();
    Ok(())
}

pub(crate) fn deactivate(s: &mut Session, c: &Connection, _origin: Option<String>) {
    if let Some(old) = s.web_logins.active.remove(&c.id) {
        logins::deactivate(&old.binding);
        cache::clear();
    }
    if let Some(origin) = login_origin(c)
        && s.database_logins.selection(WEB, &origin) == Some(c.id.as_str())
    {
        s.database_logins.drop_selection(WEB, &origin);
    }
}

pub(crate) fn logout(s: &mut Session, c: &Connection) {
    forget(s, c);
}

pub(crate) fn forget(s: &mut Session, c: &Connection) {
    deactivate(s, c, None);
    s.database_logins.forget_connection(&c.id);
}

pub fn logged_in(s: &Session, c: &Connection) -> bool {
    s.web_logins.active.get(&c.id).is_some_and(|a| a.matches(c) && logins::is_active(&a.binding))
}

pub fn known_secret(s: &Session, c: &Connection) -> Result<Secret, String> {
    let Some(secret) = s.database_logins.session_password(&c.id) else {
        return Err(REQUIRED.into());
    };
    if !s.web_logins.active.get(&c.id).is_some_and(|a| a.matches(c)) {
        return Err(UNUSABLE.into());
    }
    Ok(secret.clone())
}

/// Binds a request to a connection chosen in this session. A bare address can resolve to the
/// explicit session choice or sole saved definition, never the first of several matches.
pub fn request(s: &mut Session, connection: Option<&str>, url: &str) -> Result<String, String> {
    let plain = logins::inner(url);
    let Some(origin) = logins::origin(plain) else { return Ok(plain.to_string()) };
    if plain != url && connection.is_none() {
        let token = rustgis_data::login_address::parse(url).ok().flatten().map(|b| b.binding);
        if s.web_logins.active.values().any(|a| token == Some(a.binding.as_str())) {
            logins::check(url).map_err(str::to_string)?;
            return Ok(url.to_string());
        }
        return Err("This request's login does not belong to this session.".into());
    }
    let candidates: Vec<_> =
        s.connections.connections.iter().filter(|c| is_service(c) && login_origin(c).as_deref() == Some(origin.as_str())).collect();
    let chosen = if let Some(id) = connection {
        let c = s
            .connections
            .connections
            .iter()
            .find(|c| c.id == id && is_service(c))
            .ok_or("No saved web-service connection with this id.")?;
        // A service may link to another origin, but never grants it this login.
        if login_origin(c).as_deref() != Some(origin.as_str()) {
            return Ok(plain.to_string());
        }
        Some(c.clone())
    } else if let Some(id) = s.database_logins.selection(WEB, &origin) {
        candidates.iter().find(|c| c.id == id).map(|c| (*c).clone())
    } else if candidates.len() == 1 {
        candidates.first().map(|c| (*c).clone())
    } else {
        None
    };
    let Some(c) = chosen else {
        if candidates.len() > 1 {
            s.database_logins.request(LoginRequest {
                reason: LoginReason::Choose,
                kind: WEB,
                key: origin,
                connections: candidates.iter().map(|c| c.id.clone()).collect(),
                layers: Vec::new(),
                tiles: 0,
                opened: s.opened,
                prompted: false,
            });
            return Err("Several saved connections match this address. Choose a connection explicitly.".into());
        }
        return Ok(plain.to_string());
    };
    if c.auth.is_empty() {
        return Ok(plain.to_string());
    }
    if !logged_in(s, &c) {
        let secret = known_secret(s, &c)?;
        activate(s, &c, Some(&secret), true)?;
    }
    let active = s.web_logins.active.get(&c.id).ok_or(REQUIRED)?;
    logins::bind(&active.binding, plain).map_err(str::to_string)
}

pub fn request_params<'a>(s: &mut Session, p: &'a Value) -> Result<std::borrow::Cow<'a, Value>, String> {
    let Some(url) = p.get("url").and_then(Value::as_str) else { return Ok(std::borrow::Cow::Borrowed(p)) };
    let id = match p.get("connection") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if id.trim().is_empty() => None,
        Some(Value::String(id)) => Some(id.as_str()),
        Some(_) => return Err("connection is a saved web-service connection id.".into()),
    };
    let bound = request(s, id, url)?;
    // Carry the originating connection into internal add/download commands too. A linked
    // foreign URL must not silently select a different origin's session login.
    let implicit = if id.is_none() {
        let origin = logins::origin(url);
        let token = rustgis_data::login_address::parse(&bound).ok().flatten().map(|b| b.binding);
        s.web_logins
            .active
            .iter()
            .find(|(_, a)| token == Some(a.binding.as_str()))
            .map(|(id, _)| id.as_str())
            .or_else(|| origin.as_deref().and_then(|o| s.database_logins.selection(WEB, o)))
            .or_else(|| {
                let mut matches = s.connections.connections.iter().filter(|c| is_service(c) && login_origin(c) == origin);
                let first = matches.next()?;
                matches.next().is_none().then_some(first.id.as_str())
            })
    } else {
        None
    };
    if bound == url && implicit.is_none() {
        return Ok(std::borrow::Cow::Borrowed(p));
    }
    let mut owned = p.clone();
    *owned.get_mut("url").ok_or("url is required")? = Value::String(bound);
    if let Some(id) = implicit
        && let Some(object) = owned.as_object_mut()
    {
        object.insert("connection".into(), Value::String(id.to_string()));
    }
    Ok(std::borrow::Cow::Owned(owned))
}

/// The web address a tile layer reads (a tile template's archive for a PMTiles or COG template).
fn tile_address(url: &str) -> &str {
    crate::sources::template_archive(url).map_or(url, |(_, archive)| archive)
}

/// The web address a layer reads again later (a streamed source or a raster read in place).
fn layer_address(l: &rustgis_map::Layer) -> Option<&str> {
    l.source.as_ref().and_then(|s| s.path.as_deref()).or_else(|| l.raster.as_ref().and_then(|r| r.path.as_deref()))
}

/// Rebinds only addresses belonging to this connection. Cross-origin links stay anonymous.
fn rebind_connection(s: &mut Session, c: &Connection) {
    let binding = s.web_logins.active.get(&c.id).map(|a| a.binding.as_str());
    let rewrite = |address: &mut String| {
        let (prefix, raw) = crate::sources::template_archive(address).unwrap_or(("", address.as_str()));
        let plain = logins::inner(raw);
        let next = binding.and_then(|b| logins::bind(b, plain).ok()).unwrap_or_else(|| plain.to_string());
        let next = if prefix.is_empty() { next } else { format!("{prefix}{next}") };
        if *address != next {
            *address = next;
        }
    };
    for m in &mut s.project.maps {
        for t in m.tiles.iter_mut().filter(|t| t.connection == c.id) {
            rewrite(&mut t.url);
            if let Some(url) = &mut t.time_url {
                rewrite(url);
            }
            for (_, url) in &mut t.time_urls {
                rewrite(url);
            }
            for legend in &mut t.legends {
                rewrite(&mut legend.url);
            }
            if let Some(info) = &mut t.feature_info {
                rewrite(&mut info.url);
            }
        }
        for l in m.layers.iter_mut().filter(|l| l.connection == c.id) {
            if let Some(path) = l.source.as_mut().and_then(|source| source.path.as_mut()) {
                rewrite(path);
            }
            if let Some(path) = l.raster.as_mut().and_then(|raster| raster.path.as_mut()) {
                rewrite(path);
            }
            if let Some(path) = &mut l.dataset.source {
                rewrite(path);
            }
        }
        for group in &mut m.service_groups {
            let ids: BTreeSet<_> = group
                .children
                .iter()
                .filter_map(|child| m.layers.iter().find(|l| l.id == child.layer).map(|l| l.connection.as_str()))
                .collect();
            if ids.len() == 1 && ids.contains(c.id.as_str()) {
                rewrite(&mut group.url);
                for child in &mut group.children {
                    rewrite(&mut child.source_url);
                }
            }
        }
    }
}

fn record_choice(s: &mut Session, origin: &str, old: &str, c: &Connection) {
    for m in &mut s.project.maps {
        for t in &mut m.tiles {
            if t.connection == old && logins::origin(tile_address(&t.url)).as_deref() == Some(origin) {
                t.connection = c.id.clone();
            }
        }
        let group_layers: BTreeSet<_> = m
            .service_groups
            .iter()
            .filter(|g| logins::origin(&g.url).as_deref() == Some(origin))
            .flat_map(|g| g.children.iter().map(|child| child.layer))
            .collect();
        for l in &mut m.layers {
            if l.connection == old && (layer_address(l).and_then(logins::origin).as_deref() == Some(origin) || group_layers.contains(&l.id))
            {
                l.connection = c.id.clone();
            }
        }
    }
}

/// What a project reads from one origin again later: its layers and tile layers, and the saved
/// connections they record.
#[derive(Default)]
struct Reads {
    recorded: BTreeSet<String>,
    layers: Vec<u32>,
    tiles: usize,
    layer_keys: BTreeSet<(usize, u32)>,
}

/// The origins the project reads from, with what reads each.
fn reads(s: &Session) -> BTreeMap<(String, String), Reads> {
    let mut out = BTreeMap::<(String, String), Reads>::new();
    for (map_index, m) in s.project.maps.iter().enumerate() {
        for t in &m.tiles {
            let Some(origin) = logins::origin(tile_address(&t.url)) else { continue };
            let r = out.entry((origin, t.connection.clone())).or_default();
            r.tiles += 1;
            if !t.connection.is_empty() {
                r.recorded.insert(t.connection.clone());
            }
        }
        for l in &m.layers {
            let Some(origin) = layer_address(l).and_then(logins::origin) else { continue };
            let r = out.entry((origin, l.connection.clone())).or_default();
            if r.layer_keys.insert((map_index, l.id)) {
                r.layers.push(l.id);
            }
            if !l.connection.is_empty() {
                r.recorded.insert(l.connection.clone());
            }
        }
        // An imported service's group is read again by a refresh, from its address: it logs in
        // with the connection its layers record.
        for g in &m.service_groups {
            let Some(origin) = logins::origin(&g.url) else { continue };
            for child in &g.children {
                let id = m.layers.iter().find(|l| l.id == child.layer).map(|l| l.connection.as_str()).unwrap_or("");
                let r = out.entry((origin.clone(), id.to_string())).or_default();
                if r.layer_keys.insert((map_index, child.layer)) {
                    r.layers.push(child.layer);
                }
                if !id.is_empty() {
                    r.recorded.insert(id.to_string());
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
    let mut pending = Vec::new();
    let mut touched = BTreeSet::new();
    for ((origin, recorded_id), r) in reads(s) {
        if only.is_some_and(|only| !only.contains(&origin)) {
            continue;
        }
        touched.insert(origin.clone());
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
        let chosen = match recorded.as_slice() {
            [only] => Some(*only),
            [] => selected.or_else(|| (candidates.len() == 1).then(|| candidates.first()).flatten()),
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
            if candidates.len() > 1 || recorded.len() > 1 {
                // Several connections for the address, and none of them clear: the user picks.
                let ids: Vec<String> = if recorded.len() > 1 {
                    recorded.iter().map(|c| c.id.clone()).collect()
                } else {
                    candidates.iter().map(|c| c.id.clone()).collect()
                };
                pending.push(request(LoginReason::Choose, ids));
            } else if candidates.is_empty() && !r.recorded.is_empty() {
                // Layers came from a connection this computer doesn't have, or whose address has
                // since moved to another origin (a layer records only a connection of its origin).
                pending.push(request(LoginReason::Connection, Vec::new()));
            }
            continue;
        };
        if recorded_id != c.id {
            record_choice(s, &origin, &recorded_id, &c);
        }
        if c.auth.is_empty() || logged_in(s, &c) {
            rebind_connection(s, &c);
            continue;
        }
        // A secret given this session was confirmed for this origin when it was typed.
        match known_secret(s, &c).and_then(|secret| activate(s, &c, Some(&secret), true)) {
            Ok(()) => {}
            Err(e) => {
                let already_asked = s
                    .database_logins
                    .requests(s.opened)
                    .any(|r| r.kind == WEB && r.key == origin && r.connections == [c.id.clone()] && r.reason == LoginReason::Password);
                if e != REQUIRED && !already_asked {
                    s.notify(format!("{}: {e}", c.name), true);
                }
                pending.push(request(LoginReason::Password, vec![c.id.clone()]));
            }
        }
    }
    for origin in touched {
        if !pending.iter().any(|request| request.key == origin) {
            s.database_logins.clear_requests_for(WEB, &origin);
        }
    }
    for request in pending {
        ask(s, request);
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
pub(crate) fn tag(s: &mut Session, params: &Value, before: Before, out: &Value) -> Result<(), String> {
    let named = named(s, params)?;
    let named_origin = named.as_ref().and_then(login_origin);
    let mut origins = BTreeSet::new();
    // The connection a layer of `origin` (none: it holds no address) records.
    let pick = |origin: Option<&String>| -> Option<String> {
        match (&named, origin) {
            (Some(c), None) => Some(c.id.clone()),
            (Some(c), Some(o)) if named_origin.as_ref() == Some(o) => Some(c.id.clone()),
            (_, Some(o)) => {
                if let Some(id) = s.database_logins.selection(WEB, o) {
                    return Some(id.to_string());
                }
                let mut matches = s.connections.connections.iter().filter(|c| is_service(c) && login_origin(c).as_ref() == Some(o));
                let first = matches.next()?;
                matches.next().is_none().then(|| first.id.clone())
            }
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
            if let Some(id) = pick(origin.as_ref()).filter(|_| named.is_some() || l.connection.is_empty()) {
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
    // A tile layer the command says it added (`index`, with its `kind`): one that replaced a layer
    // of the same address and name looks unchanged to the comparison above.
    let tile_kinds = ["raster", "vector", "style", "pmtiles", "mbtiles", "cog"];
    if let (Some(at), Some(kind)) = (out.get("index").and_then(Value::as_u64), out.get("kind").and_then(Value::as_str))
        && tile_kinds.contains(&kind)
        && let Some(t) = usize::try_from(at).ok().and_then(|at| s.project.map_mut().tiles.get_mut(at))
    {
        let origin = logins::origin(tile_address(&t.url));
        if let Some(id) = pick(origin.as_ref()) {
            t.connection = id;
        }
        origins.extend(origin);
    }
    if !origins.is_empty() {
        resolve(s, Some(&origins));
    }
    Ok(())
}
