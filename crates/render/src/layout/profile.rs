//! A raster's profile beside its map frame (#2863 part 2): its samples along a section line, read
//! at each one's point of the page line through the frame's projection, what keeps one from being
//! drawn, and the curve with its axes, lined up with the frame.

use rustgis_map::layout::{MAX_PROFILE_SAMPLES, Profile, ProfileSide};

use super::*;

/// What a profile reads along its section line ([`profile_samples`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileSamples {
    /// Each sample's place along the frame's side, in the frame's points from its left (or top).
    pub along: Vec<f64>,
    /// Each sample's metres along the ground from the section line's start (its A end), on the
    /// WGS 84 ellipsoid, between one sample and the next.
    pub metres: Vec<f64>,
    /// The band's value at each sample; `None` where it has no data.
    pub values: Vec<Option<f64>>,
    /// The raster's cells along the section line, when there are more than the samples read
    /// ([`MAX_PROFILE_SAMPLES`]).
    pub cells: Option<usize>,
}

impl ProfileSamples {
    /// The least and most of the values read, if any.
    pub fn range(&self) -> Option<(f64, f64)> {
        let mut it = self.values.iter().flatten().copied();
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }
}

/// How many points of the line a profile probes to count the raster's cells it crosses.
const CELL_PROBES: usize = 64;

/// A profile's frame, section line and raster in `layout` over `maps`, and what it reads along
/// the line: a sample per cell of the raster it crosses (at most [`MAX_PROFILE_SAMPLES`]), each at
/// its point of the line on the page, taken back through the frame's projection to the ground and
/// read bilinearly there ([`rustgis_data::raster::Raster::bilinear`]), with no smoothing.
/// `Err` says why it draws nothing.
pub fn profile_samples(layout: &Layout, maps: &[Map], p: &Profile) -> Result<(ProfileSamples, Section), String> {
    let frame = layout.element(p.frame).ok_or("its map frame is missing")?;
    let ElementKind::MapFrame { map, center, scale, bearing, pitch, shape, sections, .. } = &frame.kind else {
        return Err("its map frame is missing".into());
    };
    let map = map_index(maps, map).and_then(|i| maps.get(i)).ok_or("its frame's map is missing")?;
    let section = Section::find(sections, &p.section)
        .and_then(|i| sections.get(i))
        .ok_or_else(|| format!("its frame has no section line '{}'", p.section))?;
    if !p.side.suits(section.line) {
        return Err(format!(
            "a {} section line's profile goes {}, not {}",
            section.line.code(),
            if p.side.is_horizontal() { "left or right of its frame" } else { "under or over its frame" },
            p.side.code()
        ));
    }
    let cam = frame_camera(map, *center, *scale, *bearing, *pitch);
    if map.projection.is_globe() || cam.pitch.abs() > 1e-9 {
        return Err("its frame is tilted or on the globe, where a line straight on the page isn't a section of the ground".into());
    }
    let layer = map
        .layer_by_name(&p.layer)
        .filter(|l| l.is_raster())
        .ok_or_else(|| format!("the frame's map has no raster layer '{}'", p.layer))?;
    let rl = layer.raster.as_ref().ok_or_else(|| format!("the frame's map has no raster layer '{}'", p.layer))?;
    if rl.in_place || rl.tiled {
        return Err(format!("its layer '{}' is read in place, a part at a time, not held whole", layer.name));
    }
    let r = rl.data.as_ref().ok_or_else(|| format!("its layer '{}' has no pixels read", layer.name))?;
    if r.overview.is_some() {
        return Err(format!("its layer '{}' holds only an overview of its file", layer.name));
    }
    if !r.georeferenced {
        return Err(format!("its layer '{}' isn't placed on the ground", layer.name));
    }
    let band = usize::try_from(p.band).unwrap_or(usize::MAX).saturating_sub(1);
    if band >= r.bands.len() {
        return Err(format!("its layer '{}' has no band {} (it has {})", layer.name, p.band, r.bands.len()));
    }
    let (fw, fh) = (frame.w * PT_PER_MM, frame.h * PT_PER_MM);
    let ends = section_ends(map, &cam, (fw, fh), shape, section.line)
        .map_err(|why| format!("its section line '{}' isn't drawn: {why}", section.name))?;
    // Along the line and across it: a vertical line is worked out with the axes swapped.
    let vertical = matches!(section.line, SectionLine::Vertical(_));
    let swap = |(x, y): (f64, f64)| if vertical { (y, x) } else { (x, y) };
    let ((a, across), (b, _)) = (swap(ends[0]), swap(ends[1]));
    let lonlat = |along: f64| {
        let (x, y) = swap((along, across));
        cam.try_to_map(x, y, fw, fh).and_then(|m| map.to_lonlat(m)).filter(|c| c.x.is_finite() && c.y.is_finite())
    };
    // The cells the line crosses, counted along it in the raster's own grid.
    let cell = |c: Coord| r.transformer().from_lonlat(c).map(|w| r.transform.to_pixel(w));
    let probes: Vec<(f64, f64)> =
        (0..=CELL_PROBES).filter_map(|i| lonlat(a + (b - a) * i as f64 / CELL_PROBES as f64).and_then(cell)).collect();
    let crossed: f64 = probes.windows(2).map(|w| if let [p, q] = w { (q.0 - p.0).hypot(q.1 - p.1) } else { 0.0 }).sum();
    let cells = if crossed.is_finite() { crossed.ceil() as usize } else { 0 };
    let n = cells.saturating_add(1).clamp(2, MAX_PROFILE_SAMPLES);
    let mut out = ProfileSamples { cells: (cells.saturating_add(1) > MAX_PROFILE_SAMPLES).then_some(cells), ..ProfileSamples::default() };
    let (mut metres, mut last) = (0.0, None);
    for i in 0..n {
        let along = a + (b - a) * i as f64 / (n - 1) as f64;
        let at = lonlat(along);
        if let (Some(p), Some(q)) = (last, at) {
            metres += rustgis_geom::wgs84_distance_m(p, q);
        }
        last = at.or(last);
        out.along.push(along);
        out.metres.push(metres);
        out.values.push(at.and_then(|c| r.bilinear(band, c)));
    }
    if out.values.iter().all(Option::is_none) {
        return Err(format!("its layer '{}' has no data along section line '{}'", layer.name, section.name));
    }
    Ok((out, section.clone()))
}

