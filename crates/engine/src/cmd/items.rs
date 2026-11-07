//! Project items: folder connections, map and layer files, styles, project templates, packages,
//! layer templates, tasks, and 3D export (Home › Add; Data › Layer Templates and Styles; Publish ›
//! Package, Save As and Output; Command Search for map/template imports).

use std::fmt::Write as _;
use std::path::Path;

use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType};
use rustgis_map::color::RampKind;
use rustgis_map::palette::{RampFormat, read_ramps};
use rustgis_map::{Layer, Map, Ramp};
use serde_json::{Value, json};

use super::{b, f, need_feature_layer, need_layer, ps};
use crate::project::{StyleItem, Task, TaskStep};
use crate::{CmdResult, CommandSpec as C, Project, Session};

/// Largest item file read (512 MiB, like projects).
const MAX_FILE: u64 = 512 << 20;

pub fn commands() -> Vec<C> {
    vec![
        // Folder connections.
        C::new("project.addFolder", "Add Folder", "Home › Add › Add Data", |s, p| {
            let path = ps(p, "path").map(str::trim).filter(|p| !p.is_empty()).ok_or("path is required")?;
            if !cfg!(target_arch = "wasm32") && !Path::new(path).is_dir() {
                return Err(format!("{path} isn't a folder"));
            }
            if !s.project.folders.iter().any(|f| f == path) {
                s.project.folders.push(path.to_string());
            }
            Ok(json!({"folders": s.project.folders}))
        })
        .params(r#"{"path": folder}"#),
        C::new("project.removeFolder", "Remove Folder", "Browser", |s, p| {
            let path = ps(p, "path").ok_or("path is required")?;
            let before = s.project.folders.len();
            s.project.folders.retain(|f| f != path);
            if s.project.folders.len() == before { Err(format!("{path} isn't connected")) } else { Ok(Value::Null) }
        })
        .params(r#"{"path": folder}"#),
        C::new("project.connections", "Connections", "Browser", |s, _| {
            let folders: Vec<Value> = s.project.folders.iter().map(|f| json!({"path": f, "available": Path::new(f).is_dir()})).collect();
            Ok(json!({
                "folders": folders,
                "maps": s.project.maps.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
                "layouts": s.project.layouts.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
                "tables": s.project.tables.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                "styles": s.project.styles.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                "ramps": s.project.ramps.iter().map(|r| r.name().to_string()).collect::<Vec<_>>(),
                "tasks": s.project.tasks.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            }))
        })
        .view(),
        // Map and layer files.
        C::new("map.save", "Save Map File", "Publish › Save As", |s, p| save_item(s, "map", p))
        .view()
        .params(r#"{"path": "*.rgismap", "map"?: name}"#),
        C::new("map.import", "Import", "Command Search", import_maps).params(r#"{"path": "*.rgismap|*.rgis|*.rgispkg" | "text": map file JSON | "data": base64 file}"#),
        C::new("layer.save", "Save Layer File", "Publish › Save As", |s, p| save_item(s, "layer", p))
        .view()
        .when(need_layer)
        .params(r#"{"layer"?, "path": "*.rgislyr"}"#),
        C::new("layer.addLayerFile", "Add Layer File", "Home › Add", |s, p| {
            let text = read_text(p)?;
            super::layer::add_layer_text(s, &text)
        })
        .params(r#"{"path"?: "*.rgislyr", "text"?: layer file JSON, "data"?: base64 file}"#),
        // Styles.
        C::new("style.new", "New Style", "Style › Styles", |s, p| {
            let id = s.layer_id(p)?;
            let l = s.map().layer(id).ok_or("no such layer")?;
            let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| l.name.clone());
            let item = StyleItem { name: name.clone(), geometry: l.geometry_type(), renderer: l.renderer.clone(), labels: l.labels.clone(), transparency: l.transparency };
            add_style(s, item);
            Ok(json!({"style": name}))
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "name"?: string}"#),
        C::new("style.add", "Add Style", "Style › Styles", |s, p| {
            let text = read_text(p)?;
            crate::project::check_format(&text, "style file")?;
            let items: Vec<StyleItem> = serde_json::from_str(&text).map_err(|e| format!("not a RustGIS style file: {e}"))?;
            let mut n = 0;
            for it in items.into_iter().take(1000) {
                add_style(s, it);
                n += 1;
            }
            Ok(json!({"added": n}))
        })
        .params(r#"{"path"?: "*.rgisstyle", "text"?: style file JSON, "data"?: base64 file}"#),
        C::new("style.import", "Import Styles", "Style › Styles", import_styles).params(r#"{"path"?: "*.rgislyr|*.rgismap|*.rgis", "text"?: JSON, "data"?: base64 file}"#),
        C::new("style.apply", "Apply Style", "Style › Styles", |s, p| {
            let name = ps(p, "name").ok_or("name is required")?;
            let st = s.project.styles.iter().find(|t| t.name.eq_ignore_ascii_case(name)).cloned().ok_or_else(|| format!("no style '{name}'"))?;
            let id = s.layer_id(p)?;
            let l = s.map_mut().layer_mut(id).ok_or("no such layer")?;
            if st.geometry != l.geometry_type() {
                return Err(format!("{name} is a {} style; {} is a {} layer", st.geometry.label(), l.name, l.geometry_type().label()));
            }
            l.renderer = st.renderer;
            let field = l.labels.default.field.clone();
            // Saved moves name features by id: the style's belong to the layer it was made from,
            // so this layer keeps its own, in the class of the same name (Default in Default).
            let mut labels = st.labels;
            let (foreign, lost) = carry_moves(&l.labels, &mut labels);
            // Compared with the layer as it stands, its moves still in it.
            let relock = !super::labels::lock_holds(l, &labels);
            l.labels = labels;
            // The label field stays if the style's isn't in this layer.
            if l.dataset.field_index(&l.labels.default.field).is_none() {
                l.labels.default.field = field;
            }
            l.labels.repoint_missing_fields(&l.dataset);
            l.transparency = st.transparency;
            if relock {
                s.map_mut().label_lock = None;
                s.map_mut().label_lock_line_anchors.clear();
            }
            if foreign > 0 {
                s.notify(format!("{foreign} saved label move(s) in style {name} not applied: they name the features of the layer the style was made from"), false);
            }
            if lost > 0 {
                s.notify(format!("{lost} saved label move(s) dropped with label classes style {name} doesn't have"), false);
            }
            Ok(Value::Null)
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "name": style}"#),
        C::new("style.remove", "Delete Style", "Browser", |s, p| {
            let name = ps(p, "name").ok_or("name is required")?;
            let before = s.project.styles.len();
            s.project.styles.retain(|t| !t.name.eq_ignore_ascii_case(name));
            if s.project.styles.len() == before { Err(format!("no style '{name}'")) } else { Ok(Value::Null) }
        })
        .params(r#"{"name": style}"#),
        C::new("style.save", "Save Style Items", "Publish › Save As", |s, p| save_item(s, "styles", p))
        .view()
        .params(r#"{"path": "*.rgisstyle"}"#),
        // Custom colour ramps.
        C::new("ramp.add", "Add Color Ramps", "Style › Styles", |s, p| {
            let text = read_text(p)?;
            // RustGIS's own files, QGIS styles, GDAL colour-relief text and GMT palettes.
            let format = match ps(p, "format") {
                Some(f) => Some(RampFormat::from_name(f).ok_or_else(|| format!("unknown colour ramp format '{f}'"))?),
                None => None,
            };
            let kind = match ps(p, "kind") {
                Some(k) => Some(
                    serde_json::from_value::<RampKind>(json!(k.trim().to_ascii_lowercase())).map_err(|_| format!("unknown ramp kind '{k}'"))?,
                ),
                None => None,
            };
            let file = ps(p, "path").or(ps(p, "file"));
            let ramps = read_ramps(&text, file, format, ps(p, "name"), kind)?;
            if let Some(r) = ramps.iter().find(|r| Ramp::from_name(r.name()).is_some()) {
                return Err(format!("'{}' is the name of a built-in colour scheme; rename the ramp", r.name()));
            }
            let names: Vec<String> = ramps.iter().map(|r| r.name().to_string()).collect();
            let new = names.iter().filter(|n| !s.project.ramps.iter().any(|r| r.name().eq_ignore_ascii_case(n))).count();
            if s.project.ramps.len() + new > crate::project::MAX_ITEMS {
                return Err(format!("a project holds at most {} colour ramps; delete some first", crate::project::MAX_ITEMS));
            }
            for r in ramps {
                // A ramp with the same name is replaced.
                s.project.ramps.retain(|t| !t.name().eq_ignore_ascii_case(r.name()));
                s.project.ramps.push(r);
            }
            Ok(json!({"added": names}))
        })
        .params(
            r#"{"path"?: "*.rgisramp|*.xml|*.qml|*.txt|*.cpt", "text"?: file contents, "data"?: base64 file, "file"?: file name for "text" or "data", "format"?: "rgisramp"|"qgis"|"gdal"|"cpt", "name"?: name for a single ramp, "kind"?: "sequential"|"diverging"|"qualitative"}"#,
        ),
        C::new("ramp.remove", "Delete Color Ramp", "Browser", |s, p| {
            let name = ps(p, "name").ok_or("name is required")?;
            let before = s.project.ramps.len();
            s.project.ramps.retain(|r| !r.name().eq_ignore_ascii_case(name.trim()));
            if s.project.ramps.len() == before { Err(format!("no colour ramp '{name}'")) } else { Ok(Value::Null) }
        })
        .params(r#"{"name": colour ramp}"#),
        C::new("ramp.save", "Save Color Ramps", "Publish › Save As", |s, p| save_item(s, "ramps", p))
        .view()
        .params(r#"{"path": "*.rgisramp"}"#),
        // Project templates.
        C::new("project.saveTemplate", "Save Project Template", "Publish › Save As", |s, p| save_item(s, "template", p))
        .view()
        .params(r#"{"path": "*.rgistemplate", "name"?: template name}"#),
        C::new("project.newTemplate", "New Custom Template", "Publish › Save As", |s, p| save_item(s, "template", p))
        .view()
        .params(r#"{"path": "*.rgistemplate", "name"?: template name}"#),
        C::new("project.fromTemplate", "Open Custom Template", "Command Search", |s, p| {
            let text = read_text(p)?;
            super::connections::clear_active();
            let mut proj = match Project::from_import_json(&text, None) {
                Ok(proj) => proj,
                Err(error) => {
                    super::connections::restore_active(s);
                    return Err(error.replace("RustGIS project", "RustGIS template"));
                }
            };
            proj.name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Untitled").chars().take(200).collect();
            super::project::reset_to(s, proj);
            s.path = None;
            super::connections::resolve_project(s);
            super::connections::note_waiting(s);
            Ok(Value::Null)
        })
        .view()
        .params(r#"{"path"?: "*.rgistemplate", "text"?: template JSON, "data"?: base64 file, "name"?: new project name}"#),
        // Packages.
        C::new("project.package", "Package Project", "Publish › Package", |s, p| save_item(s, "projectPackage", p))
        .view()
        .params(r#"{"path": "*.rgispkg"}"#),
        C::new("map.package", "Package Map", "Publish › Package", |s, p| save_item(s, if b(p, "mobile").unwrap_or(false) { "mobilePackage" } else { "mapPackage" }, p))
        .view()
        .params(r#"{"path": "*.rgismpk", "mobile"?: bool (simplify geometry and drop hidden layers, for phones and tablets), "tolerance"?: degrees}"#),
        C::new("gp.package", "Package Results", "Publish › Package", |s, p| save_item(s, "gpPackage", p))
        .view()
        .params(r#"{"path": "*.rgisgpk"}"#),
        // Layer templates.
        C::new("layer.template", "Layer Template", "Data › Layer Templates", layer_template)
            .params(r#"{"template": "points"|"lines"|"polygons"|"places"|"routes"|"areas"|"observations", "name"?: string}"#),
        // Tasks.
        C::new("task.new", "New Task", "Analyze › Tasks", |s, p| {
            let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Task");
            let steps: Vec<TaskStep> = match p.get("steps") {
                Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("steps: {e}"))?,
                // A task can replay the geoprocessing history, one step per successful run.
                None if b(p, "fromHistory").unwrap_or(false) => s
                    .project
                    .gp_history
                    .iter()
                    .filter(|r| r.ok)
                    .filter_map(|r| {
                        let tool = super::gp::tool_id(&r.tool)?;
                        Some(TaskStep { title: r.tool.clone(), instructions: r.messages.first().cloned().unwrap_or_default(), command: "gp.run".into(), params: json!({"tool": tool, "params": r.params}) })
                    })
                    .collect(),
                None => Vec::new(),
            };
            for st in &steps {
                if s.registry.get(&st.command).is_none() {
                    return Err(format!("step '{}' runs an unknown command '{}'", st.title, st.command));
                }
            }
            let mut name = name.chars().take(200).collect::<String>();
            let base = name.clone();
            let mut i = 1;
            while s.project.tasks.iter().any(|t| t.name.eq_ignore_ascii_case(&name)) {
                i += 1;
                name = format!("{base} {i}");
            }
            // History that came with an opened project is from a file too.
            let from_file_history = b(p, "fromHistory").unwrap_or(false) && p.get("steps").is_none() && s.history_from_file > 0;
            if !b(p, "imported").unwrap_or(false) && !from_file_history {
                s.trusted_tasks.insert(name.to_ascii_lowercase());
            }
            s.project.tasks.push(Task { name: name.clone(), description: ps(p, "description").unwrap_or("").chars().take(2000).collect(), steps: steps.into_iter().take(200).collect() });
            Ok(json!({"task": name}))
        })
        .params(r#"{"name"?: string, "description"?: string, "steps"?: [{"title", "instructions"?, "command": id, "params"?: {}}], "fromHistory"?: bool (one step per geoprocessing run), "imported"?: bool (from a file: its steps may not touch files)}"#),
        C::new("task.addStep", "Add Task Step", "Tasks", |s, p| {
            let step: TaskStep = serde_json::from_value(p.clone()).map_err(|e| format!("a step needs a title and a command: {e}"))?;
            if !can_be_step(s, &step.command) {
                return Err(format!("'{}' can't be a task step", step.command));
            }
            let t = task_mut(s, p)?;
            if t.steps.len() >= 200 {
                return Err("a task has at most 200 steps".into());
            }
            t.steps.push(step);
            Ok(json!({"steps": t.steps.len()}))
        })
        .params(r#"{"task"?: name, "title": string, "instructions"?: string, "command": id, "params"?: {}}"#),
        // Editing a step's words, its place or the step itself. Its command and parameters stay
        // as they were, so a task from a file stays as untrusted as it came.
        C::new("task.editStep", "Edit Task Step", "Tasks", |s, p| {
            let (title, instructions) = (ps(p, "title"), ps(p, "instructions"));
            if title.is_some_and(|t| t.trim().is_empty()) {
                return Err("a step needs a title".into());
            }
            if title.is_none() && instructions.is_none() {
                return Err("title or instructions is required".into());
            }
            let step = step_index(p)?;
            let t = task_mut(s, p)?;
            let n = t.steps.len();
            let st = t.steps.get_mut(step).ok_or_else(|| format!("{} has {n} steps", t.name))?;
            let title = title.map_or_else(|| st.title.clone(), |t| t.trim().chars().take(200).collect());
            let instructions = instructions.map_or_else(|| st.instructions.clone(), |t| t.chars().take(4000).collect());
            // Changing nothing would still leave an undo step.
            if title == st.title && instructions == st.instructions {
                return Err(format!("step {step} already reads so"));
            }
            st.title = title;
            st.instructions = instructions;
            Ok(json!({"step": step}))
        })
        .params(r#"{"task"?: name, "step": number (from 0), "title"?: string, "instructions"?: string}"#),
        C::new("task.moveStep", "Move Task Step", "Tasks", |s, p| {
            let to = match p.get("to") {
                None | Some(Value::Null) => return Err("to is required".into()),
                Some(v) => v.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| format!("to must be a whole number from 0, not {v}"))?,
            };
            let step = step_index(p)?;
            let t = task_mut(s, p)?;
            if step >= t.steps.len() || to >= t.steps.len() {
                return Err(format!("{} has {} steps", t.name, t.steps.len()));
            }
            if step == to {
                return Err(format!("step {step} is already there"));
            }
            let st = t.steps.remove(step);
            t.steps.insert(to, st);
            Ok(json!({"step": to}))
        })
        .params(r#"{"task"?: name, "step": number (from 0), "to": number (from 0)}"#),
        C::new("task.removeStep", "Delete Task Step", "Tasks", |s, p| {
            let step = step_index(p)?;
            let t = task_mut(s, p)?;
            if step >= t.steps.len() {
                return Err(format!("{} has {} steps", t.name, t.steps.len()));
            }
            t.steps.remove(step);
            Ok(json!({"steps": t.steps.len()}))
        })
        .params(r#"{"task"?: name, "step": number (from 0)}"#),
        C::new("task.run", "Run Task", "Tasks", run_task).params(r#"{"task"?: name, "step"?: number (one step; all when omitted)}"#),
        C::new("task.remove", "Delete Task", "Tasks", |s, p| {
            let name = ps(p, "task").ok_or("task is required")?;
            let before = s.project.tasks.len();
            s.project.tasks.retain(|t| !t.name.eq_ignore_ascii_case(name));
            // A later task by this name isn't the user's just because of its name.
            s.trusted_tasks.remove(&name.to_ascii_lowercase());
            if s.project.tasks.len() == before { Err(format!("no task '{name}'")) } else { Ok(Value::Null) }
        })
        .params(r#"{"task": name}"#),
        C::new("task.save", "Save Task Item", "Publish › Save As", |s, p| save_item(s, "task", p))
        .view()
        .params(r#"{"task"?: name, "path": "*.rgistask"}"#),
        C::new("task.import", "Import Task", "Analyze › Tasks", |s, p| {
            let text = read_text(p)?;
            let t: Task = serde_json::from_str(&text).map_err(|e| format!("not a RustGIS task: {e}"))?;
            // A task from a file is someone else's: it may not read or write files, or run scripts.
            if let Some((i, st)) = t.steps.iter().enumerate().find(|(_, st)| touches_files(&st.command, &st.params)) {
                return Err(format!("step {} ({}) reads or writes files or runs a script, which imported tasks can't do", i + 1, st.command));
            }
            let steps = serde_json::to_value(&t.steps).map_err(|e| e.to_string())?;
            s.run("task.new", &json!({"name": t.name, "description": t.description, "steps": steps, "imported": true}))
        })
        .params(r#"{"path"?: "*.rgistask", "text"?: task JSON, "data"?: base64 file}"#),
        // 3D export.
        C::new("share.export3d", "Export To 3D File", "Publish › Output", export_3d)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?, "path": "*.obj|*.stl" (STL is triangles only: no lines; metres, no CRS), "heightField"?: field (extrusion height in metres), "height"?: metres, "zScale"?: number}"#),
    ]
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(v).map_err(|e| e.to_string())
}

/// Whether command `id` can be added as a task's step (`task.addStep`): a built-in command, and
/// not a task's own (a step doesn't run a task).
pub fn can_be_step(s: &Session, id: &str) -> bool {
    s.registry.get(id).is_some() && !id.starts_with("task.")
}

/// The JSON of an item from `text`, or the file at `path`.
pub(crate) fn read_text(p: &Value) -> Result<String, String> {
    if let Some(d) = ps(p, "data") {
        use base64::Engine;
        if d.len() as u64 > MAX_FILE / 3 * 4 {
            return Err("the file is too large".into());
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(d.trim()).map_err(|_| "data must be base64".to_string())?;
        if bytes.starts_with(b"PK") {
            return package_text(&bytes);
        }
        return String::from_utf8(bytes).map_err(|_| "not a RustGIS file".to_string());
    }
    match (ps(p, "text"), ps(p, "path")) {
        (Some(t), _) if t.len() as u64 <= MAX_FILE => Ok(t.to_string()),
        (Some(_), _) => Err("the file is too large".into()),
        (None, Some(path)) => {
            let bytes = crate::project::read_file(Path::new(path), MAX_FILE)?;
            // Packages are zip archives holding the project.
            if bytes.starts_with(b"PK") {
                return package_text(&bytes);
            }
            String::from_utf8(bytes).map_err(|_| format!("{path} isn't a RustGIS file"))
        }
        (None, None) => Err("path or text is required".into()),
    }
}

/// The project JSON inside a package.
pub fn package_text(bytes: &[u8]) -> Result<String, String> {
    let entries = rustgis_data::zip::read_matching(bytes, |n| crate::project::is_project_file(n) || n.ends_with(".rgismap"))?;
    let (_, data) = entries
        .into_iter()
        .find(|(n, _)| crate::project::is_project_file(n) || n.ends_with(".rgismap"))
        .ok_or("the package holds no project or map")?;
    String::from_utf8(data).map_err(|_| "the package's project is damaged".to_string())
}

/// Most raster bytes embedded in one map, layer or package file, all rasters together. They're
/// base64'd into JSON (a third larger), which must stay within what packages can be read back
/// with (`zip::MAX_TOTAL`).
const MAX_EMBED: u64 = rustgis_data::zip::MAX_TOTAL as u64 / 2;

/// A raster layer with its file embedded (so the copy stands alone), within what's left of
/// `budget`. A file that doesn't fit stays linked by its path.
fn embed_layer(mut l: Layer, budget: &mut u64) -> Layer {
    if let Some(rl) = &mut l.raster
        && rl.embedded.is_none()
        && let Some(path) = rl.path.clone()
        && let Ok(bytes) = crate::project::read_file(Path::new(&path), *budget)
    {
        *budget -= bytes.len() as u64;
        rl.file_name = Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string());
        rl.embedded = Some(std::sync::Arc::new(bytes));
        rl.path = None;
    }
    l
}

fn embedded_map(m: &Map, budget: &mut u64) -> Map {
    let mut m = m.clone();
    m.layers = m.layers.into_iter().map(|l| embed_layer(l, budget)).collect();
    m
}

/// Adds maps from a map file, project or package.
fn import_maps(s: &mut Session, p: &Value) -> CmdResult {
    let text = read_text(p)?;
    crate::project::check_format(&text, "map file or project")?;
    let dir = ps(p, "path").and_then(|path| std::path::absolute(path).ok()).and_then(|p| p.parent().map(Path::to_path_buf));
    super::connections::clear_active();
    let maps: Vec<Map> = match serde_json::from_str::<Map>(&text) {
        Ok(mut m) => {
            if crate::project::ensure_map_imports_complete(&m).is_err() {
                super::connections::restore_active(s);
                return Err("a map file cannot contain runtime GeoLibre download reservations".into());
            }
            if let Some(d) = dir.as_deref().and_then(Path::to_str) {
                crate::project::resolve_file_paths(d, m.file_paths_mut());
            }
            // An older map file's plain COG tile layers open as raster layers read in place, as a
            // project's do (a project imported had them opened as it was read).
            super::tiles::cog_tiles_to_rasters(&mut m);
            for notice in crate::project::label_losses::map_notices(&text, &m) {
                s.notify(notice, false);
            }
            vec![m]
        }
        Err(_) => match Project::from_import_json(&text, dir.as_deref()) {
            Ok(mut project) => {
                for notice in std::mem::take(&mut project.label_read_notices) {
                    s.notify(notice, false);
                }
                project.maps
            }
            Err(error) => {
                super::connections::restore_active(s);
                return Err(error);
            }
        },
    };
    let mut names = Vec::new();
    for mut m in maps.into_iter().take(100) {
        // A binding in a file is no authority to use a login here.
        crate::s3_connections::clear_bindings(m.addresses_mut());
        crate::project::clear_login_bindings(m.addresses_mut());
        let base = m.name.clone();
        let mut i = 1;
        while s.project.maps.iter().any(|o| o.name.eq_ignore_ascii_case(&m.name)) {
            i += 1;
            m.name = format!("{base} ({i})");
        }
        names.push(m.name.clone());
        m.renew_instance_id();
        s.project.maps.push(m);
    }
    // Repair what was read (rasters reopen, cameras clamp); the sources that read S3 wait for
    // the saved connection that is to read them.
    s.project.normalize();
    super::connections::restore_active(s);
    super::connections::resolve_project(s);
    crate::s3_connections::resolve_project_s3(s);
    s.wait_in_place_reads();
    super::connections::note_waiting(s);
    if let Some(n) = names.last()
        && let Some(i) = s.project.maps.iter().position(|m| &m.name == n)
    {
        s.project.active_map = i;
        s.active_layer = s.map().layers.first().map(|l| l.id);
    }
    Ok(json!({"maps": names}))
}

fn add_style(s: &mut Session, mut item: StyleItem) {
    item.name = item.name.trim().chars().take(200).collect();
    if item.name.is_empty() {
        item.name = "Style".into();
    }
    s.project.styles.retain(|t| !t.name.eq_ignore_ascii_case(&item.name));
    s.project.styles.push(item);
}

/// Styles from every feature layer of a layer file, map file or project.
fn import_styles(s: &mut Session, p: &Value) -> CmdResult {
    let text = read_text(p)?;
    crate::project::check_format(&text, "layer file, map file or project")?;
    let layers: Vec<Layer> = if let Ok(l) = serde_json::from_str::<Layer>(&text) {
        for notice in crate::project::label_losses::layer_notices(&text, &l) {
            s.notify(notice, false);
        }
        vec![l]
    } else if let Ok(m) = serde_json::from_str::<Map>(&text) {
        for notice in crate::project::label_losses::map_notices(&text, &m) {
            s.notify(notice, false);
        }
        m.layers
    } else {
        // A project parse opens sources even though this import only takes their styles.
        super::connections::clear_active();
        let project = Project::from_import_json(&text, None);
        super::connections::restore_active(s);
        let mut project = project?;
        for notice in std::mem::take(&mut project.label_read_notices) {
            s.notify(notice, false);
        }
        project.maps.into_iter().flat_map(|m| m.layers).collect()
    };
    let mut n = 0;
    for l in layers.into_iter().filter(|l| !l.is_raster() && l.geometry_type() != GeometryType::None).take(1000) {
        add_style(
            s,
            StyleItem {
                name: l.name.clone(),
                geometry: l.geometry_type(),
                renderer: l.renderer,
                labels: l.labels,
                transparency: l.transparency,
            },
        );
        n += 1;
    }
    if n == 0 {
        return Err("no feature layers to take styles from".into());
    }
    Ok(json!({"imported": n}))
}

/// Copies `old`'s saved label moves into the class of the same name in `new` (Default into
/// Default), after dropping `new`'s own; returns how many of `new`'s were dropped, and how many
/// of `old`'s had no class to go to.
fn carry_moves(old: &rustgis_map::symbology::Labeling, new: &mut rustgis_map::symbology::Labeling) -> (usize, usize) {
    let mut foreign = 0usize;
    for c in new.classes.iter_mut().chain(std::iter::once(&mut new.default)) {
        foreign = foreign.saturating_add(c.moved.len());
        c.moved.clear();
    }
    new.default.moved.clone_from(&old.default.moved);
    let mut lost = 0usize;
    for c in old.classes.iter().filter(|c| !c.moved.is_empty()) {
        match new.classes.iter_mut().find(|n| n.name.eq_ignore_ascii_case(&c.name) && n.moved.is_empty()) {
            Some(n) => n.moved.clone_from(&c.moved),
            None => lost = lost.saturating_add(c.moved.len()),
        }
    }
    (foreign, lost)
}

/// The project without its features (layer schemas, symbology, layouts and styles stay), and how
/// many saved label moves went with them.
fn template(p: &Project) -> (Project, usize) {
    let mut t = p.clone();
    t.gp_history.clear();
    // Saved label moves name features, and a template has none: the layers', the saved styles'
    // and the offline snapshots' all go.
    let mut moves = 0usize;
    let labels = t
        .maps
        .iter_mut()
        .chain(t.offline_areas.iter_mut().map(|a| &mut a.data))
        .flat_map(|m| m.layers.iter_mut().map(|l| &mut l.labels))
        .chain(t.styles.iter_mut().map(|s| &mut s.labels));
    for labels in labels {
        for c in labels.classes.iter_mut().chain(std::iter::once(&mut labels.default)) {
            moves = moves.saturating_add(c.moved.len());
            c.moved.clear();
        }
    }
    for m in &mut t.maps {
        for l in &mut m.layers {
            l.dataset.features.clear();
            l.selection.clear();
            l.dataset.source = None;
            // A feature-less template has no loaded rows to save back to a source.
            l.postgis = None;
            l.gpkg = None;
            if let Some(rl) = &mut l.raster {
                rl.embedded = None;
            }
        }
        let dropped: Vec<u32> =
            m.layers.iter().filter(|l| l.is_raster() && l.raster.as_ref().is_none_or(|r| r.path.is_none())).map(|l| l.id).collect();
        m.release_tiles(&dropped);
        m.layers.retain(|l| !dropped.contains(&l.id));
    }
    for t in &mut t.tables {
        t.features.clear();
    }
    (t, moves)
}

/// Packages only completed imports; runtime download reservations cannot be reopened.
/// Refuses JSON beyond the project or ZIP inflation limit before creating the archive.
fn package_project(s: &Session) -> Result<Vec<u8>, String> {
    s.project.ensure_imports_complete()?;
    let mut p = s.project.clone();
    let mut budget = MAX_EMBED;
    for m in &mut p.maps {
        *m = embedded_map(m, &mut budget);
    }
    crate::project::strip_addresses(p.addresses_mut());
    // Recorded parameters (history, tasks, models) lose only their login bindings, as when saving.
    crate::project::clear_login_bindings(p.recorded_mut());
    let text = serde_json::to_string(&p).map_err(|e| e.to_string())?;
    let limit = crate::project::MAX_BYTES.min(rustgis_data::zip::MAX_TOTAL as u64);
    if text.len() as u64 > limit {
        return Err(format!("the project is too large to package (maximum {} MiB)", limit >> 20));
    }
    let readme =
        format!("{} — a RustGIS project package.\nOpen it with RustGIS (Project › Open), or unzip it for {}.rgis.\n", p.name, p.name);
    rustgis_data::zip::write(&[
        (format!("{}.{}", safe(&p.name), crate::project::EXTENSION), text.into_bytes()),
        ("README.txt".into(), readme.into_bytes()),
    ])
}

fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') { c } else { '_' }).collect();
    if s.trim().is_empty() { "project".into() } else { s }
}

/// A package of the active map; `mobile` drops hidden layers and simplifies geometry.
fn package_map(s: &Session, mobile: bool, tol: f64) -> Result<Vec<u8>, String> {
    let mut m = embedded_map(s.map(), &mut MAX_EMBED.clone());
    if mobile {
        // Groups folded into their layers: a hidden group's layers are dropped with the others.
        m.bake_groups();
        // A hidden DEM still gives the terrain its heights (it can stay hidden): it is kept.
        let dem = match &m.view.terrain {
            Some(rustgis_map::view::Terrain { source: rustgis_map::view::TerrainSource::Layer(n), .. }) => Some(n.clone()),
            _ => None,
        };
        let kept = |l: &rustgis_map::Layer| l.visible || (l.is_raster() && dem.as_deref().is_some_and(|d| l.name.eq_ignore_ascii_case(d)));
        let hidden: Vec<u32> = m.layers.iter().filter(|l| !kept(l)).map(|l| l.id).collect();
        m.release_tiles(&hidden);
        m.layers.retain(|l| kept(l));
        for l in &mut m.layers {
            if l.is_raster() || tol <= 0.0 {
                continue;
            }
            for f in &mut l.dataset.features {
                if let Some(g) = &f.geometry {
                    f.geometry = Some(rustgis_analysis::simplify_geometry(g, tol * 111_320.0));
                }
            }
            l.touch_geometry();
        }
    }
    crate::project::ensure_map_imports_complete(&m)?;
    crate::project::strip_addresses(m.addresses_mut());
    let text = serde_json::to_string(&m).map_err(|e| e.to_string())?;
    let kind = if mobile { "mobile map package" } else { "map package" };
    let readme = format!("{} — a RustGIS {kind}.\nAdd it to a project with Data › Import.\n", m.name);
    rustgis_data::zip::write(&[(format!("{}.rgismap", safe(&m.name)), text.into_bytes()), ("README.txt".into(), readme.into_bytes())])
}

/// The geoprocessing history with the layers its runs made, as GeoJSON.
fn package_gp(s: &Session) -> Result<Vec<u8>, String> {
    if s.project.gp_history.is_empty() {
        return Err("no geoprocessing has been run yet".into());
    }
    let mut history = s.project.gp_history.clone();
    let mut addresses = Vec::new();
    for record in &mut history {
        crate::project::recorded_addresses(&mut record.params, &mut addresses);
        if let Some((_, params)) = &mut record.command {
            crate::project::recorded_addresses(params, &mut addresses);
        }
    }
    crate::project::clear_login_bindings(addresses);
    let mut entries = vec![("history.json".to_string(), serde_json::to_vec_pretty(&history).map_err(|e| e.to_string())?)];
    let mut layers = std::collections::HashSet::new();
    let mut files = std::collections::HashSet::new();
    for rec in s.project.gp_history.iter().filter(|r| r.ok) {
        for name in &rec.outputs {
            for l in s.project.maps.iter().flat_map(|m| &m.layers).filter(|l| !l.is_raster() && l.name == *name) {
                if !layers.insert(l.name.clone()) {
                    continue;
                }
                // Names that clean up to the same file name get a number.
                let base = safe(&l.name);
                let mut file = format!("results/{base}.geojson");
                let mut k = 1;
                while !files.insert(file.to_ascii_lowercase()) {
                    k += 1;
                    file = format!("results/{base}_{k}.geojson");
                }
                entries.push((file, rustgis_data::io::to_geojson(&l.dataset).to_string().into_bytes()));
            }
        }
    }
    rustgis_data::zip::write(&entries)
}

fn layer_template(s: &mut Session, p: &Value) -> CmdResult {
    let t = ps(p, "template").ok_or("template is required")?;
    let text = |n: &str| Field::new(n, FieldType::Text);
    let (gt, title, fields) = match t {
        "points" => (GeometryType::Point, "Points", vec![text("name"), text("notes")]),
        "lines" => (GeometryType::Polyline, "Lines", vec![text("name"), text("notes")]),
        "polygons" => (GeometryType::Polygon, "Polygons", vec![text("name"), text("notes")]),
        "places" => {
            (GeometryType::Point, "Places", vec![text("name"), text("category"), text("address"), Field::new("rating", FieldType::Double)])
        }
        "routes" => (GeometryType::Polyline, "Routes", vec![text("name"), text("surface"), Field::new("length_km", FieldType::Double)]),
        "areas" => (GeometryType::Polygon, "Areas", vec![text("name"), text("land_use"), Field::new("area_km2", FieldType::Double)]),
        "observations" => (
            GeometryType::Point,
            "Observations",
            vec![text("observer"), text("date"), text("species"), Field::new("count", FieldType::Integer)],
        ),
        o => return Err(format!("unknown layer template '{o}'")),
    };
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(title);
    let id = super::layer::add_dataset(s, Dataset::new(name, gt, fields));
    if let Some(l) = s.map_mut().layer_mut(id) {
        l.labels.default.field = "name".into();
    }
    Ok(json!({"layer": id}))
}

fn task_ref<'a>(s: &'a Session, p: &Value) -> Result<&'a Task, String> {
    match ps(p, "task") {
        Some(n) => s.project.tasks.iter().find(|t| t.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no task '{n}'")),
        None => s.project.tasks.last().ok_or_else(|| "the project has no tasks".to_string()),
    }
}

/// A step's index from `step` (from 0).
fn step_index(p: &Value) -> Result<usize, String> {
    // Checked: on wasm32 `as usize` would wrap a large number round to a small, valid index.
    match p.get("step") {
        None | Some(Value::Null) => Err("step is required".to_string()),
        Some(v) => v.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| format!("step must be a whole number from 0, not {v}")),
    }
}

fn task_mut<'a>(s: &'a mut Session, p: &Value) -> Result<&'a mut Task, String> {
    match ps(p, "task") {
        Some(n) => s.project.tasks.iter_mut().find(|t| t.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no task '{n}'")),
        None => s.project.tasks.last_mut().ok_or_else(|| "the project has no tasks".to_string()),
    }
}

/// Runs a task without allowing its steps to re-enter tasks indirectly through scripts or
/// notebooks. Clear the session marker on errors and caught panics as well as success.
fn run_task(s: &mut Session, p: &Value) -> CmdResult {
    if s.task_running {
        return Err("a task is already running (a step can't run tasks, even through a script or notebook)".into());
    }
    s.task_running = true;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_task_steps(s, p)))
        .unwrap_or_else(|_| Err("the task failed unexpectedly".into()));
    s.task_running = false;
    result
}

/// Executes the selected task steps inside the caller's transaction and re-entry guard,
/// retaining the existing direct task ban and imported-task restrictions.
fn run_task_steps(s: &mut Session, p: &Value) -> CmdResult {
    let t = task_ref(s, p)?.clone();
    // `"step": null` runs them all, as no step does.
    let steps: Vec<(usize, TaskStep)> = match p.get("step").filter(|v| !v.is_null()) {
        Some(_) => {
            let i = step_index(p)?;
            vec![(i, t.steps.get(i).cloned().ok_or_else(|| format!("{} has {} steps", t.name, t.steps.len()))?)]
        }
        None => t.steps.iter().cloned().enumerate().collect(),
    };
    let mut done = Vec::new();
    let mut edited = false;
    let mut requested = false;
    for (i, st) in steps {
        if st.command.starts_with("task.") {
            return Err(format!("step {} would run another task", i + 1));
        }
        // Tasks that came in a file (imported, or inside an opened project) can't touch files.
        if !s.trusted_tasks.contains(&t.name.to_ascii_lowercase()) && touches_files(&st.command, &st.params) {
            return Err(format!(
                "step {} ({}) reads or writes files or runs a script, which a task from a file can't do; make a new task with that step to run it",
                i + 1,
                st.command
            ));
        }
        let spec = s.registry.get(&st.command).ok_or_else(|| format!("step {} runs an unknown command '{}'", i + 1, st.command))?;
        let (run, enabled, mutates) = (spec.run, spec.enabled, spec.mutates);
        // As when run directly, a step naming a layer is checked against that layer.
        if st.params.get("layer").is_some()
            && let Ok(id) = s.layer_id(&st.params)
        {
            s.active_layer = Some(id);
        }
        if let Some(why) = enabled(s) {
            return Err(format!("step {} ({}): {why}", i + 1, st.title));
        }
        // These handlers share the task's one outer transaction. A final request must not
        // erase the undo step for earlier edits.
        s.request_only = false;
        // A step that panics fails the task instead of the app (the whole task is one undo step).
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(s, &st.params)))
            .unwrap_or_else(|_| Err("failed unexpectedly".into()))
            .map_err(|e| format!("step {} ({}): {e}", i + 1, st.title))?;
        requested |= s.request_only;
        edited |= mutates && !s.request_only;
        done.push(json!({"step": i, "title": st.title, "result": r}));
    }
    s.request_only = requested && !edited;
    Ok(json!({"task": t.name, "ran": done}))
}

/// Whether a step may reach outside the session: a command that reads or writes files, opens
/// items or runs other steps, or a parameter that names a file, folder or address (by its key or
/// by looking like a path or URL).
pub(crate) fn touches_files(command: &str, params: &Value) -> bool {
    const KEYS: &[&str] =
        &["path", "folder", "dir", "file", "url", "uri", "source", "output", "connection", "database", "data", "text", "poster", "locator"];
    let reaches = |v: &Value| names_place(&layer_name_left_out(command, v), KEYS, 0);
    let id = command.to_ascii_lowercase();
    let verb = id.split('.').nth(1).unwrap_or("");
    ["script.", "notebook.", "model.", "project.", "task.", "share.", "offline.", "cache.", "locator."].iter().any(|p| id.starts_with(p))
        || ["import", "export", "open", "save", "add", "package", "upload", "download"].iter().any(|w| verb.starts_with(w))
        // Its inputs may be bare file names, which don't look like paths.
        || id == "raster.mosaic"
        || (id == "gp.run" && params.get("tool").and_then(Value::as_str).and_then(super::gp::tool_id)
            .and_then(rustgis_analysis::tool).is_some_and(super::gp::writes_files))
        || reaches(params)
}

/// Parameters without a built-in tool's output layer name, which stays in the session, so a task
/// (or a model, or a notebook) from a file, or a plugin without `files`, may set it: only
/// `gp.run`'s `{"tool", "params"}` shape, only `params.output`, only for the tools in
/// `rustgis_analysis::TOOLS`, which name their output layer and never a file through it (see
/// `out_name` there), not GeoLibre or plugin tools, which may write theirs as files, and only a
/// plain name, with no `/`, `\`, `.` or `:`. Anything else stays a place.
pub(crate) fn layer_name_left_out<'a>(command: &str, params: &'a Value) -> std::borrow::Cow<'a, Value> {
    use std::borrow::Cow;
    // Resolved as gp.run resolves it (by id or name, a built-in tool first), then held to the
    // built-in list, so the two can't drift: an external tool of a built-in's name isn't let in.
    let built_in = || {
        params
            .get("tool")
            .and_then(Value::as_str)
            .and_then(rustgis_analysis::tool)
            .is_some_and(|spec| !super::gp::writes_files(spec) && rustgis_analysis::TOOLS.iter().any(|b| std::ptr::eq(b, spec)))
    };
    let plain = |t: &str| !t.trim().is_empty() && !t.contains(['/', '\\', '.', ':']) && !looks_like_path(t);
    let named = || params.pointer("/params/output").and_then(Value::as_str).is_some_and(plain);
    // Copied only when there is a name to leave out: other commands' parameters (a file's
    // contents, say) are read as they are.
    if command != "gp.run" || !built_in() || !named() {
        return Cow::Borrowed(params);
    }
    let mut out = params.clone();
    if let Some(p) = out.get_mut("params").and_then(Value::as_object_mut) {
        p.remove("output");
    }
    Cow::Owned(out)
}

/// Whether a string looks like a file path or an address.
pub(crate) fn looks_like_path(t: &str) -> bool {
    let t = t.trim();
    let b = t.as_bytes();
    t.contains("://")
        || t.starts_with(['/', '\\', '~'])
        || t.starts_with("./")
        || t.starts_with(".\\")
        || t.contains("../")
        || t.contains("..\\")
        || (b.get(1) == Some(&b':') && b.first().is_some_and(u8::is_ascii_alphabetic))
}

/// Whether parameters name a file, folder or address: a key in `keys` anywhere in them, or a
/// string that looks like a path or URL. Too deep a nesting counts as yes.
pub(crate) fn names_place(v: &Value, keys: &[&str], depth: usize) -> bool {
    match v {
        Value::Object(o) if depth < 16 => {
            o.iter().any(|(k, v)| keys.contains(&k.to_ascii_lowercase().as_str()) || names_place(v, keys, depth + 1))
        }
        Value::Array(a) if depth < 16 => a.iter().any(|v| names_place(v, keys, depth + 1)),
        Value::Object(_) | Value::Array(_) => true,
        Value::String(t) => looks_like_path(t),
        _ => false,
    }
}

/// Writes a layer as a 3D model: Wavefront OBJ, or binary STL when `path` ends in `.stl`. Polygons
/// are extruded to a height, lines are polylines (OBJ only), points are small cubes. Coordinates
/// are metres east and north of the layer's south-west corner.
fn export_3d(s: &mut Session, p: &Value) -> CmdResult {
    let path = ps(p, "path").ok_or("path is required")?.to_string();
    if path.to_ascii_lowercase().ends_with(".stl") {
        let (bytes, report) = stl_bytes(s, p)?;
        rustgis_data::io::write_atomic(Path::new(&path), &bytes).map_err(|e| format!("{path}: {e}"))?;
        s.notify(format!("Exported {path}"), false);
        return Ok(json!({
            "path": path, "bytes": bytes.len(), "triangles": report.triangles,
            "skippedDegenerate": report.degenerate, "linesDropped": report.lines, "capsFanned": report.fanned,
        }));
    }
    let obj = obj_text(s, p)?;
    rustgis_data::io::write_atomic(Path::new(&path), obj.as_bytes()).map_err(|e| format!("{path}: {e}"))?;
    s.notify(format!("Exported {path}"), false);
    Ok(json!({"path": path, "bytes": obj.len()}))
}

/// Largest OBJ text or STL file written (512 MiB).
const MAX_OBJ: usize = 512 << 20;

/// Largest estimated mesh held while it is built. Above the file caps: a file that is too large is
/// refused by its exact size, and this only bounds memory.
const MAX_MESH: usize = 2 * MAX_OBJ;

/// How a face is turned into triangles for STL, as OBJ's winding is not always outward.
#[derive(Clone, Copy, PartialEq)]
enum Orient {
    /// Wound outward already.
    Keep,
    /// Wound inward: reverse.
    Flip,
    /// A top cap: the normal must point up.
    Up,
    /// A bottom cap: the normal must point down.
    Down,
}

/// One line of the mesh, in the order OBJ writes it. Vertex numbers start at 0.
enum Item {
    /// A vertex; `bare` writes a zero height as `0`, not `0.00`.
    Vertex([f64; 3], bool),
    Tri([usize; 3], Orient),
    Quad([usize; 4], Orient),
    Poly(Vec<usize>, Orient),
    Polyline(Vec<usize>),
}

/// A layer's 3D mesh, shared by the OBJ and STL exports.
#[derive(Default)]
struct Mesh {
    items: Vec<Item>,
    vertices: usize,
    /// Rough size of the file, to stop a runaway export early.
    bytes: usize,
    /// Polylines, which STL can't hold.
    lines: usize,
    /// Caps that couldn't be triangulated and are fanned from their first vertex.
    fanned: usize,
    /// Whether polylines are kept (OBJ) or only counted (STL, which can't hold them).
    keep_lines: bool,
    /// Polygons left out because their outer ring has fewer than 3 vertices.
    skipped: usize,
    /// Holes left out because they have fewer than 3 vertices, so their areas stay solid.
    holes_dropped: usize,
    /// Features whose height field isn't a finite number, so they took the default height.
    default_heights: usize,
}

impl Orient {
    /// The other way round, for a solid extruded below its base.
    fn reversed(self) -> Orient {
        match self {
            Orient::Keep => Orient::Flip,
            Orient::Flip => Orient::Keep,
            Orient::Up => Orient::Down,
            Orient::Down => Orient::Up,
        }
    }
}

impl Mesh {
    // Each push checks the size first, so one huge feature can't build past it.
    fn vertex(&mut self, x: f64, y: f64, z: f64, bare: bool) -> Result<(), String> {
        self.grow(36)?;
        self.items.push(Item::Vertex([x, y, z], bare));
        self.vertices += 1;
        Ok(())
    }

    fn face(&mut self, item: Item) -> Result<(), String> {
        // STL writes 50 bytes a triangle: a quad is two, and an n-gon is a fan of n - 2.
        let bytes = match &item {
            Item::Tri(..) if !self.keep_lines => 50,
            Item::Quad(..) if !self.keep_lines => 100,
            Item::Poly(v, _) if !self.keep_lines => 50 * v.len().saturating_sub(2),
            _ => 24,
        };
        self.grow(bytes)?;
        self.items.push(item);
        Ok(())
    }

    fn grow(&mut self, bytes: usize) -> Result<(), String> {
        self.bytes += bytes;
        if self.bytes > MAX_MESH { Err("the 3D file would be too large; export fewer features".into()) } else { Ok(()) }
    }
}

/// A feature layer as a mesh.
fn layer_mesh(s: &Session, p: &Value, keep_lines: bool) -> Result<Mesh, String> {
    let id = s.layer_id(p)?;
    let l = s.map().layer(id).ok_or("no such layer")?;
    let hf = ps(p, "heightField").map(|n| l.dataset.field_index(n).ok_or_else(|| format!("no field '{n}'"))).transpose()?;
    let base_h = f(p, "height").unwrap_or(1000.0).clamp(0.0, 1e7);
    let zs = f(p, "zScale").unwrap_or(1.0).clamp(0.0, 1e6);
    let e = l.extent();
    if e.is_empty() {
        return Err("the layer has no geometry".into());
    }
    let origin = rustgis_geom::lonlat_to_merc(rustgis_geom::Coord::new(e.min_x, e.min_y));
    // Web Mercator stretches distances by 1/cos(latitude); shrink back at the layer's centre.
    let k = e.center().y.to_radians().cos().abs().max(0.01);
    let xy = |c: rustgis_geom::Coord| {
        let m = rustgis_geom::lonlat_to_merc(c);
        ((m.x - origin.x) * k, (m.y - origin.y) * k)
    };
    let mut mesh = Mesh { keep_lines, ..Mesh::default() };
    let rows = l.visible_rows();
    for (row, ft) in l.dataset.features.iter().enumerate() {
        if rows.get(row) != Some(&true) {
            continue;
        }
        // A height that isn't a finite number takes the default, and is counted.
        let height = match hf {
            Some(fi) => match l.dataset.value(row, fi).as_f64().filter(|h| h.is_finite()) {
                Some(h) => h,
                None => {
                    mesh.default_heights += 1;
                    base_h
                }
            },
            None => base_h,
        };
        let h = height * zs;
        // A negative height builds the solid below its base, so its STL facets turn inside out
        // unless reversed.
        let below = h < 0.0;
        let orient = |o: Orient| if below { o.reversed() } else { o };
        match &ft.geometry {
            Some(Geometry::Polygon(polys)) => {
                for rings in polys {
                    // Each ring open (no closing vertex), outer first then holes.
                    let mut open: Vec<Vec<(f64, f64)>> = rings
                        .iter()
                        // Drop a ring's closing vertex only when it repeats the first.
                        .map(|r| {
                            let n = if r.len() > 1 && r.first() == r.last() { r.len() - 1 } else { r.len() };
                            r.iter().take(n).map(|c| xy(*c)).collect::<Vec<_>>()
                        })
                        .collect();
                    // A degenerate outer ring drops the polygon (a hole mustn't become solid);
                    // degenerate holes are left out.
                    if open.first().is_none_or(|r| r.len() < 3) {
                        mesh.skipped += 1;
                        continue;
                    }
                    let rings_before = open.len();
                    let mut k = 0;
                    open.retain(|r| {
                        k += 1;
                        k == 1 || r.len() >= 3
                    });
                    mesh.holes_dropped += rings_before - open.len();
                    let Some(outer) = open.first() else { continue };
                    let n: usize = open.iter().map(Vec::len).sum();
                    let v = mesh.vertices;
                    for (x, y) in open.iter().flatten() {
                        mesh.vertex(*x, 0.0, -y, false)?;
                    }
                    for (x, y) in open.iter().flatten() {
                        mesh.vertex(*x, h, -y, false)?;
                    }
                    // Walls round every ring, holes included. The OBJ winding faces right of the
                    // ring's direction: outward for a counter-clockwise outer ring or a clockwise
                    // hole, so the other way round is flipped for STL.
                    let mut first = 0;
                    for (ri, r) in open.iter().enumerate() {
                        let m = r.len();
                        let counter_clockwise = ring_area(r) > 0.0;
                        let wall = orient(if counter_clockwise == (ri == 0) { Orient::Keep } else { Orient::Flip });
                        for i in 0..m {
                            let (a, b) = (first + i, first + (i + 1) % m);
                            mesh.face(Item::Quad([v + a, v + b, v + n + b, v + n + a], wall))?;
                        }
                        first += m;
                    }
                    // Caps as triangles (concave shapes and holes would fan wrongly as n-gons).
                    match rustgis_geom::triangulate::triangulate(outer, open.get(1..).unwrap_or(&[])) {
                        Some(tris) => {
                            for [a, b, c] in tris {
                                mesh.face(Item::Tri([v + n + a, v + n + b, v + n + c], orient(Orient::Up)))?;
                                mesh.face(Item::Tri([v + c, v + b, v + a], orient(Orient::Down)))?;
                            }
                        }
                        None => {
                            let m = outer.len();
                            mesh.face(Item::Poly((0..m).map(|i| v + n + i).collect(), orient(Orient::Up)))?;
                            mesh.face(Item::Poly((0..m).rev().map(|i| v + i).collect(), orient(Orient::Down)))?;
                            mesh.fanned += 1;
                        }
                    }
                }
            }
            Some(Geometry::Line(parts)) => {
                for part in parts {
                    let n = part.len();
                    if n < 2 {
                        continue;
                    }
                    mesh.lines += 1;
                    if !mesh.keep_lines {
                        continue;
                    }
                    let v = mesh.vertices;
                    for c in part {
                        let (x, y) = xy(*c);
                        mesh.vertex(x, h, -y, false)?;
                    }
                    mesh.face(Item::Polyline((0..n).map(|i| v + i).collect()))?;
                }
            }
            Some(g @ (Geometry::Point(_) | Geometry::MultiPoint(_))) => {
                let pts: Vec<rustgis_geom::Coord> = match g {
                    Geometry::Point(c) => vec![*c],
                    Geometry::MultiPoint(c) => c.clone(),
                    _ => Vec::new(),
                };
                // Each point's cube is a twentieth as wide as it is tall.
                let r = (h * 0.05).max(1.0);
                for c in pts {
                    let (x, y) = xy(c);
                    let v = mesh.vertices;
                    for (dx, dz) in [(-r, -r), (r, -r), (r, r), (-r, r)] {
                        mesh.vertex(x + dx, 0.0, -y + dz, true)?;
                    }
                    for (dx, dz) in [(-r, -r), (r, -r), (r, r), (-r, r)] {
                        mesh.vertex(x + dx, h, -y + dz, false)?;
                    }
                    // The OBJ cube is wound inward, so STL flips every face.
                    for [a, b, c, d] in [[1, 2, 6, 5], [2, 3, 7, 6], [3, 4, 8, 7], [4, 1, 5, 8], [5, 6, 7, 8], [4, 3, 2, 1]] {
                        mesh.face(Item::Quad([v + a - 1, v + b - 1, v + c - 1, v + d - 1], orient(Orient::Flip)))?;
                    }
                }
            }
            None => {}
        }
    }
    if mesh.vertices == 0 {
        if mesh.lines > 0 && !mesh.keep_lines {
            return Err("nothing to export: STL holds polygons and points, not lines".into());
        }
        return Err("nothing to export".into());
    }
    Ok(mesh)
}

/// A ring's signed area (positive counter-clockwise).
fn ring_area(r: &[(f64, f64)]) -> f64 {
    let mut a = 0.0;
    for (i, p) in r.iter().enumerate() {
        let q = r.get((i + 1) % r.len()).unwrap_or(p);
        a += p.0 * q.1 - q.0 * p.1;
    }
    a / 2.0
}

/// A feature layer as Wavefront OBJ text.
fn obj_text(s: &mut Session, p: &Value) -> Result<String, String> {
    let mesh = layer_mesh(s, p, true)?;
    mesh_notices(s, &mesh);
    let mut obj = String::new();
    let l = s.layer_id(p).ok().and_then(|id| s.map().layer(id));
    let name = l.map(|l| l.name.replace(char::is_whitespace, "_")).unwrap_or_default();
    let _ = write!(obj, "# {name} exported by RustGIS\no {name}\n");
    let ids = |v: &[usize]| v.iter().map(|i| (i + 1).to_string()).collect::<Vec<_>>().join(" ");
    for item in &mesh.items {
        match item {
            Item::Vertex([x, y, z], false) => {
                let _ = writeln!(obj, "v {x:.2} {y:.2} {z:.2}");
            }
            Item::Vertex([x, y, z], true) => {
                if *y == 0.0 {
                    let _ = writeln!(obj, "v {x:.2} 0 {z:.2}");
                } else {
                    let _ = writeln!(obj, "v {x:.2} {y:.2} {z:.2}");
                }
            }
            Item::Tri(v, _) => {
                let _ = writeln!(obj, "f {}", ids(v));
            }
            Item::Quad(v, _) => {
                let _ = writeln!(obj, "f {}", ids(v));
            }
            Item::Poly(v, _) => {
                let _ = writeln!(obj, "f {}", ids(v));
            }
            Item::Polyline(v) => {
                let _ = writeln!(obj, "l {}", ids(v));
            }
        }
        if obj.len() > MAX_OBJ {
            return Err("the 3D file would be too large; export fewer features".into());
        }
    }
    Ok(obj)
}

/// What an STL export left out or approximated.
struct StlReport {
    triangles: usize,
    /// Zero-area triangles skipped.
    degenerate: usize,
    /// Polylines dropped.
    lines: usize,
    /// Caps fanned from their first vertex.
    fanned: usize,
}

/// Most triangles in an STL file: 50 bytes each, within [`MAX_OBJ`].
const MAX_STL_TRIANGLES: usize = (MAX_OBJ - 84) / 50;

/// [`stl_bytes`] with a smaller triangle cap, to test the size guard.
#[cfg(test)]
pub(crate) fn stl_with_cap(s: &Session, p: &Value, cap: usize) -> Result<usize, String> {
    mesh_stl(&layer_mesh(s, p, false)?, cap).map(|(_, r)| r.triangles)
}

/// Says what a layer's mesh left out, or used in place of a feature's own value.
fn mesh_notices(s: &mut Session, mesh: &Mesh) {
    if mesh.skipped > 0 {
        s.notify(format!("Skipped {} polygon(s) whose outer ring has fewer than 3 vertices", mesh.skipped), false);
    }
    if mesh.holes_dropped > 0 {
        s.notify(format!("Dropped {} hole(s) with fewer than 3 vertices: their areas are filled in", mesh.holes_dropped), false);
    }
    if mesh.default_heights > 0 {
        s.notify(format!("{} feature(s) without a numeric height used the default height instead", mesh.default_heights), false);
    }
}

/// A feature layer as binary STL, noting what it leaves out.
fn stl_bytes(s: &mut Session, p: &Value) -> Result<(Vec<u8>, StlReport), String> {
    let mesh = layer_mesh(s, p, false)?;
    let (bytes, report) = mesh_stl(&mesh, MAX_STL_TRIANGLES)?;
    mesh_notices(s, &mesh);
    if report.lines > 0 {
        s.notify(format!("STL holds triangles only: left out {} polyline(s)", report.lines), false);
    }
    if report.degenerate > 0 {
        s.notify(format!("Skipped {} zero-area triangle(s), such as walls of zero height", report.degenerate), false);
    }
    if report.fanned > 0 {
        s.notify(
            format!(
                "{} polygon cap(s) couldn't be triangulated and are approximated by fans over the outer ring: holes are filled, and concave parts may spill outside",
                report.fanned
            ),
            false,
        );
    }
    // Said on every export, as an assumed coordinate system is: STL can't record its unit.
    s.notify(
        "STL has no units: coordinates are metres east and north of the model's south-west corner and up from the features' base (below 0 for a negative height), in 32-bit floats",
        false,
    );
    Ok((bytes, report))
}

/// A mesh as binary STL: an 80-byte header, the triangle count, then per triangle a normal and
/// three vertices as little-endian f32 and a zero attribute.
fn mesh_stl(mesh: &Mesh, max_triangles: usize) -> Result<(Vec<u8>, StlReport), String> {
    // The mesh is east, up and south; STL is written east, north and up, as slicers expect.
    let mut verts: Vec<[f64; 3]> =
        mesh.items.iter().filter_map(|it| if let Item::Vertex([x, y, z], _) = it { Some([*x, -*z, *y]) } else { None }).collect();
    // Point cubes reach past the layer's corner: start east and north at the model's own corner.
    let low = |k: usize| verts.iter().map(|v| v[k]).fold(f64::INFINITY, f64::min);
    let (x0, y0) = (low(0), low(1));
    for v in &mut verts {
        v[0] -= x0;
        v[1] -= y0;
    }
    let at = |n: usize| verts.get(n).copied();
    // Each triangle is written as it comes (50 bytes), and the count filled in at the end.
    let mut out = b"RustGIS binary STL, metres, Z up".to_vec();
    out.resize(84, 0);
    let mut count = 0usize;
    let mut degenerate = 0;
    let mut add = |ids: [usize; 3], orient: Orient| -> Result<(), String> {
        let (Some(a), Some(b), Some(c)) = (at(ids[0]), at(ids[1]), at(ids[2])) else {
            return Err("the 3D mesh refers to a missing vertex".into());
        };
        // The file stores f32: judge and orient the triangle as written.
        let [a, b, c] = [a, b, c].map(|v| v.map(|x| f64::from(x as f32)));
        if [a, b, c].iter().flatten().any(|x| !x.is_finite()) {
            return Err("a coordinate is too large for a 32-bit STL file".into());
        }
        let n = normal(a, b, c);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-9 {
            degenerate += 1;
            return Ok(());
        }
        let flip = match orient {
            Orient::Keep => false,
            Orient::Flip => true,
            Orient::Up => n[2] < 0.0,
            Orient::Down => n[2] > 0.0,
        };
        count += 1;
        if count > max_triangles || count > u32::MAX as usize {
            return Err("the STL file would be too large; export fewer features".into());
        }
        let (b, c, n) = if flip { (c, b, [-n[0], -n[1], -n[2]]) } else { (b, c, n) };
        for x in n {
            out.extend_from_slice(&((x / len) as f32).to_le_bytes());
        }
        for v in [a, b, c] {
            for x in v {
                out.extend_from_slice(&(x as f32).to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
        Ok(())
    };
    for item in &mesh.items {
        match item {
            Item::Tri(v, o) => add(*v, *o)?,
            Item::Quad([a, b, c, d], o) => {
                add([*a, *b, *c], *o)?;
                add([*a, *c, *d], *o)?;
            }
            Item::Poly(v, o) => {
                if let Some(first) = v.first() {
                    for w in v.get(1..).unwrap_or(&[]).windows(2) {
                        if let [b, c] = w {
                            add([*first, *b, *c], *o)?;
                        }
                    }
                }
            }
            Item::Vertex(..) | Item::Polyline(_) => {}
        }
    }
    if count == 0 {
        return Err(if degenerate > 0 {
            "nothing to export: every triangle has zero area".into()
        } else {
            "nothing to export: the layer has no polygons or points".into()
        });
    }
    let n = u32::try_from(count).map_err(|_| "the STL file would be too large; export fewer features".to_string())?;
    if let Some(slot) = out.get_mut(80..84) {
        slot.copy_from_slice(&n.to_le_bytes());
    }
    let report = StlReport { triangles: count, degenerate, lines: mesh.lines, fanned: mesh.fanned };
    Ok((out, report))
}

/// The (unnormalised) normal of a triangle, counter-clockwise from the side it faces.
fn normal(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
    [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
}

/// The kinds of item [`item_bytes`] writes: (kind, extension, label).
pub const ITEMS: &[(&str, &str, &str)] = &[
    ("map", "rgismap", "RustGIS map file"),
    ("layer", "rgislyr", "RustGIS layer file"),
    ("styles", "rgisstyle", "RustGIS styles"),
    ("ramps", "rgisramp", "RustGIS colour ramps"),
    ("template", "rgistemplate", "RustGIS project template"),
    ("projectPackage", "rgispkg", "RustGIS project package"),
    ("mapPackage", "rgismpk", "RustGIS map package"),
    ("mobilePackage", "rgismpk", "RustGIS mobile map package"),
    ("gpPackage", "rgisgpk", "RustGIS results package"),
    ("task", "rgistask", "RustGIS task"),
    ("bookmarks", "rgisbookmarks", "RustGIS bookmarks"),
    ("obj", "obj", "Wavefront OBJ"),
    ("stl", "stl", "STL 3D model"),
    ("report", "rgisreport", "RustGIS report"),
    ("reportTemplate", "rgisrpttemplate", "RustGIS report template"),
    ("presentation", "rgispres", "RustGIS presentation"),
    ("notebook", "rgisnb", "RustGIS notebook"),
    ("jupyter", "ipynb", "Jupyter notebook"),
    ("dlPackage", "rgisdlpk", "RustGIS deep learning package"),
    ("hostedImagery", "zip", "Web map tiles"),
    ("webMap", "html", "Web map"),
    ("webLayer", "zip", "Web layer"),
    ("webStyle", "json", "Web style"),
    ("locator", "rgisloc", "Locator"),
    ("geodataService", "zip", "Geodata service"),
    ("webTool", "rgistool", "Web tool"),
    ("webScene", "gltf", "Web scene"),
    ("linkChart", "html", "Web link chart"),
];

/// An item as file bytes, with a suggested file name.
pub fn item_bytes(s: &mut Session, kind: &str, p: &Value) -> Result<(String, Vec<u8>), String> {
    let (name, bytes) = match kind {
        "map" => {
            let map = match ps(p, "map") {
                Some(n) => s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no map '{n}'"))?,
                None => s.map(),
            };
            crate::project::ensure_map_imports_complete(map)?;
            let mut map = embedded_map(map, &mut MAX_EMBED.clone());
            crate::project::strip_addresses(map.addresses_mut());
            (map.name.clone(), to_json(&map)?)
        }
        "layer" => {
            let id = s.layer_id(p)?;
            let l = s.map().layer(id).ok_or("no such layer")?.clone();
            if l.dataset.source.as_deref().is_some_and(|source| source.starts_with("geolibre-pending:")) {
                return Err("this layer is still downloading; wait for it to finish before saving".into());
            }
            let mut layer = embed_layer(l, &mut MAX_EMBED.clone());
            crate::project::strip_addresses(layer.addresses_mut().collect());
            (layer.name.clone(), to_json(&layer)?)
        }
        "styles" => {
            if s.project.styles.is_empty() {
                return Err("the project has no styles; make one with New Style".into());
            }
            ("styles".to_string(), to_json(&s.project.styles)?)
        }
        "ramps" => {
            if s.project.ramps.is_empty() {
                return Err("the project has no custom colour ramps; add some with Add Color Ramp File".into());
            }
            ("ramps".to_string(), to_json(&s.project.ramps)?)
        }
        "template" => {
            s.project.ensure_imports_complete()?;
            let (mut t, moves) = template(&s.project);
            if moves > 0 {
                s.notify(format!("{moves} saved label move(s) left out of the template, which keeps no features"), false);
            }
            // As a package: what it reads keeps its plain address, what it records no binding.
            crate::project::strip_addresses(t.addresses_mut());
            crate::project::clear_login_bindings(t.recorded_mut());
            if let Some(n) = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
                t.name = n.chars().take(200).collect();
            }
            (t.name.clone(), to_json(&t)?)
        }
        "projectPackage" => (s.project.name.clone(), package_project(s)?),
        "mapPackage" | "mobilePackage" => {
            let tol = f(p, "tolerance").unwrap_or(0.001).clamp(0.0, 1.0);
            (s.map().name.clone(), package_map(s, kind == "mobilePackage", tol)?)
        }
        "gpPackage" => ("results".to_string(), package_gp(s)?),
        "bookmarks" => (format!("{} bookmarks", s.map().name), super::navigate::bookmarks_file(s.map())?),
        "task" => {
            let mut t = task_ref(s, p)?.clone();
            let mut addresses = Vec::new();
            for step in &mut t.steps {
                crate::project::recorded_addresses(&mut step.params, &mut addresses);
            }
            crate::project::clear_login_bindings(addresses);
            (t.name.clone(), to_json(&t)?)
        }
        "obj" => {
            let id = s.layer_id(p)?;
            let name = s.map().layer(id).map(|l| l.name.clone()).unwrap_or_default();
            (name, obj_text(s, p)?.into_bytes())
        }
        "stl" => {
            let id = s.layer_id(p)?;
            let name = s.map().layer(id).map(|l| l.name.clone()).unwrap_or_default();
            (name, stl_bytes(s, p)?.0)
        }
        "report" | "reportTemplate" => {
            let i = super::report::index(s, p)?;
            let name = s.project.reports.get(i).map(|r| r.name.clone()).unwrap_or_default();
            (name, super::report::report_file(s, i, kind == "reportTemplate")?)
        }
        "presentation" => {
            let i = super::presentation::index(s, p)?;
            let name = s.project.presentations.get(i).map(|r| r.name.clone()).unwrap_or_default();
            (name, super::presentation::presentation_file(s, i)?)
        }
        "notebook" | "jupyter" => {
            let i = super::notebook::index(s, p)?;
            let name = s.project.notebooks.get(i).map(|r| r.name.clone()).unwrap_or_default();
            (name, super::notebook::notebook_file(s, i, kind == "jupyter")?)
        }
        "dlPackage" => {
            let name = match ps(p, "model") {
                Some(n) => n.to_string(),
                None => s.project.dl_models.last().map(|m| m.name.clone()).ok_or("train a model first")?,
            };
            (name.clone(), super::deeplearning::package(s, &json!({"model": name}))?)
        }
        "hostedImagery" => {
            let name = ps(p, "layer").unwrap_or("tiles").to_string();
            (format!("{name}_tiles"), super::deeplearning::hosted_bytes(s, p)?.0)
        }
        k if super::sharing::KINDS.iter().any(|x| x.0 == k) => {
            let (name, bytes, _) = super::sharing::share_bytes(s, k, p)?;
            super::sharing::note_label_classes(s, k, p);
            super::sharing::note_export(s, k, p, &name);
            (name, bytes)
        }
        k => return Err(format!("unknown item '{k}'")),
    };
    let ext = ITEMS.iter().find(|i| i.0 == kind).map_or("json", |i| i.1);
    Ok((format!("{}.{ext}", safe(&name)), bytes))
}

/// Writes an item to `path`.
pub(crate) fn save_item(s: &mut Session, kind: &str, p: &Value) -> CmdResult {
    let path = ps(p, "path").ok_or("path is required")?.to_string();
    let (_, bytes) = item_bytes(s, kind, p)?;
    rustgis_data::io::write_atomic(Path::new(&path), &bytes).map_err(|e| format!("{path}: {e}"))?;
    Ok(json!({"path": path, "bytes": bytes.len()}))
}
