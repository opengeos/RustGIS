//! Editing geometry: merging features, and topology checks (overlaps, gaps, self-intersections,
//! dangles, coverage). Computed in Web Mercator metres.

use geo::{Area, BooleanOps, BoundingRect, Contains, Intersects, Orient, orient::Direction};
use rustgis_data::{Dataset, Geometry};
use rustgis_geom::{Coord, lonlat_to_merc, merc_to_lonlat};

use crate::convert::{from_multipolygon, to_multipolygon};

/// A topology problem: the rows involved and where it is (longitude/latitude).
#[derive(Clone, Debug, PartialEq)]
pub struct TopologyError {
    pub rows: Vec<usize>,
    pub at: Coord,
}

/// Most errors reported per check.
const MAX_ERRORS: usize = 10_000;

/// One geometry from several: polygons are unioned, lines and points gathered.
pub fn merge(geoms: &[Geometry]) -> Option<Geometry> {
    match geoms.first()? {
        Geometry::Polygon(_) => {
            let polys: Vec<geo::MultiPolygon<f64>> =
                geoms.iter().filter_map(to_multipolygon).map(|m| m.orient(Direction::Default)).collect();
            let all: Vec<geo::Polygon<f64>> = polys.into_iter().flat_map(|m| m.0).collect();
            from_multipolygon(&geo::unary_union(all.iter()))
        }
        Geometry::Line(_) => {
            let parts: Vec<Vec<Coord>> =
                geoms.iter().filter_map(|g| if let Geometry::Line(p) = g { Some(p.clone()) } else { None }).flatten().collect();
            (!parts.is_empty()).then_some(Geometry::Line(parts))
        }
        Geometry::Point(_) | Geometry::MultiPoint(_) => {
            let pts: Vec<Coord> = geoms
                .iter()
                .flat_map(|g| match g {
                    Geometry::Point(c) => vec![*c],
                    Geometry::MultiPoint(v) => v.clone(),
                    _ => Vec::new(),
                })
                .collect();
            (!pts.is_empty()).then_some(Geometry::MultiPoint(pts))
        }
    }
}

/// The parts of a multipart geometry as separate geometries.
pub fn explode(g: &Geometry) -> Vec<Geometry> {
    match g {
        Geometry::MultiPoint(v) => v.iter().map(|c| Geometry::Point(*c)).collect(),
        Geometry::Line(parts) => parts.iter().map(|p| Geometry::Line(vec![p.clone()])).collect(),
        Geometry::Polygon(polys) => polys.iter().map(|p| Geometry::Polygon(vec![p.clone()])).collect(),
        Geometry::Point(_) => vec![g.clone()],
    }
}

fn polygons(ds: &Dataset, rows: &[usize]) -> Vec<(usize, geo::MultiPolygon<f64>, geo::Rect<f64>)> {
    rows.iter()
        .filter_map(|r| {
            let mp = ds.features.get(*r)?.geometry.as_ref().and_then(to_multipolygon)?.orient(Direction::Default);
            let bb = mp.bounding_rect()?;
            Some((*r, mp, bb))
        })
        .collect()
}

fn centre(mp: &geo::MultiPolygon<f64>) -> Coord {
    use geo::InteriorPoint;
    mp.interior_point().map(|p| merc_to_lonlat(Coord::new(p.x(), p.y()))).unwrap_or_default()
}

fn rects_touch(a: &geo::Rect<f64>, b: &geo::Rect<f64>) -> bool {
    a.min().x <= b.max().x && b.min().x <= a.max().x && a.min().y <= b.max().y && b.min().y <= a.max().y
}

/// Pairs of polygons that overlap (share area, not just an edge).
pub fn overlaps(ds: &Dataset, rows: &[usize]) -> Vec<TopologyError> {
    let p = polygons(ds, rows);
    let mut out = Vec::new();
    for (i, (ra, a, ba)) in p.iter().enumerate() {
        for (rb, b, bb) in p.iter().skip(i + 1) {
            if !rects_touch(ba, bb) || !a.intersects(b) {
                continue;
            }
            let inter = a.intersection(b);
            // Shared edges leave slivers of floating-point area; a square metre is a real overlap.
            if inter.unsigned_area() > 1.0 {
                out.push(TopologyError { rows: vec![*ra, *rb], at: centre(&inter) });
                if out.len() >= MAX_ERRORS {
                    return out;
                }
            }
        }
    }
    out
}

