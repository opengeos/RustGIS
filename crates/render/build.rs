//! Fetches the Natural Earth sample data (public domain) that RustGIS embeds for its basemap
//! outline and World Atlas sample. The files live in opengeos/rustgis-assets, not in this repo.
//!
//! Set `RUSTGIS_DATA_DIR` to a folder holding the files to build offline.

use std::path::{Path, PathBuf};
use std::process::Command;

const BASE: &str = "https://assets.rustgis.app/data";
const FILES: [&str; 4] = ["countries.geojson", "cities.geojson", "rivers.geojson", "lakes.geojson"];

fn valid(p: &Path) -> bool {
    std::fs::read(p).is_ok_and(|b| b.len() > 2 && b.first() == Some(&b'{'))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RUSTGIS_DATA_DIR");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    let local = std::env::var_os("RUSTGIS_DATA_DIR").map(PathBuf::from);
    for f in FILES {
        let dest = out.join(f);
        if let Some(dir) = &local {
            let src = dir.join(f);
            if std::fs::copy(&src, &dest).is_err() || !valid(&dest) {
                panic!("RUSTGIS_DATA_DIR: {} is missing or not GeoJSON", src.display());
            }
            continue;
        }
        if valid(&dest) {
            continue;
        }
        let url = format!("{BASE}/{f}");
        let ok = Command::new("curl")
            .args(["-fsSL", "--retry", "3", "--connect-timeout", "20", "-o"])
            .arg(&dest)
            .arg(&url)
            .status()
            .is_ok_and(|s| s.success());
        if !ok || !valid(&dest) {
            let _ = std::fs::remove_file(&dest);
            panic!("could not download {url}; check the network or set RUSTGIS_DATA_DIR to a folder with {}", FILES.join(", "));
        }
    }
}
