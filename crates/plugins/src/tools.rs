//! Plugin tools in the Tools pane: each enabled plugin's tools join the external tools
//! (under "Plugins ›" toolboxes) and run in the plugin's module with their layers as GeoJSON.

use std::collections::HashSet;
use std::sync::Mutex;

use rustgis_analysis::{Inputs, Output, OutputLayer, ParamKind, ParamSpec, ToolResult, ToolSpec};
use serde_json::{Map, Value, json};

use crate::manifest::is_layer_kind;
use crate::{Host, split_id};

/// The key plugin tools are registered under.
pub const SOURCE: &str = "plugins";

/// The most features a tool may return.
const MAX_FEATURES: usize = 5_000_000;

/// Adds plugin tools to the geoprocessing tools. Cheap: the list is built when first needed.
pub fn register() {
    rustgis_analysis::register_external(SOURCE, specs, run);
}

/// Rebuilds the tool list the next time it's needed (a plugin was loaded, removed or toggled).
pub(crate) fn reload() {
    rustgis_analysis::reload_external_of(SOURCE);
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

/// Collects what the tool logs, for the run's messages.
struct Messages(Vec<String>);

impl Host for Messages {
    fn call(&mut self, op: &str, _: &Value) -> Result<Value, String> {
        Err(format!("tools can't use '{op}': they work on the layers they're given"))
    }
    fn log(&mut self, text: &str) {
        if self.0.len() < 200 {
            self.0.push(text.to_string());
        }
    }
}

/// The layer `name` as a GeoJSON FeatureCollection: its selected features when it has a
/// selection (tools honour a selection like the built-in ones do).
fn layer_geojson(inputs: &dyn Inputs, name: &str) -> Result<Value, String> {
    let ds = inputs.dataset(name).ok_or_else(|| format!("no layer named '{name}'"))?;
    let sel = inputs.selection(name);
    if sel.is_empty() {
        return Ok(rustgis_data::io::to_geojson(ds));
    }
    let sub = rustgis_data::Dataset {
        name: ds.name.clone(),
        geometry_type: ds.geometry_type,
        fields: ds.fields.clone(),
        features: sel.iter().filter_map(|&i| ds.features.get(i).cloned()).collect(),
        source: None,
        windowed: false,
    };
    Ok(rustgis_data::io::to_geojson(&sub))
}

fn run(spec: &ToolSpec, inputs: &dyn Inputs, params: &Value) -> Result<ToolResult, String> {
    let (pid, tid) = split_id(spec.id).ok_or("not a plugin tool")?;
    let plugin = crate::get(pid).filter(|p| p.enabled).ok_or_else(|| format!("the plugin '{pid}' isn't loaded"))?;
    let decl = plugin.manifest.tools.iter().find(|t| t.id == tid).ok_or_else(|| format!("{} has no tool '{tid}'", plugin.manifest.name))?;
    let mut layers = Map::new();
    for p in decl.params.iter().filter(|p| is_layer_kind(&p.kind)) {
        if let Some(name) = params.get(&p.name).and_then(Value::as_str).filter(|n| !n.is_empty())
            && !layers.contains_key(name)
        {
            layers.insert(name.to_string(), layer_geojson(inputs, name)?);
        }
    }
    let request =
        json!({"abi": crate::manifest::ABI, "kind": "tool", "id": tid, "params": params, "layers": layers, "lang": crate::language()});
    let mut log = Messages(Vec::new());
    let reply = crate::call(&plugin, &mut log, &request, crate::runtime::TOOL_FUEL).map_err(|e| format!("{}: {e}", decl.name))?;
    let mut result = output(&reply, &decl.name)?;
    log.0.append(&mut result.messages);
    result.messages = log.0;
    Ok(result)
}

/// Reads a dataset a tool returned: `{"name": "...", "geojson": {FeatureCollection}}`.
fn dataset(v: &Value, fallback: &str) -> Result<rustgis_data::Dataset, String> {
    let name = v.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty()).unwrap_or(fallback);
    let gj = v.get("geojson").ok_or_else(|| format!("{name}: an output has no 'geojson'"))?;
    if gj.get("features").and_then(Value::as_array).is_some_and(|f| f.len() > MAX_FEATURES) {
        return Err(format!("{name}: too many features"));
    }
    let (ds, _) = rustgis_data::io::read_geojson(&gj.to_string(), name).map_err(|e| format!("{name}: {e}"))?;
    Ok(ds)
}

/// Turns a tool's reply into a result: new `layers` and `tables`, or a `selection`
/// (`{"layer": name, "rows": [...]}`), with `messages`.
fn output(reply: &Value, tool: &str) -> Result<ToolResult, String> {
    let messages: Vec<String> = reply
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
        return Ok(ToolResult { output: Output::Selection { layer, rows }, messages });
    }
    let mut outs = Vec::new();
    for v in reply.get("layers").and_then(Value::as_array).into_iter().flatten().take(64) {
        outs.push(OutputLayer::Vector(dataset(v, tool)?));
    }
    for v in reply.get("tables").and_then(Value::as_array).into_iter().flatten().take(64) {
        outs.push(OutputLayer::Table(dataset(v, tool)?));
    }
    Ok(ToolResult { output: Output::Layers(outs), messages })
}