/// A profile its layout doesn't draw, or draws from fewer samples than its raster's cells along
/// its section line ([`profile_samples`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileNote {
    /// The layout it is in, named where several are said together (a report's); empty otherwise.
    pub layout: String,
    pub element: u32,
    pub name: String,
    /// Why it isn't drawn; `None` for one drawn from fewer samples than cells.
    pub why: Option<String>,
    /// With no `why`, the cells along its section line.
    pub cells: usize,
}

/// What `layout`'s visible profiles say as the page is now: those not drawn, and why, and those
/// read at fewer samples than their raster's cells along the line.
pub fn profiles_not_drawn(layout: &Layout, maps: &[Map]) -> Vec<ProfileNote> {
    let profiles = layout.elements.iter().filter(|e| e.visible).filter_map(|e| match &e.kind {
        ElementKind::Profile(p) => Some((e, p)),
        _ => None,
    });
    profiles
        .filter_map(|(e, p)| {
            let (why, cells) = match profile_samples(layout, maps, p) {
                Err(why) => (Some(why), 0),
                Ok((s, _)) => (None, s.cells?),
            };
            Some(ProfileNote { layout: String::new(), element: e.id, name: e.name.clone(), why, cells })
        })
        .collect()
}

/// What an export of `pages` pages says of its profiles ([`profiles_not_drawn`]), each with the
/// pages it is on: one notice for those not drawn, by reason, and one for those read at fewer
/// samples than cells; the profiles counted, the first ten named.
pub fn profiles_notice(notes: &BTreeMap<ProfileNote, usize>, pages: usize) -> Vec<String> {
    let on = |n: usize| if pages > 1 { format!(" on {n} of {pages} pages") } else { String::new() };
    let list = |items: Vec<String>| {
        let more = items.len().saturating_sub(10);
        let mut named = items.into_iter().take(10).collect::<Vec<_>>().join("; ");
        if more > 0 {
            named.push_str(&format!("; and {more} more"));
        }
        named
    };
    // A profile can be missing for one reason on some pages and another on others: one profile still.
    let mut missing: BTreeMap<(&str, u32, &str), Vec<String>> = BTreeMap::new();
    let mut thinned: BTreeMap<(&str, u32, &str), usize> = BTreeMap::new();
    for (n, count) in notes {
        let key = (n.layout.as_str(), n.element, n.name.as_str());
        match &n.why {
            Some(why) => missing.entry(key).or_default().push(format!("{}: {why}", on(*count))),
            None => {
                let most = thinned.entry(key).or_default();
                *most = (*most).max(n.cells);
            }
        }
    }
    let named = |(layout, id, name): (&str, u32, &str)| {
        if layout.is_empty() { format!("'{name}' ({id})") } else { format!("'{name}' ({id}) in layout '{layout}'") }
    };
    let mut out = Vec::new();
    if !missing.is_empty() {
        let count = missing.len();
        let items = missing.into_iter().map(|(key, why)| format!("{}{}", named(key), why.join(","))).collect();
        out.push(format!("{count} profile(s) not drawn: {}", list(items)));
    }
    if !thinned.is_empty() {
        let count = thinned.len();
        let items = thinned.into_iter().map(|(key, cells)| format!("{}, across {cells} cells", named(key))).collect();
        out.push(format!(
            "{count} profile(s) read at {MAX_PROFILE_SAMPLES} samples, fewer than their raster's cells along the line: {}",
            list(items)
        ));
    }
    out
}

