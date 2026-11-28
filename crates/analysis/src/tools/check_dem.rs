//! Check DEM (#3151): a point for each step, spike, pit and implausible value an elevation model
//! holds, and a summary of them. It reports and changes nothing: filling or smoothing is the
//! user's choice, with the tools made for it.
//!
//! A step is measured against the terrain on either side of it, on the same line: a drop between
//! two neighbours far larger than the drops just before and just after it is a break the slope
//! doesn't explain (a seam, a void filled at another level, a reservoir's edge in a surface
//! model), where a cliff, whose face goes on dropping, is not. Measured on real models: on a
//! 1-second SRTM tile of the Quadrilátero Ferrífero (S21W044), 82% of the 1,740 steps found lie
//! within a cell of where SRTM and the Copernicus 30 m model differ by more than 10 m (12% of all
//! cells do); on the ANADEM terrain model of the Serra do Brigadeiro (Mantiqueira) there are 16 a
//! million cells, on rock walls. Comparing with the eight neighbours' median drop instead flagged
//! 438 a million there.
use super::*;
use rustgis_data::loss::{LossCode, LossMessage};
use rustgis_data::raster::Raster;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The lowest and highest values an elevation model can hold, in metres: Check DEM's defaults,
/// and what a Shaded Relief's notice looks for in a band's statistics. Below the Dead Sea's shore
/// and the deepest mine, above Everest.
pub const PLAUSIBLE_ELEVATION: (f64, f64) = (-500.0, 9000.0);
/// The most findings written as points. The rest are counted in the summary only, so a model
/// covered with them still gives a layer that can be drawn and listed.
const MAX_POINTS: usize = 100_000;
/// About how many cells one item of the scan reads: enough to share out on threads, few enough
/// items that a small model is checked in a few looks at Cancel.
const BLOCK_CELLS: usize = 1 << 16;
/// The eight neighbours, as (column, row) steps.
const AROUND: [(i64, i64); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

/// What a finding is, as its `kind` field says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Step,
    Spike,
    Pit,
    OutOfRange,
    FlatRun,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Step => "step",
            Kind::Spike => "spike",
            Kind::Pit => "pit",
            Kind::OutOfRange => "out of range",
            Kind::FlatRun => "flat run",
        }
    }

    /// The unit of its size: a height for the cells compared with their neighbours, a count of
    /// cells for a run along a row.
    fn unit(self) -> &'static str {
        match self {
            Kind::Step | Kind::Spike | Kind::Pit => "elevation",
            Kind::OutOfRange | Kind::FlatRun => "cells",
        }
    }
}

/// One finding: the cell it is placed on (a run's middle cell), what it is, its size and the
/// elevation there (a run's first value).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Finding {
    col: u32,
    row: u32,
    kind: Kind,
    size: f64,
    elevation: f64,
}

/// The thresholds, as the parameters give them.
#[derive(Clone, Copy, Debug)]
struct Rules {
    ratio: f64,
    step: f64,
    spike: f64,
    low: f64,
    high: f64,
    run: usize,
}

impl Rules {
    fn read(p: &Json) -> Result<Rules, String> {
        let get = |key: &str, default: f64| -> Result<f64, String> {
            match p.get(key) {
                None | Some(Json::Null) => Ok(default),
                Some(Json::String(s)) if s.trim().is_empty() => Ok(default),
                Some(_) => num(p, key).ok_or_else(|| format!("{key} must be a number")),
            }
        };
        let ratio = get("stepRatio", 5.0)?;
        let step = get("stepHeight", 20.0)?;
        let spike = get("spikeHeight", 30.0)?;
        let low = get("lowest", PLAUSIBLE_ELEVATION.0)?;
        let high = get("highest", PLAUSIBLE_ELEVATION.1)?;
        let run = get("runLength", 100.0)?;
        if ratio < 1.0 {
            return Err("stepRatio must be 1 or more: a step is a drop that many times the drops beside it".into());
        }
        if step <= 0.0 || spike <= 0.0 {
            return Err("stepHeight and spikeHeight must be more than 0".into());
        }
        if low >= high {
            return Err("lowest must be below highest".into());
        }
        if run < 2.0 || run.fract() != 0.0 || run > f64::from(u32::MAX) {
            return Err("runLength must be a whole number of cells, 2 or more".into());
        }
        Ok(Rules { ratio, step, spike, low, high, run: run as usize })
    }

