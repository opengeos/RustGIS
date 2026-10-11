//! Polygon triangulation by ear clipping, holes joined to the outer ring by bridges. Used for the
//! caps of extruded polygons, which viewers would otherwise fan from one corner (wrong for concave
//! shapes and holes).

/// Most vertices triangulated (ear clipping is quadratic); larger polygons get `None`.
pub const MAX_VERTICES: usize = 20_000;

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

/// Triangles (indices into `outer` followed by each hole's vertices, in order) covering the polygon.
/// Rings are open (no repeated closing vertex). `None` when the polygon is degenerate or too large.
pub fn triangulate(outer: &[(f64, f64)], holes: &[Vec<(f64, f64)>]) -> Option<Vec<[usize; 3]>> {
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
        mx(b).partial_cmp(&mx(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    for (hi, h) in hole_rings.iter().enumerate() {
        let (mpos, &m) = h.iter().enumerate().max_by(|a, b| pts[*a.1].0.partial_cmp(&pts[*b.1].0).unwrap_or(std::cmp::Ordering::Equal))?;
        let pm = pts[m];
        let edges_clear = |v: (f64, f64)| {
            let ring_ok = (0..ring.len()).all(|k| !crosses(pm, v, pts[ring[k]], pts[ring[(k + 1) % ring.len()]]));
            let holes_ok = hole_rings.iter().skip(hi).all(|r| (0..r.len()).all(|k| !crosses(pm, v, pts[r[k]], pts[r[(k + 1) % r.len()]])));
            ring_ok && holes_ok
        };
        let mut best: Option<(usize, f64)> = None;
        for (k, &vi) in ring.iter().enumerate() {
            let v = pts[vi];
            let d = (v.0 - pm.0).powi(2) + (v.1 - pm.1).powi(2);
            if best.is_none_or(|b| d < b.1) && edges_clear(v) {
                best = Some((k, d));
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
    // Ear clipping.
    let mut tris = Vec::with_capacity(ring.len());
    let mut guard = 0usize;
    while ring.len() > 3 {
        guard += 1;
        if guard > 4 * total * total + 16 {
            break;
        }
        let n = ring.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            let blocked = ring.iter().any(|&j| {
                let p = pts[j];
                p != a && p != b && p != c && in_triangle(p, a, b, c)
            });
            if !blocked {
                tris.push([ia, ib, ic]);
                ring.remove(i);
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
        }
    }
    if ring.len() == 3 {
        tris.push([ring[0], ring[1], ring[2]]);
    }
    Some(tris)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(pts: &[(f64, f64)], tris: &[[usize; 3]]) -> f64 {
        tris.iter().map(|t| cross(pts[t[0]], pts[t[1]], pts[t[2]]).abs() / 2.0).sum()
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
        // Degenerate input.
        assert!(triangulate(&[(0.0, 0.0), (1.0, 1.0)], &[]).is_none());
        assert!(triangulate(&[(0.0, 0.0), (f64::NAN, 1.0), (1.0, 0.0)], &[]).is_none());
        let flat = triangulate(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)], &[]).unwrap_or_default();
        assert!(area(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)], &flat) < 1e-12);
    }
}
