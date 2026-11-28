//! Plugin tools in the Tools pane: each enabled plugin's tools join the external tools
//! (under "Plugins ›" toolboxes) and run in the plugin's module with their layers as GeoJSON
//! and their files as base64.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rustgis_analysis::{Inputs, Output, OutputLayer, ParamKind, ParamSpec, ToolResult, ToolSpec};
use serde_json::{Map, Value, json};

use crate::manifest::is_layer_kind;
use crate::{Host, Plugin, split_id};

/// The key plugin tools are registered under.
pub const SOURCE: &str = "plugins";

/// The most features a tool may return.
const MAX_FEATURES: usize = 5_000_000;

/// The largest file a `file` parameter hands a tool. The request carries it as base64 (a third
/// larger), and the module's copy of the request and the file it decodes from it must fit in
/// its memory (`runtime::MAX_MEMORY`) beside whatever else the tool holds.
pub const MAX_FILE: usize = 64 * 1024 * 1024;

/// Adds plugin tools to the geoprocessing tools. Cheap: the list is built when first needed.
pub fn register() {
    rustgis_analysis::register_external(SOURCE, specs, run);
}

/// Rebuilds the tool list the next time it's needed (a plugin was loaded, removed or toggled).
pub(crate) fn reload() {
    rustgis_analysis::reload_external_of(SOURCE);
    let _ = rustgis_analysis::external_tools_of(SOURCE);
}

/// A `'static` copy of `s`, shared, so reloading the list doesn't leak another copy of every
/// string.
fn leak(s: impl Into<String>) -> &'static str {
    static STRINGS: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let s = s.into();
    let Ok(mut set) = STRINGS.lock() else { return Box::leak(s.into_boxed_str()) };
    let set = set.get_or_insert_with(Default::default);
    if let Some(&have) = set.get(s.as_str()) {
        return have;
    }
    let new: &'static str = Box::leak(s.into_boxed_str());
    set.insert(new);
    new
}

fn kind(decl: &crate::manifest::ParamDecl) -> ParamKind {
    match decl.kind.as_str() {
        "layer" => ParamKind::Layer,
        "vectorLayer" => ParamKind::VectorLayer,
        "polygonLayer" => ParamKind::PolygonLayer,
        "pointLayer" => ParamKind::PointLayer,
        "field" => ParamKind::Field,
        "numericField" => ParamKind::NumericField,
        "number" => ParamKind::Number,
        "choice" => {
            let choices: Vec<&'static str> = decl.choices.iter().map(|c| leak(c.as_str())).collect();
            ParamKind::Choice(Box::leak(choices.into_boxed_slice()))
        }
        "bool" => ParamKind::Bool,
        "file" => ParamKind::Path,
        _ => ParamKind::Text,
    }
}

/// A tool's parameters, made once per package (the list is rebuilt whenever a plugin is loaded
/// or toggled, and the slices are leaked like the rest of a tool spec).
fn params_of(sha256: &str, t: &crate::manifest::ToolDecl) -> &'static [ParamSpec] {
    static MADE: Mutex<Option<std::collections::HashMap<String, &'static [ParamSpec]>>> = Mutex::new(None);
    let key = format!("{sha256}/{}", t.id);
    if let Ok(m) = MADE.lock()
        && let Some(have) = m.as_ref().and_then(|m| m.get(&key))
    {
        return have;
    }
    let params: Vec<ParamSpec> = t
        .params
        .iter()
        .map(|d| ParamSpec {
            name: leak(d.name.as_str()),
            label: leak(d.label.as_str()),
            kind: kind(d),
            required: d.required,
            default: d.default.as_ref().map(|v| leak(v.to_string())).unwrap_or(""),
            help: leak(d.help.as_str()),
        })
        .collect();
    let made: &'static [ParamSpec] = Box::leak(params.into_boxed_slice());
    if let Ok(mut m) = MADE.lock() {
        m.get_or_insert_with(Default::default).insert(key, made);
    }
    made
}

