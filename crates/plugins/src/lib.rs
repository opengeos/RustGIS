//! L3: plugins. A plugin is a WebAssembly module with a `plugin.json` manifest, packaged as a
//! `.rgplugin` file; it adds commands (buttons on the Plugins tab, entries in command search,
//! ids for scripts and the command line) and geoprocessing tools.
//!
//! Plugins run sandboxed (see [`runtime`]): they can't touch files or the network, and they
//! reach the project only through RustGIS's own commands, within the permissions their
//! manifest declares and the user accepted. The same module runs on the desktop, on the
//! command line and in the browser.
//!
//! Loaded plugins live for the whole process (like the geoprocessing tools they add), in a
//! list shared by every session. See `docs/plugins.md` for the format and the ABI.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod manifest;
pub mod package;
pub mod registry;
pub mod runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod tests;
pub mod tools;

use std::sync::{Arc, RwLock};

use serde_json::{Value, json};

pub use manifest::{CommandDecl, Entry, GroupDecl, ItemDecl, Manifest, MenuDecl, Permission, TabDecl, ToolDecl};
pub use package::Package;
pub use runtime::{Host, NoHost};

/// Plugin command and tool ids start with this: `plugin:<plugin>/<command>`.
pub const PREFIX: &str = "plugin:";

/// A loaded plugin.
pub struct Plugin {
    pub manifest: Manifest,
    /// SHA-256 of the package it was loaded from.
    pub sha256: String,
    /// Where it came from: the installed package's path, a plugin folder, or a URL.
    pub source: String,
    /// Whether its commands and tools are offered.
    pub enabled: bool,
    module: wasmi::Module,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("id", &self.manifest.id)
            .field("version", &self.manifest.version)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl Plugin {
    /// A summary for lists and the command line.
    pub fn info(&self) -> Value {
        let m = &self.manifest;
        json!({
            "id": m.id, "name": m.name, "version": m.version, "description": m.description,
            "author": m.author, "homepage": m.homepage, "license": m.license,
            "permissions": m.permissions.iter().map(|p| p.code()).collect::<Vec<_>>(),
            "commands": m.commands.iter().map(|c| json!({"id": command_id(&m.id, &c.id), "label": c.label, "params": c.params})).collect::<Vec<_>>(),
            "tools": m.tools.iter().map(|t| json!({"id": command_id(&m.id, &t.id), "name": t.name})).collect::<Vec<_>>(),
            "tab": m.tab.as_ref().map(|t| t.label.clone()),
            "enabled": self.enabled, "source": self.source, "sha256": self.sha256,
        })
    }
}

static PLUGINS: RwLock<Vec<Arc<Plugin>>> = RwLock::new(Vec::new());
/// The hosted registry's blocklist (replaced when it is fetched), and what other registries
/// have added this session (they can withdraw plugins, never bring one back).
static BLOCKED: RwLock<Vec<registry::Blocked>> = RwLock::new(Vec::new());
static BLOCKED_EXTRA: RwLock<Vec<registry::Blocked>> = RwLock::new(Vec::new());

/// The loaded plugins, by name.
pub fn list() -> Vec<Arc<Plugin>> {
    let mut v = PLUGINS.read().map(|v| v.clone()).unwrap_or_default();
    v.sort_by_key(|p| p.manifest.name.to_lowercase());
    v
}

pub fn get(id: &str) -> Option<Arc<Plugin>> {
    PLUGINS.read().ok()?.iter().find(|p| p.manifest.id == id).cloned()
}

/// `plugin:<plugin>/<item>`.
pub fn command_id(plugin: &str, item: &str) -> String {
    format!("{PREFIX}{plugin}/{item}")
}

/// The plugin and item of a `plugin:<plugin>/<item>` id.
pub fn split_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(PREFIX)?.split_once('/')
}

/// Loads a package (replacing a loaded plugin with the same id, keeping whether it was
/// enabled). `source` says where it came from.
pub fn load(pkg: Package, source: &str) -> Result<Arc<Plugin>, String> {
    Ok(register(prepare(pkg, source)?))
}

/// Checks a package and compiles its module, without loading it yet ([`register`] does).
pub fn prepare(pkg: Package, source: &str) -> Result<Plugin, String> {
    if let Some(b) = blocked(&pkg.manifest.id, &pkg.sha256) {
        let why = if b.reason.is_empty() { String::new() } else { format!(": {}", b.reason) };
        return Err(format!("{} was withdrawn from the plugin registry{why}", pkg.manifest.name));
    }
    // A validator bug must not take the app down.
    let module = std::panic::catch_unwind(|| runtime::compile(&pkg.wasm))
        .unwrap_or_else(|_| Err("the module couldn't be read".into()))
        .map_err(|e| format!("{}: {e}", pkg.manifest.name))?;
    Ok(Plugin { manifest: pkg.manifest, sha256: pkg.sha256, source: source.to_string(), enabled: true, module })
}

