//! The lines of the map's measured and UTM graticules (#1042), in longitude/latitude for the
//! graticule's drawing to project like its latitudes and longitudes. A measured grid is a layout's
//! measured grid (#1012) over the view; the UTM one draws the zones and latitude bands, and in each
//! zone in view its own eastings and northings.

use rustgis_geom::utm::{UTM_MAX_LAT, UTM_MIN_LAT, band_latitudes, zone_longitudes};
use rustgis_geom::{Coord, Extent};
use rustgis_map::Map;
use rustgis_map::globe::Globe;
use rustgis_map::graticule::GraticuleType;
use rustgis_map::layout::{Grid, GridKind};

use crate::View;
use crate::layout::{measured_crs, measured_lines, nice_step};

/// The latitude bands, south to north (C to X, without I and O).
const BANDS: &str = "CDEFGHJKLMNPQRSTUVWX";

/// The most zone-and-band cells in view whose eastings and northings are drawn: past them (the
/// map zoomed out over many zones) only the zones and bands are.
const MAX_GRIDDED_CELLS: usize = 8;

/// The points a meridian or a parallel is drawn with, per degree.
const PER_DEGREE: f64 = 2.0;

/// The graticule's lines in longitude/latitude for a measured or UTM graticule; `None` for
/// latitudes and longitudes, which the graticule draws itself.
pub(crate) fn lines(map: &Map, view: &View) -> Option<Vec<Vec<Coord>>> {
    if map.graticule_type == GraticuleType::Degrees {
        return None;
    }
    let globe = map.projection.is_globe().then(|| Globe::new(&view.camera, view.width, view.height));
    let seen = in_view(map, view, globe.as_ref());
    Some(match map.graticule_type {
        GraticuleType::Degrees => Vec::new(),
        GraticuleType::Measured => measured(map, view, globe.as_ref(), &seen),
        GraticuleType::Utm => utm(&seen),
    })
}

/// The place at a point of the view (in points), in longitude/latitude: through the globe when the
/// map is one, else through the map's flat projection.
fn at(map: &Map, view: &View, globe: Option<&Globe>, sx: f64, sy: f64) -> Option<Coord> {
    let c = match globe {
        Some(g) => g.inverse(sx, sy),
        None => map.to_lonlat(view.camera.to_map(sx, sy, view.width, view.height)),
    };
    c.filter(|c| c.is_finite())
}

/// Places in view, in longitude/latitude: a grid of points over the view (on a globe, those that
/// fall on it), so a view whose corners miss the globe still has its extent.
fn in_view(map: &Map, view: &View, globe: Option<&Globe>) -> Vec<Coord> {
    let (w, h) = (view.width, view.height);
    let n = 16;
    let mut out = Vec::with_capacity((n + 1) * (n + 1));
    for i in 0..=n {
        for j in 0..=n {
            out.extend(at(map, view, globe, w * i as f64 / n as f64, h * j as f64 / n as f64));
        }
    }
    out
}

/// A measured grid over the view: the layout's measured grid in the map's graticule system.
fn measured(map: &Map, view: &View, globe: Option<&Globe>, seen: &[Coord]) -> Vec<Vec<Coord>> {
    let mut ext = Extent::EMPTY;
    for c in seen {
        ext.expand(*c);
    }
    if ext.is_empty() {
        return Vec::new();
    }
    let center = at(map, view, globe, view.width / 2.0, view.height / 2.0).unwrap_or_else(|| ext.center());
    let g = Grid { kind: GridKind::Measured, crs: map.graticule_crs.clone(), labels: false, ..Grid::default() };
    let Some(tr) = measured_crs(&g, map, center).and_then(|c| c.transformer().ok()) else { return Vec::new() };
    let mut lines = Vec::new();
    measured_lines(&g, &tr, seen, false, &mut lines);
    lines.into_iter().map(|(pts, _, _)| pts).filter(|l| l.len() >= 2).collect()
}

