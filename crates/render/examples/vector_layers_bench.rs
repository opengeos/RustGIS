//! Times CPU map renders of a national vector map (#1067) as a user zooms and pans: every feature
//! layer of a GeoPackage, and its heaviest layers alone, at 1× and 2× pixel scale, each frame the
//! exact frame drawn once the map rests.
//!
//! `cargo run --release -p rustgis-render --example vector_layers_bench -- map.gpkg`
//!
//! - **whole:** the whole map, four times (the first builds its level of detail);
//! - **zoom:** from the whole map to about a metre a point over its centre, 1.5× a step, and back
//!   out (the way out finds the levels of detail built on the way in, those still kept);
//! - **pan 10 m / pan 3 m:** west to east across it at 10 and 3 Web Mercator metres a point, a
//!   quarter of the view a step, so every frame brings new features in.
//!
//! Made for swisstopo's swiss-map-vector25 (`SMV25_CHLV95LN02.gpkg`, 336,000 features in LV95,
//! from data.geo.admin.ch, one map sheet). For a profiler, `BENCH_ONLY=2,zoom` (a pixel scale and a sequence) runs
//! that sequence three times on the whole map and nothing else; `BENCH_THREADS=1` draws on one thread.
use rustgis_geom::{Coord, lonlat_to_merc};
use rustgis_map::{Basemap, Camera, Map};
use rustgis_render::{View, render};

