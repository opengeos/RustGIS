//! GDAL, optionally: formats the built-in readers don't know (KML, GPX, File Geodatabase, DXF,
//! GML, MapInfo, OpenStreetMap…) read through GDAL's `ogr2ogr` program when it is installed.
//!
//! Each layer is converted once into a FlatGeobuf with a spatial index (in WGS 84, reprojected by
//! GDAL) in the cache folder, then read like any FlatGeobuf, so large files stream a view at a
//! time too. Nothing links to GDAL: without the program, these formats report how to get it.
//! Layers mixing geometry types (KML often does) become one converted layer per type.

use crate::{DataError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// File types read through GDAL. A File Geodatabase is a folder ending in `.gdb`.
pub const EXTENSIONS: &[&str] =
    &["kml", "kmz", "gpx", "gml", "dxf", "gdb", "tab", "mif", "sqlite", "osm", "pbf", "geojsonl", "geojsons", "gmt", "vrt"];

/// Most layers converted from one file.
const MAX_LAYERS: usize = 64;

/// Whether a file name is read through GDAL.
pub fn is_gdal_name(name: &str) -> bool {
    let n = name.trim_end_matches(['/', '\\']);
    let ext = Path::new(n).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str())
}

pub use super::Family;

/// A layer of a GDAL dataset, converted.
#[derive(Clone, Debug, PartialEq)]
pub struct Converted {
    /// The FlatGeobuf in the cache.
    pub path: PathBuf,
    /// The layer's name (with its geometry kind, when the layer mixed them).
    pub name: String,
    /// The layer in the source.
    pub layer: String,
    pub family: Option<Family>,
}

/// A GDAL program (`ogr2ogr`, `ogrinfo`): from `RUSTGIS_GDAL_BIN` (a folder), else on the `PATH`.
pub fn program(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let dirs: Vec<PathBuf> = std::env::var_os("RUSTGIS_GDAL_BIN")
        .map(PathBuf::from)
        .into_iter()
        .chain(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default())
        .collect();
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// Whether GDAL's programs are installed.
pub fn available() -> bool {
    program("ogr2ogr").is_some() && program("ogrinfo").is_some()
}

fn missing() -> DataError {
    DataError::Unsupported(
        "this format needs GDAL: install it (ogr2ogr and ogrinfo on the PATH, or RUSTGIS_GDAL_BIN naming their folder) and add the file again"
            .into(),
    )
}

/// Where converted layers are kept: the platform's cache folder, or `RUSTGIS_GDAL_CACHE`.
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("RUSTGIS_GDAL_CACHE") {
        return Some(PathBuf::from(d));
    }
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("RustGIS").join("cache"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/RustGIS"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|d| d.join("rustgis"))
    };
    base.map(|b| b.join("gdal"))
}

/// A layer: its name, whether it holds one kind of geometry (`Some`) or mixes them (`None`), and
/// its feature count when GDAL knows it.
#[derive(Clone, Debug, PartialEq)]
struct LayerInfo {
    name: String,
    single: bool,
    count: Option<i64>,
}

