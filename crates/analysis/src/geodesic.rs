//! Geodesic buffers: a distance measured on the Earth, wherever the feature is and however far it
//! spans (#394).
//!
//! A feature is buffered in an azimuthal equidistant projection centred on it, where distances
//! from the centre are true and the error elsewhere grows with the square of the distance from it.
//! So a small feature is buffered in one piece, and a long line or a large polygon's outline in
//! stretches of a few hundred kilometres, each in its own projection, and the pieces are joined.

use geo::{
    BooleanOps, Buffer, Contains, Coord as GC, LineString, MultiLineString, MultiPolygon, Orient, Point, Polygon, orient::Direction,
};
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
/// At most this many vertices are added to one line or ring (four times round the Earth at 10 km).
const MAX_ADDED: f64 = 16_000.0;

/// A feature's buffer at `metres` (negative shrinks a polygon), in lon/lat; `None` when nothing
/// is left.
pub fn geodesic_buffer(g: &Geometry, metres: f64) -> Option<Geometry> {
    if !metres.is_finite() {
        return None;
    }
    let mp = match g {
        Geometry::Point(c) => point(*c, metres),
        Geometry::MultiPoint(v) => union(v.iter().map(|c| point(*c, metres)).collect()),
        Geometry::Line(parts) => lines(&finite(parts), metres),
        Geometry::Polygon(polys) => polygons(&polys.iter().map(|rings| finite(rings)).collect::<Vec<_>>(), metres),
    };
    to_geometry(&mp)
}

/// An azimuthal equidistant plane whose centre's longitude is within 180° of `reference`, a
/// longitude of the coordinates it buffers as written, so the buffer comes back in lon/lat next to
/// them and to the polygon it may be joined to, even at the antimeridian.
struct Plane(Aeqd);

impl Plane {
    fn new(centre: Coord, reference: f64) -> Self {
        let lon = reference + wrap(centre.x - reference);
        Plane(Aeqd::new(Coord::new(if lon.is_finite() { lon } else { centre.x }, centre.y)))
    }

    fn line(&self, pts: &[Coord]) -> LineString<f64> {
        LineString::new(pts.iter().map(|c| self.0.forward(*c)).map(|p| GC { x: p.x, y: p.y }).collect())
    }

    /// A buffer drawn in the plane, in lon/lat degrees.
    fn back(&self, mp: &MultiPolygon<f64>) -> MultiPolygon<f64> {
        MultiPolygon::new(
            mp.0.iter().map(|p| Polygon::new(self.ring(p.exterior()), p.interiors().iter().map(|r| self.ring(r)).collect())).collect(),
        )
    }

    /// A ring drawn in the plane, in lon/lat degrees.
    fn ring(&self, r: &LineString<f64>) -> LineString<f64> {
        let lonlat: Vec<Coord> = r.0.iter().map(|p| self.0.inverse(Coord::new(p.x, p.y))).collect();
        let around = |lat: f64| {
            let pole = self.0.forward(Coord::new(0.0, lat));
            Polygon::new(r.clone(), vec![]).contains(&Point::new(pole.x, pole.y))
        };
        if around(90.0) {
            return LineString::new(self.capped(r, &lonlat, 90.0));
        }
        if around(-90.0) {
            return LineString::new(self.capped(r, &lonlat, -90.0));
        }
        let mut out: Vec<GC<f64>> = Vec::with_capacity(lonlat.len());
        for c in lonlat {
            // Each longitude within 180° of the one before, so the ring doesn't jump at ±180°.
            let x = out.last().map_or(c.x, |prev| prev.x + wrap(c.x - prev.x));
            out.push(GC { x: if x.is_finite() { x } else { c.x }, y: c.y });
        }
        LineString::new(out)
    }