/// The UTM zones and latitude bands in view, and in each of a few zones in view its eastings and
/// northings, one step apart in all of them.
fn utm(seen: &[Coord]) -> Vec<Vec<Coord>> {
    let mut ext = Extent::EMPTY;
    for c in seen {
        ext.expand(*c);
    }
    if ext.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    // Each zone's western edge within each band (the eastern one is the next zone's western), so
    // the Norway and Svalbard zones are drawn as they are.
    let mut cells = Vec::new();
    for band in BANDS.chars() {
        let Some((south, north)) = band_latitudes(band) else { continue };
        for zone in 1..=60u8 {
            let Some((west, east)) = zone_longitudes(zone, band) else { continue };
            cells.push((zone, band, Extent { min_x: west, min_y: south, max_x: east, max_y: north }));
            if ext.min_x <= west && west <= ext.max_x && ext.min_y <= north && south <= ext.max_y {
                lines.push(meridian(west, south, north));
            }
        }
    }
    // The bands' edges, and the zones' southern and northern limits.
    for band in BANDS.chars() {
        let Some((south, _)) = band_latitudes(band) else { continue };
        if ext.min_y <= south && south <= ext.max_y {
            lines.push(parallel(south, ext.min_x.max(-180.0), ext.max_x.min(180.0)));
        }
    }
    if ext.min_y <= UTM_MAX_LAT && UTM_MAX_LAT <= ext.max_y {
        lines.push(parallel(UTM_MAX_LAT, ext.min_x.max(-180.0), ext.max_x.min(180.0)));
    }
    // Eastings and northings in each zone in view, when few are.
    let shown: Vec<_> = cells.into_iter().filter(|(_, _, c)| overlaps(c, &ext)).collect();
    if shown.is_empty() || shown.len() > MAX_GRIDDED_CELLS {
        return lines;
    }
    // One step for every zone, from the view's span in the zone at its middle.
    let mid = ext.center();
    let Some(step) = zone_transformer(mid).and_then(|tr| {
        let mut m = Extent::EMPTY;
        for c in seen.iter().filter_map(|c| tr.from_lonlat(*c)) {
            m.expand(c);
        }
        (!m.is_empty()).then(|| nice_step(m.width().max(m.height()), rustgis_map::layout::MIN_MEASURED_STEP))
    }) else {
        return lines;
    };
    for (zone, band, cell) in shown {
        let north = band >= 'N';
        let epsg = if north { 32_600 } else { 32_700 } + u32::from(zone);
        let Some(tr) = rustgis_data::raster::Crs::from_epsg(epsg).and_then(|c| c.transformer().ok()) else { continue };
        // The part of the cell in view, its edges densified, as the frame the grid covers.
        let part = Extent {
            min_x: cell.min_x.max(ext.min_x),
            min_y: cell.min_y.max(ext.min_y),
            max_x: cell.max_x.min(ext.max_x),
            max_y: cell.max_y.min(ext.max_y),
        };
        let frame: Vec<Coord> = (0..=8)
            .flat_map(|i| {
                let t = f64::from(i) / 8.0;
                let (x, y) = (part.min_x + part.width() * t, part.min_y + part.height() * t);
                [Coord::new(x, part.min_y), Coord::new(x, part.max_y), Coord::new(part.min_x, y), Coord::new(part.max_x, y)]
            })
            .collect();
        let g = Grid { kind: GridKind::Measured, step, labels: false, ..Grid::default() };
        let mut zone_lines = Vec::new();
        measured_lines(&g, &tr, &frame, false, &mut zone_lines);
        for (pts, _, _) in zone_lines {
            lines.extend(clip(&pts, &cell));
        }
    }
    lines
}

/// The transformer of the UTM zone at `c` (by its 6° zone and hemisphere).
fn zone_transformer(c: Coord) -> Option<rustgis_data::raster::Transformer> {
    let epsg = rustgis_map::layout::utm_epsg(c.x, c.y.clamp(UTM_MIN_LAT, UTM_MAX_LAT - 1e-9));
    rustgis_data::raster::Crs::from_epsg(epsg)?.transformer().ok()
}

fn overlaps(a: &Extent, b: &Extent) -> bool {
    a.min_x < b.max_x && b.min_x < a.max_x && a.min_y < b.max_y && b.min_y < a.max_y
}

fn meridian(lon: f64, south: f64, north: f64) -> Vec<Coord> {
    let n = (((north - south) * PER_DEGREE).ceil() as usize).max(1);
    (0..=n).map(|i| Coord::new(lon, south + (north - south) * i as f64 / n as f64)).collect()
}

