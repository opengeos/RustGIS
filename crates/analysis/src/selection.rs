//! Bounded, even-odd area selection in an already projected planar coordinate space.
//!
//! Only the selection outline is normalized. Candidate geometries are borrowed, and their
//! non-finite separators are treated as breaks rather than passed to `geo`.

use geo::algorithm::bool_ops::{BooleanOps, FillRule};
use geo::line_intersection::line_intersection;
use geo::{Coord as GeoCoord, Intersects, Line, LineString, MultiPolygon, Point, Polygon};
use rustgis_data::{Geometry, geometry::ring_contains};
use rustgis_geom::{Coord, Extent};

/// Maximum number of supplied or sampled vertices in a selection outline.
pub const MAX_POINTS: usize = 20_000;
const MAX_PAIR_CHECKS: usize = 2_000_000;
const MAX_CROSSINGS: usize = 32_768;
const MAX_NORMALIZED_POINTS: usize = 100_000;
const NORMALIZATION_SCALE: f64 = 65_536.0;
const TOO_COMPLEX: &str = "Selection outline is too complex; draw a simpler area.";

/// A reusable even-odd selection area. Coordinates are planar, not longitude/latitude.
pub struct PolygonMask {
    filled: MultiPolygon<f64>,
    extent: Extent,
    origin: Coord,
    boundary: Vec<(Coord, Coord)>,
}

