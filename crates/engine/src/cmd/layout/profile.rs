//! A raster's profile beside its map frame (#2863 part 2): what `layout.addElement` and
//! `layout.update` read for one, and what they say of it.

use rustgis_map::layout::{
    MAX_PROFILE_BAND, MAX_PROFILE_TEXT, PROFILE_GAPS, PROFILE_LINE_WIDTHS, PROFILE_THICKNESSES, Profile, ProfileSide,
};

use super::*;

/// The keys of a profile's own, which `layout.addElement` and `layout.update` read.
const PROFILE_KEYS: &[&str] = &[
    "section",
    "layer",
    "band",
    "side",
    "gap",
    "thickness",
    "title",
    "unit",
    "lineColor",
    "lineWidth",
    "fill",
    "min",
    "max",
    "units",
    "guides",
    "color",
];
/// What `layout.addElement` reads for a profile, besides [`PROFILE_KEYS`].
const ADD_PROFILE_KEYS: &[&str] = &["layout", "kind", "frame", "background"];
/// What `layout.update` reads for a profile, besides [`PROFILE_KEYS`]: `x`, `y`, `w` and `h` as its
/// gap and thickness ([`Profile::take_rect`]).
const UPDATE_PROFILE_KEYS: &[&str] = &["layout", "element", "name", "visible", "locked", "background", "x", "y", "w", "h"];

/// A profile's keys that null gives back their default: no fill, the samples' own ends, the dark
/// grey ink.
const PROFILE_NULL_KEYS: &[&str] = &["fill", "min", "max", "color", "background"];

/// Whether element `id` of layout `li` is a profile.
pub(in crate::cmd::layout) fn is_profile(s: &Session, li: usize, id: u32) -> bool {
    s.project.layouts.get(li).and_then(|l| l.element(id)).is_some_and(|e| e.kind.is_profile())
}

/// A frame's map, its section lines and its box (x, y, w, h in mm).
fn frame_parts<'a>(s: &'a Session, l: &'a Layout, frame: u32) -> Result<(&'a Map, &'a [Section], (f64, f64, f64, f64)), String> {
    let e = l.element(frame).ok_or_else(|| format!("no element {frame}"))?;
    let ElementKind::MapFrame { map, sections, .. } = &e.kind else {
        return Err(format!("element {frame} isn't a map frame"));
    };
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)).ok_or_else(|| format!("no map '{map}'"))?;
    Ok((m, sections, (e.x, e.y, e.w, e.h)))
}

/// Why `layer` can't be profiled, if it can't: a profile reads a raster held whole, as a remote or
/// tiled one read in place gives only the overview drawn, which changes with the scale.
fn not_held_whole(layer: &rustgis_map::Layer) -> Option<String> {
    let rl = layer.raster.as_ref()?;
    let why = if rl.in_place || rl.tiled {
        "is read in place, a part at a time"
    } else if rl.data.as_ref().is_some_and(|r| r.overview.is_some()) {
        "holds only an overview of its file"
    } else {
        return None;
    };
    Some(format!("'{}' {why}: a profile reads a raster held whole", layer.name))
}

/// The raster layer of `map` named `name` (any case), as the map spells it, if a profile can read it.
fn profile_layer(map: &Map, name: &str) -> Result<String, String> {
    let layer =
        map.layer_by_name(name).filter(|l| l.is_raster()).ok_or_else(|| format!("no raster layer '{}' in {}", name.trim(), map.name))?;
    match not_held_whole(layer) {
        Some(why) => Err(why),
        None => Ok(layer.name.clone()),
    }
}

/// The top raster the map shows that a profile can read: a new profile's layer unless given one.
fn first_profile_layer(map: &Map) -> Result<String, String> {
    let found = map.layers.iter().filter(|l| l.is_raster() && map.shows(l)).find(|l| not_held_whole(l).is_none());
    found.map(|l| l.name.clone()).ok_or_else(|| format!("{} shows no raster a profile can read: give a layer", map.name))
}

