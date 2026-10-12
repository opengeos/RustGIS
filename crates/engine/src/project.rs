//! The project: maps, standalone tables and geoprocessing history, saved as JSON (`.rgis`).

use std::path::Path;

use rustgis_data::Dataset;
use rustgis_map::Map;
use serde::{Deserialize, Serialize};

use crate::GpRecord;

pub const EXTENSION: &str = "rgis";
/// The project extension earlier versions wrote; such files still open.
pub const LEGACY_EXTENSION: &str = "rgisx";

/// Whether a file name or path names a project file (`.rgis`, or the legacy `.rgisx`), ignoring case.
pub fn is_project_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [EXTENSION, LEGACY_EXTENSION].iter().any(|x| n.strip_suffix(x).is_some_and(|s| s.ends_with('.')))
}
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
    /// Geoprocessing environment settings.
    #[serde(default)]
    pub gp_env: GpEnvironment,
    /// Models: chains of tools, each step able to use an earlier step's output.
    #[serde(default)]
    pub models: Vec<Model>,
    /// Ortho and reality mapping workspaces.
    #[serde(default)]
    pub workspaces: Vec<crate::cmd::imagery::Workspace>,
    /// Maps move together (View › Link Views).
    #[serde(default)]
    pub link_views: bool,
    /// The project's thumbnail (base64 PNG).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// Workflow Manager jobs.
    #[serde(default)]
    pub jobs: Vec<Job>,
    /// Offline areas: snapshots of a map's data around an extent, synced back by feature id.
    #[serde(default)]
    pub offline_areas: Vec<OfflineArea>,
    /// Reports on layers' records.
    #[serde(default)]
    pub reports: Vec<rustgis_map::report::Report>,
    /// Presentations: pages of maps, text, images and videos.
    #[serde(default)]
    pub presentations: Vec<rustgis_map::presentation::Presentation>,
    /// Notebooks of text and command cells.
    #[serde(default)]
    pub notebooks: Vec<crate::cmd::notebook::Notebook>,
    /// Trained deep learning models.
    #[serde(default)]
    pub dl_models: Vec<crate::cmd::deeplearning::DlModel>,
    /// Shares made (Share › Status › Jobs) and the folders they go to.
    #[serde(default)]
    pub shared: Vec<crate::cmd::sharing::SharedItem>,
    #[serde(default)]
    pub data_stores: Vec<crate::cmd::sharing::DataStore>,
}

/// A unit of work tracked in the Workflow Manager.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// "new", "in progress", "on hold" or "done".
    pub status: String,
    #[serde(default)]
    pub assigned: String,
    /// The area the job covers (longitude/latitude), if any.
    #[serde(default)]
    pub extent: Option<rustgis_geom::Extent>,
    /// Seconds since 1970.
    pub created: f64,
    #[serde(default)]
    pub updated: f64,
}

/// A map's data clipped to an extent, to take into the field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OfflineArea {
    pub name: String,
    /// The map it came from (by name).
    pub map: String,
    pub extent: rustgis_geom::Extent,
    pub created: f64,
    #[serde(default)]
    pub synced: f64,
    /// The snapshot: the map's feature layers holding the features meeting the extent.
    pub data: Map,
}

/// Settings every geoprocessing run honours.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GpEnvironment {
    /// Tools only process features meeting this longitude/latitude extent (when no features are
    /// selected).
    #[serde(default)]
    pub extent: Option<rustgis_geom::Extent>,
    /// The cell size (degrees) of rasters tools create, when they don't say.
    #[serde(default)]
    pub cell_size: Option<f64>,
}

/// A model: geoprocessing steps run in order. A parameter value `%n` stands for the output layer
/// of step n (1-based).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub steps: Vec<ModelStep>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelStep {
    pub tool: String,
    pub params: serde_json::Value,
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
            gp_env: GpEnvironment::default(),
            models: Vec::new(),
            workspaces: Vec::new(),
            link_views: false,
            thumbnail: None,
            jobs: Vec::new(),
            offline_areas: Vec::new(),
            reports: Vec::new(),
            presentations: Vec::new(),
            notebooks: Vec::new(),
            dl_models: Vec::new(),
            shared: Vec::new(),
            data_stores: Vec::new(),
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
        // Files skip the commands' limits, and notebooks and presentations are found (and
        // notebooks trusted) by name, so names are made unique.
        for r in &mut self.reports {
            r.clamp();
        }
        self.reports.truncate(MAX_ITEMS);
        self.presentations.truncate(MAX_ITEMS);
        self.notebooks.truncate(MAX_ITEMS);
        self.dl_models.truncate(MAX_ITEMS);
        self.dl_models.retain(|m| m.model.validate().is_ok());
        self.shared.truncate(MAX_ITEMS);
        self.data_stores.truncate(MAX_ITEMS);
        for p in &mut self.presentations {
            p.clamp();
        }
        for nb in &mut self.notebooks {
            nb.clamp();
        }
        unique_names(self.presentations.iter_mut().map(|p| &mut p.name));
        unique_names(self.notebooks.iter_mut().map(|nb| &mut nb.name));
        unique_names(self.dl_models.iter_mut().map(|m| &mut m.name));
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
            // Tile layers with an address that can't be fetched (a hand edit) are dropped.
            m.tiles.retain_mut(|t| t.repair().map_err(|e| log::warn!("{}: {e}", m.name)).is_ok());
            m.tiles.truncate(rustgis_map::tiles::MAX_TILE_LAYERS);
            for l in &mut m.layers {
                if let Some(rl) = &mut l.raster
                    && rl.data.is_none()
                    && let Err(e) = rl.reload()
                {
                    log::warn!("{}: {e}", l.name);
                }
                // A streamed layer's features are read again from its source.
                if let Some(src) = &mut l.source {
                    l.dataset.windowed = true;
                    if !src.is_open()
                        && src.error.is_none()
                        && let Err(e) = src.open()
                    {
                        log::warn!("{}: {e}", l.name);
                    }
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

/// Most reports, presentations and notebooks a project file may hold.
pub const MAX_ITEMS: usize = 1000;

/// Renames names repeated (ignoring case) after their first use: "Tour", "Tour 2", …. Each base
/// name remembers the last number it gave, so many repeats cost no more than a few each.
fn unique_names<'a>(names: impl Iterator<Item = &'a mut String>) {
    let mut seen = std::collections::HashSet::new();
    let mut next: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for name in names {
        if seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let base = name.clone();
        let k = next.entry(base.to_ascii_lowercase()).or_insert(1);
        loop {
            *k += 1;
            *name = format!("{base} {k}");
            if seen.insert(name.to_ascii_lowercase()) {
                break;
            }
        }
    }
}
