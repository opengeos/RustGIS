//! Vector tiles as meshes: a tile's shapes (see `rustgis_tiles::shapes`) tessellated once and
//! drawn at every zoom of their level, as MapLibre draws its tiles, rather than drawn into a
//! picture again for every zoom and resolution.
//!
//! Strokes are tessellated at width 1 and their vertices moved by the vertex shader along lyon's
//! normals, so one mesh serves every width; circles are fans whose rims the shader moves out the
//! same way. A mesh holds a run of indices per style layer (merged with the next when it shows at
//! the same zooms), for the renderer to leave out the layers hidden at its zoom.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use lyon::path::Path;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, LineCap, LineJoin, StrokeOptions, StrokeTessellator, StrokeVertex,
    VertexBuffers,
};
use rustgis_tiles::cache::Payload;
use rustgis_tiles::mvt::Tile;
use rustgis_tiles::shapes::{DataTile, Paint, Run, shapes};
use rustgis_tiles::source::Resolved;
use rustgis_tiles::style;
use rustgis_tiles::template::TileId;

use crate::gpu_scene::TileVertex;

/// Round caps and joins, in the tessellation's units (a half width of 0.5): about 16 segments
/// a circle, smooth at the widths roads are drawn at.
const TOLERANCE: f32 = 0.02;

/// Segments of a circle's rim.
const RIM: usize = 16;

/// The most mesh memory kept, bytes.
const BUDGET: usize = if cfg!(target_arch = "wasm32") { 128 << 20 } else { 512 << 20 };

/// A tile's shapes as one mesh.
#[derive(Debug, Default)]
pub struct TileMesh {
    pub vertices: Vec<TileVertex>,
    pub indices: Vec<u32>,
    /// Bottom first.
    pub runs: Vec<MeshRun>,
}

/// The indices of one or more style layers showing at the same zooms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshRun {
    pub first: u32,
    pub count: u32,
    /// It shows from `min_zoom` up to `max_zoom` (excluded).
    pub min_zoom: f32,
    pub max_zoom: f32,
}

impl TileMesh {
    /// Its size, bytes.
    pub fn bytes(&self) -> usize {
        std::mem::size_of_val(self.vertices.as_slice()) + std::mem::size_of_val(self.indices.as_slice())
    }

    /// The index ranges showing at `zoom`, bottom first.
    pub fn shown(&self, zoom: f32) -> impl Iterator<Item = std::ops::Range<u32>> + '_ {
        self.runs.iter().filter(move |r| zoom >= r.min_zoom && zoom < r.max_zoom).map(|r| r.first..r.first + r.count)
    }
}

/// Straight RGBA, 0..1, premultiplied in bytes.
fn premul(c: [f32; 4]) -> [u8; 4] {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [byte(c[0] * c[3]), byte(c[1] * c[3]), byte(c[2] * c[3]), byte(c[3])]
}

/// A dash pattern as the vertex shader takes it: an odd count repeated once over (as in SVG,
/// and as tiny-skia draws it), at most four lengths; `None` for a longer one (see
/// `rustgis_tiles::shapes::MAX_DASHES`, past which styles are drawn otherwise).
pub fn dash_pattern(d: Option<&[f32]>) -> Option<[f32; 4]> {
    let Some(d) = d.filter(|d| !d.is_empty()) else { return Some([0.0; 4]) };
    let mut v: Vec<f32> = d.to_vec();
    if v.len() % 2 == 1 {
        v.extend_from_slice(d);
    }
    if v.len() > 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (o, x) in out.iter_mut().zip(v) {
        *o = x.max(0.0);
    }
    Some(if out.iter().sum::<f32>() <= 0.0 { [0.0; 4] } else { out })
}

