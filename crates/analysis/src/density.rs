//! Kernel density estimation for point vector layers.
//!
//! Generates a continuous magnitude-per-unit-area density raster from point features,
//! optionally weighted by a numeric attribute, using Quartic (biweight) or Gaussian kernels.
//! Calculations are performed in the linear units of a projected coordinate system (e.g. metres).

use std::f64::consts::PI;

use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};
use rustgis_geom::{Coord, Extent};

/// Supported 2D kernel functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelType {
    /// 2D Quartic (biweight) compact-support kernel (default):
    /// K(u) = (3 / (π * h²)) * (1 - u²)² for u < 1, 0 otherwise.
    Quartic,
    /// 2D Gaussian kernel truncated at 3σ = h:
    /// K(d) = (1 / (2π * σ²)) * exp(-d² / (2σ²)) for d <= h, 0 otherwise.
    Gaussian,
}

impl KernelType {
    pub fn parse(s: &str) -> Result<Self, String> {
        let t = s.trim();
        if t.eq_ignore_ascii_case("quartic") || t.eq_ignore_ascii_case("biweight") || t.is_empty() {
            Ok(KernelType::Quartic)
        } else if t.eq_ignore_ascii_case("gaussian") {
            Ok(KernelType::Gaussian)
        } else {
            Err(format!("unknown kernel type '{t}': choose 'Quartic' or 'Gaussian'"))
        }
    }
}

/// Upper limit on total output raster cells to prevent memory exhaustion (64 MB for f32).
pub const MAX_DENSITY_CELLS: usize = 16_000_000;

/// Upper limit on aggregate cell visits across all points to prevent CPU exhaustion.
pub const MAX_DENSITY_WORK: usize = 400_000_000;

/// Default target grid resolution along the longer side of the extent when pixel size is not specified.
const DEFAULT_GRID_STEPS: f64 = 250.0;

/// Computes an automatic bandwidth (search radius) for 2D point data using Silverman's rule of thumb.
///
/// h = 0.9 * min(StandardDistance, sqrt(1 / ln(2)) * MedianDistance) * N^(-0.2)
///
/// Falls back safely and deterministically to extent-based scaling for small samples or identical coordinates
/// (span / 30.0 when extent > 0, or 1000.0 coordinate-space units for single or coincident points).
pub fn silverman_bandwidth(points: &[(Coord, f64)], extent: &Extent) -> f64 {
    let n = points.len();
    if n < 2 {
        let span = extent.width().max(extent.height());
        return if span > 1e-4 { span / 30.0 } else { 1000.0 };
    }

    let (sum_x, sum_y) = points.iter().fold((0.0, 0.0), |(sx, sy), (p, _)| (sx + p.x, sy + p.y));
    let mean_x = sum_x / n as f64;
    let mean_y = sum_y / n as f64;

    let sum_sq_dist = points.iter().fold(0.0, |acc, (p, _)| acc + (p.x - mean_x).powi(2) + (p.y - mean_y).powi(2));
    let std_dist = (sum_sq_dist / n as f64).sqrt();

    let mut dists: Vec<f64> = points.iter().map(|(p, _)| ((p.x - mean_x).powi(2) + (p.y - mean_y).powi(2)).sqrt()).collect();
    dists.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_dist = if dists.len() % 2 == 1 {
        dists.get(dists.len() / 2).copied().unwrap_or(0.0)
    } else {
        let mid = dists.len() / 2;
        let d1 = dists.get(mid.saturating_sub(1)).copied().unwrap_or(0.0);
        let d2 = dists.get(mid).copied().unwrap_or(d1);
        (d1 + d2) * 0.5
    };

    // A = sqrt(1 / ln(2)) ≈ 1.2011224087864498
    let a = (1.0 / 2.0f64.ln()).sqrt();
    let term = std_dist.min(a * median_dist);

    if term > 1e-6 {
        let n_factor = (n as f64).powf(-0.2);
        let h = 0.9 * term * n_factor;
        if h.is_finite() && h > 1e-4 {
            return h;
        }
    }

    // Fallback: fraction of extent or fixed distance
    let span = extent.width().max(extent.height());
    if span > 1e-4 { (span / 30.0).max(1e-4) } else { 1000.0 }
}

/// Automatically calculates cell size so the grid spans approximately 250 cells across its maximum dimension.
pub fn auto_cell_size(extent: &Extent) -> f64 {
    let span = extent.width().max(extent.height());
    (span / DEFAULT_GRID_STEPS).max(1e-4)
}

