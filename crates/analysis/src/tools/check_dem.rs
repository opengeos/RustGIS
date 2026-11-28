//! Check DEM (#3151): a point for each step in an elevation model, and a summary of them. It
//! reports and changes nothing: filling or smoothing is the user's choice.
//!
//! A step is measured against the ground on either side of it, on the same line: a drop between
//! two neighbours far larger than the drops just before and just after it is a break the slope
//! doesn't explain (a seam, a void filled at another level, a reservoir's edge in a surface
//! model), where a cliff, whose face goes on dropping, is not. On a 1-second SRTM tile of the
//! Quadrilátero Ferrífero (S21W044), 82% of its 1,740 steps lie within a cell of where SRTM and
//! the Copernicus 30 m model differ by more than 10 m (12% of all cells do); the ANADEM terrain
//! model of the Serra do Brigadeiro has 16 a million cells, on rock walls, where comparing with
//! the eight neighbours' median drop instead flagged 438.
use super::*;
use rustgis_data::loss::{LossCode, LossMessage};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The most findings written as points. The rest are counted in the summary only, so a model
/// covered with them still gives a layer that can be drawn and listed.
const MAX_POINTS: usize = 100_000;
/// About how many cells one item of the scan reads: enough to share out on threads, few enough
/// items that a small model is checked in a few looks at Cancel.
const BLOCK_CELLS: usize = 1 << 16;
/// The eight neighbours, as (column, row) steps.
const AROUND: [(i64, i64); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

/// One finding: its cell, its size (the drop) and the elevation there.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Finding {
    col: u32,
    row: u32,
    size: f64,
    elevation: f64,
}

/// One band of the model, and the step thresholds the parameters give.
struct Dem<'a> {
    r: &'a rustgis_data::raster::Raster,
    band: usize,
    ratio: f64,
    height: f64,
}

impl Dem<'_> {
    /// The value at a cell: `None` off the grid and for no-data.
    fn level(&self, col: i64, row: i64) -> Option<f64> {
        self.r.value(self.band, col, row).map(f64::from)
    }

    /// The largest drop from a cell, at `v`, to a neighbour that is a step: more than the step
    /// height and more than `ratio` times the drops beside it on the same line (from the cell
    /// behind to this one, and from the neighbour to the one beyond it). With it, whether every
    /// line could be followed: `false` at the edge and next to no-data.
    fn step(&self, col: i64, row: i64, v: f64) -> (Option<f64>, bool) {
        let (mut largest, mut whole): (Option<f64>, bool) = (None, true);
        for (dc, dr) in AROUND {
            let (Some(next), Some(behind), Some(beyond)) =
                (self.level(col + dc, row + dr), self.level(col - dc, row - dr), self.level(col + 2 * dc, row + 2 * dr))
            else {
                whole = false;
                continue;
            };
            let (drop, beside) = (v - next, (behind - v).abs().max((next - beyond).abs()));
            if drop > self.height && drop > self.ratio * beside && largest.is_none_or(|l| drop > l) {
                largest = Some(drop);
            }
        }
        (largest, whole)
    }
}

/// What one block of rows holds, counted: cells with a value, those with a line that couldn't be
/// followed, and steps.
#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    cells: u64,
    partial: u64,
    steps: u64,
}

impl Counts {
    fn steps(&self) -> usize {
        usize::try_from(self.steps).unwrap_or(usize::MAX)
    }
}

/// Checks `rows`, counting what it finds and handing each step to `keep`, in row order.
fn scan(dem: &Dem<'_>, rows: Range<u32>, keep: &mut dyn FnMut(Finding)) -> Result<Counts, String> {
    let mut n = Counts::default();
    let mut pace = crate::control::Pace::default();
    for row in rows {
        for col in 0..dem.r.width {
            if pace.stop(1) {
                return Err(crate::control::Cancelled.into());
            }
            let Some(v) = dem.level(i64::from(col), i64::from(row)) else { continue };
            let (step, whole) = dem.step(i64::from(col), i64::from(row), v);
            n.cells += 1;
            n.partial += u64::from(!whole);
            if let Some(size) = step {
                n.steps += 1;
                keep(Finding { col, row, size, elevation: v });
            }
        }
    }
    Ok(n)
}

/// Checks a band of an elevation model and writes a point for each finding.
pub(super) fn run(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    run_on(inp, p, None)
}

/// A threshold from the parameters: `default` when left out or empty.
fn threshold(p: &Json, key: &str, default: f64) -> Result<f64, String> {
    match p.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(Json::String(s)) if s.trim().is_empty() => Ok(default),
        Some(_) => num(p, key).ok_or_else(|| format!("{key} must be a number")),
    }
}