/// Loads a prepared plugin, replacing a loaded one with the same id (and keeping whether it
/// was enabled).
pub fn register(mut plugin: Plugin) -> Arc<Plugin> {
    plugin.enabled = get(&plugin.manifest.id).is_none_or(|p| p.enabled);
    let plugin = Arc::new(plugin);
    if let Ok(mut v) = PLUGINS.write() {
        v.retain(|p| p.manifest.id != plugin.manifest.id);
        v.push(plugin.clone());
    }
    tools::reload();
    plugin
}

/// Removes a loaded plugin; returns whether there was one.
pub fn unload(id: &str) -> bool {
    let removed = PLUGINS.write().map(|mut v| {
        let n = v.len();
        v.retain(|p| p.manifest.id != id);
        v.len() != n
    });
    tools::reload();
    removed.unwrap_or(false)
}

/// Turns a plugin's commands and tools on or off.
pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    let mut v = PLUGINS.write().map_err(|_| "the plugin list is unavailable")?;
    let i = v.iter().position(|p| p.manifest.id == id).ok_or_else(|| format!("no plugin '{id}'"))?;
    let p = v.get(i).ok_or("no plugin")?;
    if p.enabled != enabled {
        let new =
            Plugin { manifest: p.manifest.clone(), sha256: p.sha256.clone(), source: p.source.clone(), enabled, module: p.module.clone() };
        if let Some(slot) = v.get_mut(i) {
            *slot = Arc::new(new);
        }
    }
    drop(v);
    tools::reload();
    Ok(())
}

/// Replaces the blocklist; loaded plugins it names are unloaded. Returns their names.
pub fn set_blocklist(list: Vec<registry::Blocked>) -> Vec<String> {
    if let Ok(mut b) = BLOCKED.write() {
        *b = list;
    }
    unload_blocked()
}

/// Applies a blocklist fetched beside the registry at `registry_url`: the hosted registry's
/// replaces the blocklist; another registry's only adds to it for this session, so a custom
/// registry can't bring back a plugin the hosted one withdrew. Returns the plugins unloaded.
pub fn apply_blocklist(registry_url: &str, list: Vec<registry::Blocked>) -> Vec<String> {
    if registry::is_default(registry_url) {
        return set_blocklist(list);
    }
    if let Ok(mut b) = BLOCKED_EXTRA.write() {
        for e in list {
            if !b.contains(&e) {
                b.push(e);
            }
        }
    }
    unload_blocked()
}

fn unload_blocked() -> Vec<String> {
    let pulled: Vec<Arc<Plugin>> = list_blocked();
    for p in &pulled {
        unload(&p.manifest.id);
    }
    pulled.iter().map(|p| p.manifest.name.clone()).collect()
}

fn list_blocked() -> Vec<Arc<Plugin>> {
    PLUGINS.read().map(|v| v.iter().filter(|p| blocked(&p.manifest.id, &p.sha256).is_some()).cloned().collect()).unwrap_or_default()
}

/// The blocklist entry for a package, if it's blocked.
pub fn blocked(id: &str, sha256: &str) -> Option<registry::Blocked> {
    [&BLOCKED, &BLOCKED_EXTRA].iter().find_map(|l| l.read().ok().and_then(|b| registry::blocked(&b, id, sha256).cloned()))
}

/// A command an enabled plugin adds.
#[derive(Clone)]
pub struct PluginCommand {
    pub plugin: Arc<Plugin>,
    pub decl: CommandDecl,
    /// `plugin:<plugin>/<command>`.
    pub id: String,
}

impl PluginCommand {
    /// Whether running it can change the project (and so takes an undo snapshot).
    pub fn mutates(&self) -> bool {
        self.plugin.manifest.has(Permission::Edit)
    }
}

/// Every enabled plugin's commands, plugin by plugin.
pub fn commands() -> Vec<PluginCommand> {
    list()
        .into_iter()
        .filter(|p| p.enabled)
        .flat_map(|p| {
            p.manifest.commands.clone().into_iter().map(move |decl| PluginCommand {
                id: command_id(&p.manifest.id, &decl.id),
                plugin: p.clone(),
                decl,
            })
        })
        .collect()
}

/// An enabled plugin's command by its full id.
pub fn command(id: &str) -> Option<PluginCommand> {
    let (pid, cid) = split_id(id)?;
    let plugin = get(pid).filter(|p| p.enabled)?;
    let decl = plugin.manifest.commands.iter().find(|c| c.id == cid)?.clone();
    Some(PluginCommand { id: id.to_string(), plugin, decl })
}

/// Runs a plugin command, letting it reach the project through `host`.
pub fn run_command(cmd: &PluginCommand, host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let request = json!({"abi": manifest::ABI, "kind": "command", "id": cmd.decl.id, "params": params});
    runtime::call(&cmd.plugin.module, host, &request, runtime::COMMAND_FUEL).map_err(|e| format!("{}: {e}", cmd.decl.label))
}

/// Calls a plugin's module directly (tools, tests).
pub(crate) fn call(plugin: &Plugin, host: &mut dyn Host, request: &Value, fuel: u64) -> Result<Value, String> {
    runtime::call(&plugin.module, host, request, fuel)
}