impl PolygonMask {
    /// Validate and normalize an open or explicitly closed outline once, before querying rows.
    pub fn new(points: &[Coord]) -> Result<Self, String> {
        if points.len() > MAX_POINTS {
            return Err(TOO_COMPLEX.into());
        }
        if points.iter().any(|p| !p.is_finite()) {
            return Err("Selection outline contains a non-finite coordinate.".into());
        }
        let mut ring = Vec::with_capacity(points.len() + 1);
        for &point in points {
            if ring.last() != Some(&point) {
                ring.push(point);
            }
        }
        if ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        let distinct_enough = ring
            .first()
            .is_some_and(|first| ring.iter().find(|p| *p != first).is_some_and(|second| ring.iter().any(|p| p != first && p != second)));
        if !distinct_enough {
            return Err("Selection outline needs at least three distinct points.".into());
        }
        let mut source_extent = Extent::EMPTY;
        for &point in &ring {
            source_extent.expand(point);
        }
        let origin =
            Coord::new(source_extent.min_x * 0.5 + source_extent.max_x * 0.5, source_extent.min_y * 0.5 + source_extent.max_y * 0.5);
        if let Some(&first) = ring.first() {
            ring.push(first);
        }
        bound_normalization(&ring)?;
        let mut local_ring = Vec::with_capacity(ring.len());
        for &point in &ring {
            let local = Coord::new((point.x - origin.x) * NORMALIZATION_SCALE, (point.y - origin.y) * NORMALIZATION_SCALE);
            if !local.is_finite() {
                return Err("Selection outline could not be normalized.".into());
            }
            local_ring.push(geo_coord(local));
        }
        let mut boundary: Vec<(Coord, Coord)> = ring.windows(2).map(|edge| canonical_edge(edge[0], edge[1])).collect();
        boundary.sort_unstable_by(|a, b| edge_cmp(*a, *b));
        let mut read = 0;
        let mut write = 0;
        while read < boundary.len() {
            let mut end = read + 1;
            while end < boundary.len() && boundary.get(end) == boundary.get(read) {
                end += 1;
            }
            if (end - read) % 2 == 1
                && let Some(edge) = boundary.get(read).copied()
            {
                if let Some(slot) = boundary.get_mut(write) {
                    *slot = edge;
                }
                write += 1;
            }
            read = end;
        }
        boundary.truncate(write);
        let raw = Polygon::new(LineString::new(local_ring), vec![]);
        let filled = raw.union_with_fill_rule(&MultiPolygon::<f64>::new(vec![]), FillRule::EvenOdd);
        if filled.0.is_empty() {
            return Err("Selection outline has no filled area.".into());
        }
        let mut count = 0;
        let mut extent = Extent::EMPTY;
        for polygon in &filled.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for p in &ring.0 {
                    count += 1;
                    if count > MAX_NORMALIZED_POINTS {
                        return Err(TOO_COMPLEX.into());
                    }
                    let p = Coord::new(p.x, p.y);
                    if !p.is_finite() {
                        return Err("Selection outline could not be normalized.".into());
                    }
                    extent.expand(p);
                }
            }
        }
        for &point in &ring {
            extent.expand(Coord::new((point.x - origin.x) * NORMALIZATION_SCALE, (point.y - origin.y) * NORMALIZATION_SCALE));
        }
        Ok(Self { filled, extent, origin, boundary })
    }

    /// Whether a candidate touches or overlaps the mask, including its boundary.
    pub fn intersects(&self, geometry: &Geometry) -> bool {
        let geometry_extent = geometry.extent();
        if geometry_extent.is_empty()
            || !self.extent.intersects(&Extent::from_points(
                self.local(Coord::new(geometry_extent.min_x, geometry_extent.min_y)),
                self.local(Coord::new(geometry_extent.max_x, geometry_extent.max_y)),
            ))
        {
            return false;
        }
        match geometry {
            Geometry::Point(p) => self.point_hits(*p),
            Geometry::MultiPoint(points) => points.iter().any(|p| self.point_hits(*p)),
            Geometry::Line(parts) => parts.iter().any(|part| self.path_hits(part, false)),
            Geometry::Polygon(polygons) => {
                if polygons.iter().flatten().any(|ring| self.path_hits(ring, true)) {
                    return true;
                }
                // Boundary tests do not find a selection area wholly inside a feature. A
                // crossing outline can normalize into several disjoint components: test each.
                self.filled.0.iter().any(|component| {
                    component.exterior().0.first().is_some_and(|p| {
                        let p = self.global(Coord::new(p.x, p.y));
                        polygons.iter().any(|rings| polygon_contains_finite(rings, p))
                    })
                })
            }
        }
    }

    fn local(&self, p: Coord) -> Coord {
        Coord::new((p.x - self.origin.x) * NORMALIZATION_SCALE, (p.y - self.origin.y) * NORMALIZATION_SCALE)
    }

    fn global(&self, p: Coord) -> Coord {
        Coord::new(p.x / NORMALIZATION_SCALE + self.origin.x, p.y / NORMALIZATION_SCALE + self.origin.y)
    }
    fn point_hits(&self, p: Coord) -> bool {
        if !p.is_finite() {
            return false;
        }
        let p = self.local(p);
        self.extent.contains(p)
            && (self.filled.intersects(&Point(geo_coord(p)))
                || self
                    .boundary
                    .iter()
                    .any(|&(a, b)| Line::new(geo_coord(self.local(a)), geo_coord(self.local(b))).intersects(&Point(geo_coord(p)))))
    }

    fn segment_hits(&self, a: Coord, b: Coord) -> bool {
        let (a, b) = (self.local(a), self.local(b));
        self.extent.intersects(&Extent::from_points(a, b)) && {
            let candidate = Line::new(geo_coord(a), geo_coord(b));
            self.filled.intersects(&candidate)
                || self
                    .boundary
                    .iter()
                    .any(|&(start, end)| Line::new(geo_coord(self.local(start)), geo_coord(self.local(end))).intersects(&candidate))
        }
    }

    fn path_hits(&self, path: &[Coord], close: bool) -> bool {
        path.split(|p| !p.is_finite()).any(|run| {
            run.iter().any(|p| self.point_hits(*p))
                || run.windows(2).any(|pair| self.segment_hits(pair[0], pair[1]))
                || (close && run.len() >= 3 && self.segment_hits(run[run.len() - 1], run[0]))
        })
    }
}

