//! `plugin.json`: what a plugin is and what it adds (commands, geoprocessing tools), checked
//! like any other hostile input.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The plugin ABI this build speaks (see `docs/plugins.md`).
pub const ABI: u32 = 1;

/// Limits on what a manifest may declare.
const MAX_COMMANDS: usize = 64;
const MAX_TOOLS: usize = 64;
const MAX_PARAMS: usize = 32;
const MAX_CHOICES: usize = 64;
const MAX_TEXT: usize = 2_000;
const MAX_LANGS: usize = 64;
const MAX_STRINGS: usize = 20_000;
const MAX_GROUPS: usize = 8;
const MAX_GROUP_ITEMS: usize = 16;
const MAX_MENU_ITEMS: usize = 32;
const MAX_TAB_LABEL: usize = 32;

/// RustGIS's own ribbon tabs (and the Project button): a plugin's tab can't take their names.
pub const RESERVED_TABS: &[&str] = &[
    "Project",
    "Map",
    "Insert",
    "Analysis",
    "View",
    "Edit",
    "Imagery",
    "Share",
    "Plugins",
    "Settings",
    "Help",
    "Feature Layer",
    "Labeling",
    "Data",
    "Raster Layer",
    "Raster Data",
    "Layout",
];

/// What a plugin may do beyond reading the map. Shown to the user before installing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// Change the project: run commands that edit it, add layers.
    Edit,
    /// Run commands that read or write files the user names.
    Files,
    /// Make the map fetch from addresses the plugin gives it (web tile layers).
    Network,
}

impl Permission {
    pub fn code(self) -> &'static str {
        match self {
            Permission::Edit => "edit",
            Permission::Files => "files",
            Permission::Network => "network",
        }
    }
    /// What it lets a plugin do, for the install prompt.
    pub fn describe(self) -> &'static str {
        match self {
            Permission::Edit => "Change the project (add layers, edit features, change symbology)",
            Permission::Files => "Read and write files on this computer",
            Permission::Network => "Load map tiles from addresses it chooses",
        }
    }
}

/// A command the plugin adds: a button on the Plugins tab, an entry in command search and a
/// command id for scripts and the command line (`plugin:<plugin>/<command>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDecl {
    pub id: String,
    pub label: String,
    /// A longer description, shown on hover.
    #[serde(default)]
    pub help: String,
    /// The name of one of RustGIS's own icons (`plugin` when unset or unknown).
    #[serde(default)]
    pub icon: String,
    /// Parameter description for scripts and agents, like a built-in command's.
    #[serde(default = "no_params")]
    pub params: String,
}

fn no_params() -> String {
    "{}".into()
}

/// A tool parameter: the Geoprocessing pane turns its kind into a widget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamDecl {
    pub name: String,
    pub label: String,
    /// `layer`, `vectorLayer`, `polygonLayer`, `pointLayer`, `field`, `numericField`, `number`,
    /// `text`, `choice` or `bool`.
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    /// The default value (any JSON).
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// For `choice`: the choices.
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub help: String,
}

/// A ribbon tab of the plugin's own, beside RustGIS's tabs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabDecl {
    pub label: String,
    pub groups: Vec<GroupDecl>,
}

/// A group of buttons on a plugin's tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupDecl {
    pub label: String,
    pub items: Vec<ItemDecl>,
}

/// A button: a command id or `tool:<id>` (opens the tool in the Geoprocessing pane), or a
/// dropdown menu of them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ItemDecl {
    Entry(String),
    Menu(MenuDecl),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuDecl {
    /// The menu button's label.
    pub menu: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub help: String,
    pub items: Vec<String>,
}

/// What a tab entry (`command-id` or `tool:tool-id`) refers to.
#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    Command(&'a CommandDecl),
    Tool(&'a ToolDecl),
}

/// The parameter kinds a tool may use.
pub const PARAM_KINDS: &[&str] =
    &["layer", "vectorLayer", "polygonLayer", "pointLayer", "field", "numericField", "number", "text", "choice", "bool"];

/// Whether a parameter kind names a layer (its features are passed to the tool).
pub fn is_layer_kind(kind: &str) -> bool {
    matches!(kind, "layer" | "vectorLayer" | "polygonLayer" | "pointLayer")
}

/// A geoprocessing tool the plugin adds (`plugin:<plugin>/<tool>`), listed in the
/// Geoprocessing pane under "Plugins › <toolbox>".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDecl {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    /// The toolbox under "Plugins ›" (the plugin's name when unset).
    #[serde(default)]
    pub toolbox: String,
    #[serde(default)]
    pub params: Vec<ParamDecl>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub license: String,
    /// The WebAssembly module, relative to `plugin.json`.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// The plugin ABI the module was built for.
    #[serde(default = "default_abi")]
    pub abi: u32,
    /// The oldest RustGIS that can run it.
    #[serde(default)]
    pub min_rustgis_version: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub commands: Vec<CommandDecl>,
    #[serde(default)]
    pub tools: Vec<ToolDecl>,
    /// A ribbon tab of its own (optional): its commands and tools in groups and menus.
    #[serde(default)]
    pub tab: Option<TabDecl>,
    /// Translations of the plugin's own strings (labels, help, tool and parameter names), by
    /// language code (`de`, `zh-CN`): `{"de": {"Say Hello": "Hallo sagen"}}`.
    #[serde(default)]
    pub i18n: BTreeMap<String, BTreeMap<String, String>>,
}