/// [`run`] on `threads` threads (the shared count when `None`): the same layer and summary on any.
///
/// The rows are scanned in blocks of about [`BLOCK_CELLS`] cells on the threads, looking at
/// Cancel along each row, the findings counted and kept as they come while fewer than
/// [`MAX_POINTS`] are. Past that many, which ones were kept depends on the threads, so a second
/// pass takes the first [`MAX_POINTS`] again, block by block from the top. Beyond the raster as it
/// is held, memory is the counts of each block and at most [`MAX_POINTS`] findings and points.
pub(super) fn run_on(inp: &dyn Inputs, p: &Json, threads: Option<usize>) -> Result<ToolResult, String> {
    let name = s(p, "input");
    let r = inp.raster(name).ok_or("choose an elevation raster")?.raster;
    r.require_coordinates()?;
    let bands = zonal_bands(p.get("band").unwrap_or(&Json::Null), r.band_count())?;
    let [band] = bands.as_slice() else { return Err("choose one band of elevations".into()) };
    let (ratio, height) = (threshold(p, "stepRatio", 5.0)?, threshold(p, "stepHeight", 20.0)?);
    if ratio < 1.0 {
        return Err("stepRatio must be 1 or more: a step is a drop that many times the drops beside it".into());
    }
    if height <= 0.0 {
        return Err("stepHeight must be more than 0".into());
    }
    let dem = Dem { r, band: *band, ratio, height };
    let per = (BLOCK_CELLS / (r.width as usize).max(1)).clamp(1, u32::MAX as usize) as u32;
    let blocks: Vec<Range<u32>> = (0..r.height).step_by(per as usize).map(|top| top..top.saturating_add(per).min(r.height)).collect();
    let cells = (r.width as usize).saturating_mul(r.height as usize);
    let threads = threads.unwrap_or_else(|| crate::parallel::threads_for_rows(blocks.len(), cells, 0));
    crate::control::begin(blocks.len().saturating_mul(2));
    let taken = AtomicUsize::new(0);
    let scanned = crate::parallel::try_map_on(
        &blocks,
        threads,
        || (),
        |_, _, rows| {
            let mut kept = Vec::new();
            let n = scan(&dem, rows.clone(), &mut |f| {
                if taken.fetch_add(1, Ordering::Relaxed) < MAX_POINTS {
                    kept.push(f);
                }
            })?;
            Ok::<_, String>((n, kept))
        },
    )?;
    let total = scanned.iter().fold(Counts::default(), |t, (n, _)| Counts {
        cells: t.cells + n.cells,
        partial: t.partial + n.partial,
        steps: t.steps + n.steps,
    });
    let findings: Vec<Finding> = if total.steps() <= MAX_POINTS {
        scanned.into_iter().flat_map(|(_, kept)| kept).collect()
    } else {
        // The first MAX_POINTS findings by block, scanned again for them alone.
        let mut left = MAX_POINTS;
        let mut plan = Vec::new();
        for (rows, (n, _)) in blocks.iter().zip(&scanned) {
            let here = n.steps().min(left);
            if here > 0 {
                plan.push((rows.clone(), here));
                left -= here;
            }
        }
        drop(scanned);
        let parts = crate::parallel::try_map_on(
            &plan,
            threads,
            || (),
            |_, _, (rows, take)| {
                let mut kept = Vec::with_capacity(*take);
                scan(&dem, rows.clone(), &mut |f| kept.extend((kept.len() < *take).then_some(f)))?;
                Ok::<_, String>(kept)
            },
        )?;
        parts.into_iter().flatten().collect()
    };
    crate::control::finish();
    if total.cells == 0 {
        return Err(format!("band {} of '{name}' has no values to check", band + 1));
    }
    let fields =
        vec![Field::new("kind", FieldType::Text), Field::new("size", FieldType::Double), Field::new("elevation", FieldType::Double)];
    let mut out = Dataset::new(out_name(p, format!("{name}_CheckDEM")), GeometryType::Point, fields);
    let mut placeless = 0usize;
    let mut pace = crate::control::Pace::default();
    for f in &findings {
        if pace.stop(64) {
            return Err(crate::control::Cancelled.into());
        }
        match r.transformer().to_lonlat(r.transform.to_world(f64::from(f.col) + 0.5, f64::from(f.row) + 0.5)) {
            Some(at) => {
                out.push_next(Some(Geometry::Point(at)), vec![Value::Text("step".into()), Value::Float(f.size), Value::Float(f.elevation)])
            }
            None => placeless += 1,
        }
    }
    let mut messages = vec![format!(
        "Checked {} cell(s) of band {} of '{name}': {} step(s); {:.3}% of the cells with a value are affected",
        total.cells,
        band + 1,
        total.steps,
        total.steps as f64 * 100.0 / total.cells as f64,
    )];
    if total.partial > 0 {
        messages.push(format!(
            "{} cell(s) at the edge or next to no-data lack a neighbour to compare with on some line: the step rule skips those lines",
            total.partial
        ));
    }
    if let Some((fw, fh)) = r.overview {
        messages.push(format!(
            "'{name}' is held as an overview of its file ({} × {} of its {fw} × {fh} cells): a step a cell or two wide at full resolution may not show in it",
            r.width, r.height
        ));
    }
    let mut losses = Vec::new();
    let found = total.steps();
    if found > findings.len() {
        let text = format!(
            "{} of the {found} findings are counted but not written as points: the layer holds the first {}, from the top row down. Raise the thresholds, or check part of the model, to see the rest",
            found - findings.len(),
            findings.len()
        );
        losses.push(LossMessage { code: LossCode::DroppedOverLimit, text });
    }
    if placeless > 0 {
        let text = format!("{placeless} finding(s) whose cell couldn't be placed in longitude and latitude were left out of the layer");
        losses.push(LossMessage { code: LossCode::DroppedFeature, text });
    }
    Ok(ToolResult { output: Output::Layer(out), messages, losses })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::raster::{ColorHint, GeoTransform, Raster};

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

    /// The steps as (size, cell), and what was said.
    fn check(r: &Raster, p: Json, threads: usize) -> (Vec<(f64, (i64, i64))>, Vec<String>) {
        let mut p = p;
        p["input"] = Json::from("dem");
        let result = run_on(&One(r), &p, Some(threads)).unwrap();
        let Output::Layer(out) = &result.output else { panic!("a point layer") };
        let rows = out.features.iter().map(|f| match (&f.attrs[..], &f.geometry) {
            ([Value::Text(kind), Value::Float(size), _], Some(Geometry::Point(at))) if kind == "step" => (*size, r.pixel_at(*at).unwrap()),
            _ => panic!("{f:?}"),
        });
        (rows.collect(), result.messages.into_iter().chain(result.losses.into_iter().map(|l| l.text)).collect())
    }

    #[test]
    fn mountain_ground_with_a_cliff_has_no_steps() {
        // A round hill and a face dropping 40 m a cell for six cells: slopes, not breaks.
        let ground = |c: u32, r: u32| {
            let (x, y) = (c as f32, r as f32);
            rough(c, r) + 300.0 * (-((x - 20.0).powi(2) + (y - 20.0).powi(2)) / 60.0).exp() - 40.0 * (x - 30.0).clamp(0.0, 6.0)
        };
        let (rows, said) = check(&dem(60, 50, ground), serde_json::json!({}), 1);
        assert_eq!(rows, []);
        assert_eq!(said[0], "Checked 3000 cell(s) of band 1 of 'dem': 0 step(s); 0.000% of the cells with a value are affected");
        // The two outer rings of cells can't follow every line, and that is said.
        assert!(said[1].starts_with("424 cell(s) at the edge"), "{said:?}");
    }

    #[test]
    fn a_seam_is_a_line_of_steps_on_its_high_side_on_any_threads() {
        // The right part 25 m lower than the left, as two tiles mosaicked at different levels.
        let r = dem(40, 300, |c, r| rough(c, r) - if c >= 12 { 25.0 } else { 0.0 });
        let (rows, said) = check(&r, serde_json::json!({}), 1);
        assert_eq!(rows.iter().map(|f| f.1).collect::<Vec<_>>(), (0..300).map(|row| (11, row)).collect::<Vec<_>>());
        assert!(rows.iter().all(|f| (20.0..33.0).contains(&f.0)), "{rows:?}");
        assert!(said[0].contains(": 300 step(s)"), "{said:?}");
        assert_eq!(check(&r, serde_json::json!({}), 4), (rows, said));
        // Below the step height, or with a ratio its sides reach, it isn't one.
        assert_eq!(check(&r, serde_json::json!({"stepHeight": 40}), 1).0, []);
        assert_eq!(check(&r, serde_json::json!({"stepRatio": 100}), 1).0, []);
    }

    #[test]
    fn past_the_point_limit_the_first_steps_are_kept_alike_on_any_threads() {
        // Terraces two cells wide, 100 m apart: each high terrace's edge cell is a step.
        let r = dem(500, 500, |c, _| if (c / 2) % 2 == 0 { 100.0 } else { 0.0 });
        let (rows, said) = check(&r, serde_json::json!({}), 1);
        assert_eq!(rows.len(), MAX_POINTS);
        assert!(said.iter().any(|s| s.starts_with("24500 of the 124500 findings")), "{said:?}");
        assert_eq!(check(&r, serde_json::json!({}), 7).0, rows);
    }

    #[test]
    fn thresholds_that_make_no_sense_are_refused() {
        let r = dem(10, 10, rough);
        for (p, says) in [
            (serde_json::json!({"stepRatio": 0.5}), "stepRatio must be 1 or more"),
            (serde_json::json!({"stepHeight": 0}), "stepHeight must be more than 0"),
            (serde_json::json!({"stepHeight": "high"}), "stepHeight must be a number"),
            (serde_json::json!({"band": 2}), "band"),
        ] {
            let mut p = p;
            p["input"] = Json::from("dem");
            assert!(run(&One(&r), &p).unwrap_err().contains(says), "{p}");
        }
        let empty = dem(5, 5, |_, _| -9999.0);
        assert!(run(&One(&empty), &serde_json::json!({"input": "dem"})).unwrap_err().contains("has no values to check"));
    }
}