/// The size of a profile's numbers, its title and its section's names, in points.
const PROFILE_LABEL_PT: f32 = 7.0;
const PROFILE_TITLE_PT: f32 = 7.5;
const PROFILE_NAME_PT: f32 = 8.0;

/// A round step that splits `span` into about `parts`: 1, 2 or 5 times a power of ten.
fn round_step(span: f64, parts: f64) -> f64 {
    let rough = (span / parts).abs();
    if !(rough.is_finite() && rough > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(rough.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|k| k * p).find(|s| *s >= rough * 0.999).unwrap_or(10.0 * p)
}

/// `v` written with the decimals a step of `step` needs, with a decimal comma when `comma`.
fn number(v: f64, step: f64, comma: bool) -> String {
    let decimals = if step >= 1.0 || step <= 0.0 { 0 } else { (-step.log10().floor()).clamp(0.0, 6.0) as usize };
    let k = 10f64.powi(decimals as i32);
    decimal(format!("{:.*}", decimals, (v * k).round() / k + 0.0), comma)
}

/// The value axis' ends and step for a profile whose samples run from `lo` to `hi`, the ends it is
/// given taking their place: each end not given out to a round number past the samples.
pub(super) fn value_axis(p: &Profile, (lo, hi): (f64, f64)) -> (f64, f64, f64) {
    let (min, max) = p.given_range();
    let (lo, hi) = (min.unwrap_or(lo), max.unwrap_or(hi));
    let (lo, hi) = if hi > lo { (lo, hi) } else { (lo - (lo.abs() * 0.1).max(1.0), hi + (hi.abs() * 0.1).max(1.0)) };
    let step = round_step(hi - lo, 4.0);
    let lo = min.unwrap_or_else(|| (lo / step).floor() * step);
    let hi = max.unwrap_or_else(|| (hi / step).ceil() * step);
    (lo, hi.max(lo + step * 1e-6), step)
}

/// A profile in `r` (pixels, at `ppp` pixels a point): the frame's side along its length, so a
/// sample `along` points from the frame's start is drawn at `along` × `ppp` pixels from the
/// element's. Along a horizontal side the values rise up the page; along a vertical one, away from
/// the frame. The distance axis runs under the curve (beside it, on the frame's side, for a
/// vertical one), the section's names at its ends over it, and the value axis' numbers on its
/// guide lines at the start (at the end for a vertical one).
pub(super) fn draw_profile(pm: &mut Canvas, r: Rect, p: &Profile, (samples, section): (&ProfileSamples, &Section), comma: bool, ppp: f64) {
    let k = ppp as f32;
    let ink = p.color.unwrap_or(INK);
    let label_px = PROFILE_LABEL_PT * 1.33 * k;
    let name_px = PROFILE_NAME_PT * 1.33 * k;
    let title_px = PROFILE_TITLE_PT * 1.33 * k;
    let width = |s: &str, px: f32, bold: bool| text::measure(s, px / 1.33, bold);
    let horizontal = p.side.is_horizontal();
    let (start, length, thick) = if horizontal { (r.x(), r.width(), r.height()) } else { (r.y(), r.height(), r.width()) };
    // The distance axis' numbers, in the unit that reads best over the whole line.
    let total = samples.metres.last().copied().unwrap_or(0.0);
    let (unit_m, abbr) = p.units.resolve(total);
    let step_d = round_step(total / unit_m, 5.0);
    let ticks: Vec<(f32, String)> = (0..=200)
        .map(|i| f64::from(i) * step_d)
        .take_while(|d| *d * unit_m <= total * (1.0 + 1e-9))
        .filter_map(|d| along_at(samples, d * unit_m).map(|a| (a as f32 * k, number(d, step_d, comma))))
        .collect();
    let tick = 3.0 * k;
    let widest = ticks.iter().map(|(_, s)| width(&format!("{s} {abbr}"), label_px, false)).fold(0.0, f32::max);
    // The strips the distance axis and the names take, across the profile.
    let axis = if horizontal { tick + label_px * 1.3 } else { tick + widest + 2.0 * k };
    let names = [section.name.clone(), format!("{}'", section.name)];
    let head = if horizontal { name_px * 1.5 } else { names.iter().map(|n| width(n, name_px, true)).fold(0.0, f32::max) + 3.0 * k };
    let plot = thick - axis - head;
    if plot < 2.0 * k {
        placeholder(pm, r, "Profile", ppp);
        return;
    }
    // A point `u` pixels along the profile and `v` above its base, on the page.
    let page = |u: f32, v: f32| match p.side {
        ProfileSide::Bottom | ProfileSide::Top => (r.x() + u, r.y() + r.height() - axis - v),
        ProfileSide::Right => (r.x() + axis + v, r.y() + u),
        ProfileSide::Left => (r.x() + r.width() - axis - v, r.y() + u),
    };
    let Some((lo_v, hi_v)) = samples.range() else { return };
    let (lo, hi, step) = value_axis(p, (lo_v, hi_v));
    let v_of = |val: f64| (((val - lo) / (hi - lo)) as f32 * plot).clamp(0.0, plot);
    let line = |pts: &[(f32, f32)]| {
        let mut pb = PathBuilder::new();
        for (i, &(u, v)) in pts.iter().enumerate() {
            let (x, y) = page(u, v);
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        pb.finish()
    };
    let thin = |pm: &mut Canvas, pts: &[(f32, f32)], colour: Rgba, w: f32| {
        if let Some(path) = line(pts) {
            pm.stroke_path(&path, &paint(colour, 1.0), &Stroke { width: w, ..Stroke::default() }, Transform::identity(), None);
        }
    };
    // Guide lines at the round values, under the curve.
    let guides: Vec<f64> = (0..=40).map(|i| ((lo / step).ceil() + f64::from(i)) * step).take_while(|g| *g <= hi + step * 1e-9).collect();
    if p.guides {
        for g in guides.iter().filter(|g| **g > lo + step * 1e-9) {
            thin(pm, &[(0.0, v_of(*g)), (length, v_of(*g))], ink.with_alpha(70), 0.35 * k);
        }
    }
    // The curve, a run between gaps in the data at a time, filled down to the base or not.
    let runs = samples.along.iter().zip(&samples.values).map(|(a, v)| v.map(|v| (*a as f32 * k, v_of(v))));
    let mut pieces: Vec<Vec<(f32, f32)>> = vec![Vec::new()];
    for pt in runs {
        match (pt, pieces.last_mut()) {
            (Some(pt), Some(run)) => run.push(pt),
            (None, Some(run)) if !run.is_empty() => pieces.push(Vec::new()),
            _ => {}
        }
    }
    for run in pieces.iter().filter(|r| !r.is_empty()) {
        if let (Some(fill), Some(first), Some(last)) = (p.fill, run.first(), run.last()) {
            let mut pb = PathBuilder::new();
            let (x, y) = page(first.0, 0.0);
            pb.move_to(x, y);
            for (x, y) in run.iter().chain([&(last.0, 0.0)]).map(|&(u, v)| page(u, v)) {
                pb.line_to(x, y);
            }
            pb.close();
            if let Some(area) = pb.finish() {
                pm.fill_path(&area, &paint(fill, 1.0), FillRule::Winding, Transform::identity(), None);
            }
        }
        if run.len() > 1
            && let Some(path) = line(run)
        {
            let stroke = Stroke {
                width: p.line_width as f32 * k,
                line_join: tiny_skia::LineJoin::Round,
                line_cap: tiny_skia::LineCap::Round,
                ..Stroke::default()
            };
            pm.stroke_path(&path, &paint(p.line_color, 1.0), &stroke, Transform::identity(), None);
        }
    }
    // The base and the distance axis: a tick and a number at each round distance, the unit after
    // the last, each number clear of the one before and inside the element.
    thin(pm, &[(0.0, 0.0), (length, 0.0)], ink, 0.6 * k);
    let mut before = f32::NEG_INFINITY;
    let last = ticks.len().saturating_sub(1);
    for (i, (u, s)) in ticks.iter().enumerate() {
        thin(pm, &[(*u, 0.0), (*u, -tick)], ink, 0.5 * k);
        let s = if i == last { format!("{s} {abbr}") } else { s.clone() };
        let (x, y) = page(*u, -tick);
        if horizontal {
            let half = width(&s, label_px, false) / 2.0 + k;
            let cx = (x).clamp(r.x() + half, (r.x() + r.width() - half).max(r.x() + half));
            if cx - half < before {
                continue;
            }
            before = cx + half;
            text::text_at(pm, &s, cx, y + label_px * 0.65, label_px, ink, false, text::Align::Center, false);
        } else {
            let half = label_px * 0.6;
            let cy = y.clamp(r.y() + half, (r.y() + r.height() - half).max(r.y() + half));
            if cy - half < before {
                continue;
            }
            before = cy + half;
            let (tx, align) = if p.side == ProfileSide::Right { (x - k, text::Align::Right) } else { (x + k, text::Align::Left) };
            text::text_at(pm, &s, tx, cy, label_px, ink, false, align, false);
        }
    }
    // The value axis' numbers, on their guides (the base's on it): at the start along a
    // horizontal side, at the end along a vertical one, each clear of the one before.
    let unit = if p.unit.trim().is_empty() { String::new() } else { format!(" {}", p.unit.trim()) };
    let mut before = f32::NEG_INFINITY;
    for g in &guides {
        let s = format!("{}{unit}", number(*g, step, comma));
        let v = v_of(*g);
        if horizontal {
            let (_, y) = page(0.0, v);
            let cy = y - label_px * 0.55;
            // Stacked up the page: each over the one before.
            if before.is_finite() && cy + label_px * 0.5 > before {
                continue;
            }
            before = cy - label_px * 0.5;
            text::text_at(pm, &s, start + 2.0 * k, cy, label_px, ink, false, text::Align::Left, true);
        } else {
            let (x, _) = page(length, v);
            let half = width(&s, label_px, false) / 2.0 + k;
            // Side by side away from the frame: each past the one before.
            let (lo_x, hi_x) = if p.side == ProfileSide::Right { (x - half, x + half) } else { (-(x + half), -(x - half)) };
            if lo_x < before || x - half < r.x() || x + half > r.x() + r.width() {
                continue;
            }
            before = hi_x;
            text::text_at(pm, &s, x, r.y() + r.height() - label_px * 0.6, label_px, ink, false, text::Align::Center, true);
        }
    }
    // The section's names at the ends, in its colour, and the title after the first.
    let [first, second] = names;
    let colour = section.color.with_alpha(255);
    if horizontal {
        let cy = r.y() + head / 2.0;
        text::text_at(pm, &first, start + 2.0 * k, cy, name_px, colour, true, text::Align::Left, false);
        text::text_at(pm, &second, start + length - 2.0 * k, cy, name_px, colour, true, text::Align::Right, false);
        if !p.title.trim().is_empty() {
            let x = start + 2.0 * k + width(&first, name_px, true) + 4.0 * k;
            text::text_at(pm, p.title.trim(), x, cy, title_px, ink, true, text::Align::Left, false);
        }
    } else {
        let (cx, align) =
            if p.side == ProfileSide::Right { (r.x() + r.width() - k, text::Align::Right) } else { (r.x() + k, text::Align::Left) };
        text::text_at(pm, &first, cx, start + name_px * 0.7, name_px, colour, true, align, false);
        text::text_at(pm, &second, cx, start + length - name_px * 0.7, name_px, colour, true, align, false);
        if !p.title.trim().is_empty() {
            let (x, _) = page(0.0, plot);
            let align = if p.side == ProfileSide::Right { text::Align::Right } else { text::Align::Left };
            text::text_at(pm, p.title.trim(), x, start + name_px * 2.0, title_px, ink, true, align, true);
        }
    }
}

/// The place along the frame's side (points) where the ground distance from the section's start is
/// `metres`, between the samples around it; `None` past the last.
fn along_at(s: &ProfileSamples, metres: f64) -> Option<f64> {
    let i = s.metres.partition_point(|m| *m < metres);
    match (i.checked_sub(1).and_then(|j| s.metres.get(j).zip(s.along.get(j))), s.metres.get(i).zip(s.along.get(i))) {
        (_, Some((m, a))) if (*m - metres).abs() < 1e-9 => Some(*a),
        (Some((m0, a0)), Some((m1, a1))) if m1 > m0 => Some(a0 + (a1 - a0) * (metres - m0) / (m1 - m0)),
        (None, Some((_, a))) => Some(*a),
        _ => None,
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
