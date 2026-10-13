//! Zarr stores: what they hold, and a variable's 2D slice added as a raster layer — from a folder
//! (read again when the project opens) or a web address (its chunks fetched; the slice is kept in
//! the project).

#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use rustgis_data::raster::{self, Crs, zarr};
use rustgis_map::raster::RasterLayer;
use serde_json::Value;

use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("raster.zarrInfo", "Zarr Variables", "Home › Add › Add Data › Zarr", info)
            .view()
            .params(r#"{"path": "a Zarr store's folder" | "url": "https://… a store with consolidated metadata"}"#),
        C::new("layer.addZarr", "Add Zarr Variable", "Home › Add › Add Data", add).params(
            r#"{"path": folder | "url": "https://…", "variable"?: "its path in the store (default: the first)", "index"?: {"time": 3, "level": 0} (other dimensions' steps, default 0), "crs"?: "EPSG:… for a store that doesn't say", "name"?: text}"#,
        ),
    ]
}

/// Where a store is: a folder, or a web address.
enum Source {
    Dir(String),
    Url(String),
}

fn source(p: &Value) -> Result<Source, String> {
    let s = ps(p, "url").or_else(|| ps(p, "path")).map(str::trim).filter(|s| !s.is_empty()).ok_or("path (a folder) or url is required")?;
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(Source::Url(s.trim_end_matches('/').to_string()))
    } else {
        Ok(Source::Dir(s.to_string()))
    }
}

/// The selection a command names: `variable` and `index` ({dim: step}).
pub fn selection(p: &Value) -> zarr::Selection {
    let index = match p.get("index") {
        Some(Value::Object(m)) => m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_u64()?))).take(32).collect(),
        _ => Vec::new(),
    };
    zarr::Selection { variable: ps(p, "variable").map(str::trim).unwrap_or("").to_string(), index }
}

/// A store's root as a layer's first name: its folder or address's last part, without `.zarr`.
pub fn store_name(root: &str) -> String {
    let last = root.trim_end_matches('/').rsplit(['/', '\\']).next().unwrap_or("zarr");
    last.strip_suffix(".zarr").unwrap_or(last).to_string()
}

/// A slice's layer name: the store's name when it is a `.zarr` folder or address (not a version
/// folder such as CMIP6's `v20190308`), then the variable and its steps (`label` for each, when
/// given, as `tas 1850-01-15`).
pub fn layer_name(root: &str, sel: &zarr::Selection, labels: &[String]) -> String {
    let what = if labels.is_empty() { sel.label() } else { format!("{} {}", sel.variable, labels.join(" ")) };
    if root.trim_end_matches('/').to_ascii_lowercase().ends_with(".zarr") { format!("{} {what}", store_name(root)) } else { what }
}

