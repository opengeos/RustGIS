//! Synthetic land-cover comparisons: full paths, projected LOD and pre-projection LOD.
use super::*;
use rustgis_data::Dataset;

/// Raster-like polygon boundaries with thirty vertices per cell, generated without fixtures.
fn landcover(side: usize) -> Map {
    let mut ds = Dataset::new("land cover", GeometryType::Polygon, vec![]);
    let step = 0.5 / side as f64;
    for x in 0..side {
        for y in 0..side {
            let (ax, ay) = (36.0 + x as f64 * step, -1.5 + y as f64 * step);
            let mut ring = Vec::new();
            for t in 0..7 {
                let d = if (x + y + t) % 3 == 0 { step * 0.3 / 7.0 } else { 0.0 };
                ring.push(Coord::new(ax + t as f64 * step / 7.0, ay + d));
                ring.push(Coord::new(ax + (t + 1) as f64 * step / 7.0, ay + d));
            }
            ring.push(Coord::new(ax + step, ay + step));
            for t in (1..=7).rev() {
                let d = if (x + y + t) % 3 == 0 { step * 0.3 / 7.0 } else { 0.0 };
                ring.push(Coord::new(ax + t as f64 * step / 7.0, ay + step - d));
                ring.push(Coord::new(ax + (t - 1) as f64 * step / 7.0, ay + step - d));
            }
            ring.push(ring[0]);
            ds.push(Some(Geometry::Polygon(vec![vec![ring]])), vec![]);
        }
    }
    let mut map = Map::new("LOD comparison");
    map.basemap = rustgis_map::Basemap::Blank;
    map.graticule = false;
    map.add_dataset(ds);
    map
}

/// Splits identical non-overlapping polygons into layers below the LOD threshold, making a
/// reference render from unthinned geometry without a production-only testing switch.
fn unthinned(map: &Map) -> Map {
    let mut full = map.clone();
    let layer = full.layers.remove(0);
    for (i, rows) in layer.dataset.features.chunks(3000).enumerate() {
        let mut copy = layer.clone();
        copy.id = i as u32 + 1;
        copy.dataset.features = rows.to_vec().into();
        copy.clear_cache();
        full.layers.push(copy);
    }
    full
}

/// Measures image differences in device-pixel RGBA channels, including antialiasing.
fn difference(a: &Pixmap, b: &Pixmap) -> (f64, u8) {
    let deltas: Vec<_> = a.data().iter().zip(b.data()).map(|(a, b)| a.abs_diff(*b)).collect();
    (deltas.iter().map(|&d| f64::from(d)).sum::<f64>() / deltas.len() as f64, deltas.into_iter().max().unwrap_or(0))
}

/// Largest mean RGBA difference in a 16-by-16 device-pixel region. Local means tolerate
/// antialiased edges while detecting a missing cell that a whole-frame mean can hide.
fn local_difference(a: &Pixmap, b: &Pixmap) -> f64 {
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    let (w, h) = (a.width() as usize, a.height() as usize);
    let mut worst = 0.0f64;
    for y in (0..h).step_by(16) {
        for x in (0..w).step_by(16) {
            let mut total = 0u64;
            let end_x = (x + 16).min(w);
            let end_y = (y + 16).min(h);
            for row in y..end_y {
                let start = (row * w + x) * 4;
                let end = (row * w + end_x) * 4;
                total += a.data()[start..end].iter().zip(&b.data()[start..end]).map(|(a, b)| u64::from(a.abs_diff(*b))).sum::<u64>();
            }
            worst = worst.max(total as f64 / ((end_x - x) * (end_y - y) * 4) as f64);
        }
    }
    worst
}

/// Compares full and thinned images at six scales, including a deliberate missing-cell control.