/// Verifies that a separately captured analysis specification belongs to this exact load's
/// manifest. The interned parameter owner is shared with registry publication and preparation;
/// no policy layer reconstructs defaults or accepts an id/hash as schema equality.
pub fn matches_pinned(plugin: &Arc<crate::Plugin>, spec: &ToolSpec) -> Result<(), String> {
    if !crate::is_current(plugin) {
        return Err("the plugin was disabled, updated or removed before its schema was pinned".into());
    }
    let (owner, id) = crate::split_id(spec.id).ok_or("not a plugin tool id")?;
    if owner != plugin.manifest.id {
        return Err("the tool belongs to another plugin".into());
    }
    let decl = plugin.manifest.tools.iter().find(|t| t.id == id).ok_or("the pinned plugin has no such tool")?;
    let toolbox = if decl.toolbox.trim().is_empty() { &plugin.manifest.name } else { &decl.toolbox };
    if spec.name != decl.name
        || spec.summary != decl.summary
        || spec.toolbox != format!("Plugins › {toolbox}")
        || !std::ptr::eq(spec.params, params_of(&plugin.sha256, decl))
    {
        return Err("the plugin tool's schema or display metadata changed before it was pinned".into());
    }
    Ok(())
}

fn specs() -> Vec<ToolSpec> {
    let mut out = Vec::new();
    for p in crate::list().into_iter().filter(|p| p.enabled) {
        let m = &p.manifest;
        for t in &m.tools {
            let params = params_of(&p.sha256, t);
            let toolbox = if t.toolbox.trim().is_empty() { &m.name } else { &t.toolbox };
            out.push(ToolSpec {
                id: leak(crate::command_id(&m.id, &t.id)),
                name: leak(t.name.as_str()),
                toolbox: leak(format!("Plugins › {toolbox}")),
                summary: leak(t.summary.as_str()),
                params,
                run: rustgis_analysis::external_placeholder,
            });
        }
    }
    out
}

/// A tool run's link to whoever started it, from another thread: it stops the run, and shows
/// the last line the tool logged or reported as progress.
#[derive(Default)]
pub struct Control {
    cancelled: AtomicBool,
    last: Mutex<Option<String>>,
}

impl Control {
    /// Stops the run: a process plugin's program is stopped, and the run fails as cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// The last line the tool logged or reported as progress.
    pub fn last_log(&self) -> Option<String> {
        self.last.lock().ok().and_then(|l| l.clone())
    }
}

/// Collects what the tool logs, for the run's messages.
struct Messages<'a> {
    lines: Vec<String>,
    control: &'a Control,
}

impl Host for Messages<'_> {
    fn call(&mut self, op: &str, _: &Value) -> Result<Value, String> {
        Err(format!("tools can't use '{op}': they work on the layers they're given"))
    }
    fn log(&mut self, text: &str) {
        if self.lines.len() < 200 {
            self.lines.push(text.to_string());
        }
        self.progress(text);
    }
    fn progress(&mut self, text: &str) {
        if let Ok(mut last) = self.control.last.lock() {
            *last = Some(text.chars().take(300).collect());
        }
    }
    fn cancelled(&self) -> bool {
        // An analysis caller carries its cancellation through the existing data progress probe.
        self.control.is_cancelled() || rustgis_data::progress::check().is_err()
    }
}

/// The layer `name` as a GeoJSON FeatureCollection: its selected features when it has a
/// selection (tools honour a selection like the built-in ones do).
fn layer_geojson(inputs: &dyn Inputs, name: &str) -> Result<Value, String> {
    let ds = inputs.dataset(name).ok_or_else(|| format!("no layer named '{name}'"))?;
    let sel = inputs.selection(name);
    // GeoJSON is longitude/latitude, whatever system the layer holds its coordinates in.
    if sel.is_empty() {
        return Ok(rustgis_data::io::to_geojson(&*ds.in_lonlat()?));
    }
    let sub = rustgis_data::Dataset {
        name: ds.name.clone(),
        geometry_type: ds.geometry_type,
        fields: ds.fields.clone(),
        features: sel.iter().filter_map(|&i| ds.features.get(i).cloned()).collect(),
        source: None,
        crs: None,
        crs_incomplete: false,
        zm_dropped: false,
        windowed: false,
        held_in: ds.held_in,
        shown_in: None,
    };
    Ok(rustgis_data::io::to_geojson(&*sub.in_lonlat()?))
}