/// Fetches files of a store by web address, several at a time; a missing one (404, or 403 as
/// S3 answers for a missing key) is left out.
#[cfg(not(target_arch = "wasm32"))]
fn fetch(root: &str, keys: &[String]) -> Result<HashMap<String, Vec<u8>>, String> {
    use std::sync::Mutex;
    let out = Mutex::new(HashMap::new());
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..8.min(keys.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(k) = keys.get(i) else { break };
                    if failure.lock().is_ok_and(|f| f.is_some()) {
                        break;
                    }
                    match crate::render::rustgis_tiles::cache::download_blocking(&format!("{root}/{k}")) {
                        Ok(b) => {
                            if let Ok(mut o) = out.lock() {
                                o.insert(k.clone(), b);
                            }
                        }
                        Err(e) if e.contains("404") || e.contains("403") => {}
                        Err(e) => {
                            if let Ok(mut f) = failure.lock() {
                                f.get_or_insert(format!("{root}/{k}: {e}"));
                            }
                        }
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().ok().flatten() {
        return Err(e);
    }
    out.into_inner().map_err(|_| "a fetch failed".to_string())
}

/// A store's metadata from its web address.
#[cfg(not(target_arch = "wasm32"))]
fn store_at(url: &str) -> Result<zarr::Store, String> {
    let keys: Vec<String> = zarr::METADATA_KEYS.iter().map(|k| k.to_string()).collect();
    let docs: Vec<(String, Vec<u8>)> = fetch(url, &keys)?.into_iter().collect();
    zarr::parse_store(&docs).map_err(|e| format!("{url}: {e}"))
}

fn info(_s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = p;
        Err("in the web app, open Zarr stores with Home › Add Data › Zarr…".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    match source(p)? {
        Source::Dir(d) => {
            let store = zarr::read_metadata(Path::new(&d)).map_err(|e| format!("{d}: {e}"))?;
            let mut chunks = HashMap::new();
            for k in zarr::info_keys(&store) {
                if let Ok(b) = std::fs::read(Path::new(&d).join(&k)) {
                    chunks.insert(k, b);
                }
            }
            Ok(zarr::info(&store, &chunks))
        }
        Source::Url(u) => {
            let store = store_at(&u)?;
            let chunks = fetch(&u, &zarr::info_keys(&store))?;
            Ok(zarr::info(&store, &chunks))
        }
    }
}

/// The variable to read: the one named, else the store's first that can be read.
pub fn pick_variable(store: &zarr::Store, sel: &mut zarr::Selection) -> Result<(), String> {
    if sel.variable.is_empty() {
        sel.variable = store
            .variables()
            .iter()
            .find(|v| v.unsupported().is_none())
            .map(|v| v.path.clone())
            .ok_or("the store has no variable that can be read")?;
    }
    Ok(())
}

pub(crate) fn add(s: &mut Session, p: &Value) -> CmdResult {
    let src = source(p)?;
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut sel = selection(p);
    let crs = ps(p, "crs").map(str::trim).filter(|c| !c.is_empty());
    let crs_parsed = crs.map(Crs::parse).transpose()?;
    let root = match &src {
        Source::Dir(d) | Source::Url(d) => d.clone(),
    };
    // A slice the app read itself (the web app, or a store by address): its GeoTIFF, handed over.
    if let Some(id) = super::f(p, "ran").filter(|v| *v >= 0.0 && v.fract() == 0.0).map(|v| v as u64) {
        let i = s.downloaded.iter().position(|(d, _)| *d == id).ok_or("no such read (it was dropped when another project opened)")?;
        let (_, tif) = s.downloaded.remove(i);
        let mut r = raster::read_bytes("zarr.tif", &tif).map_err(|e| e.to_string())?;
        r.source = Some(root.clone());
        let name = ps(p, "name").map(str::to_string).unwrap_or_else(|| layer_name(&root, &sel, &[]));
        let mut layer = RasterLayer::from_bytes(r, format!("{}.tif", store_name(&root)), tif);
        layer.subdataset = Some(sel.label());
        return super::raster::add_raster(s, &name, layer);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (crs_parsed, src);
        Err("in the web app, add Zarr variables with Home › Add Data › Zarr…".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    match src {
        Source::Dir(d) => {
            let store = zarr::read_metadata(Path::new(&d)).map_err(|e| format!("{d}: {e}"))?;
            pick_variable(&store, &mut sel)?;
            let r = zarr::read_dir(Path::new(&d), &sel.label(), crs).map_err(|e| format!("{d}: {e}"))?;
            let name = ps(p, "name").map(str::to_string).unwrap_or_else(|| r.name.clone());
            // Absolute, so the project finds the store again from anywhere.
            let full = std::path::absolute(&d).map(|p| p.display().to_string()).unwrap_or(d);
            let mut layer = RasterLayer::from_path(r, full);
            layer.subdataset = Some(sel.label());
            layer.crs = crs.map(str::to_string);
            super::raster::add_raster(s, &name, layer)
        }
        Source::Url(u) => {
            let store = store_at(&u)?;
            pick_variable(&store, &mut sel)?;
            let plan = zarr::plan(&store, &sel).map_err(|e| format!("{u}: {e}"))?;
            let chunks = fetch(&u, &plan.keys)?;
            let name = ps(p, "name").map(str::to_string).unwrap_or_else(|| layer_name(&u, &sel, &[]));
            let mut r = zarr::assemble(&store, &plan, &chunks, &name, crs_parsed).map_err(|e| format!("{u}: {e}"))?;
            r.source = Some(u.clone());
            // Kept in the project as a GeoTIFF: a web store isn't fetched again when it opens.
            let tif = raster::geotiff::write(&r, 0).map_err(|e| e.to_string())?;
            let mut layer = RasterLayer::from_bytes(r, format!("{}.tif", store_name(&u)), tif);
            layer.subdataset = Some(sel.label());
            super::raster::add_raster(s, &name, layer)
        }
    }
}