/// Tessellates `runs` of `tile` (a tile at zoom `z`) into one mesh, in map units from the
/// tile's north-west corner. Shapes lyon refuses are left out.
pub fn tessellate(tile: &Tile, runs: &[Run], z: u8) -> TileMesh {
    let span = TileId::span(z) as f32;
    let mut out: VertexBuffers<TileVertex, u32> = VertexBuffers::new();
    let mut mesh_runs: Vec<MeshRun> = Vec::new();
    let mut fill = FillTessellator::new();
    let mut stroke = StrokeTessellator::new();
    for run in runs {
        let k = span / run.extent.max(1) as f32;
        // Tessellated in tile units (small numbers, where a width of 1 keeps its precision; map
        // units at a low zoom run to millions), then put in map units, y north.
        let at = |p: &[f32; 2]| lyon::math::point(p[0], p[1]);
        let map = |x: f32, y: f32| [x * k, -y * k];
        // Indices are u32: a tile past them (none is, but tiles are hostile) stops there.
        let Ok(first) = u32::try_from(out.indices.len()) else { break };
        for shape in &run.shapes {
            let geometry: Option<&[Vec<[f32; 2]>]> =
                shape.feature.and_then(|(l, f)| tile.layers.get(l).and_then(|l| l.features.get(f))).map(|f| f.geometry.as_slice());
            match &shape.paint {
                // A pattern isn't drawn yet: left out, as the CPU path leaves it out without the
                // sprite, rather than drawn in the fill's colour.
                Paint::Fill { pattern: Some(_), .. } => {}
                Paint::Fill { color, .. } => {
                    let (c0, c1) = (premul(color[0]), premul(color[1]));
                    let vertex = |v: FillVertex| {
                        let p = v.position();
                        TileVertex { position: map(p.x, p.y), color: c0, color_next: c1, ..TileVertex::default() }
                    };
                    let mut b = Path::builder();
                    match geometry {
                        Some(rings) => {
                            for ring in rings.iter().filter(|r| r.len() >= 3) {
                                let Some((head, rest)) = ring.split_first() else { continue };
                                b.begin(at(head));
                                for p in rest {
                                    b.line_to(at(p));
                                }
                                b.close();
                            }
                        }
                        // The whole square.
                        None => {
                            let e = run.extent as f32;
                            b.begin(at(&[0.0, 0.0]));
                            b.line_to(at(&[e, 0.0]));
                            b.line_to(at(&[e, e]));
                            b.line_to(at(&[0.0, e]));
                            b.close();
                        }
                    }
                    let options = FillOptions::default().with_fill_rule(FillRule::EvenOdd);
                    let _ = fill.tessellate_path(&b.build(), &options, &mut BuffersBuilder::new(&mut out, vertex));
                }
                Paint::Line { color, width, cap, join, dashes, rings } => {
                    let Some(lines) = geometry else { continue };
                    let base = TileVertex {
                        color: premul(color[0]),
                        color_next: premul(color[1]),
                        width: *width,
                        // Declined styles aside (`shapes::vector_tiles`), a feature's own longer
                        // pattern is drawn solid rather than with the wrong dashes.
                        dashes: dash_pattern(dashes.as_deref()).unwrap_or_default(),
                        ..TileVertex::default()
                    };
                    let options = StrokeOptions::default()
                        .with_line_width(1.0)
                        .with_tolerance(TOLERANCE)
                        .with_line_cap(match cap {
                            style::LineCap::Butt => LineCap::Butt,
                            style::LineCap::Round => LineCap::Round,
                            style::LineCap::Square => LineCap::Square,
                        })
                        .with_line_join(match join {
                            style::LineJoin::Bevel => LineJoin::Bevel,
                            style::LineJoin::Round => LineJoin::Round,
                            style::LineJoin::Miter => LineJoin::Miter,
                        });
                    for line in lines.iter().filter(|l| l.len() >= 2) {
                        let Some((head, rest)) = line.split_first() else { continue };
                        let mut b = Path::builder();
                        b.begin(at(head));
                        for p in rest {
                            b.line_to(at(p));
                        }
                        b.end(*rings);
                        // On the line: lyon's position less half the width it was tessellated at.
                        let vertex = |v: StrokeVertex| {
                            let (p, n) = (v.position(), v.normal());
                            TileVertex {
                                position: map(p.x - n.x * 0.5, p.y - n.y * 0.5),
                                normal: [n.x, -n.y],
                                along: v.advancement() * k,
                                ..base
                            }
                        };
                        let _ = stroke.tessellate_path(&b.build(), &options, &mut BuffersBuilder::new(&mut out, vertex));
                    }
                }
                Paint::Circle { color, radius, stroke: ring } => {
                    let Some(points) = geometry else { continue };
                    let disc = [radius[0] * 2.0, radius[1] * 2.0];
                    for p in points.iter().flatten() {
                        let c = map(p[0], p[1]);
                        circle(&mut out, c, color, disc, disc);
                        if let Some((sc, sw)) = ring {
                            circle(&mut out, c, sc, [disc[0] - sw[0], disc[1] - sw[1]], [disc[0] + sw[0], disc[1] + sw[1]]);
                        }
                    }
                }
            }
        }
        let count = u32::try_from(out.indices.len()).map_or(0, |n| n.saturating_sub(first));
        if count == 0 {
            continue;
        }
        match mesh_runs.last_mut() {
            Some(last)
                if last.first.saturating_add(last.count) == first && last.min_zoom == run.min_zoom && last.max_zoom == run.max_zoom =>
            {
                last.count += count;
            }
            _ => mesh_runs.push(MeshRun { first, count, min_zoom: run.min_zoom, max_zoom: run.max_zoom }),
        }
    }
    TileMesh { vertices: out.vertices, indices: out.indices, runs: mesh_runs }
}