/// The bands of `map`'s layer `name` when its pixels are read.
fn bands_of(map: &Map, name: &str) -> Option<usize> {
    map.layer_by_name(name).and_then(|l| l.raster.as_ref()).and_then(|r| r.data.as_ref()).map(|r| r.bands.len())
}

/// A number given as `k`, or null for none, when `p` has it: `Some(None)` for null.
fn number_or_null(p: &Value, k: &str) -> Result<Option<Option<f64>>, String> {
    match p.get(k) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => v.as_f64().filter(|v| v.is_finite()).map(|v| Some(Some(v))).ok_or_else(|| format!("{k} is a number, or null")),
    }
}

/// A profile with the options in `p` over `base`'s, in layout `l`, and what was held to its limits;
/// a bad value is an error, leaving nothing changed.
fn profile_params(s: &Session, l: &Layout, p: &Value, mut base: Profile) -> Result<(Profile, Vec<String>), String> {
    let (map, sections, _) = frame_parts(s, l, base.frame)?;
    let mut held = Vec::new();
    if let Some(n) = given_name(p, "section", "a section line's name")? {
        let found = Section::find(sections, n).and_then(|i| sections.get(i));
        base.section = found.map(|s| s.name.clone()).ok_or_else(|| format!("the frame has no section line '{}'", n.trim()))?;
    }
    let line = Section::find(sections, &base.section).and_then(|i| sections.get(i)).map(|s| s.line);
    match given_name(p, "side", "bottom, top, left or right")? {
        Some(v) => {
            let side = ProfileSide::from_name(v).ok_or_else(|| format!("unknown side '{v}' (bottom, top, left or right)"))?;
            if let Some(line) = line.filter(|line| !side.suits(*line)) {
                let sides = if side.is_horizontal() { "left or right" } else { "bottom or top" };
                return Err(format!("a profile of a {} section line goes {sides} of its frame", line.code()));
            }
            base.side = side;
        }
        // Moved to a line running the other way, it goes to that line's side.
        None => {
            if let Some(line) = line.filter(|line| !base.side.suits(*line)) {
                base.side = ProfileSide::default_for(line);
            }
        }
    }
    if let Some(n) = given_name(p, "layer", "a raster layer's name")? {
        base.layer = profile_layer(map, n)?;
    }
    if let Some(v) = given_num(p, "band", "a band number from 1")? {
        if v.fract() != 0.0 || v < 1.0 || v > f64::from(MAX_PROFILE_BAND) {
            return Err(format!("band is a whole number from 1 to {MAX_PROFILE_BAND}"));
        }
        base.band = v as u32;
    }
    if let Some(n) = bands_of(map, &base.layer).filter(|n| base.band as usize > *n) {
        return Err(format!("'{}' has {n} band(s): band is 1 to {n}", base.layer));
    }
    let mut mm = |k: &str, range: (f64, f64), v: &mut f64| -> Result<(), String> {
        if let Some(n) = given_num(p, k, "a number of mm")? {
            let kept = n.clamp(range.0, range.1);
            if kept != n {
                held.push(format!("{k} {n} held to {kept} ({} to {} mm)", range.0, range.1));
            }
            *v = kept;
        }
        Ok(())
    };
    mm("gap", PROFILE_GAPS, &mut base.gap)?;
    mm("thickness", PROFILE_THICKNESSES, &mut base.thickness)?;
    for (k, text) in [("title", &mut base.title), ("unit", &mut base.unit)] {
        if let Some(t) = given_name(p, k, "text")? {
            let t = crate::cmd::within(k, "a profile's title or unit", t, MAX_PROFILE_TEXT)?;
            if t.contains(['\n', '\r']) {
                return Err(format!("{k} is one line"));
            }
            *text = t.trim().to_string();
        }
    }
    base.line_color = color(p, "lineColor", base.line_color)?;
    if let Some(w) = given_num(p, "lineWidth", "a number of points")? {
        let kept = w.clamp(PROFILE_LINE_WIDTHS.0, PROFILE_LINE_WIDTHS.1);
        if kept != w {
            held.push(format!("lineWidth {w} held to {kept} ({} to {} points)", PROFILE_LINE_WIDTHS.0, PROFILE_LINE_WIDTHS.1));
        }
        base.line_width = kept;
    }
    match p.get("fill") {
        None => {}
        Some(Value::Null) => base.fill = None,
        Some(_) => base.fill = Some(color(p, "fill", base.line_color)?),
    }
    if let Some(v) = number_or_null(p, "min")? {
        base.min = v;
    }
    if let Some(v) = number_or_null(p, "max")? {
        base.max = v;
    }
    if let (Some(lo), Some(hi)) = (base.min, base.max)
        && lo >= hi
    {
        return Err(format!("min ({lo}) is below max ({hi})"));
    }
    if let Some(v) = given_name(p, "units", "auto, kilometers, meters, miles or feet")? {
        base.units = ScaleUnits::from_name(v).ok_or_else(|| format!("unknown units '{v}' (auto, kilometers, meters, miles or feet)"))?;
    }
    match p.get("guides").filter(|v| !v.is_null()) {
        Some(Value::Bool(v)) => base.guides = *v,
        Some(_) => return Err("guides is true or false".into()),
        None => {}
    }
    if p.get("color").is_some() {
        base.color = given_color(p, "color")?;
    }
    Ok((base, held))
}

