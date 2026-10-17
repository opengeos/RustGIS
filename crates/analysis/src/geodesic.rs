//! Geodesic buffers: a distance measured on the Earth, wherever the feature is and however far it
//! spans (#394).
//!
//! A feature is buffered in an azimuthal equidistant projection centred on it, where distances
//! from the centre are true and the error elsewhere grows with the square of the distance from it.
//! So a small feature is buffered in one piece, and a long line or a large polygon's outline in
//! stretches of a few hundred kilometres, each in its own projection, and the pieces are joined.

use geo::{BooleanOps, Buffer, Coord as GC, LineString, MultiLineString, MultiPolygon, Orient, Point, Polygon, orient::Direction};
use rustgis_data::{Coord, Geometry};
use rustgis_geom::{Aeqd, geodesic_interpolate, haversine_m, spherical_centre};

/// A feature within this distance of its centre is buffered in one projection (the error is
/// under 0.01%).
const ONE_PIECE_M: f64 = 150_000.0;
/// A stretch stays within this distance of its first vertex (the error is under 0.05%).
const STRETCH_M: f64 = 300_000.0;
/// Edges are split to this length before buffering in stretches, so the pieces and the polygon
/// they join meet along the same path.
const DENSIFY_M: f64 = 10_000.0;

/// A feature's buffer at `metres` (negative shrinks a polygon), in lon/lat; `None` when nothing
/// is left.
pub fn geodesic_buffer(g: &Geometry, metres: f64) -> Option<Geometry> {
    if !metres.is_finite() {
        return None;
    }
    let mp = match g {
        Geometry::Point(c) => point(*c, metres, c.x),
        Geometry::MultiPoint(v) => {
            let reference = spherical_centre(v)?.x;
            union(v.iter().filter(|c| c.is_finite()).map(|c| point(*c, metres, reference)).collect())
        }
        Geometry::Line(parts) => lines(&finite(parts), metres),
        Geometry::Polygon(polys) => polygons(&polys.iter().map(|rings| finite(rings)).collect::<Vec<_>>(), metres),
    };
    to_geometry(&mp)
}

/// An azimuthal equidistant plane whose centre's longitude is within 180° of `reference`, so the
/// pieces of one feature come back in lon/lat next to each other, even across the antimeridian.
struct Plane(Aeqd);

impl Plane {
    fn new(centre: Coord, reference: f64) -> Self {
        let lon = reference + (centre.x - reference + 180.0).rem_euclid(360.0) - 180.0;
        Plane(Aeqd::new(Coord::new(if lon.is_finite() { lon } else { centre.x }, centre.y)))
    }

    fn line(&self, pts: &[Coord]) -> LineString<f64> {
        LineString::new(pts.iter().map(|c| self.0.forward(*c)).map(|p| GC { x: p.x, y: p.y }).collect())
    }

    /// A buffer drawn in the plane, in lon/lat degrees.
    fn back(&self, mp: &MultiPolygon<f64>) -> MultiPolygon<f64> {
        let ring = |r: &LineString<f64>| {
            LineString::new(r.0.iter().map(|p| self.0.inverse(Coord::new(p.x, p.y))).map(|c| GC { x: c.x, y: c.y }).collect())
        };
        MultiPolygon::new(mp.0.iter().map(|p| Polygon::new(ring(p.exterior()), p.interiors().iter().map(ring).collect())).collect())
    }
}

fn point(c: Coord, metres: f64, reference: f64) -> MultiPolygon<f64> {
    if !c.is_finite() {
        return MultiPolygon::new(vec![]);
    }
    let plane = Plane::new(c, reference);
    plane.back(&Point::new(0.0, 0.0).buffer(metres))
}

fn lines(parts: &[Vec<Coord>], metres: f64) -> MultiPolygon<f64> {
    let all: Vec<Coord> = parts.iter().flatten().copied().collect();
    let Some(centre) = spherical_centre(&all) else { return MultiPolygon::new(vec![]) };
    if fits(&all, centre) {
        let plane = Plane::new(centre, centre.x);
        return plane.back(&MultiLineString::new(parts.iter().map(|p| plane.line(p)).collect()).buffer(metres));
    }
    union(stretched(parts, metres, centre.x))
}

