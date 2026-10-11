//! The project: maps, standalone tables and geoprocessing history, saved as JSON (`.rgisx`).

use std::path::Path;

use rustgis_data::Dataset;
use rustgis_map::Map;
use serde::{Deserialize, Serialize};

use crate::GpRecord;

pub const EXTENSION: &str = "rgisx";
/// Format version written to files.
pub const FORMAT: u32 = 1;
/// Largest project file read (512 MiB).
const MAX_BYTES: u64 = 512 << 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    #[serde(default = "format")]
    pub format: u32,
    pub name: String,
    pub maps: Vec<Map>,
    pub active_map: usize,
    #[serde(default)]
    pub tables: Vec<Dataset>,
    #[serde(default)]
    pub gp_history: Vec<GpRecord>,
    #[serde(default)]
    pub layouts: Vec<rustgis_map::layout::Layout>,
    /// Folder connections shown in the Catalog.
    #[serde(default)]
    pub folders: Vec<String>,
    /// Saved symbology, applied to layers by name.
    #[serde(default)]
    pub styles: Vec<StyleItem>,
    /// Step-by-step tasks: guided command sequences.
    #[serde(default)]
    pub tasks: Vec<Task>,
}

/// A saved style: a layer's symbology and labels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleItem {
    pub name: String,
    pub geometry: rustgis_data::GeometryType,
    pub renderer: rustgis_map::Renderer,
    pub labels: rustgis_map::Labeling,
    #[serde(default)]
    pub transparency: f32,
}

/// A task: steps that each run a command, in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<TaskStep>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskStep {
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    pub command: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

fn format() -> u32 {
    FORMAT
}

impl Default for Project {
    fn default() -> Self {
        Project::new("Untitled")
    }
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Project {
            format: FORMAT,
            name: name.into(),
            maps: vec![Map::new("Map")],
            active_map: 0,
            tables: Vec::new(),
            gp_history: Vec::new(),
            layouts: Vec::new(),
            folders: Vec::new(),
            styles: Vec::new(),
            tasks: Vec::new(),
        }
    }

    /// Repairs invariants after loading (at least one map; a valid active index).
    pub fn normalize(&mut self) {
        if self.maps.is_empty() {
            self.maps.push(Map::new("Map"));
        }
        if self.active_map >= self.maps.len() {
            self.active_map = 0;
        }
        self.layouts.truncate(200);
        for l in &mut self.layouts {
            l.clamp();
        }
        for m in &mut self.maps {
            // A projection this version can't use (a hand edit, or a newer file) falls back to
            // Web Mercator rather than being named while another is drawn.
            if let rustgis_map::Projection::Custom(code) = &m.projection
                && let Err(e) = rustgis_map::projection::Projector::new(code)
            {
                log::warn!("{}: {e}; using Web Mercator", m.name);
                m.projection = rustgis_map::Projection::WebMercator;
                // The camera is in the lost projection's units: start from the whole world.
                m.camera = rustgis_map::Camera::world();
            }
            m.camera.clamp();
            for l in &mut m.layers {
                if let Some(rl) = &mut l.raster
                    && rl.data.is_none()
                    && let Err(e) = rl.reload()
                {
                    log::warn!("{}: {e}", l.name);
                }
                let n = l.dataset.features.len();
                l.selection.retain(|r| *r < n);
                let fields = l.dataset.fields.len();
                for f in &mut l.dataset.features {
                    f.attrs.resize(fields, rustgis_data::Value::Null);
                }
                l.touch_geometry();
            }
        }
    }

    pub fn map(&self) -> &Map {
        // `normalize` guarantees a map; fall back to the first for safety.
        self.maps.get(self.active_map).or_else(|| self.maps.first()).unwrap_or_else(|| empty_map())
    }

    pub fn map_mut(&mut self) -> &mut Map {
        if self.maps.is_empty() {
            self.maps.push(Map::new("Map"));
        }
        let i = if self.active_map < self.maps.len() { self.active_map } else { 0 };
        self.active_map = i;
        &mut self.maps[i]
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        rustgis_data::io::write_atomic(path, text.as_bytes()).map_err(|e| format!("could not save {}: {e}", path.display()))
    }

    /// Reads a project from its JSON text (a browser upload).
    pub fn from_json(text: &str) -> Result<Project, String> {
        if text.len() as u64 > MAX_BYTES {
            return Err("the project file is too large".into());
        }
        let mut p: Project = serde_json::from_str(text).map_err(|e| format!("not a RustGIS project: {e}"))?;
        if p.format > FORMAT {
            return Err(format!("this project was saved by a newer RustGIS (format {})", p.format));
        }
        p.normalize();
        Ok(p)
    }

    pub fn load(path: &Path) -> Result<Project, String> {
        let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if len > MAX_BYTES {
            return Err("the project file is too large".into());
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Project::from_json(&text)
    }
}

fn empty_map() -> &'static Map {
    static M: std::sync::OnceLock<Map> = std::sync::OnceLock::new();
    M.get_or_init(|| Map::new("Map"))
}