/// Frames kept so far (`BENCH_PNG`).
static FRAME: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: vector_layers_bench map.gpkg");
        return;
    };
    // `BENCH_THREADS=1` draws on one thread (the user's thread setting), to compare.
    if let Some(n) = std::env::var("BENCH_THREADS").ok().and_then(|n| n.parse().ok())
        && let Err(e) = rustgis_data::threads::set_max(Some(n))
    {
        eprintln!("{e}");
        return;
    }
    println!("{} threads", rustgis_data::threads::max());
    let t = std::time::Instant::now();
    let file = std::path::Path::new(&path);
    let names = match rustgis_data::source::layers(file) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{path}: {e}");
            return;
        }
    };
    let mut read = Vec::new();
    for name in names {
        match rustgis_data::source::open_path(file, Some(&name)).and_then(|src| rustgis_data::source::materialize(src.as_ref())) {
            Ok((d, _)) => read.push(d),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
    println!("read {} layers in {:.1} s", read.len(), t.elapsed().as_secs_f64());
    // The highest-numbered tables (land cover) at the bottom, as a topographic map stacks them.
    let mut datasets: Vec<_> = read.into_iter().filter(|d| !d.features.is_empty()).collect();
    datasets.sort_by(|a, b| b.name.cmp(&a.name));
    let mut m = Map::new("bench");
    m.basemap = Basemap::Blank;
    let mut layers = Vec::new();
    // The data's extent, longitude/latitude: the sequences are fitted to it.
    let mut extent = rustgis_geom::Extent::EMPTY;
    for d in datasets {
        extent = extent.union(&d.extent());
        let vertices: usize = d.features.iter().filter_map(|f| f.geometry.as_ref()).map(rustgis_map::lod::vertices).sum();
        let (name, rows) = (d.name.clone(), d.features.len());
        layers.push((m.add_dataset(d), name, rows, vertices));
    }
    let total: (usize, usize) = layers.iter().fold((0, 0), |(r, v), l| (r + l.2, v + l.3));
    println!("{} layers, {} features, {} vertices", layers.len(), total.0, total.1);

    let (lo, hi) = (lonlat_to_merc(Coord::new(extent.min_x, extent.min_y)), lonlat_to_merc(Coord::new(extent.max_x, extent.max_y)));
    let centre = Coord::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0);
    // Web Mercator units a pixel showing all of it across 1920 points (a 2× screen shows the same).
    let whole = ((hi.x - lo.x) / 1920.0).max((hi.y - lo.y) / 1080.0);
    println!(
        "extent {:.3},{:.3} to {:.3},{:.3}; whole map at {whole:.1} Mercator m a point",
        extent.min_x, extent.min_y, extent.max_x, extent.max_y
    );
    let camera = |c: Coord, res: f64| Camera { center: c, res, bearing: 0.0, pitch: 0.0 };
    // West to east through the centre at `res`, a quarter of the view a step, from one edge of the
    // data to the other.
    let pan = |res: f64| -> Vec<Camera> {
        let half = 960.0 * res;
        let (a, b) = (lo.x + half, (hi.x - half).max(lo.x + half));
        let steps = ((b - a) / (480.0 * res)).ceil().max(1.0) as usize;
        (0..=steps).map(|i| camera(Coord::new(a + (b - a) * i as f64 / steps as f64, centre.y), res)).collect()
    };
    // From the whole map to about a metre a point, 1.5× a step, and back out.
    let mut zoom: Vec<Camera> = Vec::new();
    let mut res = whole;
    while res > 1.0 {
        zoom.push(camera(centre, res));
        res /= 1.5;
    }
    let back: Vec<Camera> = zoom.iter().rev().skip(1).copied().collect();
    zoom.extend(back);
    let sequences: [(&str, Vec<Camera>); 4] =
        [("whole", (0..4).map(|_| camera(centre, whole)).collect()), ("zoom", zoom), ("pan 10 m", pan(10.0)), ("pan 3 m", pan(3.0))];
    let ids: Vec<u32> = layers.iter().map(|l| l.0).collect();
    let show = |m: &mut Map, only: Option<u32>| {
        for id in &ids {
            if let Some(l) = m.layer_mut(*id) {
                l.visible = only.is_none_or(|o| o == *id);
            }
        }
    };
    let view = |camera: Camera, ppp: f64| View { width: 1920.0, height: 1080.0, pixels_per_point: ppp, camera, sky: false };
    // Each frame's time, ms.
    let run = |m: &mut Map, cams: &[Camera], ppp: f64| -> Vec<f64> {
        cams.iter()
            .map(|c| {
                let t = std::time::Instant::now();
                let r = render(m, &view(*c, ppp));
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                // `BENCH_PNG=dir` keeps each frame drawn, to look at or compare.
                if let (Some(dir), Some(r)) = (std::env::var_os("BENCH_PNG"), &r) {
                    let n = FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let _ = r.pixmap.save_png(std::path::Path::new(&dir).join(format!("frame{n:03}.png")));
                }
                ms
            })
            .collect()
    };
    let summary = |times: &[f64]| {
        let mut s = times.to_vec();
        s.sort_by(f64::total_cmp);
        let median = s.get(s.len() / 2).copied().unwrap_or(0.0);
        let max = s.last().copied().unwrap_or(0.0);
        format!("{:>3} frames, median {median:6.0} ms, slowest {max:6.0} ms, total {:7.0} ms", s.len(), s.iter().sum::<f64>())
    };
    show(&mut m, None);
    if let Some((ppp, name)) =
        std::env::var("BENCH_ONLY").ok().as_deref().and_then(|o| o.split_once(',')).map(|(p, n)| (p.to_string(), n.to_string()))
        && let (Ok(ppp), Some((_, cams))) = (ppp.parse::<f64>(), sequences.iter().find(|s| s.0 == name))
    {
        for _ in 0..3 {
            let times = run(&mut m, cams, ppp);
            let each: Vec<String> = times.iter().zip(cams.iter()).map(|(t, c)| format!("{:.1}m:{t:.0}", c.res)).collect();
            println!("{ppp}x {name}: {}\n  {}", summary(&times), each.join(" "));
        }
        return;
    }
    // The first frame projects every layer and builds the whole map's level of detail.
    let first = run(&mut m, &[camera(centre, whole)], 1.0);
    println!("first frame (projects every layer): {:.0} ms", first.first().copied().unwrap_or(0.0));
    for ppp in [1.0f64, 2.0] {
        for (name, cams) in &sequences {
            show(&mut m, None);
            println!("{ppp}x {name:10} all layers: {}", summary(&run(&mut m, cams, ppp)));
            // Each layer alone on every fourth frame (each built once already, by the whole map's
            // run): the slowest six, with what they draw.
            let sample: Vec<Camera> = cams.iter().step_by(4).copied().collect();
            let mut alone: Vec<(f64, &(u32, String, usize, usize))> = layers
                .iter()
                .map(|l| {
                    show(&mut m, Some(l.0));
                    (run(&mut m, &sample, ppp).iter().sum::<f64>() / sample.len().max(1) as f64, l)
                })
                .collect();
            alone.sort_by(|a, b| b.0.total_cmp(&a.0));
            let sum: f64 = alone.iter().map(|a| a.0).sum();
            println!("{ppp}x {name:10}   every layer alone, summed: {sum:6.0} ms a frame; the slowest:");
            for (ms, (_, layer, rows, v)) in alone.iter().take(6) {
                println!("{ppp}x {name:10}   {layer:30} {rows:>7} features {v:>8} vertices: {ms:6.0} ms a frame");
            }
        }
    }
}
