//! Times map renders of large data files, zoomed to the whole layer and in a few steps:
//! `cargo run --release -p rustgis-engine --example large_bench -- file.tif [file.gpkg …]`.
//!
//! Each file is added on its own (streamed sources are loaded in place first), then rendered at
//! 1600 × 1000, from its full extent down eight halvings of the resolution (streamed layers
//! settle at each first). `--ppp 2` renders as a high-density screen.
use rustgis_engine::render::{View, render};
use rustgis_engine::{Project, Session};
use rustgis_geom::{Coord, extent_to_merc};
use rustgis_map::Camera;
use serde_json::json;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ppp = args.iter().position(|a| a == "--ppp").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let flag_value = |i: usize| i > 0 && args.get(i - 1).is_some_and(|a| a.starts_with("--"));
    let files: Vec<&String> = args.iter().enumerate().filter(|(i, a)| !a.starts_with("--") && !flag_value(*i)).map(|(_, a)| a).collect();
    for f in files {
        let mut s = Session::new(Project::default());
        s.map_mut().layers.clear();
        let t = Instant::now();
        if let Err(e) = s.run("layer.addData", &json!({ "path": f })) {
            println!("{f}: {e}");
            continue;
        }
        println!("{f}: added in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        // `--cmd 'id={json}'` styles the layer first (Unique Values, say).
        for c in args.windows(2).filter(|w| w.first().is_some_and(|a| a == "--cmd")).filter_map(|w| w.get(1)) {
            let (id, p) = c.split_once('=').unwrap_or((c, "{}"));
            if let Err(e) = s.run(id, &serde_json::from_str(p).unwrap_or_default()) {
                println!("  {id}: {e}");
            }
        }
        let e = extent_to_merc(&s.map().full_extent());
        let (w, h) = (1600.0, 1000.0);
        let res0 = (e.width() / w).max(e.height() / h).max(1e-3);
        // Let streamed sources settle at the first view (they read in the background).
        let cam0 = Camera { center: Coord::new((e.min_x + e.max_x) / 2.0, (e.min_y + e.max_y) / 2.0), res: res0 };
        let t = Instant::now();
        for _ in 0..200 {
            let r = render(s.map_mut(), &View { width: w, height: h, pixels_per_point: ppp, camera: cam0 });
            if !r.is_some_and(|r| r.data_pending) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let n: usize = s.map().layers.iter().map(|l| l.dataset.features.len()).sum();
        println!("  settled in {:.0} ms ({n} features loaded)", t.elapsed().as_secs_f64() * 1000.0);
        for z in 0..9 {
            let res = res0 / 2f64.powi(z);
            let camera = Camera { center: cam0.center, res };
            let view = View { width: w, height: h, pixels_per_point: ppp, camera };
            // Streamed layers read what this view shows first.
            for _ in 0..200 {
                if !render(s.map_mut(), &view).is_some_and(|r| r.data_pending) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let n = 5;
            let mut times = Vec::new();
            for _ in 0..n {
                let t = Instant::now();
                let _ = render(s.map_mut(), &view);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let first = times.first().copied().unwrap_or(0.0);
            let rest = times.iter().skip(1).sum::<f64>() / (n - 1) as f64;
            let n_feat: usize = s.map().layers.iter().map(|l| l.dataset.features.len()).sum();
            println!("  zoom +{z} (res {res:>12.2} m, {n_feat} features): first {first:>8.1} ms, then {rest:>8.1} ms/frame");
        }
    }
}
