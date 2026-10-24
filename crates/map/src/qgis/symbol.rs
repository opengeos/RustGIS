//! QGIS symbols (`<symbol type="fill|line|marker">`) as a RustGIS [`Symbol`], and a line's
//! symbol layers as stacked [`LineStroke`]s. A symbol layer RustGIS has no match for is drawn
//! as near as it can (a pattern fill as a tint of its colour, a font or SVG marker as a circle in
//! its colour) or left out, and noted.

use roxmltree::Node;

use super::{Report, color, points, prop, props};
use crate::Rgba;
use crate::symbology::{LineCap, LineJoin, LineStroke, MAX_STROKES, Marker, Symbol};

/// A QGIS symbol in RustGIS's terms: the symbol, and a line's strokes bottom first (each simple
/// line of it; empty for fills and markers, and for a line with none).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Converted {
    pub symbol: Symbol,
    pub strokes: Vec<LineStroke>,
}

impl Converted {
    /// Whether the strokes say more than the symbol's single line does: more than one, or one
    /// shifted sideways.
    pub fn stacked(&self) -> bool {
        self.strokes.len() > 1 || self.strokes.iter().any(|s| s.offset != 0.0)
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
    let layers: Vec<Node> = sym.children().filter(|c| c.has_tag_name("layer") && c.attribute("enabled") != Some("0")).take(64).collect();
    for l in &layers {
        data_defined(*l, layer, report);
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
    }
    out
}

/// A size setting `k` (with its `k_unit`) of a symbol layer, in points.
fn size(p: &[(String, String)], k: &str, reference: Option<f64>, layer: &str, report: &mut Report) -> Option<f32> {
    let v = prop(p, k)?.trim().parse::<f64>().ok().filter(|v| v.is_finite())?;
    let unit = prop(p, &format!("{k}_unit"));
    Some(points(v, unit, reference, layer, report))
}

fn fill(layers: &[Node], reference: Option<f64>, layer: &str, report: &mut Report) -> Converted {
    let none = Rgba::new(0, 0, 0, 0);
    let (mut fill, mut stroke, mut width) = (None::<Rgba>, None::<Rgba>, 0.0f32);
    for l in layers {
        let p = props(*l);
        match l.attribute("class").unwrap_or_default() {
            "SimpleFill" => {
                let c = prop(&p, "color").and_then(color).unwrap_or(none);
                fill = Some(match prop(&p, "style").unwrap_or("solid") {
                    "solid" => c,
                    "no" => fill.unwrap_or(none),
                    _ => {
                        report.approximated(layer, "hatched fills, drawn as a light tint of their colour");
                        tint(c)
                    }
                });
                if prop(&p, "outline_style") != Some("no") {
                    stroke = prop(&p, "outline_color").and_then(color).or(stroke);
                    width = size(&p, "outline_width", reference, layer, report).unwrap_or(0.26 * 96.0 / 25.4);
                }
            }
            "SimpleLine" => {
                if prop(&p, "line_style") != Some("no") {
                    stroke = prop(&p, "line_color").and_then(color).or(stroke);
                    width = size(&p, "line_width", reference, layer, report).unwrap_or(width);
                }
            }
            "GradientFill" | "ShapeburstFill" => {
                report.approximated(layer, "gradient fills, drawn in their first colour");
                fill = prop(&p, "color").or(prop(&p, "color1")).and_then(color).or(fill);
            }
            class @ ("LinePatternFill" | "PointPatternFill" | "SVGFill" | "RasterFill" | "RandomMarkerFill") => {
                // The pattern's own symbol's colour, as a tint over what's below.
                let sub = l.children().find(|c| c.has_tag_name("symbol")).map(|s| read(s, reference, layer, &mut Report::default()));
                let c = sub.map(|s| if s.symbol.stroke.a() > 0 && s.symbol.fill.a() == 0 { s.symbol.stroke } else { s.symbol.fill });
                if let Some(c) = c.or_else(|| prop(&p, "color").and_then(color)) {
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

fn line(layers: &[Node], reference: Option<f64>, layer: &str, report: &mut Report) -> Converted {
    let mut strokes = Vec::new();
    for l in layers {
        let p = props(*l);
        match l.attribute("class").unwrap_or_default() {
            "SimpleLine" => {
                let style = prop(&p, "line_style").unwrap_or("solid");
                let Some(c) = prop(&p, "line_color").and_then(color) else { continue };
                if style == "no" || c.a() == 0 {
                    continue;
                }
                if style != "solid" || prop(&p, "use_custom_dash") == Some("1") {
                    report.approximated(layer, "dashed lines, drawn solid");
                }
                let pass = l.attribute("pass").and_then(|v| v.trim().parse::<u8>().ok());
                strokes.push(
                    LineStroke {
                        color: Some(c),
                        width: Some(size(&p, "line_width", reference, layer, report).unwrap_or(0.26 * 96.0 / 25.4)),
                        // Positive to the right of the line's direction on the screen, in both.
                        offset: size(&p, "offset", reference, layer, report).unwrap_or(0.0),
                        cap: match prop(&p, "capstyle") {
                            Some("flat") => LineCap::Butt,
                            Some("square") => LineCap::Square,
                            _ => LineCap::Round,
                        },
                        join: match prop(&p, "joinstyle") {
                            Some("miter") => LineJoin::Miter,
                            Some("bevel") => LineJoin::Bevel,
                            _ => LineJoin::Round,
                        },
                        level: pass,
                        dash: None,
                    }
                    .clamped(),
                );
            }
            "MarkerLine" | "HashLine" => report.skipped(layer, "markers and hash marks along lines"),
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

fn marker(layers: &[Node], reference: Option<f64>, layer: &str, report: &mut Report) -> Converted {
    let mut symbol = hidden();
    for l in layers {
        let p = props(*l);
        let class = l.attribute("class").unwrap_or_default();
        let fill = prop(&p, "color").and_then(color);
        let outline = prop(&p, "outline_color").and_then(color);
        let sz = size(&p, "size", reference, layer, report).unwrap_or(2.0 * 96.0 / 25.4);
        let shape = match class {
            "SimpleMarker" => {
                let name = prop(&p, "name").unwrap_or("circle");
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
        if prop(&p, "angle").and_then(|a| a.trim().parse::<f32>().ok()).is_some_and(|a| a != 0.0) {
            report.approximated(layer, "turned markers, drawn upright");
        }
        let fill = fill.unwrap_or(Rgba::new(0, 0, 0, 0));
        // A font marker's character is all fill; an outline of no width draws none.
        let width = if class == "FontMarker" { 0.0 } else { size(&p, "outline_width", reference, layer, report).unwrap_or(0.0) };
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

/// Notes a symbol layer's data-defined settings (from fields or expressions), which are left out.
fn data_defined(l: Node, layer: &str, report: &mut Report) {
    let Some(dd) = l.children().find(|c| c.has_tag_name("data_defined_properties")) else { return };
    let active =
        dd.descendants().any(|o| o.has_tag_name("Option") && o.attribute("name") == Some("active") && o.attribute("value") == Some("true"));
    if active {
        report.skipped(layer, "data-defined symbol settings (from fields or expressions)");
    }
}
