//! Layer effects drawn in place of plain features: point aggregation into counted bins, and
//! polygons extruded in an oblique 2.5D view with lit walls and cast shadows.

use std::collections::HashMap;

use rustgis_data::{Dataset, Geometry};
use rustgis_map::effects::{Aggregation, Extrusion};
use rustgis_map::{Ramp, Rgba, Symbol};
use tiny_skia::{FillRule, PathBuilder, Pixmap, Stroke, Transform};

use crate::{Ctx, paint};

/// Points binned into hexagons or squares, each bin coloured by its count.
pub(crate) fn aggregate(pm: &mut Pixmap, ctx: &Ctx, geoms: &[Option<&Geometry>], rows: &[bool], a: Aggregation, base: &Symbol) {
    let s = (a.size.clamp(4.0, 400.0) * ctx.ppp as f64) as f32;
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let mut bins: HashMap<(i64, i64), u32> = HashMap::new();
    let sqrt3 = 3f32.sqrt();
    for (row, g) in geoms.iter().enumerate() {
        let Some(g) = g else { continue };
        if rows.get(row) != Some(&true) {
            continue;
        }
        let pts: Vec<rustgis_geom::Coord> = match g {
            Geometry::Point(c) => vec![*c],
            Geometry::MultiPoint(v) => v.clone(),
            _ => continue,
        };
        for c in pts {
            let (x, y) = ctx.px(c);
            if !(x.is_finite() && y.is_finite()) || x < -s || y < -s || x > w + s || y > h + s {
                continue;
            }
            let key = if a.hexagons {
                // Pointy-top hexagons in axial coordinates, rounded through cube coordinates.
                let q = (sqrt3 / 3.0 * x - y / 3.0) / s;
                let r = (2.0 / 3.0 * y) / s;
                let (cx, cz) = (q, r);
                let cy = -cx - cz;
                let (mut rx, ry, mut rz) = (cx.round(), cy.round(), cz.round());
                let (dx, dy, dz) = ((rx - cx).abs(), (ry - cy).abs(), (rz - cz).abs());
                if dx > dy && dx > dz {
                    rx = -ry - rz;
                } else if dy <= dz {
                    rz = -rx - ry;
                }
                (rx as i64, rz as i64)
            } else {
                ((x / s).floor() as i64, (y / s).floor() as i64)
            };
            *bins.entry(key).or_insert(0) += 1;
        }
    }
    let max = bins.values().copied().max().unwrap_or(1).max(1) as f64;
    let ramp = Ramp::Ember;
    let stroke = Stroke { width: 0.8 * ctx.ppp, ..Stroke::default() };
    for ((q, r), n) in bins {
        let t = if max > 1.0 { (n as f64).ln() / max.ln() } else { 1.0 };
        let fill = ramp.at(0.15 + 0.85 * t).with_alpha(210);
        let mut pb = PathBuilder::new();
        if a.hexagons {
            let (cx, cy) = (s * sqrt3 * (q as f32 + r as f32 / 2.0), s * 1.5 * r as f32);
            for k in 0..6 {
                let ang = (60.0 * k as f32 - 30.0).to_radians();
                let (x, y) = (cx + s * ang.cos(), cy + s * ang.sin());
                if k == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
            }
            pb.close();
        } else if let Some(rect) = tiny_skia::Rect::from_xywh(q as f32 * s, r as f32 * s, s, s) {
            pb.push_rect(rect);
        }
        if let Some(p) = pb.finish() {
            pm.fill_path(&p, &paint(fill, 1.0), FillRule::Winding, Transform::identity(), None);
            pm.stroke_path(&p, &paint(base.stroke.with_alpha(160), 1.0), &stroke, Transform::identity(), None);
        }
    }
}

