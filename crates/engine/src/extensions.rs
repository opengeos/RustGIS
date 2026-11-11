//! Extensions: optional sets of RustGIS's own commands that the user turns on or off (#1752).
//!
//! A plugin is a package the user installs; an extension ships with RustGIS and needs nothing
//! downloaded. While an extension is off, the commands it owns are refused with a message that
//! says so (from the ribbon, command search, the command line, scripts, models, Python and the
//! AI Assistant alike), and the front end hides its tab and buttons.
//!
//! What the user chose is the session's [`State`]. The desktop and web apps keep it as the
//! `extensions` setting ([`State::from_setting`], [`State::to_setting`]) and start each extension
//! the setting says nothing about at its default ([`Extension::default_on`]). A session of its
//! own (the command line, Python, a test) starts with every extension on, so scripts keep
//! working; `extension.disable` turns one off there too. A setting from a newer RustGIS may name
//! extensions this version doesn't know: they are kept as they were and written back unchanged.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

/// An extension: what it is called, what it owns and whether it starts on.
#[derive(Debug)]
pub struct Extension {
    /// Its id, in commands and the setting (`imagery`).
    pub id: &'static str,
    /// Its name in the interface.
    pub name: &'static str,
    /// One sentence on what it adds.
    pub description: &'static str,
    /// The caption of its own ribbon tab (`extension:<id>`), or `None` for a group on the Plugins
    /// tab.
    pub tab: Option<&'static str>,
    /// Whether the desktop and web apps start with it on until the user says otherwise.
    pub default_on: bool,
    /// The command ids it owns.
    pub commands: &'static [&'static str],
}

/// Imagery: orthomosaics, surface models, georeferencing, image analysis and classification,
/// deep learning, mensuration and hosted imagery, on a tab of its own.
pub const IMAGERY: Extension = Extension {
    id: "imagery",
    name: "Imagery",
    description: "Ortho and reality mapping, georeferencing, image analysis and classification, deep learning, mensuration and hosted imagery, on a tab of its own.",
    tab: Some("Imagery"),
    default_on: false,
    commands: &[
        "imagery.workspace",
        "imagery.workspaceOptions",
        "imagery.mosaic",
        "imagery.mesh",
        "imagery.hostedImagery",
        "raster.georeference",
        "raster.gcpFit",
        "raster.change",
        "raster.spectral",
        "raster.classify",
        "raster.classifySupervised",
        "raster.index",
        "raster.process",
        "raster.mosaic",
        "raster.editPixels",
        "raster.measure",
        "raster.measurements",
        "dl.train",
        "dl.classify",
        "dl.package",
        "dl.import",
        "dl.remove",
    ],
};

/// Every extension, in the order the Extensions window lists them.
pub const ALL: &[Extension] = &[IMAGERY];

/// The most entries the setting keeps (a hand-edited or hostile one can't grow without bound).
const MAX_ENTRIES: usize = 256;
/// The longest extension id the setting keeps.
const MAX_ID: usize = 64;

/// The extension with id `id`.
pub fn get(id: &str) -> Option<&'static Extension> {
    ALL.iter().find(|e| e.id == id)
}

/// The extension that owns command `command`, if one does.
pub fn owner(command: &str) -> Option<&'static Extension> {
    ALL.iter().find(|e| e.commands.contains(&command))
}

/// The tab id of an extension's own tab.
pub fn tab_id(id: &str) -> String {
    format!("extension:{id}")
}

/// What the user turned on or off, by extension id. Ids this version doesn't know (from a newer
/// RustGIS) are kept, so saving writes them back as they were. The default state (a session of
/// its own) has every extension on; one read from the setting starts each at its default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    chosen: BTreeMap<String, bool>,
    /// Whether an extension the user said nothing about is at its default (the apps) rather
    /// than on.
    defaults: bool,
}

impl State {
    /// Whether extension `id` is on: what the user chose, else its default in the apps and on
    /// elsewhere. An unknown id is off.
    pub fn is_on(&self, id: &str) -> bool {
        match get(id) {
            Some(e) => self.chosen.get(id).copied().unwrap_or(!self.defaults || e.default_on),
            None => false,
        }
    }