fn run(spec: &ToolSpec, inputs: &dyn Inputs, params: &Value) -> Result<ToolResult, String> {
    prepare(spec, inputs, params)?.run(&Control::default())
}

/// A plugin tool's run, ready to go: the plugin and the request, its layers in it. It needs
/// nothing more from the session, so it can run on another thread while the app goes on.
pub struct Job {
    pub(crate) plugin: Arc<Plugin>,
    pub(crate) name: String,
    pub(crate) request: Value,
}

impl Job {
    /// Runs the tool (blocking until it ends), and reads what it returns.
    pub fn run(&self, control: &Control) -> Result<ToolResult, String> {
        if !crate::is_current(&self.plugin) {
            return Err(format!("{} changed or was disabled since this run was prepared", self.plugin.manifest.name));
        }
        let mut log = Messages { lines: Vec::new(), control };
        let reply = crate::call(&self.plugin, &mut log, &self.request, true);
        // A WebAssembly tool isn't stopped by a cancel; what it returns then is dropped.
        if control.is_cancelled() {
            return Err(format!("{}: cancelled", self.name));
        }
        let reply = reply.map_err(|e| format!("{}: {e}", self.name))?;
        let mut result = output(&reply, &self.name)?;
        log.lines.append(&mut result.messages);
        result.messages = log.lines;
        Ok(result)
    }

    /// The tool's name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Prepares a run of the plugin tool `spec`: its layers from `inputs` and its parameters
/// (resolved, with defaults) into the request.
pub fn prepare(spec: &ToolSpec, inputs: &dyn Inputs, params: &Value) -> Result<Job, String> {
    let (pid, tid) = split_id(spec.id).ok_or("not a plugin tool")?;
    let plugin = crate::get(pid).filter(|p| p.enabled).ok_or_else(|| format!("the plugin '{pid}' isn't loaded"))?;
    prepare_pinned(&plugin, tid, inputs, params)
}

/// Prepares the tool from this exact plugin load's manifest, without substituting a newly
/// loaded plugin with the same id. Callers pass parameters resolved against the pinned
/// schema. A stale load is refused before layers or file parameters are read.
pub fn prepare_pinned(plugin: &Arc<Plugin>, tid: &str, inputs: &dyn Inputs, params: &Value) -> Result<Job, String> {
    if !plugin.enabled {
        return Err(format!("the plugin '{}' is disabled", plugin.manifest.id));
    }
    if !crate::is_current(plugin) {
        return Err(format!("{} was updated or removed since this run was approved", plugin.manifest.name));
    }
    let decl = plugin.manifest.tools.iter().find(|t| t.id == tid).ok_or_else(|| format!("{} has no tool '{tid}'", plugin.manifest.name))?;
    let mut layers = Map::new();
    for p in decl.params.iter().filter(|p| is_layer_kind(&p.kind)) {
        if let Some(name) = params.get(&p.name).and_then(Value::as_str).filter(|n| !n.is_empty())
            && !layers.contains_key(name)
        {
            layers.insert(name.to_string(), layer_geojson(inputs, name)?);
        }
    }
    let request = request(decl, params, layers)?;
    Ok(Job { name: decl.name.clone(), plugin: plugin.clone(), request })
}

/// The request running the tool `decl`: its parameters and layers, and its files beside them
/// (`files`, by parameter), the parameters keeping only the files' names. A `file` parameter in
/// `path` mode keeps its file's full path instead, and its file isn't read.
pub(crate) fn request(decl: &crate::manifest::ToolDecl, params: &Value, layers: Map<String, Value>) -> Result<Value, String> {
    let mut params = params.clone();
    let mut files = Map::new();
    for p in decl.params.iter().filter(|p| p.kind == "file") {
        let Some(v) = params.get(&p.name).filter(|v| !v.is_null() && v.as_str() != Some("")) else { continue };
        if p.mode == crate::manifest::FileMode::Path {
            params[&p.name] = Value::String(file_path(v).map_err(|e| format!("{}: {e}", p.label))?);
            continue;
        }
        let file = file_input(v).map_err(|e| format!("{}: {e}", p.label))?;
        params[&p.name] = file["name"].clone();
        files.insert(p.name.clone(), file);
    }
    let mut request =
        json!({"abi": crate::manifest::ABI, "kind": "tool", "id": decl.id, "params": params, "layers": layers, "lang": crate::language()});
    if !files.is_empty() {
        request["files"] = Value::Object(files);
    }
    Ok(request)
}

/// What a `file` parameter's value names, as `{"name", "data"}` with the contents in base64. The
/// value is a path (the desktop and the command line), or a file the user picked in the
/// browser, which has no paths: already `{"name", "data"}`.
fn file_input(v: &Value) -> Result<Value, String> {
    use base64::Engine;
    if let Some(data) = v.get("data") {
        let data = data.as_str().ok_or("the file's data must be base64 text")?;
        if !is_base64(data) {
            return Err("the file's data must be base64 text".into());
        }
        // The decoded size, exactly, without decoding it.
        let padding = data.bytes().rev().take_while(|&c| c == b'=').count();
        if (data.len() / 4 * 3).saturating_sub(padding) > MAX_FILE {
            return Err(too_big());
        }
        let name = v.get("name").and_then(Value::as_str).map(base_name).filter(|n| !n.is_empty()).unwrap_or("file");
        return Ok(json!({"name": name, "data": data}));
    }
    let path = v.as_str().ok_or("give a file path")?;
    let bytes = read_file(path)?;
    Ok(json!({"name": base_name(path), "data": base64::engine::general_purpose::STANDARD.encode(bytes)}))
}

/// The full path of the file a `path` mode parameter names, checked to be a file but not read.
/// A relative path is made full here, as the plugin's program runs in its own folder.
#[cfg(not(target_arch = "wasm32"))]
fn file_path(v: &Value) -> Result<String, String> {
    let path = v.as_str().ok_or("give a file path")?;
    let full = std::path::absolute(path).map_err(|e| format!("{path}: {e}"))?;
    if !std::fs::metadata(&full).map_err(|e| format!("{path}: {e}"))?.is_file() {
        return Err(format!("{path} is not a file"));
    }
    full.into_os_string().into_string().map_err(|_| format!("{path}: the path isn't valid Unicode"))
}

/// Process plugins, the only ones with `path` mode parameters, don't run in the browser.
#[cfg(target_arch = "wasm32")]
fn file_path(_: &Value) -> Result<String, String> {
    Err("the browser has no file paths to give a plugin".into())
}

/// Whether `s` is standard, padded base64 (checked without decoding it).
fn is_base64(s: &str) -> bool {
    let b = s.as_bytes();
    let body = b.strip_suffix(b"==").or_else(|| b.strip_suffix(b"=")).unwrap_or(b);
    b.len().is_multiple_of(4) && body.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'+' || *c == b'/')
}