/// Polygons extruded upward on screen by their height (`px_per_m` pixels per metre), drawn far
/// to near, with shadows first, then walls (lit by their facing), then roofs.
pub(crate) fn extrude(
    pm: &mut Pixmap,
    ctx: &Ctx,
    ds: &Dataset,
    geoms: &[Option<&Geometry>],
    rows: &[bool],
    x: &Extrusion,
    px_per_m: f64,
    symbol: impl Fn(usize) -> Symbol,
) {
    let fi = x.field.as_deref().and_then(|f| ds.field_index(f));
    // (base centre y, outer rings in pixels, height in pixels, row)
    let mut shapes: Vec<(f32, Vec<Vec<(f32, f32)>>, f32, usize)> = Vec::new();
    for (row, g) in geoms.iter().enumerate() {
        let Some(Geometry::Polygon(polys)) = g else { continue };
        if rows.get(row) != Some(&true) {
            continue;
        }
        let metres = fi.and_then(|i| ds.value(row, i).as_f64()).filter(|v| v.is_finite()).unwrap_or(x.height) * x.factor;
        let hp = (metres * px_per_m).clamp(0.0, 4000.0) as f32;
        let rings: Vec<Vec<(f32, f32)>> = polys
            .iter()
            .filter_map(|p| p.first())
            .map(|r| r.iter().map(|c| ctx.px(*c)).filter(|(a, b)| a.is_finite() && b.is_finite()).collect::<Vec<_>>())
            .filter(|r| r.len() >= 3)
            .collect();
        if rings.is_empty() {
            continue;
        }
        let cy = rings.iter().flatten().map(|p| p.1).sum::<f32>() / rings.iter().map(Vec::len).sum::<usize>().max(1) as f32;
        shapes.push((cy, rings, hp, row));
    }
    shapes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let poly_path = |rings: &[Vec<(f32, f32)>], dx: f32, dy: f32| {
        let mut pb = PathBuilder::new();
        for r in rings {
            for (i, (px, py)) in r.iter().enumerate() {
                if i == 0 { pb.move_to(px + dx, py + dy) } else { pb.line_to(px + dx, py + dy) }
            }
            pb.close();
        }
        pb.finish()
    };
    let quad = |a: (f32, f32), b: (f32, f32), dx: f32, dy: f32| {
        let mut pb = PathBuilder::new();
        pb.move_to(a.0, a.1);
        pb.line_to(b.0, b.1);
        pb.line_to(b.0 + dx, b.1 + dy);
        pb.line_to(a.0 + dx, a.1 + dy);
        pb.close();
        pb.finish()
    };
    if x.shadows {
        // Light from the north-west casts shadows to the south-east.
        let shade = paint(Rgba::new(0, 0, 0, 70), 1.0);
        for (_, rings, hp, _) in &shapes {
            let (sx, sy) = (hp * 0.7, hp * 0.35);
            for r in rings {
                for i in 0..r.len() {
                    if let (Some(a), Some(b)) = (r.get(i), r.get((i + 1) % r.len()))
                        && let Some(p) = quad(*a, *b, sx, sy)
                    {
                        pm.fill_path(&p, &shade, FillRule::Winding, Transform::identity(), None);
                    }
                }
            }
            if let Some(p) = poly_path(rings, sx, sy) {
                pm.fill_path(&p, &shade, FillRule::EvenOdd, Transform::identity(), None);
            }
        }
    }
    let light = (-std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2);
    let edge_stroke = Stroke { width: 0.35 * ctx.ppp, ..Stroke::default() };
    for (_, rings, hp, row) in &shapes {
        let sym = symbol(*row);
        let base = sym.fill.with_alpha(255);
        let up = -hp;
        // Walls: the back ones first (when drawn), then the ones facing the viewer.
        let mut walls: Vec<(bool, (f32, f32), (f32, f32), f32)> = Vec::new();
        for r in rings {
            let area2: f32 =
                (0..r.len()).filter_map(|i| Some((r.get(i)?, r.get((i + 1) % r.len())?))).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum();
            for i in 0..r.len() {
                let (Some(a), Some(b)) = (r.get(i), r.get((i + 1) % r.len())) else { continue };
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len = (dx * dx + dy * dy).sqrt();
                // Sub-pixel edges (detailed coastlines) add nothing but streaks.
                if len < 0.75 {
                    continue;
                }
                let n = if area2 > 0.0 { (dy / len, -dx / len) } else { (-dy / len, dx / len) };
                let facing = n.1 > 0.0;
                let lit = n.0 * light.0 + n.1 * light.1;
                walls.push((facing, *a, *b, lit));
            }
        }
        walls.sort_by_key(|w| w.0);
        for (facing, a, b, lit) in walls {
            if x.face_culling && !facing {
                continue;
            }
            let f = if x.lighting { 0.55 + 0.4 * lit.max(0.0) as f64 } else { 0.75 };
            if let Some(p) = quad(a, b, 0.0, up) {
                pm.fill_path(&p, &paint(base.shade(f), 1.0), FillRule::Winding, Transform::identity(), None);
                pm.stroke_path(&p, &paint(sym.stroke.with_alpha(45), 1.0), &edge_stroke, Transform::identity(), None);
            }
        }
        if let Some(p) = poly_path(rings, 0.0, up) {
            let roof = if x.lighting { base.shade(1.08) } else { base };
            pm.fill_path(&p, &paint(roof, 1.0), FillRule::EvenOdd, Transform::identity(), None);
            pm.stroke_path(
                &p,
                &paint(sym.stroke, 1.0),
                &Stroke { width: sym.stroke_width.max(0.5) * ctx.ppp, ..Stroke::default() },
                Transform::identity(),
                None,
            );
        }
    }
}
