//! Source-addressed candidates for the one feature whose node is being moved.
//! Rendered parts cannot supply identities: world-projection cuts can add parts and vertices.

use rustgis_data::{Geometry, VertexId};
use rustgis_geom::Coord;
use rustgis_map::{design::Snapping, projection::Flat};

/// The same projection, switches and map-coordinate pointer apply to every source path.
#[derive(Clone, Copy)]
struct SnapView<'a> {
    flat: &'a Flat,
    snapping: Snapping,
    pointer: Coord,
}

/// Visits eligible line/polygon paths in source order, without an index or whole-feature clone.
/// Other geometry kinds retain their previous whole-feature exclusion during a node move.
pub(super) fn consider(
    geometry: &Geometry,
    excluded: VertexId,
    flat: &Flat,
    snapping: Snapping,
    pointer: Coord,
    visit: &mut impl FnMut(f64, Coord, &'static str),
) {
    let view = SnapView { flat, snapping, pointer };
    let mut path = |points: &[Coord], part, ring, polygon| {
        consider_path(points, VertexId::new(part, ring, 0), polygon, excluded, view, visit);
    };
    match geometry {
        Geometry::Line(parts) => {
            for (part, points) in parts.iter().enumerate() {
                path(points, part, 0, false);
            }
        }
        Geometry::Polygon(polygons) => {
            for (part, rings) in polygons.iter().enumerate() {
                for (ring, points) in rings.iter().enumerate() {
                    path(points, part, ring, true);
                }
            }
        }
        _ => {}
    }
}

/// Excludes source endpoints before projection, preserving endpoint-before-vertex-before-edge
/// visitation. Only a polygon's actual repeated closing coordinate aliases its first node.
fn consider_path(
    path: &[Coord],
    address: VertexId,
    polygon: bool,
    excluded: VertexId,
    view: SnapView<'_>,
    visit: &mut impl FnMut(f64, Coord, &'static str),
) {
    let SnapView { flat, snapping, pointer } = view;
    let closed = polygon && path.len() > 1 && path.first() == path.last();
    let count = path.len().saturating_sub(usize::from(closed));
    let skip = |index| {
        address.part == excluded.part && address.ring == excluded.ring && excluded.index == if closed && index == count { 0 } else { index }
    };
    let mut point = |index, kind| {
        if !skip(index)
            && let Some(at) = path.get(index).and_then(|at| flat.forward_checked(*at))
        {
            visit((at.x - pointer.x).powi(2) + (at.y - pointer.y).powi(2), at, kind);
        }
    };
    if snapping.end && !polygon {
        for index in [0, path.len().saturating_sub(1)] {
            point(index, "end");
        }
    }
    if snapping.vertex {
        for index in 0..count {
            point(index, "vertex");
        }
    }
    if snapping.edge {
        for (index, pair) in path.windows(2).enumerate() {
            if !skip(index)
                && !skip(index + 1)
                && let (Some(a), Some(b)) = (pair.first(), pair.get(1))
            {
                consider_edge(*a, *b, flat, pointer, visit);
            }
        }
    }
}

/// Projects a surviving source edge using the same world-edge cuts as ordinary snapping.
/// Each projected piece is considered separately; no segment joins opposite sides of the map.
fn consider_edge(a: Coord, b: Coord, flat: &Flat, pointer: Coord, visit: &mut impl FnMut(f64, Coord, &'static str)) {
    let mut segment = |a: Coord, b: Coord| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 { (((pointer.x - a.x) * dx + (pointer.y - a.y) * dy) / length).clamp(0.0, 1.0) } else { 0.0 };
        let at = Coord::new(a.x + t * dx, a.y + t * dy);
        visit((at.x - pointer.x).powi(2) + (at.y - pointer.y).powi(2), at, "edge");
    };
    if matches!(flat, Flat::Custom(projector) if projector.edge().is_some()) {
        if let Geometry::Line(parts) = flat.project(&Geometry::Line(vec![vec![a, b]])) {
            for path in parts {
                for pair in path.windows(2) {
                    if let (Some(a), Some(b)) = (pair.first(), pair.get(1)) {
                        segment(*a, *b);
                    }
                }
            }
        }
    } else if let (Some(a), Some(b)) = (flat.forward_checked(a), flat.forward_checked(b)) {
        segment(a, b);
    }
}
