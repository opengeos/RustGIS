//! Installed plugins on disk (desktop and command line): packages in RustGIS's data folder,
//! loaded at startup, plus plugin folders added for development.
//!
//! ```text
//! <data>/plugins/<id>.rgplugin    installed packages
//! <data>/plugins/state.json       {"disabled": [ids], "folders": [plugin folders]}
//! <data>/plugins/blocklist.json   the registry's blocklist, as last fetched
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::{Package, Plugin, registry};

/// The environment variable that names the plugins folder, ahead of the usual place.
pub const ENV: &str = "RUSTGIS_PLUGINS_DIR";

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Uses `dir` as the plugins folder (tests, portable installs); `None`: the usual place.
pub fn set_dir(dir: Option<PathBuf>) {
    if let Ok(mut d) = DIR.lock() {
        *d = dir;
    }
}

/// The plugins folder: `plugins` in the platform's app-data location.
pub fn dir() -> Option<PathBuf> {
    if let Ok(d) = DIR.lock()
        && let Some(d) = d.clone()
    {
        return Some(d);
    }
    if let Some(d) = std::env::var_os(ENV).filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA")).map(|d| PathBuf::from(d).join("RustGIS"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/RustGIS"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("rustgis"))
    };
    base.map(|b| b.join("plugins"))
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default)]
    folders: Vec<String>,
}

fn state_path() -> Option<PathBuf> {
    dir().map(|d| d.join("state.json"))
}

fn read_state() -> State {
    state_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_state(st: &State) -> Result<(), String> {
    let path = state_path().ok_or("there is no folder for plugins")?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(st).map_err(|e| e.to_string())?;
    rustgis_data::io::write_atomic(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Loads every installed package and plugin folder (and the cached blocklist); returns what
/// couldn't be loaded.
pub fn load_installed() -> Vec<String> {
    let mut errors = Vec::new();
    let Some(dir) = dir() else { return errors };
    if let Some(list) = std::fs::read_to_string(dir.join("blocklist.json")).ok().and_then(|t| registry::parse_blocklist(&t).ok()) {
        crate::set_blocklist(list);
    }
    let st = read_state();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rgplugin"))).collect())
        .unwrap_or_default();
    paths.sort();
    // Folders after packages: a development copy wins over an installed one.
    paths.extend(st.folders.iter().map(PathBuf::from));
    for path in paths {
        match Package::from_path(&path).and_then(|pkg| crate::load(pkg, &path.display().to_string())) {
            Ok(p) => {
                if st.disabled.contains(&p.manifest.id) {
                    let _ = crate::set_enabled(&p.manifest.id, false);
                }
            }
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    errors
}

/// Installs a package's bytes: checks it, copies it into the plugins folder (replacing an
/// older copy) and loads it.
pub fn install_bytes(bytes: &[u8]) -> Result<Arc<Plugin>, String> {
    let pkg = Package::from_zip(bytes)?;
    let dir = dir().ok_or("there is no folder for plugins")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.rgplugin", pkg.manifest.id));
    // Load first: a package that doesn't load isn't installed.
    let plugin = crate::load(pkg, &path.display().to_string())?;
    rustgis_data::io::write_atomic(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(plugin)
}

/// Installs from a path: a `.rgplugin` file is copied in; a plugin folder (for development) is
/// remembered and loaded from where it is.
pub fn install_path(path: &Path) -> Result<Arc<Plugin>, String> {
    if !path.is_dir() {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return install_bytes(&bytes);
    }
    let pkg = Package::from_dir(path)?;
    let plugin = crate::load(pkg, &path.display().to_string())?;
    let mut st = read_state();
    let p = path.display().to_string();
    if !st.folders.contains(&p) {
        st.folders.push(p);
        write_state(&st)?;
    }
    Ok(plugin)
}

/// Uninstalls a plugin: unloads it, deletes its package, or forgets its folder.
pub fn uninstall(id: &str) -> Result<(), String> {
    let plugin = crate::get(id).ok_or_else(|| format!("no plugin '{id}'"))?;
    let mut st = read_state();
    st.folders.retain(|f| *f != plugin.source);
    st.disabled.retain(|d| d != id);
    if let Some(d) = dir() {
        let path = d.join(format!("{id}.rgplugin"));
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    write_state(&st)?;
    crate::unload(id);
    Ok(())
}

/// Enables or disables a plugin, and remembers it.
pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    crate::set_enabled(id, enabled)?;
    let mut st = read_state();
    st.disabled.retain(|d| d != id);
    if !enabled {
        st.disabled.push(id.to_string());
    }
    write_state(&st)
}

/// Keeps the blocklist for offline starts, and applies it.
pub fn save_blocklist(text: &str) -> Result<Vec<String>, String> {
    let list = registry::parse_blocklist(text)?;
    if let Some(d) = dir() {
        let _ = std::fs::create_dir_all(&d);
        let _ = rustgis_data::io::write_atomic(&d.join("blocklist.json"), text.as_bytes());
    }
    Ok(crate::set_blocklist(list))
}