/// `layout.addElement` for a profile (#2863 part 2): along a section line of its frame (its first
/// unless named), of a raster of the frame's map (the top one shown unless named), on the side the
/// line runs along, its box the frame's side ([`Profile::rect`]).
pub(in crate::cmd::layout) fn add_profile(s: &mut Session, p: &Value) -> CmdResult {
    let keys: Vec<&str> = ADD_PROFILE_KEYS.iter().chain(PROFILE_KEYS).copied().collect();
    if ["x", "y", "w", "h"].iter().any(|k| gives(p, k)) {
        return Err("a profile's box is its frame's side: give it a gap and a thickness (mm), not x, y, w or h".into());
    }
    only_add_keys(p, "a profile", &keys, &["name"])?;
    let background = match element_background(p, None)? {
        BackgroundParam::Set(b) => Some(b),
        BackgroundParam::Absent | BackgroundParam::Remove => None,
    };
    let li = index(s, p)?;
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let frame = frame_of(l, p)?;
    let (map, sections, frame_box) = frame_parts(s, l, frame)?;
    let first = sections.first().ok_or("the frame has no section line: add one first (layout.addElement with kind section)")?;
    let layer = if gives(p, "layer") { String::new() } else { first_profile_layer(map)? };
    let base = Profile::new(frame, first.name.clone(), layer, ProfileSide::default_for(first.line));
    let (mut new, held) = profile_params(s, l, p, base)?;
    // Titled with its layer's name unless given one ("" for none), as a colorbar is.
    if p.get("title").is_none_or(Value::is_null) {
        new.title = new.layer.chars().take(MAX_PROFILE_TEXT).collect();
    }
    let rect = new.rect(frame_box);
    let l = s.project.layouts.get_mut(li).ok_or("no layout")?;
    let id = l.add("Profile", rect, ElementKind::Profile(new));
    if let Some(e) = l.element_mut(id) {
        e.background = background;
    }
    say_held(s, "Add Element", held);
    if gives(p, "name") {
        s.notify("Add Element ignores \"name\" for a profile: only a section takes one (layout.update names an element)", false);
    }
    say_profile_drawn(s, li, id);
    Ok(json!({"element": id}))
}

