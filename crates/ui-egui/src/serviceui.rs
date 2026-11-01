//! Background group import/refresh for both desktop and browser. Discovery and updates live in
//! engine commands; this state machine supplies their downloaded documents and preserves the
//! saved connection id through every deferred add.

use crate::{RustGisApp, Slot, fetch_into, fetch_into_limited, i18n};
use rustgis_engine::data::ogc::{self, Pages, Step};
use serde_json::{Value, json};
use std::collections::VecDeque;

pub struct Import {
    params: Value,
    refresh: bool,
    /// The saved web-service connection to record on imported layers, not a group-plan parameter.
    connection: Option<Value>,
    token: u64,
    /// The originating map: its index and name (names are unique among a project's maps), so a
    /// deleted or replaced map at that index cancels the import.
    map: (usize, String),
    slot: Slot<Result<Vec<u8>, String>>,
    document: Option<String>,
    current: Option<String>,
    remaining: VecDeque<String>,
    responses: Vec<Value>,
    bytes: usize,
}

/// A service whose layers are listed: its address, type, capabilities document and the further
/// documents its discovery read. `service.add` reads layers from them without fetching again.
pub struct ServiceSource {
    pub url: String,
    /// `wms`, `wmts`, `wfs`, `wcs`, `ogcapi`, `arcgis`, or `auto`.
    pub kind: &'static str,
    pub text: String,
    pub documents: Vec<Value>,
}

/// A service type as the add dialog and `ui.addService` name it, `auto` for any other text.
pub(crate) fn kind_of(t: &str) -> &'static str {
    match t {
        "wms" => "wms",
        "wmts" => "wmts",
        "wfs" => "wfs",
        "wcs" => "wcs",
        "ogcapi" => "ogcapi",
        "arcgis" => "arcgis",
        _ => "auto",
    }
}

/// A dialog's provenance is usable only for a saved service of the displayed source's origin.
pub(crate) fn connection_for<'a>(app: &RustGisApp, connection: Option<&'a str>, url: &str) -> Option<&'a str> {
    connection.filter(|id| {
        app.session.connections.find(id).is_some_and(|c| {
            rustgis_engine::service_logins::is_service(c)
                && rustgis_engine::connections::login_origin(c)
                    .is_some_and(|origin| rustgis_engine::render::rustgis_tiles::logins::origin(url).as_deref() == Some(origin.as_str()))
        })
    })
}

/// A linked document never acquires another origin's saved login, even without explicit provenance.
pub(crate) fn request_from(app: &mut RustGisApp, connection: Option<&str>, base: &str, url: &str) -> Result<String, String> {
    use rustgis_engine::render::rustgis_tiles::logins;
    if logins::origin(base).is_some_and(|origin| logins::origin(url).as_deref() == Some(origin.as_str())) {
        rustgis_engine::service_logins::request(&mut app.session, connection, url).map_err(|e| crate::connectionui::translated(&e))
    } else {
        Ok(logins::inner(url).to_string())
    }
}

/// Adds one of a service's layers (as `service.layers` lists it): features are downloaded in the
/// background as From URL… does, map layers are added now, and an OGC API layer or a WCS coverage
/// waits on a document (a step the app moves along). A feature layer is asked only for the
/// features crossing `area` (longitude/latitude) when given, whole otherwise. `dialog` tells where
/// the result is reported.
pub(crate) fn add_layer(
    app: &mut RustGisApp,
    src: &ServiceSource,
    l: &Value,
    time: &str,
    area: Option<[f64; 4]>,
    dialog: bool,
) -> Result<&'static str, String> {
    let connection = dialog.then(|| app.ui.service.as_ref().and_then(|source| source.connection.clone())).flatten();
    add_layer_from(app, src, l, time, area, dialog, None, connection.as_deref())
}

