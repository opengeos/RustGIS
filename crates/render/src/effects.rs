//! Layer effects drawn in place of plain features: point aggregation into counted bins, and
//! polygons extruded with lit walls and cast shadows: in an oblique 2.5D view on a flat map, in
//! perspective on a tilted one.

use std::collections::HashMap;

use rustgis_data::{Dataset, Geometry};
use rustgis_map::effects::{Aggregation, Extrusion};
use rustgis_map::{Ramp, Rgba, Symbol};
use tiny_skia::{FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode, Stroke, Transform};

use crate::{Ctx, View, paint};

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

/// How high (metres) feature `row` of `ds` is extruded.
pub(crate) fn extrusion_metres(ds: &Dataset, x: &Extrusion, row: usize) -> f64 {
    let fi = x.field.as_deref().and_then(|f| ds.field_index(f));
    fi.and_then(|i| ds.value(row, i).as_f64()).filter(|v| v.is_finite()).unwrap_or(x.height) * x.factor
}

/// The light of a tilted view's extrusions, as MapLibre's default: from the view's upper left,
/// mostly overhead (toward it: right, down the screen and up, in ground px).
pub(crate) const LIGHT: (f64, f64, f64) = (-0.25, -0.43, 0.87);

/// Polygons extruded in perspective in a tilted view, as MapLibre's fill extrusions: each
/// feature's walls stand on its outline and its roof is raised by its height (`m_per_px`
/// metres per logical px of ground at the centre). Features and walls are drawn far to near;
/// walls facing away from the eye are culled, walls are lit by the way they face the light and
/// shade darker toward their base, and features fade into `fog` with distance as the ground
/// does.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extrude_tilted(
    pm: &mut Pixmap,
    view: &View,
    ds: &Dataset,
    geoms: &[Option<&Geometry>],
    rows: &[bool],
    x: &Extrusion,
    m_per_px: f64,
    fog: Rgba,
    symbol: impl Fn(usize) -> Symbol,
) {
    let cam = view.camera;
    let (w, h) = (view.width, view.height);
    let ppp = view.pixels_per_point as f32;
    let far = rustgis_map::camera::FAR;
    let (eu, ev, _) = cam.eye(h);
    // (centroid depth along the view, outer rings in ground px, height in ground px, fog, row)
    let mut shapes: Vec<(f64, Vec<Vec<(f64, f64)>>, f64, f64, usize)> = Vec::new();
    for (row, g) in geoms.iter().enumerate() {
        let Some(Geometry::Polygon(polys)) = g else { continue };
        if rows.get(row) != Some(&true) {
            continue;
        }
        let up = (extrusion_metres(ds, x, row) / m_per_px.max(1e-9)).clamp(0.0, 1e7);
        // Thinned on screen: a point closer than about a pixel to the last kept adds a sliver.
        let mut rings = Vec::new();
        let (mut sum, mut n) = (rustgis_geom::Coord::new(0.0, 0.0), 0.0);
        for r in polys.iter().filter_map(|p| p.first()) {
            let mut kept: Vec<(f64, f64)> = Vec::new();
            let mut last: Option<(f64, f64)> = None;
            for c in r {
                let g = cam.ground_px(*c);
                let s = cam.project_raised(g.0, g.1, 0.0, w, h);
                let near = match (s, last) {
                    (Some(s), Some(l)) => (s.0 - l.0).hypot(s.1 - l.1) < 0.75,
                    _ => false,
                };
                if !near {
                    kept.push(g);
                    last = s;
                }
                sum = rustgis_geom::Coord::new(sum.x + c.x, sum.y + c.y);
                n += 1.0;
            }
            if kept.len() >= 3 {
                rings.push(kept);
            }
        }
        if rings.is_empty() || n == 0.0 {
            continue;
        }
        let centroid = rustgis_geom::Coord::new(sum.x / n, sum.y / n);
        // Past the view's reach is sky; on the way there, fog.
        let Some(k) = cam.depth_scale(centroid, h) else { continue };
        let depth = 1.0 / k.max(1e-9);
        if depth > far {
            continue;
        }
        let t = ((depth - far * 0.5) / (far * 0.5)).clamp(0.0, 1.0);
        shapes.push((cam.ground_px(centroid).1, rings, up, t * t * (3.0 - 2.0 * t), row));
    }
    // Far (up the screen) first.
    shapes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let at = |u: f64, v: f64, up: f64| cam.project_raised(u, v, up, w, h).map(|(x, y)| (x as f32 * ppp, y as f32 * ppp));
    let path = |pts: &[(f32, f32)]| {
        let mut pb = PathBuilder::new();
        for (i, p) in pts.iter().enumerate() {
            if i == 0 { pb.move_to(p.0, p.1) } else { pb.line_to(p.0, p.1) }
        }
        pb.close();
        pb.finish()
    };
    if x.shadows {
        // Cast on the ground away from the light.
        let shade = paint(Rgba::new(0, 0, 0, 60), 1.0);
        let (lu, lv) = (-LIGHT.0 / LIGHT.2, -LIGHT.1 / LIGHT.2);
        for (_, rings, up, _, _) in &shapes {
            let (du, dv) = (lu * up, lv * up);
            for r in rings {
                let shifted: Option<Vec<_>> = r.iter().map(|&(u, v)| at(u + du, v + dv, 0.0)).collect();
                let base: Option<Vec<_>> = r.iter().map(|&(u, v)| at(u, v, 0.0)).collect();
                let (Some(shifted), Some(base)) = (shifted, base) else { continue };
                for i in 0..base.len() {
                    let j = (i + 1) % base.len();
                    if let (Some(a), Some(b), Some(c), Some(d)) = (base.get(i), base.get(j), shifted.get(j), shifted.get(i))
                        && let Some(p) = path(&[*a, *b, *c, *d])
                    {
                        pm.fill_path(&p, &shade, FillRule::Winding, Transform::identity(), None);
                    }
                }
                if let Some(p) = path(&shifted) {
                    pm.fill_path(&p, &shade, FillRule::Winding, Transform::identity(), None);
                }
            }
        }
    }
    let light_len = LIGHT.0.hypot(LIGHT.1).max(1e-9);
    for (_, rings, up, fogged, row) in &shapes {
        let sym = symbol(*row);
        let base = sym.fill.with_alpha(255).lerp(fog, *fogged);
        // Walls: (depth, foot a, foot b, top a, top b, light), far first.
        let mut walls: Vec<(f64, [(f32, f32); 4], f64)> = Vec::new();
        for r in rings {
            let area2: f64 =
                (0..r.len()).filter_map(|i| Some((r.get(i)?, r.get((i + 1) % r.len())?))).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum();
            for i in 0..r.len() {
                let (Some(&a), Some(&b)) = (r.get(i), r.get((i + 1) % r.len())) else { continue };
                let (du, dv) = (b.0 - a.0, b.1 - a.1);
                let len = du.hypot(dv);
                if len <= 0.0 {
                    continue;
                }
                // The outward normal (the rings wind either way).
                let nrm = if area2 > 0.0 { (dv / len, -du / len) } else { (-dv / len, du / len) };
                let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                if x.face_culling && nrm.0 * (eu - mid.0) + nrm.1 * (ev - mid.1) <= 0.0 {
                    continue;
                }
                let (Some(fa), Some(fb), Some(ta), Some(tb)) = (at(a.0, a.1, 0.0), at(b.0, b.1, 0.0), at(a.0, a.1, *up), at(b.0, b.1, *up))
                else {
                    continue;
                };
                let lit = ((nrm.0 * LIGHT.0 + nrm.1 * LIGHT.1) / light_len).max(0.0);
                walls.push((mid.1, [fa, fb, tb, ta], lit));
            }
        }
        walls.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, [fa, fb, tb, ta], lit) in walls {
            let f = if x.lighting { 0.6 + 0.4 * lit } else { 0.8 };
            let top = base.shade(f);
            // Darker toward the foot, as MapLibre's vertical gradient.
            let foot = top.shade(if x.lighting { 0.72 } else { 1.0 });
            // A hair longer each way, so neighbouring walls leave no seam between them.
            let grow = |p: (f32, f32), q: (f32, f32)| {
                let (dx, dy) = (p.0 - q.0, p.1 - q.1);
                let l = dx.hypot(dy).max(1e-6);
                (p.0 + dx / l * 0.4 * ppp, p.1 + dy / l * 0.4 * ppp)
            };
            let quad = [grow(fa, fb), grow(fb, fa), grow(tb, ta), grow(ta, tb)];
            let Some(p) = path(&quad) else { continue };
            let mid = |p: (f32, f32), q: (f32, f32)| Point::from_xy((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
            let color = |c: Rgba| tiny_skia::Color::from_rgba8(c.r(), c.g(), c.b(), 255);
            let shader = LinearGradient::new(
                mid(fa, fb),
                mid(ta, tb),
                vec![GradientStop::new(0.0, color(foot)), GradientStop::new(1.0, color(top))],
                SpreadMode::Pad,
                Transform::identity(),
            );
            let wall = match shader {
                Some(shader) => Paint { shader, anti_alias: true, ..Paint::default() },
                None => paint(top, 1.0),
            };
            pm.fill_path(&p, &wall, FillRule::Winding, Transform::identity(), None);
        }
        // The roof, over its walls.
        let mut pb = PathBuilder::new();
        for r in rings {
            let pts: Option<Vec<_>> = r.iter().map(|&(u, v)| at(u, v, *up)).collect();
            let Some(pts) = pts else { continue };
            for (i, p) in pts.iter().enumerate() {
                if i == 0 { pb.move_to(p.0, p.1) } else { pb.line_to(p.0, p.1) }
            }
            pb.close();
        }
        if let Some(p) = pb.finish() {
            pm.fill_path(&p, &paint(base, 1.0), FillRule::EvenOdd, Transform::identity(), None);
            let edge = sym.stroke.lerp(fog, *fogged).with_alpha(110);
            pm.stroke_path(&p, &paint(edge, 1.0), &Stroke { width: 0.6 * ppp, ..Stroke::default() }, Transform::identity(), None);
        }
    }
}
