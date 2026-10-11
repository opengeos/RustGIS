//! Layer display effects: blending, masking, display filters, point aggregation and 2.5D
//! extrusion with lighting and shadows.

use serde::{Deserialize, Serialize};

/// How a layer's pixels combine with what's beneath.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    Difference,
}

impl Blend {
    pub const ALL: [Blend; 7] =
        [Blend::Normal, Blend::Multiply, Blend::Screen, Blend::Overlay, Blend::Darken, Blend::Lighten, Blend::Difference];
    pub fn code(self) -> &'static str {
        match self {
            Blend::Normal => "normal",
            Blend::Multiply => "multiply",
            Blend::Screen => "screen",
            Blend::Overlay => "overlay",
            Blend::Darken => "darken",
            Blend::Lighten => "lighten",
            Blend::Difference => "difference",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Blend::Normal => "Normal",
            Blend::Multiply => "Multiply",
            Blend::Screen => "Screen",
            Blend::Overlay => "Overlay",
            Blend::Darken => "Darken",
            Blend::Lighten => "Lighten",
            Blend::Difference => "Difference",
        }
    }
    pub fn from_code(s: &str) -> Option<Blend> {
        Blend::ALL.into_iter().find(|b| b.code().eq_ignore_ascii_case(s.trim()))
    }
}

/// A scale-dependent filter: between two scales only features matching `where_` draw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayFilter {
    pub name: String,
    /// Applies when zoomed in beyond 1:`min_scale` (out-beyond limit), if set.
    #[serde(default)]
    pub min_scale: Option<f64>,
    /// Applies when zoomed out beyond 1:`max_scale` (in-beyond limit), if set.
    #[serde(default)]
    pub max_scale: Option<f64>,
    #[serde(rename = "where")]
    pub where_: String,
}

impl DisplayFilter {
    pub fn applies(&self, scale: f64) -> bool {
        self.min_scale.is_none_or(|m| scale <= m) && self.max_scale.is_none_or(|m| scale >= m)
    }
}

/// Points drawn as bins coloured by how many fall in each.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aggregation {
    pub hexagons: bool,
    /// Bin size in points.
    pub size: f64,
}

/// How polygons rise above the map, drawn as an oblique 2.5D view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extrusion {
    /// Height in metres from this field, else `height`.
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub height: f64,
    /// Heights are multiplied by this.
    #[serde(default = "one")]
    pub factor: f64,
    /// Walls facing away from the viewer aren't drawn.
    #[serde(default = "yes")]
    pub face_culling: bool,
    /// Walls are shaded by the way they face the light.
    #[serde(default = "yes")]
    pub lighting: bool,
    /// Extruded shapes cast a shadow on the map.
    #[serde(default)]
    pub shadows: bool,
}

fn one() -> f64 {
    1.0
}

fn yes() -> bool {
    true
}
