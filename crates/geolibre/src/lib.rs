//! L3: the GeoLibre tool suite (Whitebox's tools plus GeoLibre's own: 1,000+ raster, vector,
//! LiDAR, hydrology and terrain tools) as geoprocessing tools.
//!
//! The suite is an optional download: the `geolibre` runner, a separate executable (see
//! [`runner`]). [`register`] adds its tools to [`rustgis_analysis`], which lists them next to
//! the built-in tools under "GeoLibre ›" toolboxes; without a runner there are none. Their
//! forms come from the runner's manifests: each parameter says whether it is a raster or vector
//! input, an output, a choice, a number or a switch.
//!
//! The tools read and write files, so a run stages its inputs in a temporary folder: raster
//! layers as their own file (or the embedded copy), vector layers as GeoJSON. Vector data goes
//! in a metric coordinate system (the first raster input's, or the UTM zone of the data), so
//! distances and areas are in metres; outputs come back as new layers in longitude/latitude.
//! Native only: the browser build has no file system for the tools to use.
//!
//! Like Add Data, a tool reads any file named in its parameters (a layer name that isn't a
//! layer is taken as a path, and LiDAR inputs are paths), with the user's permissions. The
//! outputs a tool declares are written to the run's own scratch folder; the runner is trusted
//! like any program the user installs, so a tool could write elsewhere if asked to.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod runner;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rustgis_analysis::{Inputs, Output, OutputLayer, ParamKind, ParamSpec, ToolResult, ToolSpec};
use rustgis_data::raster::{Crs, Transformer};
use rustgis_data::{Coord, Dataset, Extent, Geometry};
use serde_json::{Value, json};

/// Tool ids get this prefix, so they never clash with the built-in tools.
pub const PREFIX: &str = "geolibre:";

/// Adds the suite to the geoprocessing tools. The runner is found and its manifests read the
/// first time the tools are listed.
pub fn register() {
    rustgis_analysis::register_external(specs, run);
}

/// Uses `exe` as the runner (`None`: look in the usual places), and reloads the tool list.
/// Returns the runner in use and its version.
pub fn set_runner(exe: Option<PathBuf>) -> Result<Option<(PathBuf, String)>, String> {
    let found = runner::choose(exe)?;
    rustgis_analysis::reload_external();
    Ok(found)
}

/// What a parameter is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Input,
    Output,
    Option,
}

/// The kind of data an input or output holds (the manifest's `data_kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Data {
    Raster,
    Vector,
    Table,
    Lidar,
    Text,
    Json,
    /// Unspecified: a file of any kind.
    File,
    Bool,
    Number,
    Other,
}

impl Data {
    fn of(s: &str) -> Data {
        match s {
            "raster" => Data::Raster,
            "vector" => Data::Vector,
            "table" => Data::Table,
            "lidar" => Data::Lidar,
            "text" => Data::Text,
            "json" => Data::Json,
            "file" => Data::File,
            "bool" => Data::Bool,
            "number" => Data::Number,
            _ => Data::Other,
        }
    }
}

#[derive(Clone, Debug)]
struct MetaParam {
    name: String,
    role: Role,
    data: Data,
}

/// What a run needs to know about a tool beyond its [`ToolSpec`].
#[derive(Clone, Debug)]
struct Meta {
    category: String,
    params: Vec<MetaParam>,
}

static META: Mutex<Option<HashMap<String, Meta>>> = Mutex::new(None);

fn leak(s: impl Into<String>) -> &'static str {
    Box::leak(s.into().into_boxed_str())
}

