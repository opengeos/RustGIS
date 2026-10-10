//! Project tab (backstage), undo/redo and tool switching.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::ps;
use crate::{CmdResult, CommandSpec as C, Project, Session, Tool};

pub fn commands() -> Vec<C> {
    vec![
        // Replacing or saving the project isn't an undoable edit: these are `.view()` commands
        // that manage `dirty` and the history themselves.
        C::new("project.new", "New Project", "Project", new).key("Mod+N").view().params(r#"{"name"?: string}"#),
        C::new("project.sample", "World Atlas Sample", "Project", sample).view(),
        C::new("project.open", "Open Project", "Project", open).key("Mod+O").view().params(r#"{"path": string}"#),
        C::new("project.openText", "Open Project (from text)", "Project", open_text).view().params(r#"{"text": project JSON}"#),
        C::new("project.save", "Save Project", "Project", save).key("Mod+S").view().params(r#"{"path"?: string}"#),
        C::new("project.saveAs", "Save Project As", "Project", save_as).key("Mod+Shift+S").view().params(r#"{"path": string}"#),
        C::new("project.rename", "Rename Project", "Project", rename).params(r#"{"name": string}"#),
        C::new("edit.undo", "Undo", "Quick Access Toolbar", |s, _| if s.undo() { Ok(Value::Null) } else { Err("nothing to undo".into()) })
            .key("Mod+Z")
            .view()
            .when(|s| if s.can_undo() { None } else { Some("nothing to undo") }),
        C::new("edit.redo", "Redo", "Quick Access Toolbar", |s, _| if s.redo() { Ok(Value::Null) } else { Err("nothing to redo".into()) })
            .key("Mod+Y")
            .view()
            .when(|s| if s.can_redo() { None } else { Some("nothing to redo") }),
        C::new("tool.set", "Set Map Tool", "Map › Navigate", set_tool)
            .view()
            .params(r#"{"tool": "explore"|"select"|"zoomIn"|"measure"|"measureArea"|"createPoint"|"createLine"|"createPolygon"}"#),
        C::new("info.project", "Project Info", "Project", info).view(),
        C::new("info.commands", "List Commands", "Help", list).view(),
    ]
}

fn reset(s: &mut Session, p: Project) {
    s.project = p;
    s.active_layer = s.map().layers.first().map(|l| l.id);
    s.table_layer = None;
    s.tool = Tool::Explore;
    s.clear_history();
}

fn new(s: &mut Session, p: &Value) -> CmdResult {
    reset(s, Project::new(super::ps(p, "name").unwrap_or("Untitled")));
    s.path = None;
    Ok(Value::Null)
}

fn sample(s: &mut Session, _: &Value) -> CmdResult {
    reset(s, crate::sample::world_project());
    s.active_layer = s.map().layer_by_name("Countries").map(|l| l.id);
    s.path = None;
    Ok(Value::Null)
}

fn open(s: &mut Session, p: &Value) -> CmdResult {
    let path = PathBuf::from(super::ps(p, "path").ok_or("path is required")?);
    let proj = Project::load(&path)?;
    reset(s, proj);
    s.path = Some(path);
    s.notify(format!("Opened {}", s.project.name), false);
    Ok(Value::Null)
}

fn open_text(s: &mut Session, p: &Value) -> CmdResult {
    let text = super::ps(p, "text").ok_or("text is required")?;
    let proj = Project::from_json(text)?;
    reset(s, proj);
    s.path = None;
    s.notify(format!("Opened {}", s.project.name), false);
    Ok(Value::Null)
}

fn write(s: &mut Session, path: PathBuf) -> CmdResult {
    let path = if path.extension().is_none() { path.with_extension(crate::project::EXTENSION) } else { path };
    s.project.save(&path)?;
    s.dirty = false;
    s.notify(format!("Saved {}", path.display()), false);
    s.path = Some(path.clone());
    Ok(json!({"path": path.display().to_string()}))
}

fn save(s: &mut Session, p: &Value) -> CmdResult {
    let path = match super::ps(p, "path") {
        Some(x) => PathBuf::from(x),
        None => s.path.clone().ok_or("this project hasn't been saved yet; use Save As")?,
    };
    write(s, path)
}

fn save_as(s: &mut Session, p: &Value) -> CmdResult {
    let path = PathBuf::from(super::ps(p, "path").ok_or("path is required")?);
    if let Some(stem) = path.file_stem().and_then(|x| x.to_str()) {
        s.project.name = stem.to_string();
    }
    write(s, path)
}

fn rename(s: &mut Session, p: &Value) -> CmdResult {
    let n = super::ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or("name is required")?;
    s.project.name = n.chars().take(200).collect();
    Ok(Value::Null)
}

fn set_tool(s: &mut Session, p: &Value) -> CmdResult {
    let t = parse_tool(p)?;
    if t.creates() {
        let id = s.layer_id(&Value::Null).map_err(|_| "select the layer to edit in the Contents pane first".to_string())?;
        let gt = s.map().layer(id).map(|l| l.geometry_type());
        let ok = matches!(
            (t, gt),
            (Tool::CreatePoint, Some(rustgis_data::GeometryType::Point))
                | (Tool::CreateLine, Some(rustgis_data::GeometryType::Polyline))
                | (Tool::CreatePolygon, Some(rustgis_data::GeometryType::Polygon))
        );
        if !ok {
            return Err(format!("{} needs a {} layer", t.label(), t.label().trim_start_matches("Create ").to_lowercase()));
        }
    }
    s.tool = t;
    Ok(Value::Null)
}

fn parse_tool(p: &Value) -> Result<Tool, String> {
    let name = ps(p, "tool").ok_or("tool is required")?;
    Tool::from_code(name).ok_or_else(|| format!("unknown tool '{name}'"))
}

fn info(s: &mut Session, _: &Value) -> CmdResult {
    let m = s.map();
    Ok(json!({
        "name": s.project.name,
        "path": s.path.as_ref().map(|p| p.display().to_string()),
        "maps": s.project.maps.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
        "activeMap": m.name,
        "basemap": m.basemap.code(),
        "projection": m.projection.code(),
        "scale": m.camera.scale().round(),
        "center": {"lon": m.camera.center_lonlat().x, "lat": m.camera.center_lonlat().y},
        "layers": m.layers.iter().map(|l| match &l.raster {
            Some(rl) => json!({
                "id": l.id, "name": l.name, "type": "Raster", "visible": l.visible, "renderer": rl.style.renderer.label(),
                "size": rl.data.as_ref().map(|d| json!([d.width, d.height, d.band_count()])),
                "missing": rl.data.is_none(),
            }),
            None => json!({
                "id": l.id, "name": l.name, "type": l.geometry_type().label(), "features": l.dataset.features.len(),
                "visible": l.visible, "renderer": l.renderer.kind_label(), "selected": l.selection.len(),
                "fields": l.dataset.fields.iter().map(|f| format!("{} ({})", f.name, f.ty.label())).collect::<Vec<_>>(),
            }),
        }).collect::<Vec<_>>(),
        "tables": s.project.tables.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
    }))
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    Ok(Value::Array(
        s.registry
            .commands
            .iter()
            .map(|c| json!({"id": c.id, "label": c.label, "location": c.location, "shortcut": c.shortcut, "params": c.params}))
            .collect(),
    ))
}