fn too_big() -> String {
    format!("the file is larger than {} MB", MAX_FILE / (1024 * 1024))
}

/// The last part of a path, `/` or `\` separated.
fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_file(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    if !f.metadata().map_err(|e| format!("{path}: {e}"))?.is_file() {
        return Err(format!("{path} is not a file"));
    }
    // Read one byte past the limit, so a file that grew since it was picked is still caught.
    let mut bytes = Vec::new();
    f.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("{path}: {e}"))?;
    if bytes.len() > MAX_FILE {
        return Err(too_big());
    }
    Ok(bytes)
}

#[cfg(target_arch = "wasm32")]
fn read_file(path: &str) -> Result<Vec<u8>, String> {
    Err(format!("{path}: the browser can't read a file by its path; pick it with Browse"))
}

/// The extensions a plugin tool's `file` parameter offers in the file picker (empty for any
/// file, and for a parameter that isn't a plugin's file).
pub fn file_extensions(tool: &str, param: &str) -> Vec<String> {
    let Some((pid, tid)) = split_id(tool) else { return Vec::new() };
    crate::get(pid)
        .and_then(|p| p.manifest.tools.iter().find(|t| t.id == tid)?.params.iter().find(|p| p.name == param && p.kind == "file").cloned())
        .map(|p| p.extensions)
        .unwrap_or_default()
}