#[test]
fn lod_landcover_matches_full_geometry_at_several_zooms_and_densities() {
    let base = landcover(84); // 211,680 vertices: above the whole-layer threshold.
    for ppp in [1.0, 2.0] {
        for across in [3.0, 10.0, 30.0] {
            let mut map = base.clone();
            let mut full = unthinned(&base);
            let view = View {
                width: 420.0,
                height: 320.0,
                pixels_per_point: ppp,
                sky: false,
                camera: Camera::new(
                    lonlat_to_merc(Coord::new(36.25, -1.25)),
                    0.5 / 84.0 * rustgis_geom::MERCATOR_R.to_radians() / across * ppp,
                ),
            };
            let expected = render(&mut full, &view).unwrap();
            let got = render(&mut map, &view).unwrap();
            let background = &expected.pixmap.data()[..4];
            assert!(expected.pixmap.data().as_chunks::<4>().0.iter().any(|p| p != background), "comparison must contain rendered polygons");
            let (mean, max) = difference(&expected.pixmap, &got.pixmap);
            let local = local_difference(&expected.pixmap, &got.pixmap);
            eprintln!("LOD comparison: {across}px, density {ppp}: mean {mean:.4}/255, max {max}, local {local:.4}");
            assert!(local < 12.0, "local channel difference {local} at {across}px, density {ppp}");
            if across == 30.0 && ppp == 1.0 {
                let mut omitted = base.clone();
                omitted.layers[0].dataset.features[42 * 84 + 42].geometry = None;
                let missing = render(&mut unthinned(&omitted), &view).unwrap();
                let local = local_difference(&expected.pixmap, &missing.pixmap);
                eprintln!("Missing-cell control: local {local:.4}");
                assert!(local > 12.0, "localized comparison must detect an omitted central cell");
            }
            assert!(mean < 3.0, "mean channel difference {mean} at {across}px, density {ppp}");
            if across == 3.0 {
                assert!(map.layers[0].projected_cache().is_empty(), "plain drawing must leave full projection lazy");
                assert!(map.layers[0].display_cache().is_some());
            }
            if let Ok(dir) = std::env::var("RUSTGIS_LOD_EVIDENCE_DIR") {
                got.pixmap.save_png(format!("{dir}/lod-{across}-{ppp}.png")).unwrap();
                expected.pixmap.save_png(format!("{dir}/full-{across}-{ppp}.png")).unwrap();
            }
            // Enabling an exact consumer restores full projection, while the fill stays identical.
            map.layers[0].labels.enabled = true;
            let exact = render(&mut map, &view).unwrap();
            assert_eq!(got.pixmap.data(), exact.pixmap.data());
            assert!(!map.layers[0].projected_cache().is_empty());
        }
    }
}

/// Times cold and warm complete renders for a 210,681-cell layer without import costs.
#[test]
#[ignore = "manual benchmark: cargo test --release -p rustgis-render lod_landcover_whole_frame_benchmark -- --ignored --nocapture"]
fn lod_landcover_whole_frame_benchmark() {
    let base = landcover(459);
    let view = View {
        width: 1480.0,
        height: 900.0,
        pixels_per_point: 2.0,
        sky: false,
        camera: Camera::new(lonlat_to_merc(Coord::new(36.25, -1.25)), 0.5 / 459.0 * rustgis_geom::MERCATOR_R.to_radians() / 3.0 * 2.0),
    };
    let mut full = unthinned(&base);
    let mut exact = base.clone();
    exact.layers[0].labels.enabled = true; // No label field: forces full projection without labels.
    let mut fast = base.clone();
    for (name, map) in [("unthinned", &mut full), ("full projection then LOD", &mut exact), ("preprojection LOD", &mut fast)] {
        let start = web_time::Instant::now();
        let first = render(map, &view).unwrap();
        let cold = start.elapsed();
        let start = web_time::Instant::now();
        let _ = render(map, &view).unwrap();
        eprintln!(
            "{name}: cold {cold:?}, warm {:?}, cache {} bytes",
            start.elapsed(),
            map.layers.iter().map(|l| l.view_cache_bytes()).sum::<usize>()
        );
        let background = &first.pixmap.data()[..4];
        assert!(first.pixmap.data().as_chunks::<4>().0.iter().any(|p| p != background), "benchmark must contain rendered polygons");
    }
}
