//! Layers › Copy and Paste (#491): whole layers, with their groups, from one map to another (or
//! the same one, as a duplicate).
//!
//! A copy keeps everything that belongs to the layer (source, symbology, labels, transparency,
//! blend, scale range, definition query, display filters, mask) but not its selection. The
//! features aren't copied in memory until one side edits them (they are shared copy-on-write).

use std::collections::HashMap;

use rustgis_map::{Layer, LayerGroup, MAX_GROUPS, Map};
use serde_json::{Value, json};

use super::{copy_name, uint};
use crate::{CmdResult, CommandSpec as C, Session};

/// Most layer names or ids one copy takes (parameters are hostile).
const MAX_COPIED: usize = 10_000;

/// Layers on the clipboard: copies in drawing order (top first), with the groups that were copied
/// whole (their children are ids of `layers`).
#[derive(Clone, Debug)]
pub struct LayerClip {
    pub layers: Vec<Layer>,
    pub groups: Vec<LayerGroup>,
}

pub fn commands() -> Vec<C> {
    vec![
        C::new("layer.copy", "Copy Layers", "Layers", copy).view().params(
            r#"{"layer"?: name|id | [name|id, …] (default: the active layer, unless a group is given), "group"?: name|id | [name|id, …] (the group and all its layers)}"#,
        ),
        C::new("layer.paste", "Paste Layers", "Layers", paste).when(need_copied).params(
            r#"{"map"?: name (default: the map in view), "group"?: name|id (into that group, at its top; copied groups aren't kept, as groups don't nest), "to"?: position (0 = top, the default; never inside a group: above it)}"#,
        ),
    ]
}

fn need_copied(s: &Session) -> Option<&'static str> {
    if s.layer_clipboard.is_some() { None } else { Some("copy a layer first (Layers › Copy)") }
}

/// One name or id, or a list of them.
fn list(v: &Value) -> Result<Vec<&Value>, String> {
    match v {
        Value::Array(a) if a.len() > MAX_COPIED => Err(format!("at most {MAX_COPIED} at once")),
        Value::Array(a) => Ok(a.iter().collect()),
        other => Ok(vec![other]),
    }
}

fn copy(s: &mut Session, p: &Value) -> CmdResult {
    let mut ids: Vec<u32> = Vec::new();
    let mut groups: Vec<LayerGroup> = Vec::new();
    if let Some(v) = p.get("group").filter(|v| !v.is_null()) {
        for g in list(v)? {
            let id = super::group::group_id(s.map(), &json!({"group": g}))?;
            let Some(group) = s.map().group(id) else { continue };
            if groups.iter().all(|x| x.id != id) {
                ids.extend(&group.children);
                groups.push(group.clone());
            }
        }
    }
    match p.get("layer") {
        Some(v @ Value::Array(_)) => {
            for v in list(v)? {
                ids.push(s.layer_id(&json!({"layer": v}))?);
            }
        }
        Some(v) if !v.is_null() => ids.push(s.layer_id(p)?),
        // The active layer by default, unless only groups are copied.
        _ if groups.is_empty() => ids.push(s.layer_id(p)?),
        _ => {}
    }
    let m = s.map();
    ids.sort_by_key(|id| m.layer_index(*id));
    ids.dedup();
    let layers: Vec<Layer> = ids
        .iter()
        .filter_map(|id| m.layer(*id))
        .map(|l| {
            let mut l = l.clone();
            l.selection.clear();
            l
        })
        .collect();
    // Nothing named (an empty list) leaves the clipboard as it was.
    if layers.is_empty() && groups.is_empty() {
        return Err("nothing to copy".into());
    }
    let names: Vec<String> = layers.iter().map(|l| l.name.clone()).collect();
    s.layer_clipboard = Some(LayerClip { layers, groups });
    let n = names.len();
    s.notify(if n == 1 { format!("Copied {}", names.join("")) } else { format!("Copied {n} layers") }, false);
    Ok(json!({"copied": n, "layers": names}))
}