/// Reads a dataset a tool returned: `{"name": "...", "geojson": {FeatureCollection}}`.
fn dataset(v: &Value, fallback: &str) -> Result<(rustgis_data::Dataset, rustgis_data::ReadReport), String> {
    let name = v.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty()).unwrap_or(fallback);
    let gj = v.get("geojson").ok_or_else(|| format!("{name}: an output has no 'geojson'"))?;
    if gj.get("features").and_then(Value::as_array).is_some_and(|f| f.len() > MAX_FEATURES) {
        return Err(format!("{name}: too many features"));
    }
    rustgis_data::io::read_geojson(&gj.to_string(), name).map_err(|e| format!("{name}: {e}"))
}

/// Formats a group of notices collected across one or more outputs (identified by output index)
/// into a single notice with output count, names, and distinct details.
fn format_grouped_notice(tool: &str, entries: &[(usize, String, String)]) -> String {
    const SHOWN: usize = 3;
    if let [(_, name, detail)] = entries {
        return format!("{name}: {detail}");
    }
    let mut outputs: Vec<(usize, &str)> = Vec::new();
    for (idx, name, _) in entries {
        if !outputs.iter().any(|(seen, _)| seen == idx) {
            outputs.push((*idx, name.as_str()));
        }
    }
    let mut details: Vec<&str> = Vec::new();
    for (_, _, d) in entries {
        if !details.contains(&d.as_str()) {
            details.push(d.as_str());
        }
    }
    let detail_text = if details.len() <= SHOWN {
        details.join("; ")
    } else {
        format!("{}; and {} more", details[..SHOWN].join("; "), details.len() - SHOWN)
    };
    if let [(_, only)] = outputs.as_slice() {
        format!("{only}: {} notices: {detail_text}", entries.len())
    } else {
        let listed = if outputs.len() <= SHOWN {
            outputs.iter().map(|(_, n)| *n).collect::<Vec<_>>().join(", ")
        } else {
            let first = outputs[..SHOWN].iter().map(|(_, n)| *n).collect::<Vec<_>>().join(", ");
            format!("{first}, and {} more", outputs.len() - SHOWN)
        };
        format!("{tool}: {} outputs ({listed}): {detail_text}", outputs.len())
    }
}

