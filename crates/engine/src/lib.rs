//! L4: the session (project, undo, active map/layer/tool), the command registry and the
//! ribbon feature catalog.
//!
//! **Everything is a command.** Each user-visible action is a [`CommandSpec`] with an id; the
//! ribbon, keyboard shortcuts, command search and the command line all dispatch through
//! [`Session::run`]. Programmatic calls never open dialogs.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod catalog;
pub mod cmd;
pub mod project;
pub mod sample;
#[cfg(test)]
mod tests;

pub use project::Project;
pub use rustgis_analysis as analysis;
pub use rustgis_data as data;
pub use rustgis_geom as geom;
pub use rustgis_map as map;
pub use rustgis_render as render;

use std::path::PathBuf;

use rustgis_map::{Camera, Map};
use serde_json::Value;

pub type CmdResult = Result<Value, String>;

/// A command.
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Where it lives in the UI: `Map › Navigate`, `Analysis › Geoprocessing`…
    pub location: &'static str,
    /// Default shortcut (`Mod+S`, `F5`); `Mod` = Ctrl, or ⌘ on macOS.
    pub shortcut: &'static str,
    /// Parameter description for scripts and agents.
    pub params: &'static str,
    /// Whether it changes the project (and so takes an undo snapshot).
    pub mutates: bool,
    /// `None` = enabled; `Some(reason)` = disabled.
    pub enabled: fn(&Session) -> Option<&'static str>,
    pub run: fn(&mut Session, &Value) -> CmdResult,
}

fn always(_: &Session) -> Option<&'static str> {
    None
}

impl CommandSpec {
    pub const fn new(id: &'static str, label: &'static str, location: &'static str, run: fn(&mut Session, &Value) -> CmdResult) -> Self {
        CommandSpec { id, label, location, shortcut: "", params: "{}", mutates: true, enabled: always, run }
    }
    pub const fn key(mut self, s: &'static str) -> Self {
        self.shortcut = s;
        self
    }
    pub const fn params(mut self, p: &'static str) -> Self {
        self.params = p;
        self
    }
    /// Doesn't change the project (no undo snapshot, not dirtying).
    pub const fn view(mut self) -> Self {
        self.mutates = false;
        self
    }
    pub const fn when(mut self, f: fn(&Session) -> Option<&'static str>) -> Self {
        self.enabled = f;
        self
    }
}

pub struct Registry {
    pub commands: Vec<CommandSpec>,
}

impl Registry {
    pub fn get(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.iter().find(|c| c.id == id)
    }
}

/// The interactive map tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Explore,
    Select,
    ZoomIn,
    Measure,
    MeasureArea,
    CreatePoint,
    CreateLine,
    CreatePolygon,
    /// Draws a graphic: the shape is one of [`GRAPHIC_TOOLS`]' shapes.
    Graphic(&'static str),
}

/// The graphic drawing tools: (shape, tool code, label).
pub const GRAPHIC_TOOLS: &[(&str, &str, &str)] = &[
    ("point", "graphicPoint", "Point"),
    ("text", "graphicText", "Text"),
    ("line", "graphicLine", "Line"),
    ("curve", "graphicCurve", "Curve"),
    ("freehand", "graphicFreehand", "Freehand"),
    ("polygon", "graphicPolygon", "Polygon"),
    ("lasso", "graphicLasso", "Lasso"),
    ("rectangle", "graphicRectangle", "Rectangle"),
    ("circle", "graphicCircle", "Circle"),
    ("ellipse", "graphicEllipse", "Ellipse"),
    ("picture", "graphicPicture", "Picture"),
];

impl Tool {
    pub fn code(self) -> &'static str {
        match self {
            Tool::Explore => "explore",
            Tool::Select => "select",
            Tool::ZoomIn => "zoomIn",
            Tool::Measure => "measure",
            Tool::MeasureArea => "measureArea",
            Tool::CreatePoint => "createPoint",
            Tool::CreateLine => "createLine",
            Tool::CreatePolygon => "createPolygon",
            Tool::Graphic(shape) => GRAPHIC_TOOLS.iter().find(|g| g.0 == shape).map_or("graphicPoint", |g| g.1),
        }
    }
    pub fn from_code(s: &str) -> Option<Tool> {
        if let Some(g) = GRAPHIC_TOOLS.iter().find(|g| g.1.eq_ignore_ascii_case(s.trim())) {
            return Some(Tool::Graphic(g.0));
        }
        [
            Tool::Explore,
            Tool::Select,
            Tool::ZoomIn,
            Tool::Measure,
            Tool::MeasureArea,
            Tool::CreatePoint,
            Tool::CreateLine,
            Tool::CreatePolygon,
        ]
        .into_iter()
        .find(|t| t.code().eq_ignore_ascii_case(s.trim()))
    }
    pub fn label(self) -> &'static str {
        match self {
            Tool::Explore => "Explore",
            Tool::Select => "Select",
            Tool::ZoomIn => "Zoom In",
            Tool::Measure => "Measure Distance",
            Tool::MeasureArea => "Measure Area",
            Tool::CreatePoint => "Create Point",
            Tool::CreateLine => "Create Line",
            Tool::CreatePolygon => "Create Polygon",
            Tool::Graphic(shape) => GRAPHIC_TOOLS.iter().find(|g| g.0 == shape).map_or("Point", |g| g.2),
        }
    }
    pub fn creates(self) -> bool {
        matches!(self, Tool::CreatePoint | Tool::CreateLine | Tool::CreatePolygon)
    }
}