/// [`add_layer`] for a saved connection: `connection` is its id, recorded on the resulting layer;
/// `origin` is its display name for Recent. Both survive deferred documents and downloads.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_layer_from(
    app: &mut RustGisApp,
    src: &ServiceSource,
    l: &Value,
    time: &str,
    area: Option<[f64; 4]>,
    dialog: bool,
    origin: Option<&str>,
    connection: Option<&str>,
) -> Result<&'static str, String> {
    let connection = connection_for(app, connection, &src.url);
    let name = l.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let title = l.get("title").and_then(Value::as_str).unwrap_or(&name).to_string();
    let offer = l.get("offer").cloned().unwrap_or_default();
    if offer.get("type").and_then(Value::as_str) == Some("features") {
        let items = offer.get("url").and_then(Value::as_str).unwrap_or("");
        // Over an area when its request can carry one (not a WFS 1.0's, nor across the
        // antimeridian): whole otherwise, as `service.add` does.
        let asked = area.and_then(|a| ogc::features_in_area(items, a));
        if area.is_some() && asked.is_none() {
            app.session.notify(
                crate::i18n::fmt("{name}: its request can't carry an area, so the whole layer is asked for", &[("name", &title)]),
                false,
            );
        }
        let items = asked.as_deref().unwrap_or(items);
        // The layer's bounds go along, for its features' axes (#1053).
        let bounds = l.get("bbox").cloned().unwrap_or_default();
        let mut p = json!({"url": items, "name": title, "service": true, "bounds": bounds});
        if let Some(connection) = connection {
            p["connection"] = json!(connection);
        }
        return crate::connectiontree::run_add_url(app, p, origin).map(|_| tl!("Downloading…"));
    }
    let origin = origin.map(str::to_string);
    if offer.get("type").and_then(Value::as_str) == Some("coverage") {
        // A coverage: its description first, off the frame (a step asks for it).
        let describe = offer.get("describe").and_then(Value::as_str).unwrap_or("").to_string();
        let mut p = json!({"url": src.url, "layer": name, "text": src.text, "name": title, "documents": src.documents});
        if src.kind != "auto" {
            p["type"] = json!(src.kind);
        }
        if let Some(connection) = connection {
            p["connection"] = json!(connection);
        }
        let describe = request_from(app, connection, &src.url, &describe)?;
        let slot = crate::fetch_into(&describe);
        app.ui.service_steps.push(crate::ServiceStep::Coverage { params: p, slot, dialog, origin });
        return Ok(tl!("Describing"));
    }
    let mut p = json!({"url": src.url, "layer": name, "text": src.text, "documents": src.documents, "source_url": l.get("source_url")});
    if src.kind != "auto" {
        p["type"] = json!(src.kind);
    }
    if let Some(connection) = connection {
        p["connection"] = json!(connection);
    }
    if l.get("time").is_some() && !time.is_empty() {
        p["time"] = json!(time);
    }
    match l.get("tilesets").and_then(Value::as_str) {
        // An OGC API layer: its tileset's advertised template is read first, off the frame (a
        // step adds the layer).
        Some(list) => {
            p["name"] = json!(title);
            let fallback = offer.get("url").and_then(Value::as_str).unwrap_or("").to_string();
            let vector = offer.get("type").and_then(Value::as_str) == Some("vectorTiles");
            let list = request_from(app, connection, &src.url, list)?;
            let slot = crate::fetch_into(&list);
            let resolve = crate::TilesResolve { params: p, list: list.clone(), doc: list, read: 0, vector, fallback, slot };
            app.ui.service_steps.push(crate::ServiceStep::Tiles { resolve, dialog, origin });
            Ok(tl!("Reading the tileset of"))
        }
        None => crate::connectiontree::run_named(app, "service.add", p, origin.as_deref()).map(|_| tl!("Added")),
    }
}

