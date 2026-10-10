//! Map tab › Navigate, Layer (basemap) and Insert › Project (new map), bookmarks.

use rustgis_geom::{Coord, Extent, lonlat_to_merc};
use rustgis_map::{Basemap, Bookmark, Map, Projection};
use serde_json::{Value, json};

use super::{f, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("map.zoomFull", "Full Extent", "Map › Navigate", zoom_full).view(),
        C::new("map.zoomIn", "Fixed Zoom In", "Map › Navigate", |s, _| zoom(s, 2.0)).view().key("Mod+="),
        C::new("map.zoomOut", "Fixed Zoom Out", "Map › Navigate", |s, _| zoom(s, 0.5)).view().key("Mod+-"),
        C::new("map.prevExtent", "Previous Extent", "Map › Navigate", |s, _| {
            if s.extent_back() { Ok(Value::Null) } else { Err("no previous extent".into()) }
        })
        .view()
        .when(|s| if s.can_extent_back() { None } else { Some("no previous extent") }),
        C::new("map.nextExtent", "Next Extent", "Map › Navigate", |s, _| {
            if s.extent_forward() { Ok(Value::Null) } else { Err("no next extent".into()) }
        })
        .view()
        .when(|s| if s.can_extent_forward() { None } else { Some("no next extent") }),
        C::new("map.zoomWorld", "Zoom To World", "Map › Navigate", |s, _| {
            s.zoom_to(&Extent::new(-170.0, -58.0, 180.0, 78.0));
            Ok(Value::Null)
        })
        .view(),
        C::new("map.goToXY", "Go To XY", "Map › Navigate", go_to).view().params(r#"{"lon": number, "lat": number, "scale"?: number}"#),
        C::new("map.setScale", "Set Scale", "Status Bar", |s, p| {
            let sc = f(p, "scale").ok_or("scale is required")?;
            s.map_mut().camera.set_scale(sc);
            s.record_extent();
            Ok(Value::Null)
        })
        .view()
        .params(r#"{"scale": number}"#),
        C::new("map.basemap", "Basemap", "Map › Layer", basemap).params(r#"{"name": "paper"|"midnight"|"ocean"|"atlas"|"blank"}"#),
        C::new("map.graticule", "Graticule", "View › Display", |s, p| {
            let m = s.map_mut();
            m.graticule = super::b(p, "on").unwrap_or(!m.graticule);
            Ok(Value::Null)
        }),
        C::new("map.projection", "Globe", "View › View", |s, p| {
            let m = s.map_mut();
            m.projection = match ps(p, "type") {
                Some(t) => Projection::from_code(t).ok_or_else(|| format!("unknown projection '{t}'"))?,
                None if m.projection == Projection::Globe => Projection::WebMercator,
                None => Projection::Globe,
            };
            Ok(json!({"projection": m.projection.code()}))
        })
        .params(r#"{"type"?: "mercator"|"globe" (toggles when omitted)}"#),
        C::new("bookmark.add", "New Bookmark", "Map › Navigate › Bookmarks", add_bookmark).params(r#"{"name"?: string}"#),
        C::new("bookmark.go", "Go To Bookmark", "Map › Navigate › Bookmarks", go_bookmark).view().params(r#"{"name": string}"#),
        C::new("bookmark.remove", "Remove Bookmark", "Map › Navigate › Bookmarks", |s, p| {
            let n = s_name(p)?;
            let m = s.map_mut();
            let before = m.bookmarks.len();
            m.bookmarks.retain(|b| !b.name.eq_ignore_ascii_case(n));
            if m.bookmarks.len() == before { Err(format!("no bookmark '{n}'")) } else { Ok(Value::Null) }
        }),
        C::new("map.new", "New Map", "Insert › Project", new_map).params(r#"{"name"?: string}"#),
        C::new("map.activate", "Activate Map", "Catalog", |s, p| {
            let n = s_name(p)?;
            let i = s.project.maps.iter().position(|m| m.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no map '{n}'"))?;
            s.project.active_map = i;
            s.active_layer = s.map().layers.first().map(|l| l.id);
            s.table_layer = None;
            Ok(Value::Null)
        })
        .view(),
        C::new("map.rename", "Rename Map", "Contents", |s, p| {
            let n = s_name(p)?.trim();
            if n.is_empty() {
                return Err("name is required".into());
            }
            s.map_mut().name = n.chars().take(200).collect();
            Ok(Value::Null)
        }),
        C::new("map.remove", "Delete Map", "Catalog", |s, p| {
            if s.project.maps.len() < 2 {
                return Err("a project keeps at least one map".into());
            }
            let n = s_name(p)?;
            let i = s.project.maps.iter().position(|m| m.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no map '{n}'"))?;
            s.project.maps.remove(i);
            s.project.active_map = 0;
            s.active_layer = s.map().layers.first().map(|l| l.id);
            Ok(Value::Null)
        }),
    ]
}

fn s_name(p: &Value) -> Result<&str, String> {
    ps(p, "name").ok_or_else(|| "name is required".to_string())
}

fn zoom_full(s: &mut Session, _: &Value) -> CmdResult {
    let e = s.map().full_extent();
    let e = if e.is_empty() { Extent::new(-170.0, -58.0, 180.0, 78.0) } else { e };
    s.zoom_to(&e);
    Ok(Value::Null)
}

fn zoom(s: &mut Session, factor: f64) -> CmdResult {
    let (w, h) = s.view_size;
    s.map_mut().camera.zoom_at(factor, w / 2.0, h / 2.0, w, h);
    s.record_extent();
    Ok(Value::Null)
}

fn go_to(s: &mut Session, p: &Value) -> CmdResult {
    let lon = f(p, "lon").or_else(|| f(p, "x")).ok_or("lon is required")?;
    let lat = f(p, "lat").or_else(|| f(p, "y")).ok_or("lat is required")?;
    if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
        return Err("longitude must be within ±180 and latitude within ±90".into());
    }
    let cam = &mut s.map_mut().camera;
    cam.center = lonlat_to_merc(Coord::new(lon, lat));
    if let Some(sc) = f(p, "scale") {
        cam.set_scale(sc);
    }
    cam.clamp();
    s.record_extent();
    Ok(Value::Null)
}

fn basemap(s: &mut Session, p: &Value) -> CmdResult {
    let n = s_name(p)?;
    s.map_mut().basemap = Basemap::from_code(n).ok_or_else(|| format!("unknown basemap '{n}'"))?;
    Ok(Value::Null)
}

fn add_bookmark(s: &mut Session, p: &Value) -> CmdResult {
    let m = s.map_mut();
    let name = match super::ps(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.chars().take(120).collect(),
        None => format!("Bookmark {}", m.bookmarks.len() + 1),
    };
    m.bookmarks.retain(|b| !b.name.eq_ignore_ascii_case(&name));
    m.bookmarks.push(Bookmark { name: name.clone(), camera: m.camera });
    Ok(json!({"name": name}))
}

fn go_bookmark(s: &mut Session, p: &Value) -> CmdResult {
    let n = s_name(p)?;
    let cam =
        s.map().bookmarks.iter().find(|b| b.name.eq_ignore_ascii_case(n)).map(|b| b.camera).ok_or_else(|| format!("no bookmark '{n}'"))?;
    s.map_mut().camera = cam;
    s.record_extent();
    Ok(Value::Null)
}

fn new_map(s: &mut Session, p: &Value) -> CmdResult {
    let base = super::ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Map").to_string();
    let mut name = base.clone();
    let mut i = 1;
    while s.project.maps.iter().any(|m| m.name.eq_ignore_ascii_case(&name)) {
        i += 1;
        name = format!("{base}{i}");
    }
    let mut m = Map::new(name.clone());
    m.basemap = s.map().basemap;
    s.project.maps.push(m);
    s.project.active_map = s.project.maps.len() - 1;
    s.active_layer = None;
    s.table_layer = None;
    Ok(json!({"name": name}))
}