/// The vector layers of a dataset, from `ogrinfo -json`.
fn list(src: &Path) -> Result<Vec<LayerInfo>> {
    let ogrinfo = program("ogrinfo").ok_or_else(missing)?;
    let out = Command::new(ogrinfo).arg("-json").arg("-ro").arg("-so").arg("-nomd").arg(src).output()?;
    if !out.status.success() {
        return Err(DataError::Format(format!("GDAL: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| DataError::Format(format!("GDAL: unreadable ogrinfo output ({e}); GDAL 3.7 or later is needed")))?;
    let layers = json.get("layers").and_then(|l| l.as_array()).cloned().unwrap_or_default();
    Ok(layers
        .iter()
        .filter_map(|l| {
            let name = l.get("name")?.as_str()?.to_string();
            let geom = l.get("geometryFields").and_then(|g| g.as_array()).and_then(|g| g.first())?;
            let ty = geom.get("type").and_then(|t| t.as_str()).unwrap_or("Geometry").to_ascii_lowercase();
            let single = ["point", "linestring", "polygon", "curve", "surface"].iter().any(|k| ty.contains(k));
            Some(LayerInfo { name, single, count: l.get("featureCount").and_then(|c| c.as_i64()) })
        })
        .take(MAX_LAYERS)
        .collect())
}

/// The vector layers of a dataset.
pub fn layers(src: &Path) -> Result<Vec<String>> {
    Ok(list(src)?.into_iter().map(|l| l.name).collect())
}

/// Converts the dataset's layers (or just `layer`) to FlatGeobufs, reusing earlier conversions of
/// the same file.
/// `crs` gives the coordinate system of a file that doesn't say (`EPSG:2263`; a DXF often doesn't).
pub fn convert(src: &Path, layer: Option<&str>, crs: Option<&str>) -> Result<Vec<Converted>> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for GDAL conversions".into()))?;
    convert_in(&dir, src, layer, crs)
}

fn convert_in(dir: &Path, src: &Path, layer: Option<&str>, crs: Option<&str>) -> Result<Vec<Converted>> {
    if let Some(c) = crs
        && (c.len() > 64 || c.starts_with('-') || !c.chars().all(|ch| ch.is_ascii_alphanumeric() || ":_.".contains(ch)))
    {
        return Err(DataError::Format(format!("crs '{c}': give a code such as EPSG:2263")));
    }
    if !available() {
        return Err(missing());
    }
    let all = list(src)?;
    let chosen: Vec<LayerInfo> = match layer {
        Some(l) => all.into_iter().filter(|x| x.name.eq_ignore_ascii_case(l)).collect(),
        None => all.into_iter().filter(|x| x.count != Some(0)).collect(),
    };
    if chosen.is_empty() {
        return Err(DataError::Format(match layer {
            Some(l) => format!("no layer '{l}'"),
            None => "no layers with features".into(),
        }));
    }
    let mut out = Vec::new();
    let mut last = None;
    for l in chosen {
        let families: Vec<Option<Family>> = if l.single { vec![None] } else { Family::ALL.iter().copied().map(Some).collect() };
        for family in families {
            match convert_one_in(dir, src, &l.name, family, crs) {
                Ok(Some(path)) => out.push(Converted {
                    path,
                    name: match family {
                        Some(f) => format!("{} ({})", l.name, f.label()),
                        None => l.name.clone(),
                    },
                    layer: l.name.clone(),
                    family,
                }),
                // A family the layer doesn't have.
                Ok(None) => {}
                Err(e) if family.is_none() => return Err(e),
                // A family filter the driver can't apply: the other families may still convert.
                Err(e) => last = Some(e),
            }
        }
    }
    if out.is_empty() {
        return Err(last.unwrap_or_else(|| DataError::Format("GDAL found no features to read".into())));
    }
    Ok(out)
}

/// Converts one layer (or one geometry family of it) into the cache; `None` when it has no
/// features. The cache key covers the file's path, size and modification time.
pub fn convert_one(src: &Path, layer: &str, family: Option<Family>, crs: Option<&str>) -> Result<Option<PathBuf>> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for GDAL conversions".into()))?;
    convert_one_in(&dir, src, layer, family, crs)
}

fn convert_one_in(dir: &Path, src: &Path, layer: &str, family: Option<Family>, crs: Option<&str>) -> Result<Option<PathBuf>> {
    use std::hash::{Hash, Hasher};
    let ogr2ogr = program("ogr2ogr").ok_or_else(missing)?;
    std::fs::create_dir_all(dir)?;
    let full = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let meta = std::fs::metadata(&full)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (full.to_string_lossy(), layer, family.map(Family::label), crs, meta.len(), meta.modified().ok()).hash(&mut h);
    let out = dir.join(format!("{:016x}.fgb", h.finish()));
    let empty = dir.join(format!("{:016x}.empty", h.finish()));
    if empty.exists() {
        return Ok(None);
    }
    if out.metadata().is_ok_and(|m| m.len() > 0) {
        return Ok(Some(out));
    }
    let tmp = dir.join(format!("{:016x}.{}.tmp.fgb", h.finish(), std::process::id()));
    let run = |reproject: bool| -> std::io::Result<std::process::Output> {
        let _ = std::fs::remove_file(&tmp);
        let mut c = Command::new(&ogr2ogr);
        c.args(["-f", "FlatGeobuf", "-skipfailures", "-nlt", "PROMOTE_TO_MULTI", "-dim", "XY", "-lco", "SPATIAL_INDEX=YES"]);
        if let Some(crs) = crs {
            c.args(["-s_srs", crs]);
        }
        if reproject {
            c.args(["-t_srs", "EPSG:4326"]);
        }
        if let Some(f) = family {
            c.args(["-where", f.filter()]);
        }
        c.arg(&tmp).arg(&full).arg(layer);
        c.output()
    };
    let mut res = run(true)?;
    // A layer without a coordinate system can't be reprojected: take it as it is (unless one was
    // given, which GDAL then needs to reproject from).
    if !res.status.success() && crs.is_none() {
        res = run(false)?;
    }
    if !res.status.success() {
        let _ = std::fs::remove_file(&tmp);
        let msg = String::from_utf8_lossy(&res.stderr);
        // The first lines say what went wrong; the rest is the usage text.
        let msg: Vec<&str> = msg.lines().filter(|l| l.contains("ERROR")).take(3).collect();
        return Err(DataError::Format(format!("GDAL: {}", msg.join(" "))));
    }
    // An empty family is remembered, so the file isn't converted again for it.
    // (A filter matching nothing leaves no file at all.)
    let count = if tmp.exists() { super::open_path(&tmp, None).ok().and_then(|s| s.info().count).unwrap_or(0) } else { 0 };
    if count == 0 {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::write(&empty, b"");
        return Ok(None);
    }
    std::fs::rename(&tmp, &out)?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_gdal_name("a.KML") && is_gdal_name("/x/roads.gdb/") && is_gdal_name("t.gpx"));
        assert!(!is_gdal_name("a.geojson") && !is_gdal_name("a.fgb") && !is_gdal_name("noext"));
    }

    /// A KML mixing points and polygons, through GDAL when it is installed (skipped otherwise).
    #[test]
    fn converts_a_mixed_kml_when_gdal_is_installed() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rustgis-gdal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kml = dir.join("mixed.kml");
        std::fs::write(
            &kml,
            r#"<?xml version="1.0" encoding="UTF-8"?><kml xmlns="http://www.opengis.net/kml/2.2"><Document><name>mixed</name>
<Placemark><name>a</name><Point><coordinates>10,20</coordinates></Point></Placemark>
<Placemark><name>b</name><Point><coordinates>11,21</coordinates></Point></Placemark>
<Placemark><name>c</name><Polygon><outerBoundaryIs><LinearRing><coordinates>0,0 1,0 1,1 0,0</coordinates></LinearRing></outerBoundaryIs></Polygon></Placemark>
</Document></kml>"#,
        )
        .unwrap();
        let cache = dir.join("cache");
        let out = convert_in(&cache, &kml, None, None).unwrap();
        let kinds: Vec<(String, u64)> = out
            .iter()
            .map(|c| {
                let s = crate::source::open_path(&c.path, None).unwrap();
                (c.name.clone(), s.info().count.unwrap_or(0))
            })
            .collect();
        assert_eq!(kinds, vec![("mixed (points)".to_string(), 2), ("mixed (polygons)".to_string(), 1)], "{kinds:?}");
        // Converted once: the second call reuses the cache.
        let again = convert_in(&cache, &kml, Some("mixed"), None).unwrap();
        assert_eq!(again.iter().map(|c| &c.path).collect::<Vec<_>>(), out.iter().map(|c| &c.path).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(dir);
    }
}