    /// Whether `v` is an elevation the model can hold.
    fn holds(&self, v: f64) -> bool {
        (self.low..=self.high).contains(&v)
    }
}

/// One band of the model, read through the rules.
struct Dem<'a> {
    r: &'a Raster,
    band: usize,
    rules: Rules,
}

impl Dem<'_> {
    /// The value at a cell: `None` off the grid and for no-data.
    fn value(&self, col: i64, row: i64) -> Option<f64> {
        self.r.value(self.band, col, row).map(f64::from)
    }

    /// The value at a cell when it is an elevation the model can hold: the neighbourhood rules
    /// compare only these, so a hole of undeclared no-data isn't also a field of pits.
    fn level(&self, col: i64, row: i64) -> Option<f64> {
        self.value(col, row).filter(|v| self.rules.holds(*v))
    }

    /// How far `v`, at a cell, stands above (positive) or below the median of its eight
    /// neighbours; `None` when one of them has no value it can hold.
    fn above_median(&self, col: i64, row: i64, v: f64) -> Option<f64> {
        let mut around = [0.0; 8];
        for (slot, (dc, dr)) in around.iter_mut().zip(AROUND) {
            *slot = self.level(col + dc, row + dr)?;
        }
        around.sort_unstable_by(f64::total_cmp);
        let [_, _, _, below, above, ..] = around;
        Some(v - (below + above) / 2.0)
    }

    /// The largest drop from a cell, at `v`, to a neighbour that is a step: more than the step
    /// height and more than `ratio` times the drops beside it on the same line (from the cell
    /// behind to this one, and from the neighbour to the one beyond it).
    fn step(&self, col: i64, row: i64, v: f64) -> Option<f64> {
        let mut largest: Option<f64> = None;
        for (dc, dr) in AROUND {
            let Some(next) = self.level(col + dc, row + dr) else { continue };
            let drop = v - next;
            if drop <= self.rules.step {
                continue;
            }
            let (Some(behind), Some(beyond)) = (self.level(col - dc, row - dr), self.level(col + 2 * dc, row + 2 * dr)) else {
                continue;
            };
            let beside = (behind - v).abs().max((next - beyond).abs());
            if drop > self.rules.ratio * beside && largest.is_none_or(|l| drop > l) {
                largest = Some(drop);
            }
        }
        largest
    }
}

/// What one block of rows holds, counted.
#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    /// Cells with a value, whether it can be held or not.
    cells: u64,
    /// Cells with a value the model can hold that lack a neighbour to compare with (at the edge,
    /// or next to no-data or a value out of range): the spike and pit rule skips them.
    unchecked: u64,
    steps: u64,
    spikes: u64,
    pits: u64,
    /// Cells found a step, a spike or a pit, each counted once.
    compared: u64,
    out_runs: u64,
    out_cells: u64,
    flat_runs: u64,
    flat_cells: u64,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.cells += o.cells;
        self.unchecked += o.unchecked;
        self.steps += o.steps;
        self.spikes += o.spikes;
        self.pits += o.pits;
        self.compared += o.compared;
        self.out_runs += o.out_runs;
        self.out_cells += o.out_cells;
        self.flat_runs += o.flat_runs;
        self.flat_cells += o.flat_cells;
    }

    fn findings(&self) -> u64 {
        self.steps + self.spikes + self.pits + self.out_runs + self.flat_runs
    }
}