/// Turns a tool's reply into a result: new `layers` and `tables`, or a `selection`
/// (`{"layer": name, "rows": [...]}`), with `messages`.
fn output(reply: &Value, tool: &str) -> Result<ToolResult, String> {
    let mut messages: Vec<String> = reply
        .get("messages")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(200).map(str::to_string).collect())
        .unwrap_or_default();
    if let Some(sel) = reply.get("selection") {
        let layer = sel.get("layer").and_then(Value::as_str).ok_or("a selection names no layer")?.to_string();
        let rows: Vec<usize> = sel
            .get("rows")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).filter_map(|r| usize::try_from(r).ok()).collect())
            .unwrap_or_default();
        return Ok(ToolResult { losses: Vec::new(), output: Output::Selection { layer, rows }, messages });
    }
    let mut outs = Vec::new();
    let mut grouped_losses: Vec<(rustgis_data::loss::LossCode, Vec<(usize, String, String)>)> = Vec::new();
    let mut grouped_plain: Vec<(String, Vec<(usize, String, String)>)> = Vec::new();
    let mut output_idx = 0usize;
    let mut push_report = |idx: usize, name: &str, report: rustgis_data::ReadReport| {
        for note in report.messages {
            match note.loss {
                Some(code) => {
                    if let Some((_, entries)) = grouped_losses.iter_mut().find(|(c, _)| *c == code) {
                        entries.push((idx, name.to_string(), note.text));
                    } else {
                        grouped_losses.push((code, vec![(idx, name.to_string(), note.text)]));
                    }
                }
                None => {
                    if let Some((_, entries)) = grouped_plain.iter_mut().find(|(t, _)| *t == note.text) {
                        entries.push((idx, name.to_string(), note.text));
                    } else {
                        let key = note.text.clone();
                        grouped_plain.push((key, vec![(idx, name.to_string(), note.text)]));
                    }
                }
            }
        }
    };
    for v in reply.get("layers").and_then(Value::as_array).into_iter().flatten().take(64) {
        let (ds, report) = dataset(v, tool)?;
        push_report(output_idx, &ds.name, report);
        output_idx += 1;
        outs.push(OutputLayer::Vector(ds));
    }
    for v in reply.get("tables").and_then(Value::as_array).into_iter().flatten().take(64) {
        let (ds, report) = dataset(v, tool)?;
        push_report(output_idx, &ds.name, report);
        output_idx += 1;
        outs.push(OutputLayer::Table(ds));
    }
    let losses = grouped_losses
        .into_iter()
        .map(|(code, entries)| rustgis_data::loss::LossMessage { code, text: format_grouped_notice(tool, &entries) })
        .collect();
    for (_, entries) in grouped_plain {
        messages.push(format_grouped_notice(tool, &entries));
    }
    Ok(ToolResult { losses, output: Output::Layers(outs), messages })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::raster::Crs;
    use rustgis_data::{Coord, Dataset, Geometry, GeometryType};

    /// A plugin tool is given a layer held in another system in longitude/latitude, as GeoJSON is,
    /// whole or as its selection.
    #[test]
    fn a_held_layer_reaches_a_tool_in_longitude_latitude() {
        struct Held(Dataset, Vec<usize>);
        impl Inputs for Held {
            fn dataset(&self, l: &str) -> Option<&Dataset> {
                (l == "Wells").then_some(&self.0)
            }
            fn selection(&self, _: &str) -> &[usize] {
                &self.1
            }
        }
        let mut ds = Dataset::new("Wells", GeometryType::Point, vec![]);
        ds.push(Some(Geometry::Point(Coord::new(11.5, 48.7))), vec![]);
        ds.push(Some(Geometry::Point(Coord::new(11.6, 48.8))), vec![]);
        ds.hold_in(Crs::Epsg(32632)).unwrap();
        let near = |v: &Value, lon: f64, lat: f64| {
            let c = &v["geometry"]["coordinates"];
            (c[0].as_f64().unwrap() - lon).abs() < 1e-9 && (c[1].as_f64().unwrap() - lat).abs() < 1e-9
        };
        let whole = layer_geojson(&Held(ds.clone(), vec![]), "Wells").unwrap();
        let f = whole["features"].as_array().unwrap();
        assert_eq!(f.len(), 2);
        assert!(near(&f[0], 11.5, 48.7) && near(&f[1], 11.6, 48.8), "{whole}");
        let picked = layer_geojson(&Held(ds, vec![1]), "Wells").unwrap();
        let f = picked["features"].as_array().unwrap();
        assert_eq!(f.len(), 1);
        assert!(near(&f[0], 11.6, 48.8), "{picked}");
    }

    /// GeoJSON loss notices from plugin tool outputs (layers and tables) reach `ToolResult.losses`
    /// grouped as one notice per `LossCode` across all outputs (with count and layer names), while
    /// clean outputs emit none.
    #[test]
    fn plugin_tool_output_propagates_geojson_loss_notices_and_stays_clean_when_lossless() {
        let clean = json!({
            "layers": [{
                "name": "Clean",
                "geojson": {
                    "type": "FeatureCollection",
                    "features": [{"type": "Feature", "properties": {"a": 1}, "geometry": {"type": "Point", "coordinates": [10.0, 50.0]}}]
                }
            }],
            "tables": [{
                "name": "CleanTable",
                "geojson": {
                    "type": "FeatureCollection",
                    "features": [{"type": "Feature", "properties": {"b": 2}, "geometry": null}]
                }
            }]
        });
        let res = output(&clean, "demo").unwrap();
        assert!(res.losses.is_empty(), "clean output must have no losses: {:?}", res.losses);
        assert!(res.messages.is_empty(), "clean output must have no messages: {:?}", res.messages);

        // Single lossy output keeps its direct layer-prefixed message.
        let single = json!({
            "layers": [{
                "name": "Solo",
                "geojson": {
                    "type": "FeatureCollection",
                    "features": [{"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [10.0, 50.0, 100.0]}}]
                }
            }]
        });
        let res = output(&single, "demo").unwrap();
        assert_eq!(res.losses.len(), 1, "{:?}", res.losses);
        assert_eq!(res.losses[0].code, rustgis_data::loss::LossCode::DroppedZm);
        assert!(res.losses[0].text.starts_with("Solo: "), "{:?}", res.losses[0].text);
        assert!(res.messages.is_empty(), "classified losses belong in losses only: {:?}", res.messages);

        // Multiple outputs sharing a loss kind emit one grouped notice per LossCode with count and names.
        let point_3d = |name: &str, crs: Option<&str>| {
            let mut fc = json!({
                "type": "FeatureCollection",
                "features": [{"type": "Feature", "properties": {"name": name}, "geometry": {"type": "Point", "coordinates": [10.0, 50.0, 1200.0]}}]
            });
            if let Some(crs) = crs {
                fc["crs"] = json!({"type": "name", "properties": {"name": crs}});
            }
            json!({"name": name, "geojson": fc})
        };
        let lossy = json!({
            "layers": [
                point_3d("Peaks", Some("urn:ogc:def:crs:CUSTOM::999")),
                point_3d("Ridges", None),
                point_3d("Valleys", None),
                point_3d("Passes", None),
            ],
            "tables": [{
                "name": "Summary",
                "geojson": {
                    "type": "FeatureCollection",
                    "crs": {"type": "name", "properties": {"name": "urn:ogc:def:crs:CUSTOM::888"}},
                    "features": [{"type": "Feature", "properties": {"n": 1}, "geometry": null}]
                }
            }]
        });
        let res = output(&lossy, "demo").unwrap();
        assert_eq!(res.losses.len(), 2, "must emit exactly one LossMessage per LossCode: {:?}", res.losses);
        let zm = res.losses.iter().find(|l| l.code == rustgis_data::loss::LossCode::DroppedZm).expect("DroppedZm loss");
        assert!(
            zm.text.contains("4 outputs (Peaks, Ridges, Valleys, and 1 more)"),
            "expected grouped count and layer names for DroppedZm, got: {}",
            zm.text
        );
        let crs = res.losses.iter().find(|l| l.code == rustgis_data::loss::LossCode::AssumedCrs).expect("AssumedCrs loss");
        assert!(
            crs.text.contains("2 outputs (Peaks, Summary)") && crs.text.contains("CUSTOM::999") && crs.text.contains("CUSTOM::888"),
            "expected grouped count, output names, and both distinct CRS details for AssumedCrs, got: {}",
            crs.text
        );
        assert!(res.messages.is_empty(), "classified losses belong in losses only: {:?}", res.messages);

        // Unnamed outputs share the fallback tool name, but each output is still counted accurately.
        let unnamed = json!({
            "layers": [
                {"geojson": {"type": "FeatureCollection", "features": [{"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [10.0, 50.0, 1.0]}}]}},
                {"geojson": {"type": "FeatureCollection", "features": [{"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [11.0, 51.0, 2.0]}}]}},
                point_3d("Peaks", None),
            ]
        });
        let res = output(&unnamed, "demo").unwrap();
        assert_eq!(res.losses.len(), 1, "{:?}", res.losses);
        assert!(
            res.losses[0].text.contains("3 outputs (demo, demo, Peaks)"),
            "two unnamed outputs plus Peaks must count as 3 outputs, got: {}",
            res.losses[0].text
        );

        // Plain (unclassified) GeoJSON notes (e.g. IAU planetocentric CRS) are grouped across outputs too.
        let mars_layer = |name: &str| {
            json!({
                "name": name,
                "geojson": {
                    "type": "FeatureCollection",
                    "crs": {"type": "name", "properties": {"name": "urn:ogc:def:crs:IAU_2015::49900"}},
                    "features": [{"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [10.0, 20.0]}}]
                }
            })
        };
        let plain = json!({
            "layers": [mars_layer("MarsA"), mars_layer("MarsB"), mars_layer("MarsC")]
        });
        let res = output(&plain, "demo").unwrap();
        assert!(res.losses.is_empty(), "IAU planetocentric note is plain, not a loss: {:?}", res.losses);
        assert_eq!(res.messages.len(), 1, "identical plain notes across outputs must group into 1 message: {:?}", res.messages);
        assert!(
            res.messages[0].contains("3 outputs (MarsA, MarsB, MarsC)") && res.messages[0].contains("IAU_2015::49900"),
            "expected grouped plain note with count and output names, got: {}",
            res.messages[0]
        );
    }
}