fn polygons(polys: &[Vec<Vec<Coord>>], metres: f64) -> MultiPolygon<f64> {
    let all: Vec<Coord> = polys.iter().flatten().flatten().copied().collect();
    let Some(centre) = spherical_centre(&all) else { return MultiPolygon::new(vec![]) };
    let shape = |ring: &dyn Fn(&[Coord]) -> LineString<f64>| {
        MultiPolygon::new(
            polys
                .iter()
                .filter_map(|rings| {
                    let mut it = rings.iter();
                    let outer = ring(it.next()?);
                    Some(Polygon::new(outer, it.map(|r| ring(r)).collect()).orient(Direction::Default))
                })
                .collect(),
        )
    };
    if fits(&all, centre) {
        let plane = Plane::new(centre, centre.x);
        return plane.back(&shape(&|r| plane.line(r)).buffer(metres));
    }
    // A buffer distributes over a union: the polygon's buffer is the polygon with its outline's
    // buffer added (or, inward, taken away). The polygon stays in lon/lat, along the same densified
    // edges as the outline's pieces.
    let lonlat = shape(&|r| LineString::new(densify(r).iter().map(|c| GC { x: c.x, y: c.y }).collect()));
    let rings: Vec<Vec<Coord>> = polys.iter().flatten().cloned().collect();
    let outline = union(stretched(&rings, metres.abs(), centre.x));
    if metres >= 0.0 { lonlat.union(&outline) } else { lonlat.difference(&outline) }
}

/// Lines and rings without their non-finite coordinates.
fn finite(parts: &[Vec<Coord>]) -> Vec<Vec<Coord>> {
    parts.iter().map(|p| p.iter().copied().filter(|c| c.is_finite()).collect::<Vec<_>>()).filter(|p| !p.is_empty()).collect()
}

fn fits(pts: &[Coord], centre: Coord) -> bool {
    pts.iter().all(|c| haversine_m(centre, *c) <= ONE_PIECE_M)
}

/// The buffers of lines cut into stretches, each buffered in its own plane, in lon/lat.
fn stretched(parts: &[Vec<Coord>], metres: f64, reference: f64) -> Vec<MultiPolygon<f64>> {
    let mut out = Vec::new();
    for part in parts {
        let d = densify(part);
        let near = |a: usize, b: usize| d.get(a).zip(d.get(b)).is_some_and(|(a, b)| haversine_m(*a, *b) <= STRETCH_M);
        let mut start = 0;
        loop {
            let mut end = start + 1;
            while end + 1 < d.len() && near(start, end + 1) {
                end += 1;
            }
            let Some(stretch) = d.get(start..=end.min(d.len().saturating_sub(1))) else { break };
            if let Some(centre) = spherical_centre(stretch) {
                let plane = Plane::new(centre, reference);
                out.push(plane.back(&plane.line(stretch).buffer(metres)));
            }
            // Consecutive stretches share a vertex, so their buffers overlap.
            if end + 1 >= d.len() {
                break;
            }
            start = end;
        }
    }
    out
}

/// A line with every edge split along its great circle into pieces of at most [`DENSIFY_M`].
fn densify(pts: &[Coord]) -> Vec<Coord> {
    let mut out: Vec<Coord> = pts.first().copied().into_iter().collect();
    for w in pts.windows(2) {
        let [a, b] = w else { continue };
        // At most 4000 pieces (the length of a half great circle at 10 km).
        let n = (haversine_m(*a, *b) / DENSIFY_M).ceil().clamp(1.0, 4000.0) as usize;
        out.extend((1..n).map(|i| geodesic_interpolate(*a, *b, i as f64 / n as f64)));
        out.push(*b);
    }
    out
}

fn union(pieces: Vec<MultiPolygon<f64>>) -> MultiPolygon<f64> {
    if pieces.len() == 1 {
        return pieces.into_iter().next().unwrap_or_else(|| MultiPolygon::new(vec![]));
    }
    let oriented: Vec<Polygon<f64>> = pieces.iter().flat_map(|m| m.0.iter()).map(|p| p.orient(Direction::Default)).collect();
    geo::unary_union(oriented.iter())
}

fn to_geometry(mp: &MultiPolygon<f64>) -> Option<Geometry> {
    let ring = |r: &LineString<f64>| r.0.iter().map(|c| Coord::new(c.x, c.y)).collect::<Vec<_>>();
    let polys: Vec<Vec<Vec<Coord>>> =
        mp.0.iter().map(|p| std::iter::once(p.exterior()).chain(p.interiors().iter()).map(ring).collect()).collect();
    (!polys.is_empty()).then_some(Geometry::Polygon(polys))
}
