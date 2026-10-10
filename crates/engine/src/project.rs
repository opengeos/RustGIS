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
        for m in &mut self.maps {
            m.camera.clamp();
            for l in &mut m.layers {
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
        let tmp = path.with_extension("rgisx.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("could not save {}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Project, String> {
        let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if len > MAX_BYTES {
            return Err("the project file is too large".into());
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut p: Project = serde_json::from_str(&text).map_err(|e| format!("not a RustGIS project: {e}"))?;
        if p.format > FORMAT {
            return Err(format!("this project was saved by a newer RustGIS (format {})", p.format));
        }
        p.normalize();
        Ok(p)
    }
}

fn empty_map() -> &'static Map {
    static M: std::sync::OnceLock<Map> = std::sync::OnceLock::new();
    M.get_or_init(|| Map::new("Map"))
}
