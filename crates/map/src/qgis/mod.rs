//! QGIS projects (`.qgs`) and layer styles (`.qml`) read into RustGIS: a project's layers (their
//! data sources, order, visibility, scale ranges and opacity), its coordinate system and view,
//! and each layer's style (renderer, symbols, labels, reference scale and symbol levels), as the
//! map's own types. What RustGIS can't draw as QGIS does is carried over as near as it can, and
//! the [`Report`] says so, layer by layer: nothing is left out without a note.
//!
//! The XML is hostile input: it is read with a node limit and no DTD, every list is bounded, and
//! sizes and scales are brought into range before they reach a layer. Both of QGIS's ways of
//! writing a symbol layer's settings are read: the `<Option type="Map">` of QGIS 3.26 and later,
//! and the `<prop k="…" v="…">` before it.

mod expr;
mod labels;
mod renderer;
mod source;
mod symbol;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use roxmltree::{Document, Node, ParsingOptions};
use serde::Serialize;

use crate::Rgba;
use crate::effects::Blend;
use crate::symbology::{Labeling, LineStroke, Renderer};
pub use expr::to_sql;
pub use source::{Source, is_drive_relative, is_relative};

/// The most XML nodes a project or style is read with (a large project's file is some hundred
/// thousand).
const MAX_NODES: u32 = 5_000_000;
/// The most layers read from a project.
pub const MAX_LAYERS: usize = 1000;
/// The most notes a report keeps.
const MAX_NOTES: usize = 5000;

/// RustGIS's scale takes a screen pixel to be 0.28 mm (OGC); QGIS works out its scale at the
/// screen's 96 dots to the inch. The same view is a larger scale in QGIS by this much, so QGIS's
/// scales (reference scales, visibility ranges) are multiplied by it.
pub const QGIS_SCALE: f64 = (25.4 / 96.0) / 0.28;

/// What an import carried over only in part, or left out, layer by layer.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    /// Added to by [`Report::note`] only, which keeps `index` in step.
    pub notes: Vec<Note>,
    /// Where each note is in `notes`, by layer, kind and text: a hostile project can make
    /// thousands of different notes, and a repeat is found without going through them all.
    #[serde(skip)]
    index: HashMap<(String, NoteKind, String), usize>,
}

/// One thing an import couldn't carry over as QGIS has it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    /// The layer it is about (empty for the project).
    pub layer: String,
    pub kind: NoteKind,
    pub text: String,
    /// How many times it came up (once per symbol, class or rule it applies to).
    pub count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    /// Drawn, but not quite as QGIS draws it.
    Approximated,
    /// Not carried over at all.
    Skipped,
}

impl Report {
    /// Notes `text` about `layer`, counting a repeat rather than listing it again.
    pub fn note(&mut self, layer: &str, kind: NoteKind, text: impl Into<String>) {
        let key = (layer.to_string(), kind, text.into());
        if let Some(n) = self.index.get(&key).and_then(|&i| self.notes.get_mut(i)) {
            n.count = n.count.saturating_add(1);
        } else if self.notes.len() < MAX_NOTES {
            self.index.insert(key.clone(), self.notes.len());
            let (layer, kind, text) = key;
            self.notes.push(Note { layer, kind, text, count: 1 });
        }
    }

    pub fn approximated(&mut self, layer: &str, text: impl Into<String>) {
        self.note(layer, NoteKind::Approximated, text);
    }

    pub fn skipped(&mut self, layer: &str, text: impl Into<String>) {
        self.note(layer, NoteKind::Skipped, text);
    }
}

/// A layer's look as a QGIS style has it, in RustGIS's terms. Each part is `None` (or empty) when
/// the style doesn't set it, so applying it leaves the layer's own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub renderer: Option<Renderer>,
    /// A line layer's stacked strokes (a single-symbol line with more than one line, or an
    /// offset one).
    pub line_strokes: Option<Vec<LineStroke>>,
    pub stroke_levels: Option<bool>,
    pub reference_scale: Option<f64>,
    pub labels: Option<Labeling>,
    /// 0 opaque to 100 invisible.
    pub transparency: Option<f32>,
    pub blend: Option<Blend>,
    /// Drawn between these scales (RustGIS's, see [`QGIS_SCALE`]): zoomed in beyond 1:`min_scale`
    /// and out beyond 1:`max_scale`. `Some(None)` clears a limit.
    pub min_scale: Option<Option<f64>>,
    pub max_scale: Option<Option<f64>>,
}