    /// A ring round a pole, which goes once round the globe in longitude, as a lon/lat ring: cut at
    /// the antimeridian and closed along the pole, so in lon/lat it holds the cap between the ring and
    /// the top (or bottom) of the map.
    fn capped(&self, planar: &LineString<f64>, ring: &[Coord], pole: f64) -> Vec<GC<f64>> {
        let mut v: Vec<Coord> = ring.iter().map(|c| Coord::new(wrap(c.x), c.y)).collect();
        if v.len() > 1 && v.first() == v.last() {
            v.pop();
        }
        let n = v.len();
        // The edge that crosses the antimeridian: its ends are more than 180° apart.
        let seam = (0..n).find(|k| match (v.get((k + n - 1) % n), v.get(*k)) {
            (Some(a), Some(b)) => (b.x - a.x).abs() > 180.0,
            _ => false,
        });
        let found = seam.and_then(|k| Some((k, v.get((k + n - 1) % n)?, planar.0.get((k + n - 1) % n)?, planar.0.get(k)?)));
        let Some((k, a, pa, pb)) = found else {
            return ring.iter().map(|c| GC { x: c.x, y: c.y }).collect();
        };
        // Where the edge from `a` meets ±180° (`edge`, on `a`'s side), found along the edge in the
        // plane, where it's straight.
        let edge = if a.x > 0.0 { 180.0 } else { -180.0 };
        let at = |t: f64| self.0.inverse(Coord::new(pa.x + t * (pb.x - pa.x), pa.y + t * (pb.y - pa.y)));
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..50 {
            let mid = (lo + hi) / 2.0;
            // Before the crossing a longitude is short of `edge`, after it past (wrapped round).
            if wrap(at(mid).x - edge) * edge < 0.0 { lo = mid } else { hi = mid }
        }
        let y = at((lo + hi) / 2.0).y;
        let mut out = vec![GC { x: -edge, y }];
        out.extend((0..n).filter_map(|i| v.get((k + i) % n)).map(|c| GC { x: c.x, y: c.y }));
        out.extend([GC { x: edge, y }, GC { x: edge, y: pole }, GC { x: -edge, y: pole }, GC { x: -edge, y }]);
        out
    }
}

/// A longitude difference folded into −180..180.
fn wrap(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

fn point(c: Coord, metres: f64) -> MultiPolygon<f64> {
    if !c.is_finite() {
        return MultiPolygon::new(vec![]);
    }
    let plane = Plane::new(c, c.x);
    plane.back(&Point::new(0.0, 0.0).buffer(metres))
}

fn lines(parts: &[Vec<Coord>], metres: f64) -> MultiPolygon<f64> {
    let all: Vec<Coord> = parts.iter().flatten().copied().collect();
    let Some(centre) = spherical_centre(&all) else { return MultiPolygon::new(vec![]) };
    if fits(&all, centre) {
        let plane = Plane::new(centre, all.first().map_or(centre.x, |c| c.x));
        return plane.back(&MultiLineString::new(parts.iter().map(|p| plane.line(p)).collect()).buffer(metres));
    }
    union(stretched(parts, metres))
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
        let plane = Plane::new(centre, all.first().map_or(centre.x, |c| c.x));
        return plane.back(&shape(&|r| plane.line(r)).buffer(metres));
    }
    // A buffer distributes over a union: the polygon's buffer is the polygon with its outline's
    // buffer added (or, inward, taken away). The polygon stays in lon/lat, along the same densified
    // edges as the outline's pieces.
    let lonlat = shape(&|r| LineString::new(densify(r).iter().map(|c| GC { x: c.x, y: c.y }).collect()));
    let rings: Vec<Vec<Coord>> = polys.iter().flatten().cloned().collect();
    let outline = union(stretched(&rings, metres.abs()));
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
fn stretched(parts: &[Vec<Coord>], metres: f64) -> Vec<MultiPolygon<f64>> {
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
            if let (Some(centre), Some(first)) = (spherical_centre(stretch), stretch.first()) {
                let plane = Plane::new(centre, first.x);
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

/// A line with every edge split along its great circle into pieces of at most [`DENSIFY_M`], or
/// longer ones for a line so long it would gain more than [`MAX_ADDED`] vertices.
fn densify(pts: &[Coord]) -> Vec<Coord> {
    let step = DENSIFY_M.max(rustgis_geom::geodesic_length_m(pts) / MAX_ADDED);
    let mut out: Vec<Coord> = pts.first().copied().into_iter().collect();
    for w in pts.windows(2) {
        let [a, b] = w else { continue };
        let n = (haversine_m(*a, *b) / step).ceil().clamp(1.0, MAX_ADDED) as usize;
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
