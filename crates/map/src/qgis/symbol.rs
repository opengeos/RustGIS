//! QGIS symbols (`<symbol type="fill|line|marker">`) as a RustGIS [`Symbol`], and a line's
//! symbol layers as stacked [`LineStroke`]s. A symbol layer RustGIS has no match for is drawn
//! as near as it can (a pattern fill as a tint of its colour, a font or SVG marker as a circle in
//! its colour) or left out, and noted.

use roxmltree::Node;

use super::{Report, color, data_defined_on, has_effect, points, prop, props, signed_points};
use crate::Rgba;
use crate::symbology::{
    Dash, LineCap, LineJoin, LineMark, LineMarkers, LineStroke, MAX_DASH_STEPS, MAX_STROKES, Marker, Placements, Symbol,
};

/// A symbol layer's settings by name.
type Props = Vec<(String, String)>;

/// The setting that stands for QGIS 1's symbol-wide unit (`<symbol outputUnit>`), for sizes a
/// layer gives no unit of its own. No symbol layer setting is named so.
const SYMBOL_UNIT: &str = "(symbol unit)";

/// A QGIS symbol in RustGIS's terms: the symbol, and a line's strokes bottom first (each simple
/// line of it; empty for fills and markers, and for a line with none).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Converted {
    pub symbol: Symbol,
    pub strokes: Vec<LineStroke>,
}

impl Converted {
    /// Whether the strokes say more than the symbol's single line does: more than one, or one
    /// shifted sideways, dashed or of markers.
    pub fn stacked(&self) -> bool {
        self.strokes.len() > 1 || self.strokes.iter().any(|s| s.offset != 0.0 || s.dash.is_some() || s.markers.is_some())
    }
}

/// Nothing drawn: QGIS's hidden categories and empty symbols.
pub(super) fn hidden() -> Symbol {
    let none = Rgba::new(0, 0, 0, 0);
    Symbol { fill: none, stroke: none, stroke_width: 0.0, size: 0.0, marker: Marker::Circle }
}

/// The symbol `sym` (a `<symbol>` element), its sizes at the reference scale `reference` where
/// they are in map units.
pub(super) fn read(sym: Node, reference: Option<f64>, layer: &str, report: &mut Report) -> Converted {
    let alpha = sym.attribute("alpha").and_then(|a| a.trim().parse::<f32>().ok()).filter(|a| a.is_finite()).unwrap_or(1.0).clamp(0.0, 1.0);
    let fade = |c: Rgba| c.with_alpha((f32::from(c.a()) * alpha).round() as u8);
    let unit = sym.attribute("outputUnit");
    let layers: Vec<(Node, Props)> = sym
        .children()
        .filter(|c| c.has_tag_name("layer") && c.attribute("enabled") != Some("0"))
        .take(64)
        .map(|l| (l, settings(l, unit)))
        .collect();
    for (l, _) in &layers {
        if data_defined_on(*l) {
            report.skipped(layer, "data-defined symbol settings (from fields or expressions)");
        }
        if has_effect(*l) {
            report.skipped(layer, "draw effects (shadows, glows, blurs and the like)");
        }
    }
    let mut out = match sym.attribute("type") {
        Some("fill") => fill(&layers, reference, layer, report),
        Some("line") => line(&layers, reference, layer, report),
        _ => marker(&layers, reference, layer, report),
    };
    out.symbol.fill = fade(out.symbol.fill);
    out.symbol.stroke = fade(out.symbol.stroke);
    for s in &mut out.strokes {
        s.color = s.color.map(fade);
        if let Some(m) = &mut s.markers {
            m.outline = m.outline.map(fade);
        }
    }
    out
}

/// A symbol layer's settings, with those QGIS 2 named otherwise also under today's names (where
/// the layer has only the old), and the symbol's own unit (QGIS 1) for sizes without one.
fn settings(l: Node, unit: Option<&str>) -> Props {
    let mut p = props(l);
    let renamed: &[(&str, &str)] = match l.attribute("class") {
        Some("SimpleLine") => {
            &[("color", "line_color"), ("width", "line_width"), ("width_unit", "line_width_unit"), ("penstyle", "line_style")]
        }
        Some("SimpleFill") => &[
            ("color_border", "outline_color"),
            ("width_border", "outline_width"),
            ("border_width_unit", "outline_width_unit"),
            ("style_border", "outline_style"),
        ],
        Some("SimpleMarker") => &[("color_border", "outline_color")],
        _ => &[],
    };
    for (old, new) in renamed {
        if prop(&p, new).is_none()
            && let Some(v) = prop(&p, old).map(str::to_string)
        {
            p.push((new.to_string(), v));
        }
    }
    if let Some(u) = unit {
        p.push((SYMBOL_UNIT.to_string(), u.to_string()));
    }
    p
}

