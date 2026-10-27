//! A polygon's stacked fills (`Layer::fill_layers`, #882 piece 5), drawn bottom first inside it
//! in place of its symbol's fill, its outline over them: a fill in a colour, hatch lines, or
//! markers on a grid. A pattern is a small tile repeated by tiny-skia's pattern shader, its
//! lattice anchored to the map's origin (worked out in f64 and reduced to within a tile, so it
//! keeps its place on the map at every zoom and neighbouring polygons share one), turned for a
//! hatch's angle.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use rustgis_geom::Coord;
use rustgis_map::fills::{FillLayer, FillPattern};
use rustgis_map::{Rgba, Symbol};
use tiny_skia::{FillRule, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::{Ctx, paint};

/// The most pattern tiles kept (they are kept between frames: a map's patterns are few).
const MAX_TILES: usize = 64;
/// The longest side of a tile in pixels: a coarser pattern is drawn at a lower resolution and
/// scaled up, so a tile never takes more than a megabyte.
const MAX_TILE_PX: f32 = 512.0;

thread_local! {
    /// Pattern tiles drawn so far, by what they draw at what size.
    static TILES: RefCell<HashMap<Vec<u32>, Option<Arc<Pixmap>>>> = RefCell::new(HashMap::new());
}

impl Ctx<'_> {
    /// Draws `polys` with its stacked `fills`, bottom first (a fill without a colour of its own
    /// takes the symbol's), then the symbol's outline over them.
    pub(crate) fn filled(&self, pm: &mut Pixmap, polys: &[Vec<Vec<Coord>>], s: &Symbol, fills: &[FillLayer]) {
        let mut pb = PathBuilder::new();
        for rings in polys {
            for r in rings {
                self.ring_path(&mut pb, r, true);
            }
        }
        let Some(path) = pb.finish() else { return };
        // The map's origin in device pixels, in f64: the patterns' lattice point.
        let origin = match self.globe {
            Some(_) => (0.0, 0.0),
            None => {
                let (x, y) = self.view.to_screen(Coord::new(0.0, 0.0));
                // (Placed as the polygons are: by the view's pixels a point, not the layer's
                // reference-scaled one, so a pattern keeps its place on the map as it pans.)
                let ppp = self.view.pixels_per_point;
                (x * ppp, y * ppp)
            }
        };
        for f in fills.iter().filter(|f| f.draws(s.fill)) {
            let color = f.color.unwrap_or(s.fill);
            match f.pattern {
                FillPattern::Solid => pm.fill_path(&path, &paint(color, 1.0), FillRule::EvenOdd, Transform::identity(), None),
                pattern => {
                    let Some((tile, t)) = pattern_tile(pattern, color, self.ppp, origin) else { continue };
                    let shader = tiny_skia::Pattern::new(
                        tile.as_ref().as_ref(),
                        tiny_skia::SpreadMode::Repeat,
                        tiny_skia::FilterQuality::Bilinear,
                        1.0,
                        t,
                    );
                    let fill = Paint { shader, anti_alias: true, ..Paint::default() };
                    pm.fill_path(&path, &fill, FillRule::EvenOdd, Transform::identity(), None);
                }
            }
        }
        if s.stroke_width > 0.0 && s.stroke.a() > 0 {
            let st = Stroke { width: s.stroke_width * self.ppp, line_join: LineJoin::Round, ..Stroke::default() };
            pm.stroke_path(&path, &paint(s.stroke, 1.0), &st, Transform::identity(), None);
        }
    }
}

/// `v` modulo `m` (positive), as f32: a lattice offset reduced to within one period.
fn reduce(v: f64, m: f64) -> f32 {
    if m > 0.0 && v.is_finite() { v.rem_euclid(m) as f32 } else { 0.0 }
}