/// A disc (`inner == outer`, a fan about its centre) or a ring between two widths, centred at
/// `c`: its rim vertices sit at the centre, moved out by the shader.
fn circle(out: &mut VertexBuffers<TileVertex, u32>, c: [f32; 2], color: &[[f32; 4]; 2], inner: [f32; 2], outer: [f32; 2]) {
    let base = TileVertex { position: c, color: premul(color[0]), color_next: premul(color[1]), ..TileVertex::default() };
    let at = |i: usize, width: [f32; 2]| {
        let a = i as f32 / RIM as f32 * std::f32::consts::TAU;
        TileVertex { normal: [a.cos(), a.sin()], width: width.map(|w| w.max(0.0)), ..base }
    };
    let Ok(first) = u32::try_from(out.vertices.len()) else { return };
    let ring = inner != outer;
    if ring {
        for i in 0..RIM {
            out.vertices.push(at(i, inner));
            out.vertices.push(at(i, outer));
        }
        for i in 0..RIM as u32 {
            let (a, b) = (first + 2 * i, first + 2 * ((i + 1) % RIM as u32));
            out.indices.extend_from_slice(&[a, a + 1, b + 1, a, b + 1, b]);
        }
    } else {
        out.vertices.push(base);
        for i in 0..RIM {
            out.vertices.push(at(i, outer));
        }
        for i in 0..RIM as u32 {
            out.indices.extend_from_slice(&[first, first + 1 + i, first + 1 + (i + 1) % RIM as u32]);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    /// The tile's payload and the resolved style, by address ([`Entry`] tells whether they are
    /// still the same).
    payload: usize,
    style: usize,
    level: u8,
}

struct Entry {
    payload: Weak<Payload>,
    style: Weak<Resolved>,
    mesh: Arc<TileMesh>,
    used: u64,
}

#[derive(Default)]
struct Store {
    entries: HashMap<Key, Entry>,
    bytes: usize,
    tick: u64,
}

fn lock() -> MutexGuard<'static, Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The meshes of `tiles` (bottom first) for `style`'s `source` at `level`, each kept once made
/// (while its tile and the style are, and within a memory budget); `None` for one that is no
/// vector tile. Natively, those missing are made on several threads.
pub fn meshes(style: &Arc<Resolved>, source: &str, tiles: &[DataTile], level: u8) -> Vec<Option<Arc<TileMesh>>> {
    let key = |t: &DataTile| Key { payload: Arc::as_ptr(&t.payload) as usize, style: Arc::as_ptr(style) as usize, level };
    let mut out: Vec<Option<Arc<TileMesh>>> = Vec::with_capacity(tiles.len());
    let mut missing = Vec::new();
    let (mut seen, mut repeats) = (HashMap::new(), Vec::new());
    // This call's tick: what it uses and makes is spared by its eviction, whatever other calls
    // (another thread's) do meanwhile.
    let tick = {
        let mut s = lock();
        s.tick += 1;
        let tick = s.tick;
        for (i, t) in tiles.iter().enumerate() {
            let found = s.entries.get_mut(&key(t)).filter(|e| {
                e.payload.upgrade().is_some_and(|p| Arc::ptr_eq(&p, &t.payload))
                    && e.style.upgrade().is_some_and(|x| Arc::ptr_eq(&x, style))
            });
            match found {
                Some(e) => {
                    e.used = tick;
                    out.push(Some(e.mesh.clone()));
                }
                None => {
                    // A tile standing in for several (the same payload) is made once.
                    match seen.get(&key(t)) {
                        Some(&first) => repeats.push((i, first)),
                        None => {
                            seen.insert(key(t), i);
                            missing.push(i);
                        }
                    }
                    out.push(None);
                }
            }
        }
        tick
    };
    let make = |i: &usize| -> Option<Arc<TileMesh>> {
        let t = tiles.get(*i)?;
        let Payload::Vector(tile) = &*t.payload else { return None };
        Some(Arc::new(tessellate(tile, &shapes(style, source, tile, level), t.z)))
    };
    let made: Vec<Option<Arc<TileMesh>>> = parallel(&missing, make).into_iter().map(Option::flatten).collect();
    let mut s = lock();
    for (&i, mesh) in missing.iter().zip(made) {
        let (Some(mesh), Some(t)) = (mesh, tiles.get(i)) else { continue };
        let entry = Entry { payload: Arc::downgrade(&t.payload), style: Arc::downgrade(style), mesh: mesh.clone(), used: tick };
        // The bytes are counted again by `evict`, below.
        s.entries.insert(key(t), entry);
        if let Some(slot) = out.get_mut(i) {
            *slot = Some(mesh);
        }
    }
    for (i, first) in repeats {
        let mesh = out.get(first).cloned().flatten();
        if let Some(slot) = out.get_mut(i) {
            *slot = mesh;
        }
    }
    // Every call, so meshes of tiles the cache let go go as the map moves over others.
    evict(&mut s, tick);
    out
}

/// `f` of every item (`None` for one that panicked), natively shared among threads. In the
/// browser a panic aborts (`wasm32-unknown-unknown` unwinds nothing): there a panic is caught
/// nowhere, as everywhere else in the web app.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<Option<R>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(items.len()).min(16);
        if threads > 1 {
            let next = std::sync::atomic::AtomicUsize::new(0);
            let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
            std::thread::scope(|scope| {
                for _ in 0..threads {
                    scope.spawn(|| {
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(item) = items.get(i) else { break };
                            // A tile that panics (hostile data) gives no mesh, rather than the frame.
                            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(item))).ok();
                            if let Some(slot) = out.lock().unwrap_or_else(|e| e.into_inner()).get_mut(i) {
                                *slot = r;
                            }
                        }
                    });
                }
            });
            return out.into_inner().unwrap_or_else(|e| e.into_inner());
        }
    }
    items.iter().map(|i| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(i))).ok()).collect()
}