fn geo_coord(p: Coord) -> GeoCoord<f64> {
    GeoCoord { x: p.x, y: p.y }
}

fn coord_cmp(a: Coord, b: Coord) -> std::cmp::Ordering {
    a.x.total_cmp(&b.x).then_with(|| a.y.total_cmp(&b.y))
}

fn canonical_edge(a: Coord, b: Coord) -> (Coord, Coord) {
    if coord_cmp(a, b).is_gt() { (b, a) } else { (a, b) }
}

fn edge_cmp(a: (Coord, Coord), b: (Coord, Coord)) -> std::cmp::Ordering {
    coord_cmp(a.0, b.0).then_with(|| coord_cmp(a.1, b.1))
}

fn ring_contains_finite(ring: &[Coord], p: Coord) -> bool {
    ring.split(|c| !c.is_finite()).filter(|run| run.len() >= 3).fold(false, |inside, run| inside ^ ring_contains(run, p))
}

fn polygon_contains_finite(rings: &[Vec<Coord>], p: Coord) -> bool {
    let mut rings = rings.iter();
    rings.next().is_some_and(|outer| ring_contains_finite(outer, p) && !rings.any(|hole| ring_contains_finite(hole, p)))
}

// Bound pair examination as well as actual crossings before invoking the normalizer. This
// iterative x sweep deliberately avoids geo's recursive intersections iterator on hostile input.
fn bound_normalization(ring: &[Coord]) -> Result<(), String> {
    let n = ring.len().saturating_sub(1);
    let mut order: Vec<usize> = (0..n).collect();
    let min_x = |i: usize| ring[i].x.min(ring[i + 1].x);
    order.sort_unstable_by(|a, b| min_x(*a).total_cmp(&min_x(*b)));
    let mut pairs = 0;
    let mut crossings = 0;
    for (position, &i) in order.iter().enumerate() {
        let a = ring[i];
        let b = ring[i + 1];
        let max_x = a.x.max(b.x);
        for &j in &order[position + 1..] {
            if min_x(j) > max_x {
                break;
            }
            if i.abs_diff(j) == 1 || (i == 0 && j == n - 1) || (j == 0 && i == n - 1) {
                continue;
            }
            pairs += 1;
            if pairs > MAX_PAIR_CHECKS {
                return Err(TOO_COMPLEX.into());
            }
            let c = ring[j];
            let d = ring[j + 1];
            if a.y.min(b.y) > c.y.max(d.y) || c.y.min(d.y) > a.y.max(b.y) {
                continue;
            }
            if line_intersection(Line::new(geo_coord(a), geo_coord(b)), Line::new(geo_coord(c), geo_coord(d))).is_some() {
                crossings += 1;
                if crossings > MAX_CROSSINGS {
                    return Err(TOO_COMPLEX.into());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coords(points: &[(f64, f64)]) -> Vec<Coord> {
        points.iter().map(|&(x, y)| Coord::new(x, y)).collect()
    }

    fn l_outline() -> Vec<Coord> {
        coords(&[(0.0, 0.0), (3.0, 0.0), (3.0, 1.0), (1.0, 1.0), (1.0, 3.0), (0.0, 3.0)])
    }

    fn mask(points: &[Coord]) -> PolygonMask {
        PolygonMask::new(points).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn projected_l_boundary_point_intersects_after_normalization() {
        let project = |lon, lat| {
            let p = rustgis_geom::lonlat_to_merc(Coord::new(lon, lat));
            Coord::new(p.x, p.y)
        };
        let outline = [(0.0, 0.0), (3.0, 0.0), (3.0, 1.0), (1.0, 1.0), (1.0, 3.0), (0.0, 3.0)].map(|(lon, lat)| project(lon, lat));
        let mask = mask(&outline);
        let boundary = project(3.0, 0.5);
        let edge = Line::new(geo_coord(project(3.0, 0.0)), geo_coord(project(3.0, 1.0)));
        assert!(edge.intersects(&Point(geo_coord(boundary))));
        assert!(mask.intersects(&Geometry::Point(boundary)), "{boundary:?}: {:?}", mask.filled);
    }
    #[test]
    fn l_mask_points_crossings_containment_and_holes() {
        let mask = mask(&l_outline());
        for (x, y, hit) in [(0.5, 0.5, true), (3.0, 0.5, true), (2.0, 2.0, false), (4.0, 4.0, false)] {
            assert_eq!(mask.intersects(&Geometry::Point(Coord::new(x, y))), hit);
        }
        assert!(mask.intersects(&Geometry::Line(vec![coords(&[(-1.0, 0.5), (4.0, 0.5)])])));
        let outer = coords(&[(-1.0, -1.0), (5.0, -1.0), (5.0, 5.0), (-1.0, 5.0)]);
        assert!(mask.intersects(&Geometry::Polygon(vec![vec![outer.clone()]])));
        let hole = coords(&[(-0.5, -0.5), (3.5, -0.5), (3.5, 3.5), (-0.5, 3.5)]);
        assert!(!mask.intersects(&Geometry::Polygon(vec![vec![outer, hole]])));
        assert!(mask.intersects(&Geometry::MultiPoint(coords(&[(4.0, 4.0), (0.5, 0.5)]))));
        assert!(mask.intersects(&Geometry::Polygon(vec![
            vec![coords(&[(4.0, 4.0), (5.0, 4.0), (5.0, 5.0)])],
            vec![coords(&[(0.2, 0.2), (0.4, 0.2), (0.3, 0.4)])],
        ])));
    }

    #[test]
    fn crossing_outline_uses_even_odd_and_all_components() {
        let mask = mask(&coords(&[(0.0, 0.0), (4.0, 4.0), (0.0, 4.0), (4.0, 0.0)]));
        for (x, y, hit) in [(2.0, 0.5, true), (2.0, 3.5, true), (0.5, 2.0, false), (2.0, 2.0, true)] {
            assert_eq!(mask.intersects(&Geometry::Point(Coord::new(x, y))), hit);
        }
        for ring in
            [coords(&[(-1.0, -1.0), (5.0, -1.0), (5.0, 1.5), (-1.0, 1.5)]), coords(&[(-1.0, 2.5), (5.0, 2.5), (5.0, 5.0), (-1.0, 5.0)])]
        {
            assert!(mask.intersects(&Geometry::Polygon(vec![vec![ring]])));
        }
    }

    #[test]
    fn containment_checks_every_normalized_component() {
        // The twice-traversed bridge has no even-odd fill, leaving two separate squares.
        let mask = mask(&coords(&[
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
            (0.0, 0.0),
            (3.0, 0.0),
            (4.0, 0.0),
            (4.0, 1.0),
            (3.0, 1.0),
            (3.0, 0.0),
        ]));
        assert_eq!(mask.filled.0.len(), 2);
        assert!(mask.intersects(&Geometry::Point(Coord::new(0.5, 0.0))), "the square boundary remains filled");
        assert!(!mask.intersects(&Geometry::Point(Coord::new(2.0, 0.0))), "the twice-traversed bridge cancels");
        for left in [-0.1, 2.9] {
            let ring = coords(&[(left, -0.1), (left + 1.2, -0.1), (left + 1.2, 1.1), (left, 1.1)]);
            assert!(mask.intersects(&Geometry::Polygon(vec![vec![ring]])));
        }
    }

    #[test]
    fn finite_runs_never_bridge_nan_separators() {
        let mask = mask(&l_outline());
        let nan = Coord::new(f64::NAN, f64::NAN);
        assert!(!mask.intersects(&Geometry::Point(nan)));
        assert!(!mask.intersects(&Geometry::Line(vec![vec![Coord::new(-1.0, 0.5), nan, Coord::new(4.0, 0.5)]])));
        assert!(mask.intersects(&Geometry::Line(vec![vec![nan, Coord::new(-1.0, 0.5), Coord::new(4.0, 0.5), nan]])));
        let outer = coords(&[(-1.0, -1.0), (5.0, -1.0), (5.0, 5.0), (-1.0, 5.0)]);
        let mut separated = vec![Coord::new(9.0, 9.0), nan];
        separated.extend_from_slice(&outer);
        assert!(mask.intersects(&Geometry::Polygon(vec![vec![separated.clone()]])));
        // Two identical finite runs cancel under ring parity.
        separated.push(nan);
        separated.extend_from_slice(&outer);
        assert!(!mask.intersects(&Geometry::Polygon(vec![vec![separated]])));
        let mut hole = vec![nan];
        hole.extend(coords(&[(-0.5, -0.5), (3.5, -0.5), (3.5, 3.5), (-0.5, 3.5)]));
        hole.push(nan);
        assert!(!mask.intersects(&Geometry::Polygon(vec![vec![outer, hole]])));
        // The closing edge is implicit and must be tested for finite polygon runs.
        assert!(mask.intersects(&Geometry::Polygon(vec![vec![coords(&[(-1.0, 0.5), (-1.0, -1.0), (4.0, 0.5)])]])));
    }

    #[test]
    fn closure_duplicates_and_degenerate_input() {
        let open = l_outline();
        let mut closed = open.clone();
        closed.push(open[0]);
        let mut duplicates = Vec::new();
        for &p in &closed {
            duplicates.extend([p, p]);
        }
        let masks = [mask(&open), mask(&closed), mask(&duplicates)];
        for x in -1..=4 {
            for y in -1..=4 {
                let point = Geometry::Point(Coord::new(x as f64, y as f64));
                assert_eq!(masks[0].intersects(&point), masks[1].intersects(&point));
                assert_eq!(masks[0].intersects(&point), masks[2].intersects(&point));
            }
        }
        for invalid in [
            vec![],
            vec![Coord::new(1.0, 1.0); 10],
            coords(&[(0.0, 0.0), (1.0, 1.0), (0.0, 0.0), (1.0, 1.0)]),
            coords(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]),
            coords(&[(0.0, 0.0), (1.0, f64::NAN), (1.0, 1.0)]),
            coords(&[(0.0, 0.0), (f64::INFINITY, 0.0), (1.0, 1.0)]),
            vec![Coord::new(0.0, 0.0); MAX_POINTS + 1],
        ] {
            assert!(PolygonMask::new(&invalid).is_err());
        }
    }

    #[test]
    fn complexity_guards_and_detailed_ordinary_outline() {
        // A coprime, near-half-turn step makes a dense crossing star.
        let n = 1001;
        let star: Vec<_> = (0..n)
            .map(|i| {
                let angle = std::f64::consts::TAU * ((i * 500) % n) as f64 / n as f64;
                Coord::new(angle.cos(), angle.sin())
            })
            .collect();
        assert_eq!(PolygonMask::new(&star).err().as_deref(), Some(TOO_COMPLEX));
        // Many x-overlapping, y-disjoint edges bound examination work independently of crossings.
        let mut sparse: Vec<_> = (0..3000).map(|i| Coord::new((i % 2) as f64, i as f64)).collect();
        sparse.push(sparse[0]);
        assert_eq!(bound_normalization(&sparse).err().as_deref(), Some(TOO_COMPLEX));
        let detailed: Vec<_> = (0..12_000)
            .map(|i| {
                let angle = std::f64::consts::TAU * i as f64 / 12_000.0;
                Coord::new(10.0 * angle.cos(), 10.0 * angle.sin())
            })
            .collect();
        let mask = mask(&detailed);
        assert!(mask.intersects(&Geometry::Point(Coord::new(0.0, 0.0))));
        assert!(!mask.intersects(&Geometry::Point(Coord::new(11.0, 0.0))));
    }
}