/// `s` in language `lang` from `i18n` (translations by language code), or `s`. Tries the exact
/// code, then the language without its region, then any region of it, so `zh` finds `zh-CN`.
pub fn translate<'a>(i18n: &'a BTreeMap<String, BTreeMap<String, String>>, lang: &str, s: &'a str) -> &'a str {
    let base_of = |l: &str| l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase();
    let base = base_of(lang);
    let pick = |want: &dyn Fn(&str) -> bool| {
        i18n.iter().filter(|(k, _)| want(k)).find_map(|(_, t)| t.get(s)).map(String::as_str).filter(|t| !t.is_empty())
    };
    pick(&|k| k.eq_ignore_ascii_case(lang))
        .or_else(|| pick(&|k| k.eq_ignore_ascii_case(&base)))
        .or_else(|| pick(&|k| base_of(k) == base))
        .unwrap_or(s)
}

fn default_entry() -> String {
    "plugin.wasm".into()
}

fn default_abi() -> u32 {
    ABI
}

/// A plugin, command, tool or parameter id: lower-case letters, digits and hyphens (and
/// underscores, for parameter names), starting with a letter, at most 64 characters.
pub fn valid_id(s: &str, underscore: bool) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || (underscore && (c == '_' || c.is_ascii_uppercase())))
}

/// A path inside a package: relative, forward slashes, no `..`.
pub fn valid_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && !s.starts_with('/')
        && !s.contains('\\')
        && !s.contains(':')
        && s.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// A version's numbers (`1.2.3`, `1.2`, `1.2.3-beta.1`: the pre-release part is ignored).
pub fn version_parts(v: &str) -> Option<[u64; 3]> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut out = [0u64; 3];
    let mut n = 0;
    for (i, part) in core.split('.').enumerate() {
        *out.get_mut(i)? = part.parse().ok()?;
        n = i + 1;
    }
    (n > 0).then_some(out)
}

/// Compares two versions; an unreadable one sorts first.
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    version_parts(a).cmp(&version_parts(b))
}

/// RustGIS's own version.
pub const RUSTGIS_VERSION: &str = env!("CARGO_PKG_VERSION");

fn check_text(what: &str, s: &str, required: bool) -> Result<(), String> {
    if required && s.trim().is_empty() {
        return Err(format!("{what} is required"));
    }
    if s.len() > MAX_TEXT {
        return Err(format!("{what} is too long"));
    }
    Ok(())
}