/// Drops the meshes of tiles and styles gone, and past the budget those unused longest (but
/// never those of the call at `tick`: a view's own meshes may go past the budget, bounded by
/// the size of its tiles).
fn evict(s: &mut Store, tick: u64) {
    // Those of tiles the tile cache let go, and styles gone, go first, budget or not.
    s.entries.retain(|_, e| e.payload.strong_count() > 0 && e.style.strong_count() > 0);
    s.bytes = s.entries.values().map(|e| e.mesh.bytes()).sum();
    if s.bytes <= BUDGET {
        return;
    }
    let mut old: Vec<(u64, Key, usize)> =
        s.entries.iter().filter(|(_, e)| e.used != tick).map(|(k, e)| (e.used, *k, e.mesh.bytes())).collect();
    old.sort_unstable_by_key(|o| o.0);
    for (_, k, bytes) in old {
        if s.bytes <= BUDGET {
            break;
        }
        s.entries.remove(&k);
        s.bytes = s.bytes.saturating_sub(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_geom::Extent;
    use rustgis_tiles::mvt::{Feature, GeomKind, Layer};
    use rustgis_tiles::shapes::Shape;
    use rustgis_tiles::style::{LineCap as Cap, LineJoin as Join};

    fn tile() -> Tile {
        let f = |kind, geometry: Vec<Vec<[f32; 2]>>| Feature { id: None, kind, geometry, props: Vec::new() };
        Tile {
            layers: vec![Layer {
                name: "l".into(),
                extent: 4096,
                features: vec![
                    f(GeomKind::Polygon, vec![vec![[0.0, 0.0], [2048.0, 0.0], [2048.0, 2048.0], [0.0, 2048.0], [0.0, 0.0]]]),
                    f(GeomKind::LineString, vec![vec![[0.0, 1024.0], [1024.0, 1024.0], [1024.0, 3072.0]]]),
                    f(GeomKind::Point, vec![vec![[2048.0, 2048.0]]]),
                ],
            }],
        }
    }

    const RED: [[f32; 4]; 2] = [[1.0, 0.0, 0.0, 0.5], [1.0, 0.0, 0.0, 1.0]];

    fn run(layer: usize, min_zoom: f32, feature: Option<usize>, paint: Paint) -> Run {
        Run { layer, min_zoom, max_zoom: 24.0, extent: 4096, shapes: vec![Shape { feature: feature.map(|f| (0, f)), paint }] }
    }

    #[test]
    fn tessellates_fills_strokes_and_circles() {
        let span = TileId::span(1) as f32;
        let line = Paint::Line { color: RED, width: [2.0, 4.0], cap: Cap::Butt, join: Join::Miter, dashes: Some(vec![3.0]), rings: false };
        let runs = [
            run(0, 0.0, None, Paint::Fill { color: RED, pattern: None }),
            run(1, 0.0, Some(0), Paint::Fill { color: RED, pattern: None }),
            run(2, 5.0, Some(1), line),
            run(3, 5.0, Some(2), Paint::Circle { color: RED, radius: [3.0, 3.0], stroke: Some((RED, [1.0, 1.0])) }),
        ];
        let m = tessellate(&tile(), &runs, 1);
        let patterned = [run(1, 0.0, Some(0), Paint::Fill { color: RED, pattern: Some("dots".into()) })];
        assert!(tessellate(&tile(), &patterned, 1).indices.is_empty(), "patterns aren't drawn yet");
        // The background and the fill show at the same zooms: one run; the line and circle another.
        assert_eq!(m.runs.len(), 2);
        assert_eq!(m.shown(4.0).count(), 1);
        assert_eq!(m.shown(6.0).map(|r| r.len()).sum::<usize>(), m.indices.len());
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        let fills: Vec<&TileVertex> = m.vertices.iter().filter(|v| v.width == [0.0, 0.0]).collect();
        // Premultiplied, at the level and the next; in map units, y north from the corner.
        assert!(fills.iter().all(|v| v.color == [128, 0, 0, 128] && v.color_next == [255, 0, 0, 255]));
        assert!(fills.iter().all(|v| (0.0..=span).contains(&v.position[0]) && (-span..=0.0).contains(&v.position[1])));
        assert!(fills.iter().any(|v| v.position == [span, -span]), "the background fills the square");
        // The line's vertices lie on it, each side of it, dashed (an odd pattern repeated),
        // further along at its end.
        let strokes: Vec<&TileVertex> = m.vertices.iter().filter(|v| v.width == [2.0, 4.0]).collect();
        // Unit normals, longer at the mitred corner.
        let reach = |v: &&TileVertex| v.normal[0].hypot(v.normal[1]);
        assert!(strokes.iter().all(|v| (1.0 - 1e-3..=4.0).contains(&reach(v)) && v.dashes == [3.0, 3.0, 0.0, 0.0]));
        assert!(strokes.iter().any(|v| (reach(v) - std::f32::consts::SQRT_2).abs() < 1e-3));
        let quarter = span / 4.0;
        assert!(strokes.iter().any(|v| (v.along - 3.0 * quarter).abs() < 1.0));
        // The circle: a disc of its diameter and a ring across its edge.
        let widths: Vec<[f32; 2]> = m.vertices.iter().filter(|v| v.dashes == [0.0; 4] && v.width != [0.0, 0.0]).map(|v| v.width).collect();
        assert!(widths.contains(&[6.0, 6.0]) && widths.contains(&[5.0, 5.0]) && widths.contains(&[7.0, 7.0]));
    }

    #[test]
    fn a_tile_that_panics_gives_no_mesh() {
        let out = parallel(&[1, 2, 3, 4], |&i: &i32| if i == 2 { panic!("hostile") } else { i * 10 });
        assert_eq!(out, [Some(10), None, Some(30), Some(40)]);
    }

    #[test]
    fn dash_patterns_fit_four_lengths_or_none() {
        assert_eq!(dash_pattern(None), Some([0.0; 4]));
        assert_eq!(dash_pattern(Some(&[3.0])), Some([3.0, 3.0, 0.0, 0.0]));
        assert_eq!(dash_pattern(Some(&[3.0, 1.0, 2.0, 1.0])), Some([3.0, 1.0, 2.0, 1.0]));
        // Three repeat to six, and five to ten: too long, not cut short.
        assert_eq!(dash_pattern(Some(&[3.0, 2.0, 1.0])), None);
        assert_eq!(dash_pattern(Some(&[1.0, 2.0, 3.0, 4.0, 5.0])), None);
    }

    #[test]
    fn meshes_are_made_once() {
        let mut t = tile();
        t.layers[0].name = "parks".into();
        let payload = Arc::new(Payload::Vector(t));
        use rustgis_tiles::style::{Source, Style, StyleLayer};
        let layer = serde_json::json!({"id": "p", "type": "fill", "source": "s", "source-layer": "parks", "paint": {"fill-color": "#f00"}});
        let style = Arc::new(Resolved {
            style: Style {
                name: String::new(),
                sources: vec![("s".into(), Source::parse(&serde_json::json!({"type": "vector", "tiles": ["x/{z}/{x}/{y}"]})))],
                layers: StyleLayer::parse(&layer).into_iter().collect(),
                glyphs: None,
                sprite: None,
                view: Default::default(),
            },
            sprite: None,
            attribution: Vec::new(),
        });
        let at = Extent::new(0.0, 0.0, 1.0, 1.0);
        let tiles =
            [DataTile { payload: payload.clone(), at, z: 1, clip: at }, DataTile { payload: Arc::new(Payload::Empty), at, z: 1, clip: at }];
        let a = meshes(&style, "s", &tiles, 3);
        assert!(a[0].as_ref().is_some_and(|m| !m.indices.is_empty()) && a[1].is_none());
        let b = meshes(&style, "s", &tiles, 3);
        // A payload standing in twice is made once, for both.
        let twice = [tiles[0].clone(), tiles[0].clone()];
        let t = meshes(&style, "s", &twice, 6);
        assert!(Arc::ptr_eq(t[0].as_ref().unwrap(), t[1].as_ref().unwrap()));
        assert!(Arc::ptr_eq(a[0].as_ref().unwrap(), b[0].as_ref().unwrap()), "kept");
        // Another level is another mesh.
        let c = meshes(&style, "s", &tiles, 4);
        assert!(!Arc::ptr_eq(a[0].as_ref().unwrap(), c[0].as_ref().unwrap()));
    }
}