/// Generates a continuous kernel density raster from weighted points in a projected CRS.
pub fn kernel_density(
    points: &[(Coord, f64)],
    custom_extent: Option<&Extent>,
    cell_size: f64,
    bandwidth: f64,
    kernel: KernelType,
    name: &str,
    crs: Crs,
) -> Result<Raster, String> {
    if points.is_empty() {
        return Err("kernel density requires at least one point".into());
    }
    if !bandwidth.is_finite() || bandwidth <= 0.0 {
        return Err("search radius (bandwidth) must be a positive number".into());
    }
    let h_sq = bandwidth * bandwidth;
    if !h_sq.is_finite() || h_sq <= 0.0 {
        return Err("search radius is too large or invalid".into());
    }
    if !cell_size.is_finite() || cell_size <= 0.0 {
        return Err("pixel size must be a positive number".into());
    }
    if crs.is_geographic() {
        return Err("kernel density requires a projected coordinate system with linear units (e.g. meters or feet), not geographic coordinates in degrees".into());
    }

    // Determine spatial extent
    let mut pt_extent = Extent::EMPTY;
    for (p, _) in points {
        pt_extent.expand(*p);
    }
    if pt_extent.is_empty() {
        return Err("points have empty extent".into());
    }

    let base_extent = match custom_extent {
        Some(e) if !e.is_empty() => *e,
        _ => {
            Extent::new(pt_extent.min_x - bandwidth, pt_extent.min_y - bandwidth, pt_extent.max_x + bandwidth, pt_extent.max_y + bandwidth)
        }
    };

    let w = ((base_extent.width() / cell_size).ceil().max(1.0)) as usize;
    let h_rows = ((base_extent.height() / cell_size).ceil().max(1.0)) as usize;

    let total_cells = w.checked_mul(h_rows).ok_or("grid dimensions overflow")?;
    if total_cells > MAX_DENSITY_CELLS {
        return Err(format!("{w} × {h_rows} cells is too large; choose a larger pixel size"));
    }

    // Estimate total cell visits bounded by the point kernel window and output grid
    let cells_across = (2.0 * bandwidth / cell_size).ceil();
    let cells_per_point = if cells_across.is_finite() && cells_across < total_cells as f64 {
        let dim = (cells_across as usize).saturating_add(2);
        dim.saturating_mul(dim).min(total_cells)
    } else {
        total_cells
    };
    let total_work = points.len().saturating_mul(cells_per_point);
    if total_work > MAX_DENSITY_WORK {
        return Err(format!(
            "{w} × {h_rows} cells from {} points is too much work; choose a larger pixel size or a smaller search radius",
            points.len()
        ));
    }

    let origin_x = base_extent.min_x;
    let origin_y = base_extent.max_y;
    let gt = GeoTransform { origin_x, origin_y, pixel_width: cell_size, pixel_height: -cell_size };

    let mut grid = vec![0.0f32; total_cells];

    enum KernelParams {
        Quartic { factor: f64 },
        Gaussian { factor: f64, two_sigma_sq: f64 },
    }

    let kernel_params = match kernel {
        KernelType::Quartic => KernelParams::Quartic { factor: 3.0 / (PI * h_sq) },
        KernelType::Gaussian => {
            // Truncate at 3σ = bandwidth (standard compact-support approximation capturing >98.8% of mass)
            let sigma = bandwidth / 3.0;
            let two_sigma_sq = 2.0 * sigma * sigma;
            KernelParams::Gaussian { factor: 1.0 / (PI * two_sigma_sq), two_sigma_sq }
        }
    };

    for &(p, wgt) in points {
        if wgt <= 0.0 || !wgt.is_finite() || !p.is_finite() {
            continue;
        }

        let col_min = (((p.x - bandwidth - origin_x) / cell_size).floor() as isize).clamp(0, (w - 1) as isize) as usize;
        let col_max = (((p.x + bandwidth - origin_x) / cell_size).ceil() as isize).clamp(0, (w - 1) as isize) as usize;

        let row_min = (((origin_y - (p.y + bandwidth)) / cell_size).floor() as isize).clamp(0, (h_rows - 1) as isize) as usize;
        let row_max = (((origin_y - (p.y - bandwidth)) / cell_size).ceil() as isize).clamp(0, (h_rows - 1) as isize) as usize;

        for row in row_min..=row_max {
            let y_cell = origin_y - (row as f64 + 0.5) * cell_size;
            let dy = p.y - y_cell;
            let dy_sq = dy * dy;
            if dy_sq >= h_sq {
                continue;
            }
            let rem_sq = h_sq - dy_sq;
            let row_offset = row * w;

            for col in col_min..=col_max {
                let x_cell = origin_x + (col as f64 + 0.5) * cell_size;
                let dx = p.x - x_cell;
                let dx_sq = dx * dx;
                if dx_sq >= rem_sq {
                    continue;
                }

                let d_sq = dx_sq + dy_sq;
                let contrib = match kernel_params {
                    KernelParams::Quartic { factor } => {
                        let val = 1.0 - d_sq / h_sq;
                        wgt * factor * val * val
                    }
                    KernelParams::Gaussian { factor, two_sigma_sq } => wgt * factor * (-d_sq / two_sigma_sq).exp(),
                };

                if let Some(cell) = grid.get_mut(row_offset + col) {
                    *cell += contrib as f32;
                }
            }
        }
    }

    Raster::new(name, w as u32, h_rows as u32, vec![grid], gt, crs, None, ColorHint::Gray, "Float32").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_point_quartic_density_shape() {
        let pt = Coord::new(100.0, 100.0);
        let points = vec![(pt, 1.0)];
        let h = 20.0;
        let cell = 2.0;
        let r = kernel_density(&points, None, cell, h, KernelType::Quartic, "test_density", Crs::WebMercator).unwrap();

        assert_eq!(r.bands.len(), 1);
        let band = &r.bands[0].data;

        // Peak should be at or near point center
        let (p_col, p_row) = r.transform.to_pixel(pt);
        let c_idx = (p_row.floor() as usize) * (r.width as usize) + (p_col.floor() as usize);
        let peak_val = band[c_idx];
        assert!(peak_val > 0.0, "peak value must be positive, got {peak_val}");

        // Outside radius h (e.g. corner of grid), density must be 0.0
        let corner_val = band[0];
        assert_eq!(corner_val, 0.0, "corner value outside bandwidth must be zero");

        // Verify values are strictly non-negative and finite
        for &v in band {
            assert!(v >= 0.0 && v.is_finite());
        }
    }

    #[test]
    fn weights_are_proportional() {
        let pt = Coord::new(500.0, 500.0);
        let r1 = kernel_density(&[(pt, 1.0)], None, 5.0, 50.0, KernelType::Quartic, "d1", Crs::WebMercator).unwrap();
        let r2 = kernel_density(&[(pt, 3.5)], None, 5.0, 50.0, KernelType::Quartic, "d2", Crs::WebMercator).unwrap();

        let b1 = &r1.bands[0].data;
        let b2 = &r2.bands[0].data;
        assert_eq!(b1.len(), b2.len());

        for (v1, v2) in b1.iter().zip(b2.iter()) {
            let diff = (*v2 as f64) - (*v1 as f64) * 3.5;
            assert!(diff.abs() < 1e-8, "weight scaling mismatch: {v1} vs {v2}");
        }
    }

    #[test]
    fn additive_point_contributions() {
        let pt1 = Coord::new(100.0, 100.0);
        let pt2 = Coord::new(110.0, 100.0);
        let h = 30.0;
        let cell = 5.0;

        let r_both = kernel_density(&[(pt1, 1.0), (pt2, 1.0)], None, cell, h, KernelType::Quartic, "both", Crs::WebMercator).unwrap();
        let r1 = kernel_density(
            &[(pt1, 1.0)],
            Some(&Extent::new(70.0, 70.0, 140.0, 130.0)),
            cell,
            h,
            KernelType::Quartic,
            "r1",
            Crs::WebMercator,
        )
        .unwrap();
        let r2 = kernel_density(
            &[(pt2, 1.0)],
            Some(&Extent::new(70.0, 70.0, 140.0, 130.0)),
            cell,
            h,
            KernelType::Quartic,
            "r2",
            Crs::WebMercator,
        )
        .unwrap();

        assert_eq!(r1.bands[0].data.len(), r2.bands[0].data.len());
        for (i, (&v1, &v2)) in r1.bands[0].data.iter().zip(r2.bands[0].data.iter()).enumerate() {
            let sum = v1 + v2;
            let actual = r_both.bands[0].data[i];
            assert!((sum - actual).abs() < 1e-5, "additive discrepancy at {i}: {sum} vs {actual}");
        }
    }

    #[test]
    fn bandwidth_spread_effect() {
        let pt = Coord::new(100.0, 100.0);
        let r_narrow = kernel_density(&[(pt, 1.0)], None, 2.0, 10.0, KernelType::Quartic, "narrow", Crs::WebMercator).unwrap();
        let r_wide = kernel_density(&[(pt, 1.0)], None, 2.0, 40.0, KernelType::Quartic, "wide", Crs::WebMercator).unwrap();

        // Narrow bandwidth has higher peak density and smaller spatial footprint
        let peak_narrow = r_narrow.bands[0].data.iter().copied().fold(0.0f32, f32::max);
        let peak_wide = r_wide.bands[0].data.iter().copied().fold(0.0f32, f32::max);
        assert!(peak_narrow > peak_wide, "narrow bandwidth peak ({peak_narrow}) should exceed wide bandwidth peak ({peak_wide})");
    }

    #[test]
    fn gaussian_kernel_works() {
        let pt = Coord::new(200.0, 200.0);
        let r = kernel_density(&[(pt, 2.0)], None, 5.0, 50.0, KernelType::Gaussian, "gauss", Crs::WebMercator).unwrap();
        assert_eq!(r.bands.len(), 1);
        let peak = r.bands[0].data.iter().copied().fold(0.0f32, f32::max);
        assert!(peak > 0.0);
    }

    #[test]
    fn silverman_bandwidth_is_deterministic_and_positive() {
        let pts = vec![
            (Coord::new(0.0, 0.0), 1.0),
            (Coord::new(100.0, 0.0), 1.0),
            (Coord::new(0.0, 100.0), 1.0),
            (Coord::new(100.0, 100.0), 1.0),
            (Coord::new(50.0, 50.0), 1.0),
        ];
        let ext = Extent::new(0.0, 0.0, 100.0, 100.0);
        let h1 = silverman_bandwidth(&pts, &ext);
        let h2 = silverman_bandwidth(&pts, &ext);
        assert_eq!(h1, h2, "bandwidth must be deterministic");
        assert!(h1 > 0.0 && h1.is_finite());

        // Coincident points fallback
        let coincident = vec![(Coord::new(50.0, 50.0), 1.0), (Coord::new(50.0, 50.0), 1.0)];
        let h_coincident = silverman_bandwidth(&coincident, &Extent::new(50.0, 50.0, 50.0, 50.0));
        assert!(h_coincident > 0.0 && h_coincident.is_finite());
    }

    #[test]
    fn geotiff_roundtrip_preserves_values_and_metadata() {
        let pt = Coord::new(500_000.0, 4_500_000.0); // UTM coordinates
        let crs = Crs::from_epsg(32618).unwrap();
        let r = kernel_density(&[(pt, 1.0)], None, 50.0, 500.0, KernelType::Quartic, "utm_density", crs).unwrap();

        let bytes = rustgis_data::raster::geotiff::write(&r, 0).unwrap();
        assert!(!bytes.is_empty());

        let reopened = rustgis_data::raster::geotiff::read(std::io::Cursor::new(&bytes), "utm_density").unwrap();
        assert_eq!(reopened.width, r.width);
        assert_eq!(reopened.height, r.height);
        assert_eq!(reopened.crs, crs);
        assert_eq!(reopened.transform, r.transform);

        assert_eq!(reopened.bands[0].data, r.bands[0].data, "GeoTIFF Float32 roundtrip must be bit-exact");
    }

    #[test]
    fn excessive_workload_is_rejected() {
        // 50,000 points in a 100x100 grid (10,000 cells <= MAX_DENSITY_CELLS) with large radius
        let pts: Vec<(Coord, f64)> = (0..50_000).map(|_| (Coord::new(500.0, 500.0), 1.0)).collect();
        let ext = Extent::new(0.0, 0.0, 1000.0, 1000.0);
        let err = kernel_density(&pts, Some(&ext), 10.0, 1000.0, KernelType::Quartic, "huge", Crs::WebMercator).unwrap_err();
        assert!(err.contains("too much work"), "{err}");
    }

    #[test]
    fn bandwidth_overflow_is_rejected() {
        let pts = vec![(Coord::new(0.0, 0.0), 1.0)];
        let err = kernel_density(&pts, None, 1.0, 1e200, KernelType::Quartic, "huge_bw", Crs::WebMercator).unwrap_err();
        assert!(err.contains("too large or invalid"), "{err}");
    }

    #[test]
    fn geographic_crs_is_rejected() {
        let pts = vec![(Coord::new(10.0, 20.0), 1.0)];
        let err = kernel_density(&pts, None, 1.0, 10.0, KernelType::Quartic, "geo_crs", Crs::LonLat).unwrap_err();
        assert!(err.contains("requires a projected coordinate system"), "{err}");
    }

    #[test]
    fn non_finite_points_are_skipped() {
        let pts = vec![(Coord::new(f64::NAN, 100.0), 1.0), (Coord::new(100.0, f64::INFINITY), 1.0), (Coord::new(100.0, 100.0), 1.0)];
        let r = kernel_density(&pts, None, 2.0, 20.0, KernelType::Quartic, "finite_test", Crs::WebMercator).unwrap();
        for &val in &r.bands[0].data {
            assert!(val.is_finite());
            assert!(val >= 0.0);
        }
    }
}