impl Manifest {
    /// Reads and checks `plugin.json`.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_str(text).map_err(|e| format!("plugin.json: {e}"))?;
        m.validate()?;
        Ok(m)
    }

    /// Checks everything a host relies on: ids, sizes, kinds, versions.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id, false) {
            return Err(format!(
                "plugin.json: id '{}' must be lower-case letters, digits and hyphens, starting with a letter",
                self.id.chars().take(80).collect::<String>()
            ));
        }
        check_text("name", &self.name, true)?;
        check_text("description", &self.description, false)?;
        check_text("author", &self.author, false)?;
        check_text("homepage", &self.homepage, false)?;
        check_text("license", &self.license, false)?;
        if version_parts(&self.version).is_none() {
            return Err(format!("plugin.json: version '{}' is not a version number like 1.0.0", self.version));
        }
        if !valid_path(&self.entry) || !self.entry.ends_with(".wasm") {
            return Err("plugin.json: entry must be a relative path to a .wasm file inside the package".into());
        }
        if self.abi != ABI {
            return Err(format!("{} was built for plugin ABI {}; this RustGIS runs ABI {ABI}", self.name, self.abi));
        }
        if !self.min_rustgis_version.is_empty() {
            if version_parts(&self.min_rustgis_version).is_none() {
                return Err("plugin.json: minRustgisVersion is not a version number".into());
            }
            if version_cmp(RUSTGIS_VERSION, &self.min_rustgis_version).is_lt() {
                return Err(format!("{} needs RustGIS {} or newer (this is {RUSTGIS_VERSION})", self.name, self.min_rustgis_version));
            }
        }
        if self.commands.len() > MAX_COMMANDS || self.tools.len() > MAX_TOOLS {
            return Err(format!("plugin.json: at most {MAX_COMMANDS} commands and {MAX_TOOLS} tools"));
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.commands {
            if !valid_id(&c.id, false) || !seen.insert(("c", c.id.as_str())) {
                return Err(format!("plugin.json: command id '{}' is invalid or repeated", c.id));
            }
            check_text("command label", &c.label, true)?;
            check_text("command help", &c.help, false)?;
            check_text("command params", &c.params, false)?;
            check_text("command icon", &c.icon, false)?;
        }
        for t in &self.tools {
            if !valid_id(&t.id, false) || !seen.insert(("t", t.id.as_str())) {
                return Err(format!("plugin.json: tool id '{}' is invalid or repeated", t.id));
            }
            check_text("tool name", &t.name, true)?;
            check_text("tool summary", &t.summary, false)?;
            check_text("toolbox", &t.toolbox, false)?;
            if t.params.len() > MAX_PARAMS {
                return Err(format!("plugin.json: {} has more than {MAX_PARAMS} parameters", t.name));
            }
            let mut names = std::collections::HashSet::new();
            for p in &t.params {
                if !valid_id(&p.name, true) || !names.insert(p.name.as_str()) {
                    return Err(format!("plugin.json: {}: parameter '{}' is invalid or repeated", t.name, p.name));
                }
                check_text("parameter label", &p.label, true)?;
                check_text("parameter help", &p.help, false)?;
                if !PARAM_KINDS.contains(&p.kind.as_str()) {
                    return Err(format!("plugin.json: {}: unknown parameter kind '{}'", t.name, p.kind));
                }
                if p.kind == "choice" && (p.choices.is_empty() || p.choices.len() > MAX_CHOICES) {
                    return Err(format!("plugin.json: {}: '{}' needs 1 to {MAX_CHOICES} choices", t.name, p.name));
                }
                for c in &p.choices {
                    check_text("choice", c, true)?;
                }
                if let Some(d) = &p.default
                    && d.to_string().len() > MAX_TEXT
                {
                    return Err(format!("plugin.json: {}: '{}' has too long a default", t.name, p.name));
                }
            }
        }
        if let Some(tab) = &self.tab {
            self.validate_tab(tab)?;
        }
        if self.i18n.len() > MAX_LANGS || self.i18n.values().map(BTreeMap::len).sum::<usize>() > MAX_STRINGS {
            return Err("plugin.json: too many translations".into());
        }
        for (lang, strings) in &self.i18n {
            check_text("language", lang, true)?;
            for (k, v) in strings {
                check_text("translated string", k, true)?;
                check_text("translation", v, false)?;
            }
        }
        Ok(())
    }

    /// `s` in language `lang` (`de`, `zh-CN`), or `s` (see [`translate`]).
    pub fn translate<'a>(&'a self, lang: &str, s: &'a str) -> &'a str {
        translate(&self.i18n, lang, s)
    }

    /// The command or tool a tab entry names.
    pub fn entry(&self, e: &str) -> Option<Entry<'_>> {
        match e.strip_prefix("tool:") {
            Some(t) => self.tools.iter().find(|x| x.id == t).map(Entry::Tool),
            None => self.commands.iter().find(|x| x.id == e).map(Entry::Command),
        }
    }

    fn validate_tab(&self, tab: &TabDecl) -> Result<(), String> {
        let label = tab.label.trim();
        if label.is_empty() || label.chars().count() > MAX_TAB_LABEL || label.contains('\n') {
            return Err(format!("plugin.json: the tab's label must be one line of 1 to {MAX_TAB_LABEL} characters"));
        }
        if RESERVED_TABS.iter().any(|r| r.eq_ignore_ascii_case(label)) {
            return Err(format!("plugin.json: the tab can't be called '{label}', the name of one of RustGIS's tabs"));
        }
        if tab.groups.is_empty() || tab.groups.len() > MAX_GROUPS {
            return Err(format!("plugin.json: the tab needs 1 to {MAX_GROUPS} groups"));
        }
        let entry = |e: &str| -> Result<(), String> {
            self.entry(e)
                .map(|_| ())
                .ok_or_else(|| format!("plugin.json: the tab names '{e}', which isn't one of the plugin's commands or tools (tool:<id>)"))
        };
        for g in &tab.groups {
            check_text("group label", &g.label, true)?;
            if g.items.is_empty() || g.items.len() > MAX_GROUP_ITEMS {
                return Err(format!("plugin.json: tab group '{}' needs 1 to {MAX_GROUP_ITEMS} items", g.label));
            }
            for item in &g.items {
                match item {
                    ItemDecl::Entry(e) => entry(e)?,
                    ItemDecl::Menu(m) => {
                        check_text("menu label", &m.menu, true)?;
                        check_text("menu help", &m.help, false)?;
                        check_text("menu icon", &m.icon, false)?;
                        if m.items.is_empty() || m.items.len() > MAX_MENU_ITEMS {
                            return Err(format!("plugin.json: menu '{}' needs 1 to {MAX_MENU_ITEMS} items", m.menu));
                        }
                        for e in &m.items {
                            entry(e)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn has(&self, p: Permission) -> bool {
        self.permissions.contains(&p)
    }
}