/// `pattern`'s tile in `color` at `ppp` pixels a point, and the transform that repeats it with
/// its lattice through `origin` (device pixels); `None` for a pattern of no size.
fn pattern_tile(pattern: FillPattern, color: Rgba, ppp: f32, origin: (f64, f64)) -> Option<(Arc<Pixmap>, Transform)> {
    match pattern {
        FillPattern::Solid => None,
        FillPattern::Hatch { angle, spacing, width, offset } => {
            // A tile a few pixels across and a period high, its line along the middle, turned to
            // the angle (anticlockwise from east; the screen's y runs down).
            let period = spacing * ppp;
            let k = (MAX_TILE_PX / period).min(1.0);
            let h = (period * k).ceil().max(1.0);
            let tile = tile(&key(&pattern, color, ppp), 4.0, h, |pm| {
                let w = (width * ppp * k).max(0.0);
                let mid = h / 2.0;
                if let Some(r) = tiny_skia::Rect::from_ltrb(-1.0, mid - w / 2.0, 5.0, mid + w / 2.0) {
                    pm.fill_rect(r, &paint(color, 1.0), Transform::identity(), None);
                }
            })?;
            let rad = -angle.to_radians();
            // The lattice across the lines (their normal) through the origin, shifted by the offset.
            let (nx, ny) = (-f64::from(rad.sin()), f64::from(rad.cos()));
            let across = reduce(origin.0 * nx + origin.1 * ny + f64::from(offset * ppp), f64::from(period));
            let t = Transform::from_scale(1.0, period / h).post_translate(0.0, across - period / 2.0).post_rotate(rad.to_degrees());
            Some((tile, t))
        }
        FillPattern::Dots { marker, size, outline, outline_width, spacing, displacement, offset } => {
            // Two cells each way, for the odd rows' and columns' displacement.
            let [dx, dy] = spacing.map(|s| s * ppp);
            let [sx, sy] = displacement.map(|d| d * ppp);
            let (tw, th) = (2.0 * dx, 2.0 * dy);
            let k = (MAX_TILE_PX / tw.max(th)).min(1.0);
            let symbol =
                Symbol { fill: color, stroke: outline.unwrap_or(Rgba::new(0, 0, 0, 0)), stroke_width: outline_width, size, marker };
            let tile = tile(&key(&pattern, color, ppp), (tw * k).ceil().max(1.0), (th * k).ceil().max(1.0), |pm| {
                for (col, row) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    // A marker in each cell's middle, odd rows shifted across, odd columns down;
                    // drawn again a tile away each way where it crosses the tile's edges.
                    let x = (col as f32 + 0.5) * dx + if row == 1 { sx } else { 0.0 };
                    let y = (row as f32 + 0.5) * dy + if col == 1 { sy } else { 0.0 };
                    for (ox, oy) in [-1.0, 0.0, 1.0].into_iter().flat_map(|i| [-1.0, 0.0, 1.0].map(|j| (i * tw, j * th))) {
                        crate::draw_marker(pm, (x + ox) * k, (y + oy) * k, &symbol, 1.0, None, ppp * k);
                    }
                }
            })?;
            let (ax, ay) = (
                reduce(origin.0 + f64::from(offset[0] * ppp), f64::from(tw)),
                reduce(origin.1 + f64::from(offset[1] * ppp), f64::from(th)),
            );
            Some((tile, Transform::from_scale(tw / (tw * k).ceil().max(1.0), th / (th * k).ceil().max(1.0)).post_translate(ax, ay)))
        }
    }
}

/// What a tile draws at what size (and whether antialiased), as its cache key.
fn key(pattern: &FillPattern, color: Rgba, ppp: f32) -> Vec<u32> {
    let mut k = vec![u32::from_le_bytes(color.0), ppp.to_bits(), u32::from(crate::antialiased())];
    match *pattern {
        FillPattern::Solid => k.push(0),
        FillPattern::Hatch { spacing, width, .. } => k.extend([1, spacing.to_bits(), width.to_bits()]),
        FillPattern::Dots { marker, size, outline, outline_width, spacing, displacement, .. } => {
            k.extend([2, size.to_bits(), outline_width.to_bits(), outline.map_or(0, |o| u32::from_le_bytes(o.0))]);
            k.extend(spacing.iter().chain(&displacement).map(|v| v.to_bits()));
            // (The marker's shape, by its debug form: a few kinds and a character.)
            k.extend(format!("{marker:?}").bytes().map(u32::from));
        }
    }
    k
}

/// The tile `key` names, drawn by `draw` on a `w` × `h` pixel picture the first time; `None` for
/// one that can't be made.
fn tile(key: &[u32], w: f32, h: f32, draw: impl FnOnce(&mut Pixmap)) -> Option<Arc<Pixmap>> {
    if let Some(t) = TILES.with(|t| t.borrow().get(key).cloned()) {
        return t;
    }
    let made = (w.is_finite() && h.is_finite() && w <= MAX_TILE_PX + 1.0 && h <= MAX_TILE_PX + 1.0)
        .then(|| Pixmap::new(w.max(1.0) as u32, h.max(1.0) as u32))
        .flatten()
        .map(|mut pm| {
            draw(&mut pm);
            Arc::new(pm)
        });
    TILES.with(|t| {
        let mut t = t.borrow_mut();
        if t.len() >= MAX_TILES {
            t.clear();
        }
        t.insert(key.to_vec(), made.clone());
    });
    made
}