/// Holes in the union of a layer's polygons that no polygon covers (the outer boundary isn't a
/// gap).
pub fn gaps(ds: &Dataset, rows: &[usize]) -> Vec<TopologyError> {
    let all: Vec<geo::Polygon<f64>> = polygons(ds, rows).into_iter().flat_map(|(_, m, _)| m.0).collect();
    let u = geo::unary_union(all.iter());
    let mut out = Vec::new();
    for poly in &u.0 {
        for hole in poly.interiors() {
            let mp = geo::MultiPolygon::new(vec![geo::Polygon::new(hole.clone(), vec![])]);
            if mp.unsigned_area() > 1.0 {
                out.push(TopologyError { rows: Vec::new(), at: centre(&mp) });
            }
        }
    }
    out
}

fn segments_cross(a: (Coord, Coord), b: (Coord, Coord)) -> Option<Coord> {
    let d = (a.1.x - a.0.x) * (b.1.y - b.0.y) - (a.1.y - a.0.y) * (b.1.x - b.0.x);
    if d.abs() < 1e-12 {
        return None;
    }
    let t = ((b.0.x - a.0.x) * (b.1.y - b.0.y) - (b.0.y - a.0.y) * (b.1.x - b.0.x)) / d;
    let u = ((b.0.x - a.0.x) * (a.1.y - a.0.y) - (b.0.y - a.0.y) * (a.1.x - a.0.x)) / d;
    // Strictly inside both segments: touching at shared vertices isn't crossing.
    let e = 1e-9;
    (t > e && t < 1.0 - e && u > e && u < 1.0 - e).then(|| Coord::new(a.0.x + t * (a.1.x - a.0.x), a.0.y + t * (a.1.y - a.0.y)))
}

/// Where a path crosses itself (rings and lines; ring closures are allowed).
fn self_crossing(path: &[Coord], closed: bool) -> Option<Coord> {
    let segs: Vec<(Coord, Coord)> = path.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).collect();
    let n = segs.len();
    if n > 5000 {
        return None;
    }
    for i in 0..n {
        for j in i + 2..n {
            if closed && i == 0 && j == n - 1 {
                continue;
            }
            if let (Some(a), Some(b)) = (segs.get(i), segs.get(j))
                && let Some(c) = segments_cross(*a, *b)
            {
                return Some(c);
            }
        }
    }
    None
}

/// Lines and polygon rings that cross themselves.
pub fn self_intersections(ds: &Dataset, rows: &[usize]) -> Vec<TopologyError> {
    let mut out = Vec::new();
    for r in rows {
        let Some(g) = ds.features.get(*r).and_then(|f| f.geometry.as_ref()) else { continue };
        let paths: Vec<(Vec<Coord>, bool)> = match g {
            Geometry::Line(parts) => parts.iter().map(|p| (p.clone(), false)).collect(),
            Geometry::Polygon(polys) => polys.iter().flatten().map(|r| (r.clone(), true)).collect(),
            _ => Vec::new(),
        };
        for (p, closed) in paths {
            let m: Vec<Coord> = p.iter().map(|c| lonlat_to_merc(*c)).collect();
            if let Some(c) = self_crossing(&m, closed) {
                out.push(TopologyError { rows: vec![*r], at: merc_to_lonlat(c) });
                break;
            }
        }
        if out.len() >= MAX_ERRORS {
            break;
        }
    }
    out
}