impl Style {
    /// Gives `layer` this style's parts (those it sets).
    pub fn apply(&self, layer: &mut crate::Layer) {
        if let Some(r) = &self.renderer {
            layer.renderer = r.clone();
        }
        if let Some(s) = &self.line_strokes {
            layer.line_strokes = s.clone();
        }
        if let Some(on) = self.stroke_levels {
            layer.stroke_levels = on;
        }
        if let Some(r) = self.reference_scale {
            layer.reference_scale = Some(r);
        }
        if let Some(l) = &self.labels {
            layer.labels = l.clone();
        }
        if let Some(t) = self.transparency {
            layer.transparency = t;
        }
        if let Some(b) = self.blend {
            layer.blend = b;
        }
        if let Some(s) = self.min_scale {
            layer.min_scale = s;
        }
        if let Some(s) = self.max_scale {
            layer.max_scale = s;
        }
    }
}

/// A QGIS project's layers and view, read from its `.qgs` text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Project {
    pub title: String,
    /// The project's coordinate system (`EPSG:2056`, or a PROJ string), the map's projection.
    pub crs: Option<String>,
    /// The view when it was saved: [xmin, ymin, xmax, ymax] in the project's coordinate system.
    pub extent: Option<[f64; 4]>,
    /// Its layers, top first, as its layer tree orders them.
    pub layers: Vec<ProjectLayer>,
}

/// One layer of a QGIS project.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectLayer {
    pub id: String,
    pub name: String,
    /// QGIS's data provider (`ogr`, `gdal`, `wms`, `delimitedtext`, …).
    pub provider: String,
    pub source: Source,
    /// Its coordinate system (`EPSG:…`), when QGIS records one.
    pub crs: Option<String>,
    /// A vector layer (`false`: raster, mesh, point cloud and the like).
    pub vector: bool,
    /// Checked in the layer tree, and every group holding it too.
    pub visible: bool,
    /// Its subset string, as a RustGIS definition query when it reads as one.
    pub definition_query: Option<String>,
    pub style: Style,
}

/// Reads a QGIS project. `dir` is the folder the project is in, which its relative paths start
/// from. Notes on what can't be carried over go to `report`.
pub fn read_project(text: &str, dir: &str, report: &mut Report) -> Result<Project, String> {
    let doc = parse(text, "a QGIS project")?;
    let root = doc.root_element();
    if !root.has_tag_name("qgis") {
        return Err("not a QGIS project (.qgs): its root element isn't <qgis>".into());
    }
    let mut project = Project {
        title: child(root, "title").and_then(|t| t.text()).map(str::trim).unwrap_or_default().to_string(),
        crs: child(root, "projectCrs").and_then(crs_of),
        extent: None,
        layers: Vec::new(),
    };
    // The main map canvas's view.
    let canvas = root.children().find(|c| c.has_tag_name("mapcanvas") && c.attribute("name").is_none_or(|n| n == "theMapCanvas"));
    project.extent = canvas.and_then(|c| child(c, "extent")).and_then(extent_of);
    // The layer tree: the order layers draw in (top first) and which are checked.
    let tree = child(root, "layer-tree-group");
    let mut order: Vec<(String, bool)> = Vec::new();
    if let Some(t) = tree {
        tree_layers(t, true, &mut order, 0);
    }
    let maplayers: Vec<Node> =
        child(root, "projectlayers").map(|p| p.children().filter(|c| c.has_tag_name("maplayer")).collect()).unwrap_or_default();
    if maplayers.len() > MAX_LAYERS {
        report.skipped("", format!("layers past the first {MAX_LAYERS}"));
    }
    let mut layers: Vec<(Option<usize>, ProjectLayer)> = Vec::new();
    for ml in maplayers.into_iter().take(MAX_LAYERS) {
        let id = child(ml, "id").and_then(|n| n.text()).unwrap_or_default().trim().to_string();
        let name = child(ml, "layername").and_then(|n| n.text()).unwrap_or_default().trim().to_string();
        let name = if name.is_empty() { id.clone() } else { name };
        let at = order.iter().position(|(i, _)| *i == id);
        let checked = at.and_then(|i| order.get(i)).is_none_or(|(_, c)| *c);
        let provider = child(ml, "provider").and_then(|n| n.text()).unwrap_or_default().trim().to_string();
        let datasource = child(ml, "datasource").and_then(|n| n.text()).unwrap_or_default().trim().to_string();
        let source = source::read(&provider, &datasource, dir);
        let vector = ml.attribute("type") == Some("vector");
        let definition_query = match &source {
            Source::File { subset: Some(q), .. } => match expr::to_sql(q) {
                Some(sql) => Some(sql),
                None => {
                    report.skipped(&name, format!("its filter (subset string) `{q}`, which doesn't read as a RustGIS definition query"));
                    None
                }
            },
            _ => None,
        };
        let style = read_style_node(ml, &name, report);
        layers.push((
            at,
            ProjectLayer {
                id,
                name,
                provider,
                source,
                crs: child(ml, "srs").and_then(crs_of),
                vector,
                visible: checked,
                definition_query,
                style,
            },
        ));
    }
    // In the layer tree's order; layers it doesn't hold go below.
    layers.sort_by_key(|(at, _)| at.unwrap_or(usize::MAX));
    project.layers = layers.into_iter().map(|(_, l)| l).collect();
    Ok(project)
}