/// `z_factor` → `Z factor`.
fn label_of(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut c = words.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Builds every tool's spec from the runner's manifests (none without a runner).
fn specs() -> Vec<ToolSpec> {
    let manifests = runner::manifests().unwrap_or_else(|e| {
        log::info!("GeoLibre tools: {e}");
        Vec::new()
    });
    let mut meta = HashMap::new();
    let mut out: Vec<ToolSpec> = manifests
        .iter()
        .filter_map(|m| {
            let (spec, info) = spec_of(m)?;
            meta.insert(spec.id.trim_start_matches(PREFIX).to_string(), info);
            Some(spec)
        })
        .collect();
    if let Ok(mut m) = META.lock() {
        *m = Some(meta);
    }
    out.sort_by(|a, b| a.toolbox.cmp(b.toolbox).then(a.name.cmp(b.name)));
    out
}

fn spec_of(m: &Value) -> Option<(ToolSpec, Meta)> {
    let id = m.get("id")?.as_str()?;
    let name = m.get("display_name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(id);
    let category = m.get("category").and_then(Value::as_str).unwrap_or("Other").to_string();
    let summary = m.get("summary").and_then(Value::as_str).unwrap_or("");
    let defaults = m.get("defaults");
    let mut params = Vec::new();
    let mut meta = Vec::new();
    for p in m.get("params").and_then(Value::as_array).into_iter().flatten() {
        let Some(pname) = p.get("name").and_then(Value::as_str) else { continue };
        let schema = p.get("schema");
        let data = Data::of(p.get("data_kind").and_then(Value::as_str).unwrap_or(""));
        let role = match p.get("io_role").and_then(Value::as_str) {
            Some("input") => Role::Input,
            Some("output") => Role::Output,
            _ => Role::Option,
        };
        let many = schema.and_then(|s| s.get("cardinality")).and_then(Value::as_str).is_some_and(|c| c != "single");
        meta.push(MetaParam { name: pname.to_string(), role, data });
        // Outputs are written to a scratch folder and come back as layers: no form field.
        if role == Role::Output {
            continue;
        }
        let kind = match role {
            Role::Input if many => ParamKind::Text,
            Role::Input => match data {
                Data::Raster => ParamKind::RasterLayer,
                Data::Vector => ParamKind::VectorLayer,
                Data::Table => ParamKind::Layer,
                _ => ParamKind::Path,
            },
            _ => match schema.and_then(|s| s.get("kind")).and_then(Value::as_str) {
                Some("enum") => {
                    let options: Vec<&'static str> = schema
                        .and_then(|s| s.get("options"))
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|o| o.get("value").and_then(Value::as_str).map(leak))
                        .collect();
                    if options.is_empty() { ParamKind::Text } else { ParamKind::Choice(Box::leak(options.into_boxed_slice())) }
                }
                _ if data == Data::Bool => ParamKind::Bool,
                _ if data == Data::Number => ParamKind::Number,
                _ => ParamKind::Text,
            },
        };
        // Example file names in the defaults ("dem.tif") are not defaults for a layer.
        let default = match role {
            Role::Input => String::new(),
            _ => defaults.and_then(|d| d.get(pname)).filter(|v| !v.is_null()).map(Value::to_string).unwrap_or_default(),
        };
        let mut help = p.get("description").and_then(Value::as_str).unwrap_or("").to_string();
        if many {
            help.push_str(" (layer names or file paths, separated by ;)");
        }
        params.push(ParamSpec {
            name: leak(pname),
            label: leak(label_of(pname)),
            kind,
            required: p.get("required").and_then(Value::as_bool).unwrap_or(false),
            default: leak(default),
            help: leak(help),
        });
    }
    let spec = ToolSpec {
        id: leak(format!("{PREFIX}{id}")),
        name: leak(name),
        toolbox: leak(format!("GeoLibre › {category}")),
        summary: leak(summary),
        params: Box::leak(params.into_boxed_slice()),
        run: rustgis_analysis::external_placeholder,
    };
    Some((spec, Meta { category, params: meta }))
}

/// A scratch folder for one run, removed afterwards.
struct Scratch {
    dir: PathBuf,
    /// Staged inputs so far, so each gets its own file name.
    staged: std::cell::Cell<usize>,
}

impl Scratch {
    /// A new, empty folder: never one that already exists (left over, or made by someone else
    /// on a shared machine), so no old file can pass for a tool's output.
    fn new() -> Result<Scratch, String> {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let mut last = None;
        for _ in 0..16 {
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("rustgis-gp-{}-{nanos:x}-{n}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Scratch { dir, staged: std::cell::Cell::new(0) }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(format!("can't create a scratch folder: {e}")),
            }
        }
        Err(format!("can't create a scratch folder: {}", last.map(|e| e.to_string()).unwrap_or_default()))
    }
}

impl Scratch {
    /// A new file name for a staged input. Numbered, because two layer names can map to the
    /// same safe name (`Roads 2020` and `Roads_2020`, or any two non-ASCII names), and one
    /// file must never replace another.
    fn input(&self, name: &str, ext: &str) -> PathBuf {
        let n = self.staged.get();
        self.staged.set(n + 1);
        self.dir.join(format!("in{n}-{}.{ext}", safe(name)))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `g` with every point converted by `f`, or `None` when a point can't be (rather than
/// geometry with null coordinates).
fn convert(g: &Geometry, f: impl Fn(Coord) -> Option<Coord>) -> Option<Geometry> {
    let ok = std::cell::Cell::new(true);
    let out = g.map_coords(&|c| {
        f(c).filter(|m| m.is_finite()).unwrap_or_else(|| {
            ok.set(false);
            c
        })
    });
    ok.get().then_some(out)
}

/// Converts every feature's geometry, clearing those that can't be converted; returns how many.
fn convert_features(ds: &mut Dataset, f: impl Fn(Coord) -> Option<Coord>) -> usize {
    let mut lost = 0;
    for ft in &mut ds.features {
        if let Some(g) = &ft.geometry {
            ft.geometry = convert(g, &f);
            lost += usize::from(ft.geometry.is_none());
        }
    }
    lost
}

/// A file-name-safe version of a layer name.
fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(60).collect();
    if s.is_empty() { "layer".into() } else { s }
}

/// The coordinate system vector inputs are written in: the first raster input's, else the UTM
/// zone around the vector data (metres), else longitude/latitude for data spanning the world.
fn working_crs(rasters: &[Crs], vectors: &[Extent]) -> Crs {
    if let Some(c) = rasters.first() {
        return *c;
    }
    let e = vectors.iter().fold(Extent::EMPTY, |a, b| a.union(b));
    if e.is_empty() || e.width() > 40.0 {
        return Crs::LonLat;
    }
    let c = e.center();
    let zone = (((c.x + 180.0) / 6.0).floor() as i64 + 1).clamp(1, 60) as u32;
    Crs::from_epsg(if c.y >= 0.0 { 32600 + zone } else { 32700 + zone }).unwrap_or(Crs::LonLat)
}

/// A vector layer (its selection, if any) as GeoJSON in `crs`; returns how many features'
/// geometry couldn't be converted (they're written without geometry).
fn write_vector(ds: &Dataset, rows: &[usize], crs: Crs, tf: &Transformer, path: &Path) -> Result<usize, String> {
    let mut out = ds.clone();
    if !rows.is_empty() {
        out.features = rows.iter().filter_map(|&r| ds.features.get(r).cloned()).collect();
    }
    let lost = if crs != Crs::LonLat { convert_features(&mut out, |c| tf.from_lonlat(c)) } else { 0 };
    let mut json = rustgis_data::io::to_geojson(&out);
    if crs != Crs::LonLat
        && let Some(o) = json.as_object_mut()
    {
        // The legacy GeoJSON 2008 `crs` member, which the suite's reader honours.
        o.insert("crs".into(), json!({"type": "name", "properties": {"name": format!("urn:ogc:def:crs:EPSG::{}", crs.epsg())}}));
    }
    let text = serde_json::to_string(&json).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("can't write {}: {e}", path.display()))?;
    Ok(lost)
}

/// Writes one input (a layer by name, or a file path) to the scratch folder; returns its path.
/// Problems that don't stop the run go to `notes`.
fn stage(
    inputs: &dyn Inputs,
    value: &str,
    data: Data,
    crs: Crs,
    tf: &Transformer,
    scratch: &Scratch,
    notes: &mut Vec<String>,
) -> Result<String, String> {
    if let Some(r) = inputs.raster(value) {
        if let Some(p) = r.path.filter(|p| Path::new(p).is_file()) {
            return Ok(p.to_string());
        }
        if let Some((file_name, bytes)) = r.embedded {
            // The tools tell formats apart by extension.
            let ext = Path::new(file_name).extension().and_then(|e| e.to_str()).map(safe).unwrap_or_else(|| "tif".into());
            let path = scratch.input(value, &ext);
            std::fs::write(&path, bytes).map_err(|e| format!("can't write {}: {e}", path.display()))?;
            return Ok(path.display().to_string());
        }
        let bytes = rustgis_data::raster::geotiff::write(r.raster, 0).map_err(|e| format!("{value}: {e}"))?;
        let path = scratch.input(value, "tif");
        std::fs::write(&path, bytes).map_err(|e| format!("can't write {}: {e}", path.display()))?;
        return Ok(path.display().to_string());
    }
    if let Some(ds) = inputs.dataset(value) {
        if data == Data::Table || (ds.features.iter().all(|f| f.geometry.is_none()) && data != Data::Vector) {
            let path = scratch.input(value, "csv");
            let text = rustgis_data::io::to_csv(ds).map_err(|e| format!("{value}: {e}"))?;
            std::fs::write(&path, text).map_err(|e| format!("can't write {}: {e}", path.display()))?;
            return Ok(path.display().to_string());
        }
        let path = scratch.input(value, "geojson");
        let lost = write_vector(ds, inputs.selection(value), crs, tf, &path)?;
        if lost > 0 {
            notes.push(format!("{value}: {lost} features lie outside the working coordinate system and were passed without geometry"));
        }
        return Ok(path.display().to_string());
    }
    if Path::new(value).exists() {
        return Ok(value.to_string());
    }
    Err(format!("there is no layer or file named '{value}'"))
}

/// The extension an output is written with.
fn output_ext(p: &MetaParam, meta: &Meta) -> &'static str {
    let has_raster_input = meta.params.iter().any(|q| q.role == Role::Input && q.data == Data::Raster);
    match p.data {
        Data::Raster => "tif",
        Data::Vector => "geojson",
        Data::Table => "csv",
        Data::Lidar => "las",
        Data::Text => "txt",
        Data::Json => "json",
        _ if p.name.contains("html") => "html",
        _ if p.name.contains("csv") => "csv",
        _ if p.name.contains("json") => "json",
        _ if matches!(meta.category.as_str(), "Raster" | "Terrain" | "Hydrology") || has_raster_input => "tif",
        _ => "geojson",
    }
}