/// Line ends that touch no other line (within `tol_m` metres).
pub fn dangles(ds: &Dataset, rows: &[usize], tol_m: f64) -> Vec<TopologyError> {
    let lines: Vec<(usize, Vec<Vec<Coord>>)> = rows
        .iter()
        .filter_map(|r| match ds.features.get(*r)?.geometry.as_ref()? {
            Geometry::Line(parts) => Some((*r, parts.iter().map(|p| p.iter().map(|c| lonlat_to_merc(*c)).collect()).collect())),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for (r, parts) in &lines {
        for (pi, part) in parts.iter().enumerate() {
            // A closed part has no ends.
            if part.len() < 2 || part.first() == part.last() {
                continue;
            }
            for end in [part.first(), part.last()].into_iter().flatten() {
                let tol = tol_m / merc_to_lonlat(*end).y.to_radians().cos().max(0.01);
                // Any other part (of this line or another) within the tolerance.
                let touches = lines.iter().any(|(o, oparts)| {
                    oparts
                        .iter()
                        .enumerate()
                        .any(|(opi, op)| (o != r || opi != pi) && op.len() >= 2 && rustgis_data::geometry::seg_dist2(op, *end) <= tol * tol)
                });
                if !touches {
                    out.push(TopologyError { rows: vec![*r], at: merc_to_lonlat(*end) });
                }
            }
        }
        if out.len() >= MAX_ERRORS {
            break;
        }
    }
    out
}

/// Points of `ds` that fall in no polygon of `by`.
pub fn not_covered(ds: &Dataset, rows: &[usize], by: &Dataset) -> Vec<TopologyError> {
    let all: Vec<usize> = (0..by.features.len()).collect();
    let polys = polygons(by, &all);
    let mut out = Vec::new();
    for r in rows {
        let pts: Vec<Coord> = match ds.features.get(*r).and_then(|f| f.geometry.as_ref()) {
            Some(Geometry::Point(c)) => vec![*c],
            Some(Geometry::MultiPoint(v)) => v.clone(),
            _ => continue,
        };
        for c in pts {
            let m = lonlat_to_merc(c);
            let p = geo::Point::new(m.x, m.y);
            if !polys.iter().any(|(_, mp, bb)| rects_touch(bb, &geo::Rect::new(p.0, p.0)) && mp.contains(&p)) {
                out.push(TopologyError { rows: vec![*r], at: c });
            }
        }
        if out.len() >= MAX_ERRORS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::GeometryType;

    fn square(x: f64, y: f64, s: f64) -> Geometry {
        Geometry::Polygon(vec![vec![vec![
            Coord::new(x, y),
            Coord::new(x + s, y),
            Coord::new(x + s, y + s),
            Coord::new(x, y + s),
            Coord::new(x, y),
        ]]])
    }

    #[test]
    fn checks_find_problems() {
        let mut ds = Dataset::new("p", GeometryType::Polygon, vec![]);
        ds.push(Some(square(0.0, 0.0, 1.0)), vec![]);
        ds.push(Some(square(0.5, 0.5, 1.0)), vec![]); // overlaps the first
        ds.push(Some(square(1.0, 0.0, 1.0)), vec![]); // shares an edge with the first
        let rows: Vec<usize> = (0..3).collect();
        let o = overlaps(&ds, &rows);
        assert!(o.iter().any(|e| e.rows == vec![0, 1]) && !o.iter().any(|e| e.rows == vec![0, 2]), "{o:?}");
        // A ring of four squares around a hole.
        let mut ring = Dataset::new("r", GeometryType::Polygon, vec![]);
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (0.0, 1.0), (2.0, 1.0), (0.0, 2.0), (1.0, 2.0), (2.0, 2.0)] {
            ring.push(Some(square(x, y, 1.0)), vec![]);
        }
        let g = gaps(&ring, &(0..8).collect::<Vec<_>>());
        assert_eq!(g.len(), 1);
        assert!((g[0].at.x - 1.5).abs() < 0.1 && (g[0].at.y - 1.5).abs() < 0.1, "{:?}", g[0].at);
        // A bow tie crosses itself.
        let mut lines = Dataset::new("l", GeometryType::Polyline, vec![]);
        lines.push(
            Some(Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(1.0, 1.0), Coord::new(1.0, 0.0), Coord::new(0.0, 1.0)]])),
            vec![],
        );
        lines.push(Some(Geometry::Line(vec![vec![Coord::new(5.0, 5.0), Coord::new(6.0, 5.0)]])), vec![]);
        lines.push(Some(Geometry::Line(vec![vec![Coord::new(6.0, 5.0), Coord::new(6.0, 6.0)]])), vec![]);
        assert_eq!(self_intersections(&lines, &[0, 1, 2]).len(), 1);
        // Line 1 starts and line 2 ends in the open: two dangles (they meet at 6,5).
        assert_eq!(dangles(&lines, &[1, 2], 1.0).len(), 2);
        let mut pts = Dataset::new("pt", GeometryType::Point, vec![]);
        pts.push(Some(Geometry::Point(Coord::new(0.5, 0.5))), vec![]);
        pts.push(Some(Geometry::Point(Coord::new(9.0, 9.0))), vec![]);
        assert_eq!(not_covered(&pts, &[0, 1], &ds).iter().map(|e| e.rows.clone()).collect::<Vec<_>>(), vec![vec![1]]);
        // Merging two overlapping squares makes one polygon.
        let m = merge(&[square(0.0, 0.0, 1.0), square(0.5, 0.0, 1.0)]).unwrap_or(Geometry::Point(Coord::default()));
        assert!(matches!(&m, Geometry::Polygon(p) if p.len() == 1));
        assert_eq!(explode(&Geometry::MultiPoint(vec![Coord::new(0.0, 0.0), Coord::new(1.0, 1.0)])).len(), 2);
    }
}