/// Reads a QGIS layer style (a `.qml` file's text) for the layer named `layer` (for the notes).
pub fn read_style(text: &str, layer: &str, report: &mut Report) -> Result<Style, String> {
    let doc = parse(text, "a QGIS layer style")?;
    let root = doc.root_element();
    if !root.has_tag_name("qgis") {
        return Err("not a QGIS layer style (.qml): its root element isn't <qgis>".into());
    }
    Ok(read_style_node(root, layer, report))
}

/// A layer's style from its `<maplayer>` (a project's) or a style file's `<qgis>` root.
fn read_style_node(node: Node, layer: &str, report: &mut Report) -> Style {
    let mut style = Style::default();
    let opacity = child(node, "layerOpacity").and_then(|n| n.text()).and_then(|t| t.trim().parse::<f32>().ok());
    if let Some(o) = opacity.filter(|o| o.is_finite()) {
        style.transparency = Some(((1.0 - o.clamp(0.0, 1.0)) * 100.0).round());
    }
    if let Some(mode) = child(node, "blendMode").and_then(|n| n.text()).and_then(|t| t.trim().parse::<i32>().ok()) {
        style.blend = Some(blend_of(mode).unwrap_or_else(|| {
            report.approximated(layer, "its blending mode, drawn as Normal");
            Blend::Normal
        }));
    }
    if node.attribute("hasScaleBasedVisibilityFlag") == Some("1") {
        // QGIS's minScale is the out-beyond limit (the larger number), maxScale the in-beyond.
        let scale = |k: &str| node.attribute(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
        style.min_scale = Some(scale("minScale").map(|s| s * QGIS_SCALE));
        style.max_scale = Some(scale("maxScale").map(|s| s * QGIS_SCALE));
    }
    if let Some(r) = child(node, "renderer-v2") {
        renderer::read(r, layer, &mut style, report);
    }
    // A raster's renderer: its opacity carried over, its colouring not yet.
    if let Some(r) = child(node, "pipe").and_then(|p| child(p, "rasterrenderer")) {
        if let Some(o) = r.attribute("opacity").and_then(|o| o.trim().parse::<f32>().ok()).filter(|o| o.is_finite() && *o < 1.0) {
            style.transparency = Some(((1.0 - o.clamp(0.0, 1.0)) * 100.0).round());
        }
        report.skipped(layer, format!("its raster colouring ({} renderer)", r.attribute("type").unwrap_or("unknown")));
    }
    if let Some(l) = child(node, "labeling") {
        // Set up but switched off: carried over, switched off.
        let off = node.attribute("labelsEnabled") == Some("0");
        style.labels = labels::read(l, layer, report).map(|mut l| {
            l.enabled &= !off;
            l
        });
    }
    style
}

/// The layers in a layer tree group, top first, each with whether it is checked (and every
/// group above it). Groups are walked to a bounded depth.
fn tree_layers(group: Node, checked: bool, out: &mut Vec<(String, bool)>, depth: usize) {
    if depth > 64 || out.len() > MAX_LAYERS * 4 {
        return;
    }
    let on = |n: Node| n.attribute("checked").is_none_or(|c| c == "Qt::Checked" || c == "Qt::PartiallyChecked");
    for c in group.children().filter(Node::is_element) {
        if c.has_tag_name("layer-tree-layer") {
            if let Some(id) = c.attribute("id") {
                out.push((id.to_string(), checked && on(c)));
            }
        } else if c.has_tag_name("layer-tree-group") {
            tree_layers(c, checked && on(c), out, depth + 1);
        }
    }
}

/// QGIS's blending modes (`QPainter::CompositionMode`) RustGIS has.
fn blend_of(mode: i32) -> Option<Blend> {
    Some(match mode {
        0 => Blend::Normal,
        13 => Blend::Multiply,
        14 => Blend::Screen,
        15 => Blend::Overlay,
        16 => Blend::Darken,
        17 => Blend::Lighten,
        22 => Blend::Difference,
        _ => return None,
    })
}

/// A coordinate system element's (`<srs>`, `<projectCrs>`) `EPSG:…`, or its PROJ string.
fn crs_of(node: Node) -> Option<String> {
    let srs = node.descendants().find(|d| d.has_tag_name("spatialrefsys"))?;
    let text = |tag: &str| child(srs, tag).and_then(|n| n.text()).map(str::trim).filter(|t| !t.is_empty());
    text("authid").filter(|a| a.contains(':')).or_else(|| text("proj4")).map(str::to_string)
}

/// An `<extent>`'s [xmin, ymin, xmax, ymax], when all four are numbers and it isn't empty.
fn extent_of(node: Node) -> Option<[f64; 4]> {
    let v = |tag: &str| child(node, tag).and_then(|n| n.text()).and_then(|t| t.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
    let e = [v("xmin")?, v("ymin")?, v("xmax")?, v("ymax")?];
    (e[0] < e[2] && e[1] < e[3]).then_some(e)
}

/// Parses QGIS's XML with a node limit. QGIS starts its files with `<!DOCTYPE qgis …>`, which
/// declares nothing: the document is read from past it, and one declaring entities (`[…]`) is
/// refused rather than expanded.
fn parse<'a>(text: &'a str, what: &str) -> Result<Document<'a>, String> {
    let opts = ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..ParsingOptions::default() };
    let doctype = text.find("<!DOCTYPE").filter(|at| text.get(..*at).is_some_and(|before| !before.contains("<q")));
    let Some(at) = doctype else {
        return Document::parse_with_options(text, opts).map_err(|e| format!("not {what}: {e}"));
    };
    let end = text.get(at..).and_then(|t| t.find('>')).map(|e| at + e + 1).ok_or_else(|| format!("not {what}: an unclosed <!DOCTYPE"))?;
    if text.get(at..end).is_some_and(|d| d.contains('[')) {
        return Err(format!("not {what}: it declares entities (a DTD), which aren't read"));
    }
    let rest = text.get(end..).unwrap_or_default();
    Document::parse_with_options(rest, opts).map_err(|e| format!("not {what}: {e}"))
}