/// `layout.update` for a profile: its own options, its name, visibility, lock and background, and
/// `x`, `y`, `w` and `h` read as its gap and thickness, along its frame it keeping the frame's.
pub(in crate::cmd::layout) fn update_profile(s: &mut Session, p: &Value, li: usize, id: u32) -> CmdResult {
    let keys: Vec<&str> = UPDATE_PROFILE_KEYS.iter().chain(PROFILE_KEYS).copied().collect();
    if let Some(k) = p.as_object().and_then(|o| o.keys().find(|k| !keys.contains(&k.as_str()) && gives(p, k))) {
        let shown: String = k.chars().take(40).map(|c| if c.is_control() { '?' } else { c }).collect();
        return Err(format!("layout.update: a profile takes no \"{shown}\" (it reads {})", keys.join(", ")));
    }
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let e = l.element(id).ok_or_else(|| format!("no element {id}"))?;
    let ElementKind::Profile(current) = &e.kind else { return Err(format!("element {id} isn't a profile")) };
    let changes = p.as_object().is_some_and(|o| {
        o.iter().any(|(k, v)| {
            (!v.is_null() || PROFILE_NULL_KEYS.contains(&k.as_str())) && !matches!(k.as_str(), "layout" | "element" | "locked")
        })
    });
    if changes && b(p, "locked") != Some(false) {
        unlocked(l, id)?;
    }
    let new_name = crate::cmd::name_given(p, "an element's name", 200)?;
    let new_background = match element_background(p, e.background)? {
        BackgroundParam::Set(b) => Some(Some(b)),
        BackgroundParam::Remove => Some(None),
        BackgroundParam::Absent => None,
    };
    let boxed = ["x", "y", "w", "h"].iter().any(|k| gives(p, k));
    if boxed && (gives(p, "gap") || gives(p, "thickness")) {
        return Err("give a profile a gap and thickness, or x, y, w and h, not both".into());
    }
    let (mut new, mut held) = profile_params(s, l, p, current.clone())?;
    let (_, _, frame_box) = frame_parts(s, l, new.frame)?;
    if boxed {
        let rect = (f(p, "x").unwrap_or(e.x), f(p, "y").unwrap_or(e.y), f(p, "w").unwrap_or(e.w), f(p, "h").unwrap_or(e.h));
        if new.take_rect(frame_box, rect) {
            let ((g0, g1), (t0, t1)) = (PROFILE_GAPS, PROFILE_THICKNESSES);
            held.push(format!("its gap held to {} ({g0} to {g1} mm) and thickness to {} ({t0} to {t1} mm)", new.gap, new.thickness));
        }
    }
    let reread = ["section", "layer", "band", "side"].iter().any(|k| gives(p, k));
    let l = s.project.layouts.get_mut(li).ok_or("no layout")?;
    let e = l.element_mut(id).ok_or_else(|| format!("no element {id}"))?;
    e.kind = ElementKind::Profile(new);
    if let Some(n) = new_name {
        e.name = n;
    }
    if let Some(v) = b(p, "visible") {
        e.visible = v;
    }
    if let Some(v) = b(p, "locked") {
        e.locked = v;
    }
    if let Some(v) = new_background {
        e.background = v;
    }
    l.fit_profiles();
    say_held(s, "Element Properties", held);
    if reread {
        say_profile_drawn(s, li, id);
    }
    Ok(Value::Null)
}

/// Says that profile `id` of layout `li` isn't drawn, and why, or that it is read at fewer samples
/// than its raster's cells along its section line, when it is.
pub(in crate::cmd::layout) fn say_profile_drawn(s: &mut Session, li: usize, id: u32) {
    let Some(l) = s.project.layouts.get(li) else { return };
    let notes = rustgis_render::layout::profiles_not_drawn(l, &s.project.maps);
    let notes = notes.into_iter().filter(|n| n.element == id).map(|n| (n, 1)).collect();
    for note in rustgis_render::layout::profiles_notice(&notes, 1) {
        s.warn(note);
    }
}

/// The profiles of section line `old` of frame `frame` named `new` instead, after the line was
/// renamed: they follow it.
pub(in crate::cmd::layout) fn profiles_follow_section(l: &mut Layout, frame: u32, old: &str, new: &str) {
    for e in &mut l.elements {
        if let ElementKind::Profile(p) = &mut e.kind
            && p.frame == frame
            && p.section.eq_ignore_ascii_case(old)
        {
            p.section = new.to_string();
        }
    }
}

/// The raster layer `layout.addElement` reads for a profile, for its inputs: the one given, else the
/// top one its frame's map shows that it can read.
pub(in crate::cmd::layout) fn added_profile_layer(map: &Map, p: &Value) -> Result<String, String> {
    match given_name(p, "layer", "a raster layer's name")? {
        Some(n) => profile_layer(map, n),
        None => first_profile_layer(map),
    }
}