    /// Turns extension `id` on or off. An id that is no extension is an error.
    pub fn set(&mut self, id: &str, on: bool) -> Result<&'static Extension, String> {
        let e = get(id).ok_or_else(|| unknown(id))?;
        self.chosen.insert(e.id.to_string(), on);
        Ok(e)
    }

    /// Why command `command` can't run now: its extension is off. `None` when it can.
    pub fn blocks(&self, command: &str) -> Option<String> {
        let e = owner(command)?;
        (!self.is_on(e.id)).then(|| {
            format!("{command} is part of the {} extension, which is off; turn it on in Plugins › Extensions (extension.enable)", e.name)
        })
    }

    /// The state the `extensions` setting holds (`{"<id>": true|false, …}`) and what of it couldn't
    /// be read, said in one line (`None` when all of it could): entries that aren't true or false,
    /// over-long ids and those past [`MAX_ENTRIES`] are left out. An empty or missing setting is
    /// every extension at its default.
    pub fn from_setting(text: Option<&str>) -> (State, Option<String>) {
        let at_defaults = State { chosen: BTreeMap::new(), defaults: true };
        let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else { return (at_defaults, None) };
        let map = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => map,
            Ok(_) | Err(_) => {
                return (at_defaults, Some("the extensions setting isn't a JSON object; every extension is at its default".into()));
            }
        };
        let mut chosen = BTreeMap::new();
        let mut skipped = 0usize;
        for (id, on) in map {
            match on.as_bool() {
                Some(on) if !id.is_empty() && id.chars().count() <= MAX_ID && chosen.len() < MAX_ENTRIES => {
                    chosen.insert(id, on);
                }
                _ => skipped += 1,
            }
        }
        let said = (skipped > 0).then(|| {
            format!(
                "{skipped} entr{} of the extensions setting couldn't be read (not true or false, an id over {MAX_ID} characters, or past {MAX_ENTRIES} entries) and {} left out",
                if skipped == 1 { "y" } else { "ies" },
                if skipped == 1 { "was" } else { "were" }
            )
        });
        (State { chosen, defaults: true }, said)
    }

    /// The `extensions` setting: every choice made, known ids or not.
    pub fn to_setting(&self) -> String {
        let map: Map<String, Value> = self.chosen.iter().map(|(k, v)| (k.clone(), Value::Bool(*v))).collect();
        Value::Object(map).to_string()
    }

    /// Every extension with whether it's on, for `extension.list` and the Extensions window.
    pub fn list(&self) -> Value {
        Value::Array(
            ALL.iter()
                .map(|e| {
                    json!({
                        "id": e.id,
                        "name": e.name,
                        "description": e.description,
                        "tab": e.tab,
                        "enabled": self.is_on(e.id),
                        "default": e.default_on,
                        "chosen": self.chosen.get(e.id),
                        "commands": e.commands,
                    })
                })
                .collect(),
        )
    }
}

