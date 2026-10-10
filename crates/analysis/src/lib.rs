//! L2: geoprocessing tools.
//!
//! Each tool is a [`ToolSpec`]: an id, a toolbox, parameters for the Geoprocessing pane, and a
//! pure `run` function over datasets. Inputs refer to layers by name through [`Inputs`]. Work is
//! done in Web Mercator metres; distances are corrected for scale distortion at each feature's
//! latitude so a 100 km buffer is about 100 km on the ground.
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
}

#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    /// Default value as JSON text (`"10"`, `"\"km\""`), or empty.
    pub default: &'static str,
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
}

pub struct ToolSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub toolbox: &'static str,
    pub summary: &'static str,
    pub params: &'static [ParamSpec],
    pub run: fn(&dyn Inputs, &Value) -> Result<ToolResult, String>,
}

pub fn tool(id: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|t| t.id.eq_ignore_ascii_case(id) || t.name.eq_ignore_ascii_case(id))
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
    // Tools are pure functions over in-memory data; a bug in one must not take the app down.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(inputs, &p)))
        .unwrap_or_else(|_| Err(format!("{} failed unexpectedly", spec.name)))
}