pub fn start(app: &mut RustGisApp, p: &Value, refresh: bool) -> Result<(), String> {
    if !p.is_object() {
        return Err(tl!("Give the service's web address").into());
    }
    if refresh && p.get("group").is_none() && p.get("layer").is_none() {
        return Err(tl!("Select a layer in the Layers pane.").into());
    }
    if app.ui.service_import.is_some() {
        return Err(tl!("A service import or refresh is already running.").into());
    }
    // ui.importService accepts `connection`, as service.import does. The discovery command does
    // not: keep it apart until the final add, without leaking it into service.groupPlan calls.
    let bound = rustgis_engine::service_logins::request_params(&mut app.session, p).map_err(|e| crate::connectionui::translated(&e))?;
    let mut source = bound.into_owned();
    let connection = if refresh { None } else { source.as_object_mut().and_then(|p| p.remove("connection")) };
    source["sourceOnly"] = json!(true);
    let mut params = app.session.run("service.groupPlan", &source)?;
    if let Some(p) = params.as_object_mut() {
        p.remove("sourceOnly");
        // Refresh uses the stored connection, not a previous discovery result.
        if refresh {
            p.remove("text");
            p.remove("documents");
        }
    }
    let mut task = Import {
        params,
        refresh,
        connection,
        token: app.session.download_started(),
        map: active_map(app),
        slot: Slot::default(),
        document: None,
        current: None,
        remaining: VecDeque::new(),
        responses: Vec::new(),
        bytes: 0,
    };
    if task.params.get("text").and_then(Value::as_str).is_some() {
        if let Err(e) = task.discover(app) {
            app.session.download_failed(task.token);
            return Err(e);
        }
    } else {
        let url = task.params.get("url").and_then(Value::as_str).unwrap_or_default();
        let kind = task.params.get("type").and_then(Value::as_str);
        let caps = rustgis_engine::data::ogc::capabilities_url(url, kind);
        let caps = match request_from(app, task.connection.as_ref().and_then(Value::as_str), url, &caps) {
            Ok(caps) => caps,
            Err(e) => {
                app.session.download_failed(task.token);
                return Err(e);
            }
        };
        task.slot = fetch_into(&caps);
        task.document = Some(caps);
    }
    app.dialogs.error = None;
    app.ui.service_import = Some(task);
    Ok(())
}