/// The first child element named `tag`.
fn child<'a, 'i>(node: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| c.has_tag_name(tag))
}

/// A symbol layer's (or ramp's) settings, by name: QGIS 3.26's `<Option type="Map">` and the
/// `<prop k v>` before it.
pub(crate) fn props(node: Node) -> Vec<(String, String)> {
    let mut props = Vec::new();
    for c in node.children().filter(Node::is_element) {
        if c.has_tag_name("prop")
            && let (Some(k), Some(v)) = (c.attribute("k"), c.attribute("v"))
        {
            props.push((k.to_string(), v.to_string()));
        }
        if c.has_tag_name("Option") {
            for o in c.children().filter(|o| o.has_tag_name("Option")) {
                if let (Some(k), Some(v)) = (o.attribute("name"), o.attribute("value")) {
                    props.push((k.to_string(), v.to_string()));
                }
            }
        }
    }
    props
}

/// The setting `k` of `props`.
fn prop<'a>(props: &'a [(String, String)], k: &str) -> Option<&'a str> {
    props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

/// A QGIS colour: `r,g,b[,a]` (newer files add `,rgb:…` after), or `#rrggbb[aa]`.
pub(crate) fn color(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if s.starts_with('#') {
        return Rgba::from_hex(s);
    }
    let v: Vec<u8> = s.split(',').take(4).map(|p| p.trim().parse::<u8>().ok()).collect::<Option<_>>()?;
    match v.as_slice() {
        [r, g, b] => Some(Rgba::rgb(*r, *g, *b)),
        [r, g, b, a] => Some(Rgba::new(*r, *g, *b, *a)),
        _ => None,
    }
}

/// A size in QGIS's `unit` as RustGIS points (1/96 inch, a screen pixel at 1×). Map units have
/// no fixed size on screen: they are taken at the layer's reference scale when it has one
/// (`reference`, RustGIS's), else as metres at 1:25,000, and noted.
fn points(v: f64, unit: Option<&str>, reference: Option<f64>, layer: &str, report: &mut Report) -> f32 {
    let pt = match unit.unwrap_or("MM") {
        "MM" => v * 96.0 / 25.4,
        "Point" => v * 96.0 / 72.0,
        "Pixel" => v,
        "Inch" => v * 96.0,
        "MapUnit" | "RenderMetersInMapUnits" => {
            report.approximated(layer, "sizes in map units, taken at a fixed scale");
            v / reference.unwrap_or(25_000.0) * 96.0 / 0.0254
        }
        other => {
            report.approximated(layer, format!("sizes in {other}, taken as millimetres"));
            v * 96.0 / 25.4
        }
    };
    if pt.is_finite() { pt.clamp(0.0, 1000.0) as f32 } else { 0.0 }
}
