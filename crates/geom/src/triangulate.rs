//! Polygon triangulation by ear clipping, holes joined to the outer ring by bridges. Used for the
//! caps of extruded polygons, which viewers would otherwise fan from one corner (wrong for concave
//! shapes and holes).

/// Most vertices triangulated; larger polygons get `None`.
pub const MAX_VERTICES: usize = 5000;
/// Most point tests spent on one polygon (bridging and ear checks), so no input can stall.
const MAX_WORK: usize = 60_000_000;

fn area2(p: &[(f64, f64)]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum()
}

fn cross(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Inside the triangle or on its edges (a vertex touching an edge would make the ear overlap).
fn in_triangle(p: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
}

/// Whether segments ab and cd cross at a point inside both (touching ends don't count).
fn crosses(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// Whether the ray `cur -> target` leaves strictly into the polygon interior at vertex `cur`
/// (to the left of `prev -> cur -> next`, which holds for both CCW outer rings and CW holes).
fn in_interior_wedge(prev: (f64, f64), cur: (f64, f64), next: (f64, f64), target: (f64, f64)) -> bool {
    if target == cur {
        return false;
    }
    let turn = cross(prev, cur, next);
    let c1 = cross(prev, cur, target);
    let c2 = cross(cur, next, target);
    if turn > 0.0 {
        c1 > 0.0 && c2 > 0.0
    } else if turn < 0.0 {
        c1 > 0.0 || c2 > 0.0
    } else {
        let dot = (cur.0 - prev.0) * (next.0 - cur.0) + (cur.1 - prev.1) * (next.1 - cur.1);
        if dot > 0.0 {
            c1 > 0.0
        } else if dot < 0.0 {
            c1 != 0.0 || ((target.0 - cur.0) * (next.0 - cur.0) + (target.1 - cur.1) * (next.1 - cur.1)) < 0.0
        } else {
            false
        }
    }
}

/// Whether segment `a..b` blocks the open bridge segment `(pm, v)`: by strictly crossing it, by
/// placing a vertex strictly between `pm` and `v`, or by overlapping it collinearly.
fn blocks_bridge(pm: (f64, f64), v: (f64, f64), a: (f64, f64), b: (f64, f64)) -> bool {
    if crosses(pm, v, a, b) {
        return true;
    }
    let dx = v.0 - pm.0;
    let dy = v.1 - pm.1;
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return true;
    }
    let tol2 = 1e-24 * len2 * len2;
    let ca = cross(pm, v, a);
    let cb = cross(pm, v, b);
    let a_on_line = ca * ca <= tol2;
    let b_on_line = cb * cb <= tol2;
    if a_on_line && a != pm && a != v {
        let ta = (a.0 - pm.0) * dx + (a.1 - pm.1) * dy;
        if ta > 0.0 && ta < len2 {
            return true;
        }
    }
    if b_on_line && b != pm && b != v {
        let tb = (b.0 - pm.0) * dx + (b.1 - pm.1) * dy;
        if tb > 0.0 && tb < len2 {
            return true;
        }
    }
    if a_on_line && b_on_line {
        let ta = (a.0 - pm.0) * dx + (a.1 - pm.1) * dy;
        let tb = (b.0 - pm.0) * dx + (b.1 - pm.1) * dy;
        if ta.min(tb) < len2 && ta.max(tb) > 0.0 {
            return true;
        }
    }
    false
}

/// Nearest vertices before and after `pos` in `ring` whose coordinates differ from `pts[ring[pos]]`,
/// wrapping around the ring. Returns `None` when every vertex in `ring` coincides with `pos`.
fn distinct_neighbors(pts: &[(f64, f64)], ring: &[usize], pos: usize) -> Option<((f64, f64), (f64, f64))> {
    let n = ring.len();
    let cur = pts[ring[pos]];
    let prev = (1..n).map(|step| pts[ring[(pos + n - step) % n]]).find(|&p| p != cur)?;
    let next = (1..n).map(|step| pts[ring[(pos + step) % n]]).find(|&p| p != cur)?;
    Some((prev, next))
}

/// Triangles (indices into `outer` followed by each hole's vertices, in order) covering the polygon.
/// Rings are open (no repeated closing vertex). `None` when the polygon is degenerate or too large.
pub fn triangulate(outer: &[(f64, f64)], holes: &[Vec<(f64, f64)>]) -> Option<Vec<[usize; 3]>> {
    triangulate_reporting(outer, holes).map(|(tris, _)| tris)
}

/// Like [`triangulate`], and also says whether the last-resort fan was used: a ring left with no
/// ear to cut (self-touching or self-crossing) is finished as a fan from its first vertex, which can
/// spill outside the polygon. Callers that report approximations should count those.
pub fn triangulate_reporting(outer: &[(f64, f64)], holes: &[Vec<(f64, f64)>]) -> Option<(Vec<[usize; 3]>, bool)> {
    let total = outer.len() + holes.iter().map(Vec::len).sum::<usize>();
    if outer.len() < 3 || total > MAX_VERTICES || !outer.iter().chain(holes.iter().flatten()).all(|p| p.0.is_finite() && p.1.is_finite()) {
        return None;
    }
    let pts: Vec<(f64, f64)> = outer.iter().chain(holes.iter().flatten()).copied().collect();
    // The outer ring counter-clockwise, holes clockwise; each a list of indices into `pts`.
    let mut ring: Vec<usize> = (0..outer.len()).collect();
    if area2(outer) < 0.0 {
        ring.reverse();
    }
    let mut start = outer.len();
    let mut hole_rings: Vec<Vec<usize>> = Vec::new();
    for h in holes {
        let mut r: Vec<usize> = (start..start + h.len()).collect();
        start += h.len();
        if h.len() < 3 {
            continue;
        }
        if area2(h) > 0.0 {
            r.reverse();
        }
        hole_rings.push(r);
    }
    // Bridge holes in from the rightmost, each from its rightmost vertex to the nearest vertex of
    // the ring so far that it can see.
    hole_rings.sort_by(|a, b| {
        let mx = |r: &Vec<usize>| r.iter().map(|&i| pts[i].0).fold(f64::MIN, f64::max);
        mx(b).total_cmp(&mx(a))
    });
    let mut work = 0usize;
    for (hi, h) in hole_rings.iter().enumerate() {
        let (mpos, &m) = h.iter().enumerate().max_by(|a, b| pts[*a.1].0.total_cmp(&pts[*b.1].0))?;
        let pm = pts[m];
        let (h_prev, h_next) = distinct_neighbors(&pts, h, mpos)?;
        let edges_clear = |v: (f64, f64), work: &mut usize| {
            let ring_ok = (0..ring.len()).all(|k| {
                *work += 1;
                !blocks_bridge(pm, v, pts[ring[k]], pts[ring[(k + 1) % ring.len()]])
            });
            let holes_ok = ring_ok
                && hole_rings.iter().skip(hi).all(|r| {
                    (0..r.len()).all(|k| {
                        *work += 1;
                        !blocks_bridge(pm, v, pts[r[k]], pts[r[(k + 1) % r.len()]])
                    })
                });
            ring_ok && holes_ok
        };
        let mut best: Option<(usize, f64)> = None;
        for (k, &vi) in ring.iter().enumerate() {
            let v = pts[vi];
            if !in_interior_wedge(h_prev, pm, h_next, v) {
                continue;
            }
            let Some((r_prev, r_next)) = distinct_neighbors(&pts, &ring, k) else {
                continue;
            };
            if !in_interior_wedge(r_prev, v, r_next, pm) {
                continue;
            }
            let d = (v.0 - pm.0).powi(2) + (v.1 - pm.1).powi(2);
            if best.is_none_or(|b| d < b.1) {
                let clear = edges_clear(v, &mut work);
                if work > MAX_WORK {
                    return None;
                }
                if clear {
                    best = Some((k, d));
                }
            }
        }
        let (k, _) = best?;
        // ring[..=k], hole from m round to m, then back to ring[k].
        let mut joined = Vec::with_capacity(ring.len() + h.len() + 2);
        joined.extend_from_slice(&ring[..=k]);
        joined.extend(h.iter().cycle().skip(mpos).take(h.len() + 1));
        joined.extend_from_slice(&ring[k..]);
        ring = joined;
    }
    // Ear clipping: only reflex vertices can block an ear, and the search resumes where the
    // last ear was cut.
    let mut tris = Vec::with_capacity(ring.len());
    let mut fanned = false;
    let mut start = 0usize;
    while ring.len() > 3 {
        let n = ring.len();
        let reflex: Vec<usize> =
            (0..n).filter(|&i| cross(pts[ring[(i + n - 1) % n]], pts[ring[i]], pts[ring[(i + 1) % n]]) <= 0.0).map(|i| ring[i]).collect();
        let mut clipped = false;
        for step in 0..n {
            let i = (start + step) % n;
            let (ia, ib, ic) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            work += reflex.len() + 1;
            if work > MAX_WORK {
                return None;
            }
            let blocked = reflex.iter().any(|&j| {
                let p = pts[j];
                p != a && p != b && p != c && in_triangle(p, a, b, c)
            });
            if !blocked {
                tris.push([ia, ib, ic]);
                ring.remove(i);
                start = i.saturating_sub(1);
                clipped = true;
                break;
            }
        }
        if !clipped {
            // Left with something degenerate (collinear or self-touching): drop a flat vertex,
            // or finish as a fan rather than loop.
            if let Some(i) = (0..n).find(|&i| cross(pts[ring[(i + n - 1) % n]], pts[ring[i]], pts[ring[(i + 1) % n]]).abs() < 1e-12) {
                ring.remove(i);
                continue;
            }
            for i in 1..n - 1 {
                tris.push([ring[0], ring[i], ring[i + 1]]);
            }
            ring.clear();
            fanned = true;
        }
    }
    if ring.len() == 3 {
        tris.push([ring[0], ring[1], ring[2]]);
    }
    Some((tris, fanned))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(pts: &[(f64, f64)], tris: &[[usize; 3]]) -> f64 {
        tris.iter().map(|t| cross(pts[t[0]], pts[t[1]], pts[t[2]]).abs() / 2.0).sum()
    }

    #[test]
    fn a_ring_left_with_no_ear_is_finished_as_a_fan_and_says_so() {
        // A 4×4 square with a triangular hole that shares the square's whole left side (a hole
        // touching the outer ring along an edge, as shared boundaries in real data do). Bridging
        // the hole in leaves a ring whose every vertex is reflex or blocked, so no ear can be cut
        // and it ends as a fan from the first vertex.
        let sq = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let hole = vec![(0.0, 0.0), (3.0, 2.0), (0.0, 4.0)];
        let (tris, fanned) = triangulate_reporting(&sq, std::slice::from_ref(&hole)).unwrap();
        assert!(fanned);
        assert_eq!(triangulate(&sq, &[hole]).unwrap(), tris, "triangulate gives the same triangles");
        // Clean polygons are not fanned: convex, concave, and with holes.
        let l = vec![(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0), (1.0, 2.0), (0.0, 2.0)];
        assert!(!triangulate_reporting(&sq, &[]).unwrap().1);
        assert!(!triangulate_reporting(&l, &[]).unwrap().1);
        let inner = vec![vec![(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)]];
        assert!(!triangulate_reporting(&sq, &inner).unwrap().1);
        // A hole touching the left side at points, without sharing an edge, is fine.
        assert!(!triangulate_reporting(&sq, &[vec![(0.0, 1.0), (2.0, 2.0), (0.0, 3.0)]]).unwrap().1);
    }

    #[test]
    fn concave_and_holed_polygons_cover_their_area() {
        // An L shape (concave): area 3.
        let l = vec![(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0), (1.0, 2.0), (0.0, 2.0)];
        let t = triangulate(&l, &[]).unwrap();
        assert_eq!(t.len(), 4);
        assert!((area(&l, &t) - 3.0).abs() < 1e-9);
        // Clockwise input works too.
        let rev: Vec<_> = l.iter().rev().copied().collect();
        assert!((area(&rev, &triangulate(&rev, &[]).unwrap()) - 3.0).abs() < 1e-9);
        // A 4×4 square with two 1×1 holes: area 14.
        let sq = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let holes = vec![vec![(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)], vec![(2.5, 2.5), (3.5, 2.5), (3.5, 3.5), (2.5, 3.5)]];
        let all: Vec<(f64, f64)> = sq.iter().chain(holes.iter().flatten()).copied().collect();
        let t = triangulate(&sq, &holes).unwrap();
        assert!((area(&all, &t) - 14.0).abs() < 1e-9, "{}", area(&all, &t));
        assert!(t.iter().flatten().all(|i| *i < all.len()));
        // A large spiky polygon (every other vertex reflex) finishes, with its area covered.
        let star: Vec<(f64, f64)> = (0..4000)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 4000.0;
                let r = if i % 2 == 0 { 100.0 } else { 60.0 };
                (r * a.cos(), r * a.sin())
            })
            .collect();
        let t = triangulate(&star, &[]).unwrap();
        assert_eq!(t.len(), star.len() - 2);
        assert!((area(&star, &t) - area2(&star) / 2.0).abs() < 1e-6 * area2(&star));
        assert!(triangulate(&vec![(0.0, 0.0); MAX_VERTICES + 1], &[]).is_none());
        // Degenerate input.
        assert!(triangulate(&[(0.0, 0.0), (1.0, 1.0)], &[]).is_none());
        assert!(triangulate(&[(0.0, 0.0), (f64::NAN, 1.0), (1.0, 0.0)], &[]).is_none());
        let flat = triangulate(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)], &[]).unwrap_or_default();
        assert!(area(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)], &flat) < 1e-12);

        // A smooth circle of 3000 vertices with 8 small holes inside.
        // Should successfully triangulate within MAX_VERTICES (5000) and not falsely abort on work budget.
        let circle: Vec<(f64, f64)> = (0..3000)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 3000.0;
                (100.0 * a.cos(), 100.0 * a.sin())
            })
            .collect();
        let circle_holes: Vec<Vec<(f64, f64)>> = (0..8)
            .map(|i| {
                let ox = (i as f64 - 3.5) * 15.0;
                vec![(ox, -2.0), (ox + 4.0, -2.0), (ox + 4.0, 2.0), (ox, 2.0)]
            })
            .collect();
        let tr = triangulate(&circle, &circle_holes);
        assert!(tr.is_some(), "3000-vertex polygon with 8 holes should succeed");
    }

    #[test]
    fn diamond_hole_bridge_does_not_cut_through_hole_interior() {
        // Outer diamond (area 50) with an interior diamond hole (area 4). The closest outer vertex
        // to the hole's rightmost vertex (3, 5) is (0, 5), directly across the hole interior and
        // through vertex (1, 5); bridging there corrupts the triangulation.
        let outer = vec![(0.0, 5.0), (5.0, 0.0), (10.0, 5.0), (5.0, 10.0)];
        let hole = vec![(1.0, 5.0), (2.0, 7.0), (3.0, 5.0), (2.0, 3.0)];
        let all: Vec<(f64, f64)> = outer.iter().chain(&hole).copied().collect();
        let (tris, fanned) = triangulate_reporting(&outer, std::slice::from_ref(&hole)).unwrap();
        assert!(!fanned, "clean diamond with hole should not fall back to fan");
        assert!((area(&all, &tris) - 46.0).abs() < 1e-9, "expected area 46, got {}", area(&all, &tris));
        let hole_center = (2.0, 5.0);
        assert!(!tris.iter().any(|t| in_triangle(hole_center, all[t[0]], all[t[1]], all[t[2]])), "no triangle may cover the hole interior");

        // Two horizontally aligned diamond holes inside a square: bridging the left hole must not
        // pass through a vertex of the right hole or its interior.
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let h1 = vec![(1.0, 5.0), (2.0, 6.0), (3.0, 5.0), (2.0, 4.0)];
        let h2 = vec![(5.0, 5.0), (6.0, 6.0), (7.0, 5.0), (6.0, 4.0)];
        let holes = vec![h1, h2];
        let all2: Vec<(f64, f64)> = sq.iter().chain(holes.iter().flatten()).copied().collect();
        let (tris2, fanned2) = triangulate_reporting(&sq, &holes).unwrap();
        assert!(!fanned2);
        assert!((area(&all2, &tris2) - 96.0).abs() < 1e-9, "expected area 96, got {}", area(&all2, &tris2));
        for pt in [(2.0, 5.0), (6.0, 5.0)] {
            assert!(!tris2.iter().any(|t| in_triangle(pt, all2[t[0]], all2[t[1]], all2[t[2]])));
        }

        // Repeated consecutive coordinates at the hole's rightmost vertex must not produce a
        // zero-length wedge edge and fail bridging.
        let dup_hole = vec![(6.0, 4.0), (8.0, 5.0), (8.0, 5.0), (6.0, 6.0)];
        let all3: Vec<(f64, f64)> = sq.iter().chain(&dup_hole).copied().collect();
        let (tris3, fanned3) = triangulate_reporting(&sq, std::slice::from_ref(&dup_hole)).unwrap();
        assert!(!fanned3, "hole with repeated rightmost vertex should bridge cleanly");
        assert!((area(&all3, &tris3) - 98.0).abs() < 1e-9, "expected area 98, got {}", area(&all3, &tris3));
        assert!(!tris3.iter().any(|t| in_triangle((6.5, 5.0), all3[t[0]], all3[t[1]], all3[t[2]])));
    }
}