fn parallel(lat: f64, west: f64, east: f64) -> Vec<Coord> {
    let n = (((east - west) * PER_DEGREE).ceil() as usize).max(1);
    (0..=n).map(|i| Coord::new(west + (east - west) * i as f64 / n as f64, lat)).collect()
}

/// The runs of a polyline within a box (longitude/latitude), each segment cut at the box's edges.
fn clip(pts: &[Coord], b: &Extent) -> Vec<Vec<Coord>> {
    let mut out = Vec::new();
    let mut run: Vec<Coord> = Vec::new();
    for pair in pts.windows(2) {
        let &[p, q] = pair else { continue };
        match clip_segment(p, q, b) {
            Some((a, z)) => {
                if run.last().is_none_or(|l| (l.x - a.x).abs() > 1e-12 || (l.y - a.y).abs() > 1e-12) {
                    if run.len() >= 2 {
                        out.push(std::mem::take(&mut run));
                    }
                    run.clear();
                    run.push(a);
                }
                run.push(z);
            }
            None => {
                if run.len() >= 2 {
                    out.push(std::mem::take(&mut run));
                }
                run.clear();
            }
        }
    }
    if run.len() >= 2 {
        out.push(run);
    }
    out
}

/// A segment's part within a box (Liang–Barsky); `None` when none of it is.
fn clip_segment(p: Coord, q: Coord, b: &Extent) -> Option<(Coord, Coord)> {
    let (dx, dy) = (q.x - p.x, q.y - p.y);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (den, num) in [(-dx, p.x - b.min_x), (dx, b.max_x - p.x), (-dy, p.y - b.min_y), (dy, b.max_y - p.y)] {
        if den == 0.0 {
            if num < 0.0 {
                return None;
            }
        } else {
            let t = num / den;
            if den < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((Coord::new(p.x + t0 * dx, p.y + t0 * dy), Coord::new(p.x + t1 * dx, p.y + t1 * dy)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(w: f64, s: f64, e: f64, n: f64) -> Extent {
        Extent { min_x: w, min_y: s, max_x: e, max_y: n }
    }

    /// A view of Winterthur, 10 m a point, on a map in `projection`.
    fn winterthur(projection: &str) -> (Map, View) {
        let mut m = Map::new("m");
        m.projection = rustgis_map::Projection::Custom(projection.into());
        let center = m.to_map(Coord::new(8.73, 47.5)).unwrap();
        let view =
            View { width: 800.0, height: 600.0, pixels_per_point: 1.0, camera: rustgis_map::camera::Camera::new(center, 10.0), sky: false };
        (m, view)
    }

    /// Whether every line keeps one easting or one northing in `epsg`, and some of each.
    fn straight_in(lines: &[Vec<Coord>], epsg: u32) -> bool {
        let tr = rustgis_data::raster::Crs::from_epsg(epsg).unwrap().transformer().unwrap();
        let (mut eastings, mut northings) = (0, 0);
        for l in lines {
            let pts: Vec<Coord> = l.iter().map(|c| tr.from_lonlat(*c).unwrap()).collect();
            if pts.iter().all(|p| (p.x - pts[0].x).abs() < 0.01) {
                eastings += 1;
            } else if pts.iter().all(|p| (p.y - pts[0].y).abs() < 0.01) {
                northings += 1;
            } else {
                return false;
            }
        }
        eastings > 2 && northings > 2
    }

    /// A measured graticule on a map in LV95 draws LV95's eastings and northings; given a system,
    /// that one's; latitudes and longitudes are the graticule's own.
    #[test]
    fn a_measured_graticule_draws_the_maps_own_system_or_the_one_given() {
        let (mut m, view) = winterthur("EPSG:2056");
        assert!(lines(&m, &view).is_none(), "latitudes and longitudes");
        m.graticule_type = GraticuleType::Measured;
        let own = lines(&m, &view).unwrap();
        assert!(straight_in(&own, 2056), "{own:?}");
        m.graticule_crs = Some("EPSG:32632".into());
        let utm = lines(&m, &view).unwrap();
        assert!(straight_in(&utm, 32632) && !straight_in(&utm, 2056));
        // On a Web Mercator map with no system given: the UTM zone at the view's centre (32N).
        let (mut m, _) = winterthur("EPSG:3857");
        m.projection = rustgis_map::Projection::WebMercator;
        let center = m.to_map(Coord::new(8.73, 47.5)).unwrap();
        let view = View { camera: rustgis_map::camera::Camera::new(center, 10.0), ..view };
        m.graticule_type = GraticuleType::Measured;
        assert!(straight_in(&lines(&m, &view).unwrap(), 32632));
    }

    #[test]
    fn a_polyline_is_cut_at_a_box() {
        let b = bx(0.0, 0.0, 10.0, 10.0);
        // Across the box: the part inside.
        let runs = clip(&[Coord::new(-5.0, 5.0), Coord::new(15.0, 5.0)], &b);
        assert_eq!(runs, vec![vec![Coord::new(0.0, 5.0), Coord::new(10.0, 5.0)]]);
        // Out, in, out and in again: two runs.
        let runs = clip(&[Coord::new(5.0, -5.0), Coord::new(5.0, 5.0), Coord::new(5.0, 15.0), Coord::new(6.0, 5.0)], &b);
        assert_eq!(runs.len(), 2, "{runs:?}");
        // Wholly outside: nothing.
        assert!(clip(&[Coord::new(20.0, 20.0), Coord::new(30.0, 30.0)], &b).is_empty());
    }

    /// The UTM graticule over Scandinavia draws the zones' edges as they are there: southern
    /// Norway's 32V from 3° E to 12° E (no line at 6° E in band V), and in Svalbard's band X the
    /// edges at 9°, 21° and 33° E.
    #[test]
    fn utm_zones_keep_the_norway_and_svalbard_exceptions() {
        let seen: Vec<Coord> = (0..=20)
            .flat_map(|i| (0..=20).map(move |j| Coord::new(-2.0 + 46.0 * f64::from(i) / 20.0, 54.0 + 30.0 * f64::from(j) / 20.0)))
            .collect();
        let lines = utm(&seen);
        // Meridians: a line whose points share a longitude, from its southern to northern end.
        let meridian_at = |lon: f64, lat: f64| {
            lines.iter().any(|l| l.len() >= 2 && l.iter().all(|c| (c.x - lon).abs() < 1e-9) && l[0].y <= lat && lat <= l[l.len() - 1].y)
        };
        // Band V (56° to 64° N).
        assert!(meridian_at(3.0, 60.0) && meridian_at(12.0, 60.0) && !meridian_at(6.0, 60.0));
        // Band W (64° to 72° N): the ordinary 6° zones.
        assert!(meridian_at(6.0, 68.0) && meridian_at(12.0, 68.0));
        // Band X (72° to 84° N).
        assert!(meridian_at(9.0, 78.0) && meridian_at(21.0, 78.0) && meridian_at(33.0, 78.0));
        assert!(!meridian_at(12.0, 78.0) && !meridian_at(18.0, 78.0));
        // Many zones in view: zones and bands only, no eastings or northings.
        assert!(lines.iter().all(|l| l.iter().all(|c| (c.x - l[0].x).abs() < 1e-9) || l.iter().all(|c| (c.y - l[0].y).abs() < 1e-9)));
    }

    /// Zoomed in on the edge between zones 32T (6° to 12° E) and 33T (12° to 18° E), near
    /// Salzburg: each zone's grid stays within it, and both are drawn.
    #[test]
    fn a_zones_grid_stays_within_it() {
        // Around 12° E, 47.5° N: zones 32T (6°–12° E) and 33T (12°–18° E).
        let seen: Vec<Coord> = (0..=10)
            .flat_map(|i| (0..=10).map(move |j| Coord::new(11.6 + 0.8 * f64::from(i) / 10.0, 47.3 + 0.4 * f64::from(j) / 10.0)))
            .collect();
        let lines = utm(&seen);
        let inner: Vec<_> = lines.iter().filter(|l| !(l.iter().all(|c| (c.x - l[0].x).abs() < 1e-9))).collect();
        assert!(inner.len() > 4, "eastings and northings drawn: {}", inner.len());
        // No line of either zone's grid crosses 12° E.
        for l in &inner {
            let west = l.iter().all(|c| c.x <= 12.0 + 1e-9);
            let east = l.iter().all(|c| c.x >= 12.0 - 1e-9);
            assert!(west || east, "a line across the zones' edge: {l:?}");
        }
        // Lines on both sides.
        assert!(inner.iter().any(|l| l.iter().all(|c| c.x < 12.0)) && inner.iter().any(|l| l.iter().all(|c| c.x > 12.0)));
    }
}
