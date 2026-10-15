//! Plugin packages: a `.rgplugin` file (a zip archive) or a folder, holding `plugin.json` and
//! the WebAssembly module it names, or a process plugin's program and files.

use std::io::Read;

use sha2::{Digest, Sha256};

use crate::manifest::{Manifest, Runtime};

/// The largest package accepted, and the largest module and manifest inside one.
pub const MAX_PACKAGE: usize = 64 * 1024 * 1024;
pub const MAX_MODULE: usize = 48 * 1024 * 1024;
const MAX_MANIFEST: usize = 512 * 1024;
const MAX_ENTRIES: usize = 4_096;

/// A plugin ready to load: its manifest, its module and the SHA-256 of the package it came from.
#[derive(Clone, Debug)]
pub struct Package {
    pub manifest: Manifest,
    /// The WebAssembly module (empty for a process plugin).
    pub wasm: Vec<u8>,
    /// Lower-case hex SHA-256 of the package bytes (for a folder, of its manifest's and module's
    /// hashes).
    pub sha256: String,
    /// A process plugin's folder, where its program runs: the plugin folder itself, or where an
    /// installed package was unpacked (see `store`). `None` until then.
    pub dir: Option<std::path::PathBuf>,
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads at most `limit` bytes of `r`; more is an error.
fn read_limited(r: impl Read, limit: usize, what: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    r.take(limit as u64 + 1).read_to_end(&mut buf).map_err(|e| format!("{what}: {e}"))?;
    if buf.len() > limit {
        return Err(format!("{what} is larger than {} MB", limit / (1024 * 1024)));
    }
    Ok(buf)
}

/// Where `plugin.json` is in a package: at its root (`""`), else in exactly one top-level
/// folder (`"name/"`).
pub(crate) fn root_of<R: std::io::Read + std::io::Seek>(zip: &zip::ZipArchive<R>) -> Result<String, String> {
    let names: Vec<&str> = zip.file_names().collect();
    if names.contains(&"plugin.json") {
        return Ok(String::new());
    }
    let mut tops: Vec<&str> =
        names.iter().filter_map(|n| n.strip_suffix("/plugin.json")).filter(|d| !d.is_empty() && !d.contains('/')).collect();
    tops.sort_unstable();
    tops.dedup();
    match tops.as_slice() {
        [one] => Ok(format!("{one}/")),
        [] => Err("the plugin package has no plugin.json".into()),
        _ => Err("the plugin package has more than one plugin.json".into()),
    }
}

impl Package {
    /// Reads a `.rgplugin` archive: `plugin.json` at its root or inside one top-level folder,
    /// with the module beside it.
    pub fn from_zip(bytes: &[u8]) -> Result<Package, String> {
        if bytes.len() > MAX_PACKAGE {
            return Err(format!("the plugin package is larger than {} MB", MAX_PACKAGE / (1024 * 1024)));
        }
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a plugin package: {e}"))?;
        if zip.len() > MAX_ENTRIES {
            return Err("the plugin package has too many files".into());
        }
        let root = root_of(&zip)?;
        let text = {
            let f = zip.by_name(&format!("{root}plugin.json")).map_err(|e| format!("plugin.json: {e}"))?;
            read_limited(f, MAX_MANIFEST, "plugin.json")?
        };
        let manifest = Manifest::parse(std::str::from_utf8(&text).map_err(|_| "plugin.json is not UTF-8 text")?)?;
        // A process plugin's files are unpacked when it's installed.
        let wasm = if manifest.runtime == Runtime::Process {
            Vec::new()
        } else {
            let f = zip.by_name(&format!("{root}{}", manifest.entry)).map_err(|_| format!("the package has no {}", manifest.entry))?;
            read_limited(f, MAX_MODULE, &manifest.entry)?
        };
        Ok(Package { manifest, wasm, sha256: sha256_hex(bytes), dir: None })
    }

    /// Reads an unpacked plugin: a folder with `plugin.json` (for plugin development).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_dir(dir: &std::path::Path) -> Result<Package, String> {
        let open = |name: &str, limit: usize| -> Result<Vec<u8>, String> {
            let path = dir.join(name);
            let f = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            read_limited(f, limit, name)
        };
        let text = open("plugin.json", MAX_MANIFEST)?;
        let manifest = Manifest::parse(std::str::from_utf8(&text).map_err(|_| "plugin.json is not UTF-8 text")?)?;
        // A process plugin runs from the folder, as it is.
        if manifest.runtime == Runtime::Process {
            return Ok(Package { manifest, wasm: Vec::new(), sha256: sha256_hex(&text), dir: Some(dir.to_path_buf()) });
        }
        let wasm = open(&manifest.entry, MAX_MODULE)?;
        // The manifest and the module: changing either (permissions, commands, code) changes it.
        let sha256 = sha256_hex(&[sha256_hex(&text).as_bytes(), sha256_hex(&wasm).as_bytes()].concat());
        Ok(Package { manifest, wasm, sha256, dir: None })
    }

    /// Reads a package file or a plugin folder.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_path(path: &std::path::Path) -> Result<Package, String> {
        if path.is_dir() {
            return Package::from_dir(path);
        }
        let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Package::from_zip(&read_limited(f, MAX_PACKAGE, "the plugin package")?)
    }
}