/// One geoprocessing history row.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GpRecord {
    pub tool: String,
    pub params: Value,
    pub ok: bool,
    pub messages: Vec<String>,
    pub millis: u64,
    /// The layers the run added (by name at the time).
    #[serde(default)]
    pub outputs: Vec<String>,
}

/// Something worth telling the user (status bar / notifications).
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

const UNDO_LIMIT: usize = 40;

pub struct Session {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub registry: Registry,
    /// Bumped on every change so views can cache.
    pub revision: u64,
    pub tool: Tool,
    /// The layer selected in the Contents pane (drives contextual tabs).
    pub active_layer: Option<u32>,
    /// Map view size in logical points (set by the UI each frame).
    pub view_size: (f64, f64),
    pub notices: Vec<Notice>,
    /// Layer whose attribute table is open.
    pub table_layer: Option<u32>,
    /// The layout shown instead of the map view, if any (an index into `project.layouts`).
    pub active_layout: Option<usize>,
    /// Drawing tools stay active after a graphic is drawn (Insert › Graphics).
    pub keep_tool: bool,
    /// Copied features (Edit › Clipboard).
    pub clipboard: Option<cmd::clipboard::Clipboard>,
    undo: Vec<(Project, &'static str)>,
    redo: Vec<(Project, &'static str)>,
    back: Vec<Camera>,
    forward: Vec<Camera>,
    /// Last camera recorded in the extent history.
    last_camera: Option<Camera>,
    /// The projection the extent history's cameras are in (their units change with it).
    history_projection: String,
    /// Finished asynchronous tool runs waiting for `gp.finish` (the browser's GeoLibre tools).
    pub(crate) pending: Vec<PendingRun>,
    #[cfg_attr(not(feature = "geolibre"), allow(dead_code))]
    pub(crate) next_pending: u64,
    /// Asynchronous runs started and not yet complete: (id, job, resolved params, start time).
    #[cfg(feature = "geolibre")]
    pub(crate) started: Vec<(u64, rustgis_geolibre::Job, serde_json::Value, u64)>,
}

/// A finished tool run, held until `gp.finish` applies it.
pub(crate) struct PendingRun {
    pub id: u64,
    pub tool: String,
    pub params: serde_json::Value,
    pub result: Result<rustgis_analysis::ToolResult, String>,
    pub millis: u64,
}

impl Session {
    pub fn new(project: Project) -> Self {
        // Cheap: the suite's tools load the first time a tool list is needed.
        #[cfg(feature = "geolibre")]
        rustgis_geolibre::register();
        Session {
            project,
            path: None,
            dirty: false,
            registry: Registry { commands: cmd::all() },
            revision: 1,
            tool: Tool::Explore,
            active_layer: None,
            view_size: (1200.0, 720.0),
            notices: Vec::new(),
            table_layer: None,
            clipboard: None,
            keep_tool: false,
            active_layout: None,
            undo: Vec::new(),
            redo: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
            last_camera: None,
            history_projection: String::new(),
            pending: Vec::new(),
            next_pending: 0,
            #[cfg(feature = "geolibre")]
            started: Vec::new(),
        }
    }

    pub fn map(&self) -> &Map {
        self.project.map()
    }
    pub fn map_mut(&mut self) -> &mut Map {
        self.project.map_mut()
    }

    /// Runs a command by id. Mutating commands snapshot the project for undo; an error or a
    /// panic restores it.
    pub fn run(&mut self, id: &str, params: &Value) -> CmdResult {
        let Some(spec) = self.registry.get(id) else { return Err(format!("unknown command '{id}'")) };
        // Layer conditions apply to the layer the parameters name, falling back to the active one.
        let named = params.get("layer").and_then(|_| self.layer_id(params).ok());
        let active = self.active_layer;
        if named.is_some() {
            self.active_layer = named;
        }
        // Restored on every path, even if a condition were to panic.
        let enabled = spec.enabled;
        let blocked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| enabled(self))).unwrap_or(Some("unavailable"));
        self.active_layer = active;
        if let Some(why) = blocked {
            return Err(format!("{}: {why}", spec.label));
        }
        let (run, mutates, label) = (spec.run, spec.mutates, spec.label);
        let snapshot = mutates.then(|| self.project.clone());
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(self, params)))
            .unwrap_or_else(|_| Err(format!("{label} failed unexpectedly; the project was restored")));
        match (&res, snapshot) {
            (Ok(_), Some(snap)) => {
                self.undo.push((snap, label));
                if self.undo.len() > UNDO_LIMIT {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.dirty = true;
            }
            (Err(_), Some(snap)) => self.project = snap,
            _ => {}
        }
        if let Err(e) = &res {
            self.notify(e.clone(), true);
        }
        self.fix_active_layer();
        self.revision = self.revision.wrapping_add(1);
        res
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notices.push(Notice { text: text.into(), error });
        if self.notices.len() > 200 {
            self.notices.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(|u| u.1)
    }
    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|u| u.1)
    }

    /// Forgets undo/redo and extent history and marks the project clean (after new/open).
    pub(crate) fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.back.clear();
        self.forward.clear();
        self.last_camera = None;
        self.dirty = false;
        // Runs of the previous project mustn't land in this one.
        self.pending.clear();
        #[cfg(feature = "geolibre")]
        self.started.clear();
    }

    pub(crate) fn undo(&mut self) -> bool {
        let Some((p, label)) = self.undo.pop() else { return false };
        let cur = std::mem::replace(&mut self.project, p);
        self.redo.push((cur, label));
        self.touch_all_geometry();
        true
    }

    pub(crate) fn redo(&mut self) -> bool {
        let Some((p, label)) = self.redo.pop() else { return false };
        let cur = std::mem::replace(&mut self.project, p);
        self.undo.push((cur, label));
        self.touch_all_geometry();
        true
    }

    fn touch_all_geometry(&mut self) {
        for m in &mut self.project.maps {
            for l in &mut m.layers {
                l.touch_geometry();
            }
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Drops the extent history when the map has switched projection since it was recorded (by a
    /// command, undo or opening a project): its cameras are in the old projection's units.
    fn check_history_projection(&mut self) {
        let code = self.map().projection.code();
        if code != self.history_projection {
            self.back.clear();
            self.forward.clear();
            self.last_camera = None;
            self.history_projection = code;
        }
    }

    /// Records the camera in the previous/next extent history when it settles somewhere new.
    pub fn record_extent(&mut self) {
        self.check_history_projection();
        let cam = self.map().camera;
        if let Some(last) = self.last_camera {
            let moved = (last.center.x - cam.center.x).abs() > last.res * 2.0 || (last.res / cam.res - 1.0).abs() > 0.01;
            if !moved {
                return;
            }
            self.back.push(last);
            if self.back.len() > 50 {
                self.back.remove(0);
            }
            self.forward.clear();
        }
        self.last_camera = Some(cam);
    }

    pub(crate) fn extent_back(&mut self) -> bool {
        self.check_history_projection();
        let Some(c) = self.back.pop() else { return false };
        let cur = self.map().camera;
        self.forward.push(cur);
        self.map_mut().camera = c;
        self.last_camera = Some(c);
        true
    }

    pub(crate) fn extent_forward(&mut self) -> bool {
        self.check_history_projection();
        let Some(c) = self.forward.pop() else { return false };
        let cur = self.map().camera;
        self.back.push(cur);
        self.map_mut().camera = c;
        self.last_camera = Some(c);
        true
    }

    pub fn can_extent_back(&self) -> bool {
        !self.back.is_empty() && self.map().projection.code() == self.history_projection
    }
    pub fn can_extent_forward(&self) -> bool {
        !self.forward.is_empty() && self.map().projection.code() == self.history_projection
    }

    /// Keeps `active_layer` pointing at a layer of the active map.
    fn fix_active_layer(&mut self) {
        if self.active_layout.is_some_and(|i| i >= self.project.layouts.len()) {
            self.active_layout = None;
        }
        let map = self.project.map();
        if let Some(id) = self.active_layer
            && map.layer(id).is_none()
        {
            self.active_layer = map.layers.first().map(|l| l.id);
        }
        if let Some(id) = self.table_layer
            && map.layer(id).is_none()
        {
            self.table_layer = None;
        }
    }

    /// Resolves a `layer` parameter (name or id), defaulting to the active layer.
    pub fn layer_id(&self, p: &Value) -> Result<u32, String> {
        let map = self.map();
        match p.get("layer") {
            Some(Value::Number(n)) => n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|id| map.layer(*id).is_some())
                .ok_or_else(|| format!("no layer with id {n}")),
            Some(Value::String(s)) => map
                .layer_by_name(s)
                .map(|l| l.id)
                .or_else(|| s.parse::<u32>().ok().filter(|id| map.layer(*id).is_some()))
                .ok_or_else(|| format!("no layer named '{s}'")),
            _ => self
                .active_layer
                .filter(|id| map.layer(*id).is_some())
                .ok_or_else(|| "select a layer in the Contents pane first".to_string()),
        }
    }

    /// The scale denominator now.
    pub fn scale(&self) -> f64 {
        self.map().scale()
    }

    /// Fits the camera to a lon/lat extent in the current view size.
    pub fn zoom_to(&mut self, e: &rustgis_geom::Extent) {
        let (w, h) = self.view_size;
        let m = self.map_mut();
        if m.projection.is_globe() {
            rustgis_map::Globe::fit(&mut m.camera, e, w, h);
        } else {
            m.fit_lonlat(e, w, h);
        }
        self.record_extent();
    }

    pub fn title(&self) -> String {
        self.project.name.clone()
    }
}