/// The lowest and highest values of `rows` the model can hold.
fn extremes(dem: &Dem<'_>, rows: Range<u32>) -> Result<Option<(f64, f64)>, String> {
    let mut found: Option<(f64, f64)> = None;
    let mut pace = crate::control::Pace::default();
    for row in rows {
        for col in 0..dem.r.width {
            if pace.stop(1) {
                return Err(crate::control::Cancelled.into());
            }
            if let Some(v) = dem.level(i64::from(col), i64::from(row)) {
                found = Some(found.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
            }
        }
    }
    Ok(found)
}

/// A run along a row going on: where it started, and its first value.
type Run = Option<(u32, f64)>;

/// Ends a run of values out of range at column `end`: always a finding.
fn close_out(run: &mut Run, end: u32, row: u32, n: &mut Counts, keep: &mut dyn FnMut(Finding)) {
    if let Some((start, v)) = run.take() {
        let len = end.saturating_sub(start);
        n.out_runs += 1;
        n.out_cells += u64::from(len);
        keep(Finding { col: start + len / 2, row, kind: Kind::OutOfRange, size: f64::from(len), elevation: v });
    }
}

/// Ends a run of one value at column `end`: a finding when it is as long as the rules ask and its
/// value is the band's lowest or highest (`ends`), where an undeclared no-data lies. A lake held
/// level, at any other value, is not one.
fn close_flat(run: &mut Run, end: u32, row: u32, at_least: usize, ends: Option<(f64, f64)>, n: &mut Counts, keep: &mut dyn FnMut(Finding)) {
    if let Some((start, v)) = run.take() {
        let len = end.saturating_sub(start);
        if len as usize >= at_least && ends.is_some_and(|(lo, hi)| v == lo || v == hi) {
            n.flat_runs += 1;
            n.flat_cells += u64::from(len);
            keep(Finding { col: start + len / 2, row, kind: Kind::FlatRun, size: f64::from(len), elevation: v });
        }
    }
}

/// Checks `rows`, counting what it finds and handing each finding to `keep`, in row order and
/// along each row.
fn scan(dem: &Dem<'_>, rows: Range<u32>, ends: Option<(f64, f64)>, keep: &mut dyn FnMut(Finding)) -> Result<Counts, String> {
    let mut n = Counts::default();
    let mut pace = crate::control::Pace::default();
    let rules = dem.rules;
    for row in rows {
        let r = i64::from(row);
        let (mut out, mut flat): (Run, Run) = (None, None);
        for col in 0..dem.r.width {
            if pace.stop(1) {
                return Err(crate::control::Cancelled.into());
            }
            let c = i64::from(col);
            let Some(v) = dem.value(c, r) else {
                close_out(&mut out, col, row, &mut n, keep);
                close_flat(&mut flat, col, row, rules.run, ends, &mut n, keep);
                continue;
            };
            n.cells += 1;
            if !rules.holds(v) {
                close_flat(&mut flat, col, row, rules.run, ends, &mut n, keep);
                out.get_or_insert((col, v));
                continue;
            }
            close_out(&mut out, col, row, &mut n, keep);
            if flat.is_some_and(|(_, f)| f != v) {
                close_flat(&mut flat, col, row, rules.run, ends, &mut n, keep);
            }
            flat.get_or_insert((col, v));
            let mut hit = false;
            match dem.above_median(c, r, v) {
                None => n.unchecked += 1,
                Some(d) if d > rules.spike => {
                    n.spikes += 1;
                    hit = true;
                    keep(Finding { col, row, kind: Kind::Spike, size: d, elevation: v });
                }
                Some(d) if -d > rules.spike => {
                    n.pits += 1;
                    hit = true;
                    keep(Finding { col, row, kind: Kind::Pit, size: -d, elevation: v });
                }
                Some(_) => {}
            }
            if let Some(drop) = dem.step(c, r, v) {
                n.steps += 1;
                hit = true;
                keep(Finding { col, row, kind: Kind::Step, size: drop, elevation: v });
            }
            n.compared += u64::from(hit);
        }
        close_out(&mut out, dem.r.width, row, &mut n, keep);
        close_flat(&mut flat, dem.r.width, row, rules.run, ends, &mut n, keep);
    }
    Ok(n)
}

/// Checks a band of an elevation model and writes a point for each finding.
pub(super) fn run(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    run_on(inp, p, None)
}

/// [`run`] on `threads` threads (the shared count when `None`): the same layer and summary on any.
///
/// Three passes over the rows, each in blocks of about [`BLOCK_CELLS`] cells on the threads,
/// looking at Cancel along each row: the band's lowest and highest values (for the flat runs),
/// then the findings, counted, kept as they come while fewer than [`MAX_POINTS`] are. Past that
/// many, which ones were kept depends on the threads, so a third pass takes the first
/// [`MAX_POINTS`] again, block by block from the top. Beyond the raster as it is held, memory is
/// the counts of each block and at most [`MAX_POINTS`] findings and their points.
pub(super) fn run_on(inp: &dyn Inputs, p: &Json, threads: Option<usize>) -> Result<ToolResult, String> {
    let name = s(p, "input");
    let r = inp.raster(name).ok_or("choose an elevation raster")?.raster;
    r.require_coordinates()?;
    let bands = zonal_bands(p.get("band").unwrap_or(&Json::Null), r.band_count())?;
    let [band] = bands.as_slice() else { return Err("choose one band of elevations".into()) };
    let rules = Rules::read(p)?;
    let dem = Dem { r, band: *band, rules };
    let per = (BLOCK_CELLS / (r.width as usize).max(1)).clamp(1, u32::MAX as usize) as u32;
    let blocks: Vec<Range<u32>> = (0..r.height).step_by(per as usize).map(|top| top..top.saturating_add(per).min(r.height)).collect();
    let cells = (r.width as usize).saturating_mul(r.height as usize);
    let threads = threads.unwrap_or_else(|| crate::parallel::threads_for_rows(blocks.len(), cells, 0));
    crate::control::begin(blocks.len().saturating_mul(3));
    let ends = crate::parallel::try_map_on(&blocks, threads, || (), |_, _, rows| extremes(&dem, rows.clone()))?
        .into_iter()
        .flatten()
        .reduce(|(lo, hi), (l, h)| (lo.min(l), hi.max(h)));
    let taken = AtomicUsize::new(0);
    let scanned = crate::parallel::try_map_on(
        &blocks,
        threads,
        || (),
        |_, _, rows| {
            let mut kept = Vec::new();
            let n = scan(&dem, rows.clone(), ends, &mut |f| {
                if taken.fetch_add(1, Ordering::Relaxed) < MAX_POINTS {
                    kept.push(f);
                }
            })?;
            Ok::<_, String>((n, kept))
        },
    )?;
    let mut total = Counts::default();
    for (n, _) in &scanned {
        total.add(n);
    }
    let found = usize::try_from(total.findings()).unwrap_or(usize::MAX);
    let findings: Vec<Finding> = if found <= MAX_POINTS {
        scanned.into_iter().flat_map(|(_, kept)| kept).collect()
    } else {
        // The first MAX_POINTS findings by block, scanned again for them alone.
        let mut left = MAX_POINTS;
        let mut plan = Vec::new();
        for (rows, (n, _)) in blocks.iter().zip(&scanned) {
            let here = usize::try_from(n.findings()).unwrap_or(usize::MAX).min(left);
            if here > 0 {
                plan.push((rows.clone(), here));
                left -= here;
            }
            if left == 0 {
                break;
            }
        }
        drop(scanned);
        let parts = crate::parallel::try_map_on(
            &plan,
            threads,
            || (),
            |_, _, (rows, take)| {
                let mut kept = Vec::with_capacity(*take);
                scan(&dem, rows.clone(), ends, &mut |f| {
                    if kept.len() < *take {
                        kept.push(f);
                    }
                })?;
                Ok::<_, String>(kept)
            },
        )?;
        parts.into_iter().flatten().collect()
    };
    crate::control::finish();
    if total.cells == 0 {
        return Err(format!("band {} of '{name}' has no values to check", band + 1));
    }
    let fields = vec![
        Field::new("kind", FieldType::Text),
        Field::new("size", FieldType::Double),
        Field::new("unit", FieldType::Text),
        Field::new("elevation", FieldType::Double),
    ];
    let mut out = Dataset::new(out_name(p, format!("{name}_CheckDEM")), GeometryType::Point, fields);
    let mut placeless = 0usize;
    let mut pace = crate::control::Pace::default();
    for f in &findings {
        if pace.stop(64) {
            return Err(crate::control::Cancelled.into());
        }
        let centre = r.transform.to_world(f64::from(f.col) + 0.5, f64::from(f.row) + 0.5);
        let Some(at) = r.transformer().to_lonlat(centre) else {
            placeless += 1;
            continue;
        };
        out.push_next(
            Some(Geometry::Point(at)),
            vec![Value::Text(f.kind.name().into()), Value::Float(f.size), Value::Text(f.kind.unit().into()), Value::Float(f.elevation)],
        );
    }
    let affected = total.compared + total.out_cells + total.flat_cells;
    let mut messages = vec![format!(
        "Checked {} cell(s) of band {} of '{name}': {} step(s), {} spike(s), {} pit(s), {} run(s) of values out of range ({} cells), {} flat run(s) at the band's lowest or highest value ({} cells); {:.3}% of the cells with a value are affected",
        total.cells,
        band + 1,
        total.steps,
        total.spikes,
        total.pits,
        total.out_runs,
        total.out_cells,
        total.flat_runs,
        total.flat_cells,
        affected as f64 * 100.0 / total.cells as f64,
    )];
    if total.unchecked > 0 {
        messages.push(format!(
            "{} cell(s) at the edge, or next to no-data or a value out of range, lack a neighbour to compare with: the spike and pit rule skips them, and the step rule the directions it can't follow",
            total.unchecked
        ));
    }
    if let Some((fw, fh)) = r.overview {
        messages.push(format!(
            "'{name}' is held as an overview of its file ({} × {} of its {fw} × {fh} cells): a step or spike a cell or two wide at full resolution may not show in it",
            r.width, r.height
        ));
    }
    let mut losses = Vec::new();
    if found > findings.len() {
        losses.push(LossMessage {
            code: LossCode::DroppedOverLimit,
            text: format!(
                "{} of the {found} findings are counted but not written as points: the layer holds the first {}, from the top row down. Raise the thresholds, or check part of the model, to see the rest",
                found - findings.len(),
                findings.len()
            ),
        });
    }
    if placeless > 0 {
        losses.push(LossMessage {
            code: LossCode::DroppedFeature,
            text: format!("{placeless} finding(s) whose cell couldn't be placed in longitude and latitude were left out of the layer"),
        });
    }
    Ok(ToolResult { output: Output::Layer(out), messages, losses })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::raster::{ColorHint, GeoTransform};

    struct One<'a>(&'a Raster);

    impl Inputs for One<'_> {
        fn dataset(&self, _: &str) -> Option<&Dataset> {
            None
        }
        fn selection(&self, _: &str) -> &[usize] {
            &[]
        }
        fn raster(&self, layer: &str) -> Option<crate::RasterInput<'_>> {
            (layer == "dem").then_some(crate::RasterInput { raster: self.0, path: None, embedded: None })
        }
    }

    /// A 30 m grid in UTM 23S of `width × height` cells, each `z(col, row)`.
    fn dem(width: u32, height: u32, z: impl Fn(u32, u32) -> f32) -> Raster {
        let values = (0..height).flat_map(|row| (0..width).map(move |col| (col, row))).map(|(c, r)| z(c, r)).collect();
        let transform = GeoTransform { origin_x: 600_000., origin_y: 7_750_000., pixel_width: 30., pixel_height: -30. };
        Raster::new("dem", width, height, vec![values], transform, Crs::Epsg(32723), Some(-9999.), ColorHint::Gray, "Float32").unwrap()
    }

    /// A slope with a little roughness.
    fn rough(col: u32, row: u32) -> f32 {
        800.0 + 3.0 * row as f32 + ((col * 7 + row * 13) % 5) as f32
    }

    /// Mountain-like ground with no defect: [`rough`] with a round hill and a steep face (a 40 m
    /// drop a cell for six cells).
    fn clean(col: u32, row: u32) -> f32 {
        let (x, y) = (col as f32, row as f32);
        let hill = 300.0 * (-((x - 20.0).powi(2) + (y - 20.0).powi(2)) / 60.0).exp();
        let face = 40.0 * (x - 30.0).clamp(0.0, 6.0);
        rough(col, row) + hill - face
    }

    type Row = (String, f64, f64, Coord);

    fn check(r: &Raster, p: Json) -> (ToolResult, Vec<Row>) {
        let mut p = p;
        p["input"] = Json::from("dem");
        let result = run(&One(r), &p).unwrap();
        let Output::Layer(out) = &result.output else { panic!("a point layer") };
        let rows = out
            .features
            .iter()
            .map(|f| {
                let Value::Text(kind) = &f.attrs[0] else { panic!("a kind") };
                let (Value::Float(size), Value::Float(z)) = (&f.attrs[1], &f.attrs[3]) else { panic!("numbers") };
                let Some(Geometry::Point(at)) = f.geometry else { panic!("a point") };
                (kind.clone(), *size, *z, at)
            })
            .collect();
        (result, rows)
    }

    fn kinds(rows: &[Row], kind: &str) -> usize {
        rows.iter().filter(|r| r.0 == kind).count()
    }

    #[test]
    fn clean_mountain_ground_has_no_findings() {
        let r = dem(60, 50, clean);
        let (result, rows) = check(&r, serde_json::json!({}));
        assert_eq!(rows.len(), 0, "{rows:?}");
        assert!(result.losses.is_empty());
        let summary = &result.messages[0];
        assert!(summary.starts_with("Checked 3000 cell(s) of band 1 of 'dem': 0 step(s), 0 spike(s), 0 pit(s)"), "{summary}");
        assert!(summary.ends_with("0.000% of the cells with a value are affected"), "{summary}");
        // The 216 cells of the edge lack neighbours, and that is said.
        assert!(result.messages[1].starts_with("216 cell(s) at the edge"), "{:?}", result.messages);
    }

    #[test]
    fn a_seam_is_a_line_of_steps_on_its_high_side() {
        // The right part 25 m lower than the left, as two tiles mosaicked at different levels.
        let r = dem(40, 30, |c, r| rough(c, r) - if c >= 12 { 25.0 } else { 0.0 });
        let (result, rows) = check(&r, serde_json::json!({}));
        // Every row but the first and last, whose diagonals go off the grid: those still have
        // the step across the row, so all 30 are found.
        assert_eq!(kinds(&rows, "step"), 30, "{rows:?}");
        assert_eq!(rows.len(), 30, "{rows:?}");
        // Each on the last cell of the high side, one a row.
        let cells: Vec<_> = rows.iter().map(|(_, _, _, at)| r.pixel_at(*at).unwrap()).collect();
        assert_eq!(cells, (0..30).map(|row| (11, row)).collect::<Vec<_>>());
        assert!(rows.iter().all(|(_, size, _, _)| (20.0..33.0).contains(size)), "{rows:?}");
        assert!(result.messages[0].contains(": 30 step(s), 0 spike(s)"), "{}", result.messages[0]);
        // Below the step height, it isn't one.
        let (_, rows) = check(&r, serde_json::json!({"stepHeight": 40}));
        assert_eq!(rows.len(), 0);
    }

    #[test]
    fn spikes_and_pits_stand_off_the_median_of_their_neighbours() {
        let r = dem(30, 30, |c, r| match (c, r) {
            (10, 10) => clean(c, r) + 50.0,
            (20, 15) => clean(c, r) - 45.0,
            _ => clean(c, r),
        });
        let (_, rows) = check(&r, serde_json::json!({}));
        assert_eq!(kinds(&rows, "spike"), 1, "{rows:?}");
        assert_eq!(kinds(&rows, "pit"), 1, "{rows:?}");
        // Neither is also a step, nor are its neighbours: the drops beside them are as large.
        assert_eq!(rows.len(), 2, "{rows:?}");
        let spike = rows.iter().find(|r| r.0 == "spike").unwrap();
        assert!((45.0..55.0).contains(&spike.1), "{spike:?}");
        assert!(rows.iter().find(|r| r.0 == "pit").is_some_and(|p| (40.0..50.0).contains(&p.1)), "{rows:?}");
        let (_, rows) = check(&r, serde_json::json!({"spikeHeight": 60}));
        assert_eq!(rows.len(), 0);
    }

    #[test]
    fn values_out_of_range_and_flat_runs_at_the_ends_are_runs() {
        // An undeclared no-data of -32768 five cells long, a sea at 0 (the band's lowest) along
        // the last row, and a lake held level at 809 (inside the range): only the first two count.
        let r = dem(120, 12, |c, r| match (c, r) {
            (40..45, 5) => -32768.0,
            (_, 11) => 0.0,
            (60..120, 3) => 809.0,
            _ => rough(c, r),
        });
        let (result, rows) = check(&r, serde_json::json!({}));
        let out: Vec<_> = rows.iter().filter(|r| r.0 == "out of range").collect();
        assert_eq!(out.len(), 1, "{rows:?}");
        assert_eq!((out[0].1, out[0].2), (5.0, -32768.0));
        let flat: Vec<_> = rows.iter().filter(|r| r.0 == "flat run").collect();
        assert_eq!(flat.len(), 1, "{rows:?}");
        assert_eq!((flat[0].1, flat[0].2), (120.0, 0.0));
        // The out-of-range cells aren't compared, so their neighbours aren't spikes or pits.
        assert_eq!(kinds(&rows, "spike") + kinds(&rows, "pit"), 0, "{rows:?}");
        assert!(result.messages[0].contains("1 run(s) of values out of range (5 cells), 1 flat run(s)"), "{}", result.messages[0]);
        // A run shorter than asked is not one, nor a value inside a wider range.
        let (_, rows) = check(&r, serde_json::json!({"runLength": 121}));
        assert_eq!(kinds(&rows, "flat run"), 0);
        let (_, rows) = check(&r, serde_json::json!({"lowest": -40000}));
        assert_eq!(kinds(&rows, "out of range"), 0);
    }

    #[test]
    fn past_the_point_limit_the_first_findings_are_kept_alike_on_any_threads() {
        // A checkerboard of 0 and 100 m: every cell inside the edge a spike or a pit.
        let r = dem(400, 400, |c, r| if (c + r) % 2 == 0 { 100.0 } else { 0.0 });
        let p = serde_json::json!({"input": "dem"});
        let layers: Vec<_> = [1, 2, 7]
            .into_iter()
            .map(|threads| {
                let result = run_on(&One(&r), &p, Some(threads)).unwrap();
                assert_eq!(result.losses.len(), 1);
                assert!(result.losses[0].text.starts_with("58404 of the 158404 findings"), "{}", result.losses[0].text);
                let Output::Layer(out) = result.output else { panic!("a layer") };
                out.features
            })
            .collect();
        assert_eq!(layers[0].len(), MAX_POINTS);
        assert!(layers.iter().all(|l| *l == layers[0]));
    }

    #[test]
    fn the_same_findings_on_any_threads_below_the_limit() {
        let r =
            dem(300, 260, |c, r| clean(c % 60, r % 50) - if c >= 150 { 30.0 } else { 0.0 } + if (c * r) % 997 == 3 { 60.0 } else { 0.0 });
        let p = serde_json::json!({"input": "dem"});
        let runs: Vec<_> = [1, 3, 8]
            .into_iter()
            .map(|threads| {
                let result = run_on(&One(&r), &p, Some(threads)).unwrap();
                let Output::Layer(out) = result.output else { panic!("a layer") };
                (out.features, result.messages)
            })
            .collect();
        assert!(!runs[0].0.is_empty());
        assert!(runs.iter().all(|r| *r == runs[0]));
    }

    #[test]
    fn the_range_s_defaults_are_the_plausible_elevations() {
        let spec = crate::tool("checkDem").unwrap();
        let default = |n: &str| spec.params.iter().find(|p| p.name == n).unwrap().default.parse::<f64>().unwrap();
        assert_eq!((default("lowest"), default("highest")), PLAUSIBLE_ELEVATION);
    }

    #[test]
    fn thresholds_that_make_no_sense_are_refused() {
        let r = dem(10, 10, clean);
        for (p, says) in [
            (serde_json::json!({"stepRatio": 0.5}), "stepRatio must be 1 or more"),
            (serde_json::json!({"stepHeight": 0}), "must be more than 0"),
            (serde_json::json!({"lowest": 10, "highest": 10}), "lowest must be below highest"),
            (serde_json::json!({"runLength": 1.5}), "runLength must be a whole number"),
            (serde_json::json!({"spikeHeight": "high"}), "spikeHeight must be a number"),
            (serde_json::json!({"band": 2}), "band"),
        ] {
            let mut p = p;
            p["input"] = Json::from("dem");
            let e = run(&One(&r), &p).unwrap_err();
            assert!(e.contains(says), "{e}");
        }
        let empty = dem(5, 5, |_, _| -9999.0);
        assert!(run(&One(&empty), &serde_json::json!({"input": "dem"})).unwrap_err().contains("has no values to check"));
    }
}