impl Import {
    fn discover(&mut self, app: &mut RustGisApp) -> Result<(), String> {
        let listing = app.session.run("service.groupPlan", &self.params)?;
        if let Some(next) = listing.get("next").and_then(Value::as_str) {
            let url = self.params.get("url").and_then(Value::as_str).unwrap_or_default();
            let next = request_from(app, self.connection.as_ref().and_then(Value::as_str), url, next)?;
            self.slot = fetch_into(&next);
            self.document = Some(next);
        } else {
            self.remaining = listing
                .get("layers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|l| l.get("offer").and_then(|o| o.get("url")).and_then(Value::as_str).map(str::to_string))
                .collect();
            self.next(app)?;
        }
        Ok(())
    }

    fn next(&mut self, app: &mut RustGisApp) -> Result<(), String> {
        self.current = self.remaining.pop_front();
        if let Some(url) = &self.current {
            let base = self.params.get("url").and_then(Value::as_str).unwrap_or_default();
            let url = request_from(app, self.connection.as_ref().and_then(Value::as_str), base, url)?;
            self.slot = fetch_into_limited(&url, rustgis_engine::cmd::service::MAX_GROUP_BYTES.saturating_sub(self.bytes));
        }
        Ok(())
    }

    fn landed(&mut self, app: &mut RustGisApp, bytes: Vec<u8>) -> Result<(), String> {
        if let Some(document) = self.document.take() {
            let text = crate::service_text(bytes).map_err(|_| tl!("The service's answer isn't UTF-8 text").to_string())?;
            if self.params.get("text").is_none() {
                self.params["text"] = json!(text);
                self.params["documents"] = json!([]);
            } else if let Some(docs) = self.params.get_mut("documents").and_then(Value::as_array_mut) {
                docs.push(json!({"url":document,"text":text}));
            } else {
                self.params["documents"] = json!([{"url":document,"text":text}]);
            }
            self.discover(app)
        } else {
            self.bytes = self.bytes.saturating_add(bytes.len());
            if self.bytes > rustgis_engine::cmd::service::MAX_GROUP_BYTES {
                return Err(tl!("Service group data exceeds 64 MB.").into());
            }
            let text = String::from_utf8(bytes).map_err(|_| tl!("The service's answer isn't UTF-8 text").to_string())?;
            self.responses.push(json!({"url":self.current.take(),"text":text}));
            self.next(app)?;
            Ok(())
        }
    }
}

fn active_map(app: &RustGisApp) -> (usize, String) {
    let i = app.session.project.active_map;
    (i, app.session.project.maps.get(i).map(|m| m.name.clone()).unwrap_or_default())
}

pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(mut task) = app.ui.service_import.take() else { return };
    if !app.session.download_is_pending(task.token) || active_map(app) != task.map {
        app.session.download_failed(task.token);
        return;
    }
    let landed = task.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(answer) = landed
        && let Err(e) = answer.and_then(|bytes| task.landed(app, bytes))
    {
        app.session.download_failed(task.token);
        app.session.notify(e.clone(), true);
        app.dialogs.error = Some(e);
        return;
    }
    if task.document.is_none() && task.current.is_none() {
        app.session.download_failed(task.token);
        task.params["responses"] = json!(task.responses);
        // The plan's connection parameters are not public refresh parameters.
        if task.refresh
            && let Some(p) = task.params.as_object_mut()
        {
            for key in ["url", "type", "name"] {
                p.remove(key);
            }
        }
        if let Some(connection) = task.connection {
            task.params["connection"] = connection;
        }
        let result = app.run(if task.refresh { "service.refresh" } else { "service.import" }, task.params);
        match result {
            Ok(_) => {
                app.dialogs.info = Some(tl!("Done").into());
                app.map.invalidate();
            }
            Err(e) => app.dialogs.error = Some(crate::connectionui::translated(&e)),
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        app.ui.service_import = Some(task);
    }
}

/// A download that landed, with what it is added with: its id, address and `layer.addDownload`
/// parameters, and the connection it came from (named in Recent).
pub(crate) struct Landed {
    pub id: u64,
    pub url: String,
    pub params: Value,
    pub origin: Option<String>,
}

/// A service layer's features read page by page off the frame, after its first page landed (#764):
/// each further answer [`Pages`] asks for is fetched in the background and handed to it, until
/// the collection is read; then it is added as the download it began as.
pub struct PageRead {
    landed: Landed,
    pages: Box<Pages>,
    /// The bytes the answers still to come may take (the download's budget, less those read).
    left: usize,
    slot: Slot<Result<Vec<u8>, String>>,
}

/// Starts reading a service's features past their first page (`first`, landed): added now when it
/// leads nowhere, else read on in the background ([`poll_pages`]).
pub(crate) fn read_pages(app: &mut RustGisApp, landed: Landed, first: Vec<u8>) {
    let left = rustgis_engine::render::rustgis_tiles::cache::MAX_DOWNLOAD.saturating_sub(first.len());
    let step = ogc::read_pages(&landed.url, first, ogc::MAX_PAGED_FEATURES);
    if matches!(step, Step::Fetch(_)) {
        app.session.notify(i18n::fmt("Reading {url} page by page…", &[("url", &landed.url)]), false);
    }
    go_on(app, landed, left, step);
}

/// Hands each page that landed to its read, and adds the layers whose features are all read. A
/// read whose project was closed meanwhile is dropped.
pub fn poll_pages(app: &mut RustGisApp, ctx: &egui::Context) {
    if app.ui.service_pages.is_empty() {
        return;
    }
    for read in std::mem::take(&mut app.ui.service_pages) {
        if !app.session.download_is_pending(read.landed.id) {
            let url = &read.landed.url;
            app.session.notify(i18n::fmt("{url}: downloaded after its project was closed, so it wasn't added", &[("url", url)]), false);
            continue;
        }
        let answer = read.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        match answer {
            Some(answer) => {
                let left = read.left.saturating_sub(answer.as_ref().map_or(0, Vec::len));
                let step = (*read.pages).land(answer);
                go_on(app, read.landed, left, step);
            }
            None => app.ui.service_pages.push(read),
        }
    }
    if !app.ui.service_pages.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}

/// The next answer fetched, or the collection added (with a note when some features were left
/// unread).
fn go_on(app: &mut RustGisApp, landed: Landed, left: usize, step: Step) {
    match step {
        Step::Fetch(pages) => {
            let next = rustgis_engine::render::rustgis_tiles::address::check(pages.next_url(), Some(pages.referrer()))
                .and_then(|()| request_from(app, landed.params.get("connection").and_then(Value::as_str), &landed.url, pages.next_url()));
            let slot = match next {
                Ok(url) => fetch_into_limited(&url, left),
                Err(e) => Slot::new(std::sync::Mutex::new(Some(Err(e)))),
            };
            app.ui.service_pages.push(PageRead { landed, pages, left, slot });
        }
        Step::Done(bytes, note) => {
            if let Some(note) = note {
                let name = landed.params.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(&landed.url);
                app.session.notify(format!("{name}: {note}"), false);
            }
            app.add_landed(landed, bytes);
        }
    }
}