/// A form value as the JSON the tool expects (the form sends text).
fn option_value(v: &Value, data: Data) -> Value {
    match (v.as_str().map(str::trim), data) {
        (Some("true"), Data::Bool) => Value::Bool(true),
        (Some("false"), Data::Bool) => Value::Bool(false),
        (Some(s), Data::Number) => s.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| json!(n)).unwrap_or_else(|| v.clone()),
        _ => v.clone(),
    }
}

/// Coordinates that can't be longitude/latitude (a tool kept the working coordinate system).
fn looks_projected(ds: &Dataset) -> bool {
    let e = ds.extent();
    !e.is_empty() && (e.min_x < -180.5 || e.max_x > 180.5 || e.min_y < -90.5 || e.max_y > 90.5)
}

/// Reads one output file back as a layer, table or message.
fn load(path: &Path, layer_name: &str, crs: Crs, tf: &Transformer, messages: &mut Vec<String>) -> Option<OutputLayer> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("output").to_string();
    match ext.as_str() {
        "tif" | "tiff" | "asc" => {
            let bytes = std::fs::read(path).ok()?;
            match rustgis_data::raster::read_bytes(&file_name, &bytes) {
                Ok(_) => Some(OutputLayer::Raster { name: layer_name.to_string(), file_name, bytes }),
                Err(e) => {
                    messages.push(format!("{layer_name}: {e}"));
                    None
                }
            }
        }
        "geojson" | "json" | "shp" | "csv" => match rustgis_data::io::read_path(path) {
            Ok((mut ds, _)) => {
                ds.name = layer_name.to_string();
                if crs != Crs::LonLat && looks_projected(&ds) {
                    let lost = convert_features(&mut ds, |c| tf.to_lonlat(c));
                    if lost > 0 {
                        messages.push(format!(
                            "{layer_name}: {lost} features couldn't be converted to longitude/latitude and have no geometry"
                        ));
                    }
                }
                Some(if ds.features.iter().all(|f| f.geometry.is_none()) { OutputLayer::Table(ds) } else { OutputLayer::Vector(ds) })
            }
            // A JSON report rather than GeoJSON: show it.
            Err(_) if ext == "json" => {
                text_message(path, layer_name, messages);
                None
            }
            Err(e) => {
                messages.push(format!("{layer_name}: {e}"));
                None
            }
        },
        "txt" | "html" | "htm" => {
            text_message(path, layer_name, messages);
            None
        }
        _ => {
            messages.push(format!("{layer_name}: wrote a .{ext} file, which RustGIS can't open as a layer"));
            None
        }
    }
}

