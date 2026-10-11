//! L2: geoprocessing tools.
//!
//! Each tool is a [`ToolSpec`]: an id, a toolbox, parameters for the Geoprocessing pane, and a
//! pure `run` function over datasets. Inputs refer to layers by name through [`Inputs`]. Work is
//! done in Web Mercator metres; distances are corrected for scale distortion at each feature's
//! latitude so a 100 km buffer is about 100 km on the ground.
//!
//! Tools from an external suite (the GeoLibre/Whitebox tools, in `rustgis-geolibre`) join
//! the built-in ones through [`register_external`]: they're listed by [`tools`], found by
//! [`tool`] and run by [`run`] like the rest.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod convert;
mod tools;

pub use tools::TOOLS;

use rustgis_data::Dataset;
use serde_json::Value;

/// Parameter kinds, which the Geoprocessing pane turns into widgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// Any layer.
    Layer,
    /// A polygon layer.
    PolygonLayer,
    /// A point layer.
    PointLayer,
    /// A field of the layer given by the first `Layer` parameter.
    Field,
    /// A numeric field of the first layer.
    NumericField,
    Number,
    Text,
    /// One of a fixed list of choices.
    Choice(&'static [&'static str]),
    /// A raster layer.
    RasterLayer,
    /// Any vector layer (no standalone tables).
    VectorLayer,
    /// On or off.
    Bool,
    /// A file path typed by the user (for data RustGIS doesn't open as a layer, such as LiDAR).
    Path,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    /// Default value as JSON text (`"10"`, `"\"km\""`), or empty.
    pub default: &'static str,
    /// A longer description, shown on hover (may be empty).
    pub help: &'static str,
}

/// What a tool produced.
#[derive(Clone, Debug)]
pub enum Output {
    /// A new layer to add to the map.
    Layer(Dataset),
    /// Rows to select in an existing layer.
    Selection { layer: String, rows: Vec<usize> },
    /// An existing layer's dataset replaced (e.g. new fields).
    Update { layer: String, dataset: Dataset },
    /// A standalone table.
    Table(Dataset),
    /// Several new layers and tables (external tools can write more than one output).
    Layers(Vec<OutputLayer>),
}

/// One output of [`Output::Layers`].
#[derive(Clone, Debug)]
pub enum OutputLayer {
    Vector(Dataset),
    /// A raster file, kept as its bytes so the project can embed it.
    Raster {
        name: String,
        file_name: String,
        bytes: Vec<u8>,
    },
    Table(Dataset),
}

/// A raster layer handed to a tool: its file (on disk or embedded), or failing that the pixels.
pub struct RasterInput<'a> {
    pub raster: &'a rustgis_data::raster::Raster,
    /// The file it was read from, when it is on disk.
    pub path: Option<&'a str>,
    /// The file's name and bytes, when it is embedded in the project.
    pub embedded: Option<(&'a str, &'a [u8])>,
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub output: Output,
    pub messages: Vec<String>,
}

/// Access to the map's layers by name.
pub trait Inputs {
    fn dataset(&self, layer: &str) -> Option<&Dataset>;
    /// Selected rows of a layer; tools honour a selection like desktop GIS does.
    fn selection(&self, layer: &str) -> &[usize];
    /// A raster layer by name.
    fn raster(&self, _layer: &str) -> Option<RasterInput<'_>> {
        None
    }
}

pub struct ToolSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub toolbox: &'static str,
    pub summary: &'static str,
    pub params: &'static [ParamSpec],
    pub run: fn(&dyn Inputs, &Value) -> Result<ToolResult, String>,
}

/// Runs an external tool (the spec says which).
pub type ExternalRunner = fn(&ToolSpec, &dyn Inputs, &Value) -> Result<ToolResult, String>;

/// Where external tools come from: called once, the first time the tools are listed.
pub type ExternalProvider = fn() -> Vec<ToolSpec>;

static PROVIDER: std::sync::OnceLock<(ExternalProvider, ExternalRunner)> = std::sync::OnceLock::new();
/// The loaded external tools. Each load is leaked (it lives as long as the app, and reloads are
/// rare: the user installs or locates the suite), so specs can be handed out as `&'static`.
static EXTERNAL: std::sync::RwLock<Option<&'static [ToolSpec]>> = std::sync::RwLock::new(None);

/// Adds an external tool suite. Only the first registration counts; the provider runs lazily,
/// so registering costs nothing until a tool list is needed.
pub fn register_external(provider: ExternalProvider, runner: ExternalRunner) {
    let _ = PROVIDER.set((provider, runner));
}

/// The external tools (empty when none are registered).
pub fn external_tools() -> &'static [ToolSpec] {
    let Some((provider, _)) = PROVIDER.get() else { return &[] };
    if let Ok(cur) = EXTERNAL.read()
        && let Some(tools) = *cur
    {
        return tools;
    }
    let tools: &'static [ToolSpec] = Box::leak(
        std::panic::catch_unwind(provider)
            .unwrap_or_else(|_| {
                log::error!("the external tool suite failed to load");
                Vec::new()
            })
            .into_boxed_slice(),
    );
    match EXTERNAL.write() {
        Ok(mut cur) => cur.get_or_insert(tools),
        Err(_) => tools,
    }
}

/// Loads the external tools again the next time they're needed (the suite was installed,
/// located or removed).
pub fn reload_external() {
    if let Ok(mut cur) = EXTERNAL.write() {
        *cur = None;
    }
}

/// Every tool: the built-in ones, then the external ones.
pub fn tools() -> impl Iterator<Item = &'static ToolSpec> {
    TOOLS.iter().chain(external_tools())
}

/// Whether a tool comes from the external suite.
pub fn is_external(spec: &ToolSpec) -> bool {
    external_tools().iter().any(|t| std::ptr::eq(t, spec))
}

pub fn tool(id: &str) -> Option<&'static ToolSpec> {
    let by_id = |t: &&ToolSpec| t.id.eq_ignore_ascii_case(id);
    let by_name = |t: &&ToolSpec| t.name.eq_ignore_ascii_case(id);
    // A built-in tool wins over an external one with the same name.
    tools().find(by_id).or_else(|| TOOLS.iter().find(by_name)).or_else(|| external_tools().iter().find(by_name))
}

/// The runner for external tools; never called (external specs carry the runner's id).
pub fn external_placeholder(_: &dyn Inputs, _: &Value) -> Result<ToolResult, String> {
    Err("this tool runs through the external suite".into())
}

/// Runs a tool with JSON parameters, filling defaults and checking required ones.
pub fn run(id: &str, inputs: &dyn Inputs, params: &Value) -> Result<ToolResult, String> {
    let spec = tool(id).ok_or_else(|| format!("no tool '{id}'"))?;
    let mut p = params.as_object().cloned().unwrap_or_default();
    for ps in spec.params {
        if !p.contains_key(ps.name)
            && !ps.default.is_empty()
            && let Ok(v) = serde_json::from_str::<Value>(ps.default)
        {
            p.insert(ps.name.to_string(), v);
        }
        if ps.required && p.get(ps.name).is_none_or(|v| v.is_null() || v.as_str() == Some("")) {
            return Err(format!("{}: '{}' is required", spec.name, ps.label));
        }
    }
    let p = Value::Object(p);
    // A bug in a tool must not take the app down.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match PROVIDER.get() {
        Some((_, runner)) if is_external(spec) => runner(spec, inputs, &p),
        _ => (spec.run)(inputs, &p),
    }))
    .unwrap_or_else(|_| Err(format!("{} failed unexpectedly", spec.name)))
}