/// A size setting `k` (with its `k_unit`) of a symbol layer, in points.
fn size(p: &[(String, String)], k: &str, reference: Option<f64>, layer: &str, report: &mut Report) -> Option<f32> {
    let v = prop(p, k)?.trim().parse::<f64>().ok().filter(|v| v.is_finite())?;
    let unit = prop(p, &format!("{k}_unit")).or(prop(p, SYMBOL_UNIT));
    Some(points(v, unit, reference, layer, report))
}

/// An offset setting `k` (with its `k_unit`) of a symbol layer, in points: negative to the left.
fn offset(p: &[(String, String)], k: &str, reference: Option<f64>, layer: &str, report: &mut Report) -> Option<f32> {
    let v = prop(p, k)?.trim().parse::<f64>().ok().filter(|v| v.is_finite())?;
    let unit = prop(p, &format!("{k}_unit")).or(prop(p, SYMBOL_UNIT));
    Some(signed_points(v, unit, reference, layer, report))
}

/// Whether a line style (`line_style`, `outline_style`) is dashed or dotted.
fn dashed(style: Option<&str>) -> bool {
    !matches!(style, None | Some("solid" | "no"))
}

/// A simple line's dash pattern in points: its own (`use_custom_dash`, in its unit), or Qt's for
/// a named pen style (in line widths, as Qt sizes them). `None` for a solid line, and for a
/// pattern RustGIS can't draw (noted).
fn dash(p: &[(String, String)], width: f32, reference: Option<f64>, layer: &str, report: &mut Report) -> Option<Dash> {
    let style = prop(p, "line_style").unwrap_or("solid");
    let steps: Vec<f32> = if prop(p, "use_custom_dash") == Some("1") {
        let unit = prop(p, "customdash_unit").or(prop(p, SYMBOL_UNIT));
        let parsed: Option<Vec<f64>> = prop(p, "customdash")
            .unwrap_or_default()
            .split(';')
            .filter(|s| !s.trim().is_empty())
            .take(MAX_DASH_STEPS + 2)
            .map(|s| s.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
            .collect();
        let mut v = parsed.unwrap_or_default();
        // An odd pattern ending in a step of no length: that step adds nothing.
        if v.len() % 2 == 1 && v.last() == Some(&0.0) {
            v.pop();
        }
        v.into_iter().map(|s| points(s, unit, reference, layer, report)).collect()
    } else {
        let qt: &[f32] = match style {
            "solid" | "no" => return None,
            "dash" => &[4.0, 2.0],
            "dot" => &[1.0, 2.0],
            "dash dot" => &[4.0, 2.0, 1.0, 2.0],
            "dash dot dot" => &[4.0, 2.0, 1.0, 2.0, 1.0, 2.0],
            _ => &[],
        };
        // Qt takes a line thinner than a pixel as a pixel wide for its pattern.
        qt.iter().map(|s| s * width.max(1.0)).collect()
    };
    let d = Dash::new(&steps);
    if d.is_none() {
        report.approximated(layer, "dashed lines whose pattern RustGIS can't draw, drawn solid");
    }
    if d.is_some() && prop(p, "dash_pattern_offset").and_then(|o| o.trim().parse::<f64>().ok()).is_some_and(|o| o != 0.0) {
        report.approximated(layer, "dash patterns' offsets along the line, left out");
    }
    d
}

fn fill(layers: &[(Node, Props)], reference: Option<f64>, layer: &str, report: &mut Report) -> Converted {
    let none = Rgba::new(0, 0, 0, 0);
    let (mut fill, mut stroke, mut width) = (None::<Rgba>, None::<Rgba>, 0.0f32);
    for (l, p) in layers {
        match l.attribute("class").unwrap_or_default() {
            "SimpleFill" => {
                let c = prop(p, "color").and_then(color).unwrap_or(none);
                fill = Some(match prop(p, "style").unwrap_or("solid") {
                    "solid" => c,
                    "no" => fill.unwrap_or(none),
                    _ => {
                        report.approximated(layer, "hatched fills, drawn as a light tint of their colour");
                        tint(c)
                    }
                });
                if prop(p, "outline_style") != Some("no") {
                    if dashed(prop(p, "outline_style")) {
                        report.approximated(layer, "dashed polygon outlines, drawn solid");
                    }
                    stroke = prop(p, "outline_color").and_then(color).or(stroke);
                    width = size(p, "outline_width", reference, layer, report).unwrap_or(0.26 * 96.0 / 25.4);
                }
            }
            "SimpleLine" => {
                if prop(p, "line_style") != Some("no") {
                    if dashed(prop(p, "line_style")) || prop(p, "use_custom_dash") == Some("1") {
                        report.approximated(layer, "dashed polygon outlines, drawn solid");
                    }
                    stroke = prop(p, "line_color").and_then(color).or(stroke);
                    width = size(p, "line_width", reference, layer, report).unwrap_or(width);
                }
            }
            "GradientFill" | "ShapeburstFill" => {
                report.approximated(layer, "gradient fills, drawn in their first colour");
                fill = prop(p, "color").or(prop(p, "color1")).and_then(color).or(fill);
            }
            class @ ("LinePatternFill" | "PointPatternFill" | "SVGFill" | "RasterFill" | "RandomMarkerFill") => {
                // The pattern's own symbol's colour, as a tint over what's below.
                let sub = l.children().find(|c| c.has_tag_name("symbol")).map(|s| read(s, reference, layer, &mut Report::default()));
                let c = sub.map(|s| if s.symbol.stroke.a() > 0 && s.symbol.fill.a() == 0 { s.symbol.stroke } else { s.symbol.fill });
                if let Some(c) = c.or_else(|| prop(p, "color").and_then(color)) {
                    fill = Some(tint(c));
                }
                report.approximated(layer, format!("{} fills, drawn as a light tint of their colour", pattern_name(class)));
            }
            "CentroidFill" => report.skipped(layer, "markers at polygon centroids (centroid fills)"),
            other => report.skipped(layer, format!("{other} symbol layers")),
        }
    }
    let symbol =
        Symbol { fill: fill.unwrap_or(none), stroke: stroke.unwrap_or(none), stroke_width: width, size: 0.0, marker: Marker::Circle };
    Converted { symbol, strokes: Vec::new() }
}

fn line(layers: &[(Node, Props)], reference: Option<f64>, layer: &str, report: &mut Report) -> Converted {
    let mut strokes = Vec::new();
    for (l, p) in layers {
        match l.attribute("class").unwrap_or_default() {
            "SimpleLine" => {
                let style = prop(p, "line_style").unwrap_or("solid");
                let Some(c) = prop(p, "line_color").and_then(color) else { continue };
                if style == "no" || c.a() == 0 {
                    continue;
                }
                let pass = l.attribute("pass").and_then(|v| v.trim().parse::<u8>().ok());
                let width = size(p, "line_width", reference, layer, report).unwrap_or(0.26 * 96.0 / 25.4);
                strokes.push(
                    LineStroke {
                        color: Some(c),
                        width: Some(width),
                        // Positive to the right of the line's direction on the screen, in both.
                        offset: offset(p, "offset", reference, layer, report).unwrap_or(0.0),
                        cap: match prop(p, "capstyle") {
                            Some("flat") => LineCap::Butt,
                            Some("square") => LineCap::Square,
                            _ => LineCap::Round,
                        },
                        join: match prop(p, "joinstyle") {
                            Some("miter") => LineJoin::Miter,
                            Some("bevel") => LineJoin::Bevel,
                            _ => LineJoin::Round,
                        },
                        level: pass,
                        dash: dash(p, width, reference, layer, report),
                        markers: None,
                    }
                    .clamped(),
                );
            }
            class @ ("MarkerLine" | "HashLine") => strokes.extend(along(*l, p, class == "HashLine", reference, layer, report)),
            "ArrowLine" => report.skipped(layer, "arrow lines"),
            other => report.skipped(layer, format!("{other} line symbol layers")),
        }
    }
    if strokes.len() > MAX_STROKES {
        report.skipped(layer, format!("line strokes past the first {MAX_STROKES}"));
        strokes.truncate(MAX_STROKES);
    }
    // The symbol's line: the top stroke.
    let top = strokes.last();
    let symbol = match top {
        Some(s) => {
            let c = s.color.unwrap_or(Rgba::new(0, 0, 0, 0));
            Symbol { fill: c, stroke: c, stroke_width: s.width.unwrap_or(1.0), size: 0.0, marker: Marker::Circle }
        }
        None => hidden(),
    };
    Converted { symbol, strokes }
}

/// A marker line (`hash` false) or hashed line as strokes of markers or hash marks along the line:
/// one for each layer of the symbol it places, bottom first, all at its places.
fn along(l: Node, p: &[(String, String)], hash: bool, reference: Option<f64>, layer: &str, report: &mut Report) -> Vec<LineStroke> {
    let num = |k: &str| prop(p, k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
    let at = placements(p, layer, report);
    if at.is_empty() {
        return Vec::new();
    }
    if prop(p, "place_on_every_part") == Some("false") {
        report.approximated(layer, "markers along only the first part of a line, drawn along every part");
    }
    if num("trim_distance_start").is_some_and(|v| v != 0.0) || num("trim_distance_end").is_some_and(|v| v != 0.0) {
        report.approximated(layer, "markers along lines trimmed at their ends, drawn along the whole line");
    }
    let base = LineMarkers {
        at,
        interval: size(p, "interval", reference, layer, report).unwrap_or(3.0 * 96.0 / 25.4),
        offset_along: offset(p, "offset_along_line", reference, layer, report).unwrap_or(0.0),
        rotate: prop(p, "rotate") != Some("0"),
        // QGIS's default stretch is 4 mm.
        average_angle: size(p, "average_angle_length", reference, layer, report).unwrap_or(4.0 * 96.0 / 25.4),
        ..LineMarkers::default()
    };
    let across = offset(p, "offset", reference, layer, report).unwrap_or(0.0);
    let pass = l.attribute("pass").and_then(|v| v.trim().parse::<u8>().ok());
    let Some(sym) = l.children().find(|c| c.has_tag_name("symbol")) else { return Vec::new() };
    let alpha = sym.attribute("alpha").and_then(|a| a.trim().parse::<f32>().ok()).filter(|a| a.is_finite()).unwrap_or(1.0).clamp(0.0, 1.0);
    let fade = |c: Rgba| c.with_alpha((f32::from(c.a()) * alpha).round() as u8);
    let unit = sym.attribute("outputUnit");
    let mut out = Vec::new();
    for sub in sym.children().filter(|c| c.has_tag_name("layer") && c.attribute("enabled") != Some("0")).take(MAX_STROKES) {
        if data_defined_on(sub) {
            report.skipped(layer, "data-defined symbol settings (from fields or expressions)");
        }
        if has_effect(sub) {
            report.skipped(layer, "draw effects (shadows, glows, blurs and the like)");
        }
        let sp = settings(sub, unit);
        let class = sub.attribute("class").unwrap_or_default();
        let stroke = if hash {
            hash_mark(class, &sp, p, base, reference, layer, report)
        } else {
            marker_mark(class, &sp, base, reference, layer, report)
        };
        if let Some(mut st) = stroke {
            st.offset += across;
            st.level = pass;
            st.color = st.color.map(fade);
            if let Some(m) = &mut st.markers {
                m.outline = m.outline.map(fade);
            }
            out.push(st.clamped());
        }
    }
    out
}

/// Where a marker or hashed line puts its marks: `placements` (QGIS 3.24 on, several at once) or
/// `placement` (before); none for a placement RustGIS has no match for (noted).
fn placements(p: &[(String, String)], layer: &str, report: &mut Report) -> Placements {
    let text = prop(p, "placements").or(prop(p, "placement")).unwrap_or("Interval");
    let mut at = Placements::NONE;
    for code in text.split('|').map(str::trim).filter(|c| !c.is_empty()) {
        at = at.with(match code.to_ascii_lowercase().as_str() {
            "interval" => Placements::INTERVAL,
            "vertex" => Placements::VERTICES,
            "innervertices" => Placements::INNER_VERTICES,
            "firstvertex" => Placements::FIRST,
            "lastvertex" => Placements::LAST,
            "centralpoint" => Placements::MIDDLE,
            "segmentcenter" => Placements::SEGMENT_MIDDLES,
            "curvepoint" => {
                report.approximated(layer, "markers at curve points, drawn at the vertices");
                Placements::VERTICES
            }
            other => {
                report.skipped(layer, format!("markers along lines placed at \"{other}\""));
                Placements::NONE
            }
        });
    }
    at
}

/// A marker line's marker layer `class` (its settings `p`) as a stroke of markers at `base`'s
/// places, or `None` for one RustGIS has no match for (noted).
fn marker_mark(
    class: &str,
    p: &[(String, String)],
    base: LineMarkers,
    reference: Option<f64>,
    layer: &str,
    report: &mut Report,
) -> Option<LineStroke> {
    let shape = match class {
        "SimpleMarker" => {
            let name = prop(p, "name").unwrap_or("circle");
            shape_of(name).unwrap_or_else(|| {
                report.approximated(layer, format!("{name} markers, drawn as circles"));
                Marker::Circle
            })
        }
        // A character is mostly thin strokes: a dot of its size would draw a heavy blot along the
        // line where QGIS draws a tick or a dash, so it is left out.
        "FontMarker" => {
            report.skipped(layer, "font markers (characters of a symbol font) along lines");
            return None;
        }
        "SvgMarker" | "RasterMarker" | "EllipseMarker" | "FilledMarker" => {
            report.approximated(layer, format!("{} markers, drawn as circles in their colour", marker_name(class)));
            Marker::Circle
        }
        other => {
            report.skipped(layer, format!("{other} marker symbol layers"));
            return None;
        }
    };
    let fill = prop(p, "color").and_then(color).unwrap_or(Rgba::new(0, 0, 0, 0));
    let outline = if prop(p, "outline_style") == Some("no") { None } else { prop(p, "outline_color").and_then(color) };
    let width = if outline.is_some() { size(p, "outline_width", reference, layer, report).unwrap_or(0.0) } else { 0.0 };
    let (along_x, across) = marker_offset(p, base.rotate, reference, layer, report);
    let markers = LineMarkers {
        mark: LineMark::Marker(shape),
        size: size(p, "size", reference, layer, report).unwrap_or(2.0 * 96.0 / 25.4),
        offset_along: base.offset_along + along_x,
        angle: prop(p, "angle").and_then(|a| a.trim().parse::<f32>().ok()).unwrap_or(0.0),
        outline,
        ..base
    };
    Some(LineStroke { color: Some(fill), width: Some(width), offset: across, markers: Some(markers), ..LineStroke::default() })
}

/// A hashed line's line layer `class` (its settings `p`; the hashed line's `hp`) as a stroke of
/// hash marks at `base`'s places, or `None` for one RustGIS has no match for (noted).
fn hash_mark(
    class: &str,
    p: &[(String, String)],
    hp: &[(String, String)],
    base: LineMarkers,
    reference: Option<f64>,
    layer: &str,
    report: &mut Report,
) -> Option<LineStroke> {
    if class != "SimpleLine" {
        report.skipped(layer, format!("{class} line symbol layers in hash marks"));
        return None;
    }
    if prop(p, "line_style") == Some("no") {
        return None;
    }
    if dashed(prop(p, "line_style")) || prop(p, "use_custom_dash") == Some("1") {
        report.approximated(layer, "dashed hash marks, drawn solid");
    }
    let c = prop(p, "line_color").and_then(color)?;
    let markers = LineMarkers {
        mark: LineMark::Hash,
        size: size(hp, "hash_length", reference, layer, report).unwrap_or(3.0 * 96.0 / 25.4),
        angle: prop(hp, "hash_angle").and_then(|a| a.trim().parse::<f32>().ok()).unwrap_or(0.0),
        ..base
    };
    let width = size(p, "line_width", reference, layer, report).unwrap_or(0.26 * 96.0 / 25.4);
    let across = offset(p, "offset", reference, layer, report).unwrap_or(0.0);
    Some(LineStroke { color: Some(c), width: Some(width), offset: across, markers: Some(markers), ..LineStroke::default() })
}

/// A marker's offset (`offset`, "x,y" in its unit) along and across the line, in points: when it
/// turns with the line its x runs along it and its y to the right of it; when it doesn't, taken
/// the same way (noted).
fn marker_offset(p: &[(String, String)], rotate: bool, reference: Option<f64>, layer: &str, report: &mut Report) -> (f32, f32) {
    let Some((x, y)) = prop(p, "offset").and_then(|o| o.split_once(',')) else { return (0.0, 0.0) };
    let parse = |v: &str| v.trim().parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0);
    let (x, y) = (parse(x), parse(y));
    if x == 0.0 && y == 0.0 {
        return (0.0, 0.0);
    }
    if !rotate {
        report.approximated(layer, "offsets of upright markers along lines, taken along and across the line");
    }
    let unit = prop(p, "offset_unit").or(prop(p, SYMBOL_UNIT));
    (signed_points(x, unit, reference, layer, report), signed_points(y, unit, reference, layer, report))
}

fn marker(layers: &[(Node, Props)], reference: Option<f64>, layer: &str, report: &mut Report) -> Converted {
    let mut symbol = hidden();
    for (l, p) in layers {
        let class = l.attribute("class").unwrap_or_default();
        let fill = prop(p, "color").and_then(color);
        let outline = prop(p, "outline_color").and_then(color);
        let sz = size(p, "size", reference, layer, report).unwrap_or(2.0 * 96.0 / 25.4);
        let shape = match class {
            "SimpleMarker" => {
                let name = prop(p, "name").unwrap_or("circle");
                shape_of(name).unwrap_or_else(|| {
                    report.approximated(layer, format!("{name} markers, drawn as circles"));
                    Marker::Circle
                })
            }
            "FontMarker" => {
                report.approximated(layer, "font markers (characters of a symbol font), drawn as circles in their colour");
                Marker::Circle
            }
            "SvgMarker" | "RasterMarker" | "EllipseMarker" | "FilledMarker" => {
                report.approximated(layer, format!("{} markers, drawn as circles in their colour", marker_name(class)));
                Marker::Circle
            }
            other => {
                report.skipped(layer, format!("{other} marker symbol layers"));
                continue;
            }
        };
        if prop(p, "angle").and_then(|a| a.trim().parse::<f32>().ok()).is_some_and(|a| a != 0.0) {
            report.approximated(layer, "turned markers, drawn upright");
        }
        let fill = fill.unwrap_or(Rgba::new(0, 0, 0, 0));
        // A font marker's character is all fill; an outline of no width draws none.
        let width = if class == "FontMarker" { 0.0 } else { size(p, "outline_width", reference, layer, report).unwrap_or(0.0) };
        // Each layer drawn over the one before: the top one's look, the biggest size.
        symbol = Symbol {
            fill,
            stroke: outline.unwrap_or(fill),
            stroke_width: width,
            size: sz.max(if symbol.size > 0.0 { symbol.size } else { 0.0 }).clamp(0.0, 200.0),
            marker: shape,
        };
    }
    Converted { symbol, strokes: Vec::new() }
}

/// QGIS's simple marker shapes RustGIS has.
fn shape_of(name: &str) -> Option<Marker> {
    Some(match name {
        "circle" => Marker::Circle,
        "square" | "rectangle" => Marker::Square,
        "triangle" | "equilateral_triangle" => Marker::Triangle,
        "diamond" => Marker::Diamond,
        "star" | "regular_star" => Marker::Star,
        _ => return None,
    })
}

/// A light tint of `c`, standing in for a pattern of it.
fn tint(c: Rgba) -> Rgba {
    c.with_alpha((f32::from(c.a()) * 0.35).round() as u8)
}

fn pattern_name(class: &str) -> &'static str {
    match class {
        "LinePatternFill" => "line pattern",
        "PointPatternFill" => "point pattern",
        "SVGFill" => "SVG",
        "RasterFill" => "raster image",
        _ => "random marker",
    }
}

fn marker_name(class: &str) -> &'static str {
    match class {
        "SvgMarker" => "SVG",
        "RasterMarker" => "raster image",
        "EllipseMarker" => "ellipse",
        _ => "filled",
    }
}