/// The start of a text or HTML report as a message.
fn text_message(path: &Path, layer_name: &str, messages: &mut Vec<String>) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let mut plain = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => plain.push(c),
            _ => {}
        }
    }
    let lines: Vec<&str> = plain.lines().map(str::trim).filter(|l| !l.is_empty()).take(30).collect();
    if !lines.is_empty() {
        messages.push(format!("{layer_name}:\n{}", lines.join("\n")));
    }
}

fn run(spec: &ToolSpec, inputs: &dyn Inputs, params: &Value) -> Result<ToolResult, String> {
    let id = spec.id.strip_prefix(PREFIX).ok_or("not a GeoLibre tool")?;
    let meta =
        META.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(id)).cloned()).ok_or_else(|| format!("{}: unknown tool", spec.name))?;
    let meta = &meta;
    let scratch = Scratch::new()?;
    let value_of = |name: &str| params.get(name).filter(|v| !v.is_null() && v.as_str() != Some(""));
    let names = |v: &Value| -> Vec<String> {
        v.as_str().map(|s| s.split(';').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect()).unwrap_or_default()
    };

    // Rasters set the coordinate system; otherwise the vector data's UTM zone.
    let mut raster_crs = Vec::new();
    let mut vector_extents = Vec::new();
    for p in meta.params.iter().filter(|p| p.role == Role::Input) {
        for n in value_of(&p.name).map(names).unwrap_or_default() {
            if let Some(r) = inputs.raster(&n) {
                raster_crs.push(r.raster.crs);
            } else if let Some(ds) = inputs.dataset(&n) {
                vector_extents.push(ds.extent());
            }
        }
    }
    let crs = working_crs(&raster_crs, &vector_extents);
    let tf = crs.transformer()?;

    let mut args: Vec<(String, Value)> = Vec::new();
    let mut notes = Vec::new();
    let mut outputs = Vec::new();
    let n_outputs = meta.params.iter().filter(|p| p.role == Role::Output).count();
    for p in &meta.params {
        match p.role {
            Role::Input => {
                let Some(v) = value_of(&p.name) else { continue };
                let paths =
                    names(v).iter().map(|n| stage(inputs, n, p.data, crs, &tf, &scratch, &mut notes)).collect::<Result<Vec<_>, _>>()?;
                if !paths.is_empty() {
                    args.push((p.name.clone(), Value::String(paths.join(";"))));
                }
            }
            Role::Output => {
                let path = scratch.dir.join(format!("{}.{}", safe(&p.name), output_ext(p, meta)));
                args.push((p.name.clone(), Value::String(path.display().to_string())));
                let name =
                    if n_outputs > 1 { format!("{} {}", spec.name, label_of(&p.name).to_lowercase()) } else { spec.name.to_string() };
                outputs.push((path, name));
            }
            Role::Option => {
                if let Some(v) = value_of(&p.name) {
                    args.push((p.name.clone(), option_value(v, p.data)));
                }
            }
        }
    }

    let ran = runner::run(id, &args, &scratch.dir).map_err(|e| format!("{}: {e}", spec.name))?;
    let mut messages = notes;
    messages.extend(ran.messages);

    let mut layers = Vec::new();
    for (path, name) in &outputs {
        // A tool may pick its own extension (a shapefile for a vector output).
        let found = if path.is_file() {
            Some(path.clone())
        } else {
            let stem = path.file_stem().map(|s| s.to_os_string());
            std::fs::read_dir(&scratch.dir).ok().into_iter().flatten().filter_map(|e| e.ok().map(|e| e.path())).find(|f| {
                f.file_stem().map(|s| s.to_os_string()) == stem && f.extension().is_some_and(|e| e != "dbf" && e != "shx" && e != "prj")
            })
        };
        if let Some(f) = found
            && let Some(l) = load(&f, name, crs, &tf, &mut messages)
        {
            layers.push(l);
        }
    }
    // Statistics and other results the tool returns rather than writes.
    let scratch_dir = scratch.dir.display().to_string();
    for (k, v) in &ran.outputs {
        let text = match v {
            Value::String(s) if s.starts_with(&scratch_dir) => continue,
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let text: String = text.chars().take(2000).collect();
        messages.push(format!("{}: {text}", label_of(k)));
    }
    if layers.is_empty() && messages.is_empty() {
        messages.push(format!("{} finished without output", spec.name));
    }
    Ok(ToolResult { output: Output::Layers(layers), messages })
}

#[cfg(test)]
mod tests;