/// A name no layer of the map has (case aside): the name itself, else `<name> copy`, numbered.
fn unique_layer_name(m: &Map, name: &str) -> String {
    let taken = |n: &str| m.layers.iter().any(|l| l.name.eq_ignore_ascii_case(n));
    if taken(name) { copy_name(None, name, taken) } else { name.to_string() }
}

fn unique_group_name(m: &Map, name: &str) -> String {
    let taken = |n: &str| m.groups.iter().any(|g| g.name.eq_ignore_ascii_case(n));
    if taken(name) { copy_name(None, name, taken) } else { name.to_string() }
}

fn paste(s: &mut Session, p: &Value) -> CmdResult {
    let clip = s.layer_clipboard.clone().ok_or("no layers have been copied")?;
    let mi = super::navigate::map_at(s, p)?;
    let m = s.project.maps.get(mi).ok_or("no such map")?;
    let into = match p.get("group") {
        None | Some(Value::Null) => None,
        Some(_) => Some(super::group::group_id(m, p)?),
    };
    let to = match p.get("to") {
        None | Some(Value::Null) => None,
        Some(_) => Some(uint(p, "to").ok_or("to must be a position: 0 at the top")?),
    };
    if into.is_some() && to.is_some() {
        return Err("give a group or a position, not both".into());
    }
    if into.is_some() && clip.layers.is_empty() {
        return Err("the copied groups have no layers to paste into a group".into());
    }
    // Groups don't nest: pasted into a group, the copied groups' layers just join it.
    let groups = if into.is_some() { Vec::new() } else { clip.groups };
    if m.groups.len().saturating_add(groups.len()) > MAX_GROUPS {
        return Err(format!("a map holds at most {MAX_GROUPS} groups"));
    }
    // Where the copies go: above this layer (`None`: the bottom, as on an empty map).
    let before = match into {
        Some(g) => m.layers.iter().find(|l| m.group(g).is_some_and(|g| g.children.contains(&l.id))).map(|l| l.id),
        None => m.layers.get(usize::try_from(to.unwrap_or(0)).unwrap_or(usize::MAX)).map(|l| l.id),
    };
    let m = s.project.maps.get_mut(mi).ok_or("no such map")?;
    let mut new_ids: HashMap<u32, u32> = HashMap::new();
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut pasted: Vec<u32> = Vec::new();
    for mut l in clip.layers {
        let old = l.id;
        let name = unique_layer_name(m, &l.name);
        renamed.insert(l.name.to_lowercase(), name.clone());
        l.dataset.name = name.clone();
        l.name = name;
        let id = m.add_layer(l);
        m.place_layer(id, into, before)?;
        new_ids.insert(old, id);
        pasted.push(id);
    }
    // A mask follows its layer when that was pasted too; one the map lacks is cleared.
    let mut unmasked: Vec<String> = Vec::new();
    for id in &pasted {
        let Some(mask) = m.layer(*id).and_then(|l| l.mask.clone()) else { continue };
        let target = renamed.get(&mask.to_lowercase()).cloned().or_else(|| m.layer_by_name(&mask).map(|l| l.name.clone()));
        if let Some(l) = m.layer_mut(*id) {
            if target.is_none() {
                unmasked.push(l.name.clone());
            }
            l.mask = target;
        }
    }
    let mut group_ids = Vec::new();
    for mut g in groups {
        g.children = g.children.iter().filter_map(|c| new_ids.get(c).copied()).collect();
        g.name = unique_group_name(m, &g.name);
        let gid = m.add_group(g);
        m.gather_group(gid);
        group_ids.push(gid);
    }
    let map = m.name.clone();
    if mi == s.project.active_map
        && let Some(first) = pasted.first()
    {
        s.active_layer = Some(*first);
    }
    // Said, as a paste from the Browser can go to a map out of view.
    let n = pasted.len();
    s.notify(if n == 1 { format!("Pasted 1 layer into {map}") } else { format!("Pasted {n} layers into {map}") }, false);
    if !unmasked.is_empty() {
        s.notify(format!("{} lost their mask: {map} has no layer by that name", unmasked.join(", ")), false);
    }
    Ok(json!({"map": map, "layers": pasted, "groups": group_ids, "unmasked": unmasked}))
}