/// The error for an id that is no extension, naming the ones there are.
fn unknown(id: &str) -> String {
    let id: String = id.chars().take(MAX_ID).collect();
    let ids: Vec<&str> = ALL.iter().map(|e| e.id).collect();
    format!("no extension '{id}' (there are: {})", ids.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_owned_command_is_registered_and_owned_once() {
        let registry = crate::cmd::all();
        for e in ALL {
            assert!(e.id.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{}", e.id);
            for c in e.commands {
                assert!(registry.iter().any(|s| s.id == *c), "{} owns {c}, which isn't a command", e.id);
                assert_eq!(owner(c).map(|o| o.id), Some(e.id), "{c} is owned twice");
            }
        }
    }

    #[test]
    fn a_session_of_its_own_has_every_extension_on_and_the_apps_start_at_the_defaults() {
        assert!(!IMAGERY.default_on);
        assert!(State::default().is_on("imagery"));
        let (apps, said) = State::from_setting(None);
        assert_eq!(said, None);
        assert!(!apps.is_on("imagery"));
        assert!(apps.blocks("raster.classify").is_some());
        let (chosen, _) = State::from_setting(Some(r#"{"imagery": true}"#));
        assert!(chosen.is_on("imagery"));
    }

    #[test]
    fn defaults_choices_and_unknown_ids() {
        let mut st = State::default();
        assert!(st.is_on("imagery"));
        assert!(!st.is_on("nonesuch"));
        assert_eq!(st.blocks("raster.classify"), None);
        assert_eq!(st.blocks("layer.zoomTo"), None);
        assert!(st.set("nonesuch", true).is_err());
        assert!(st.set("imagery", false).is_ok());
        let why = st.blocks("raster.classify").unwrap_or_default();
        assert!(why.contains("Imagery extension, which is off"), "{why}");
        assert_eq!(st.blocks("layer.zoomTo"), None);
    }

    #[test]
    fn setting_round_trips_and_keeps_what_a_newer_version_wrote() {
        let (st, said) = State::from_setting(Some(r#"{"imagery": false, "from-newer": true}"#));
        assert_eq!(said, None);
        assert!(!st.is_on("imagery"));
        // Not an extension here, so off here; but kept for the version that knows it.
        assert!(!st.is_on("from-newer"));
        let (again, _) = State::from_setting(Some(&st.to_setting()));
        assert_eq!(again, st);
        assert!(again.to_setting().contains("\"from-newer\":true"));
    }

    #[test]
    fn a_session_refuses_an_off_extensions_commands_every_way_in() {
        let mut s = crate::Session::new(crate::Project::default());
        let listed = s.run("extension.list", &json!({})).unwrap_or_default();
        assert_eq!(listed[0]["id"], "imagery");
        assert_eq!(listed[0]["enabled"], true);
        assert!(s.run("extension.disable", &json!({"id": "nonesuch"})).is_err_and(|e| e.contains("no extension 'nonesuch'")));
        assert!(s.run("extension.enable", &json!({"id": "imagery", "enabled": "maybe"})).is_err());
        assert!(s.run("extension.enable", &json!({})).is_err());
        let off = s.run("extension.disable", &json!({"id": "imagery"})).unwrap_or_default();
        assert_eq!(off["enabled"], false);
        assert!(!s.extensions().is_on("imagery"));
        let off_said = |r: Result<Value, String>| r.is_err_and(|e| e.contains("Imagery extension, which is off"));
        assert!(off_said(s.run("raster.classify", &json!({}))));
        assert!(off_said(s.run("dl.remove", &json!({"name": "m"}))));
        // A background raster run is refused before it starts.
        #[cfg(not(target_arch = "wasm32"))]
        assert!(s.start_raster("raster.classify", &json!({})).is_err_and(|e| e.contains("Imagery extension, which is off")));
        // Commands no extension owns still run.
        assert!(s.run("extension.list", &json!({})).is_ok());
        s.run("extension.enable", &json!({"id": "imagery", "enabled": true})).unwrap_or_default();
        assert!(!off_said(s.run("raster.classify", &json!({}))));
    }

    #[test]
    fn unreadable_setting_is_said() {
        let at_defaults = State { chosen: BTreeMap::new(), defaults: true };
        assert_eq!(State::from_setting(None), (at_defaults.clone(), None));
        assert_eq!(State::from_setting(Some("  ")), (at_defaults.clone(), None));
        let (st, said) = State::from_setting(Some("[1, 2]"));
        assert_eq!(st, at_defaults);
        assert!(said.is_some_and(|s| s.contains("isn't a JSON object")));
        let long = "x".repeat(MAX_ID + 1);
        let (st, said) = State::from_setting(Some(&format!(r#"{{"imagery": "no", "{long}": true, "ok": false}}"#)));
        // Left at its default.
        assert!(!st.is_on("imagery"));
        assert!(said.is_some_and(|s| s.starts_with("2 entries")));
        let many: Map<String, Value> = (0..MAX_ENTRIES + 3).map(|i| (format!("e{i}"), Value::Bool(true))).collect();
        let (st, said) = State::from_setting(Some(&Value::Object(many).to_string()));
        assert_eq!(st.chosen.len(), MAX_ENTRIES);
        assert!(said.is_some_and(|s| s.starts_with("3 entries")));
    }
}
