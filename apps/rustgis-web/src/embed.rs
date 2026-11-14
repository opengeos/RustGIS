//! The web app inside another page: the `rustgis` Python package's Jupyter widget, or any page
//! that frames `web.rustgis.app/?embed`.
//!
//! With `?embed` the app starts blank, keeps its settings in memory rather than in the browser's
//! storage (the embedding page must not read what the user saved on the site, such as an AI
//! provider's key), doesn't ask before the page closes, and talks to the page that embeds it with
//! `postMessage`. Every message is an object whose `rustgis` member names its kind:
//!
//! | From the page | Does |
//! |---|---|
//! | `{"rustgis": "project", "text": <project JSON>}` | replaces the project with the page's copy |
//! | `{"rustgis": "run", "id": <any>, "command": <id>, "params"?: {…}}` | runs one of the app's own commands (`ui.*`), answered with `result` |
//!
//! | From the app | Says |
//! |---|---|
//! | `{"rustgis": "ready"}` | the app has started and takes messages |
//! | `{"rustgis": "project", "text": <project JSON>}` | the project changed here (an edit, a pan) |
//! | `{"rustgis": "result", "id", "ok": true, "value"}` or `{…, "ok": false, "error"}` | a `run`'s outcome |
//!
//! Exchanged projects remove recognized credentials from supported configuration fields.
//! Preserved user data, foreign notebook source and opaque parameters may still contain secrets.
//! Incoming projects use peer/import semantics: no identical-save restore, item trust or earlier
//! approvals. Parent origin checks identify the peer; they grant no user authority. The existing
//! `ui.*` command policy is separate from this project-exchange contract.
//!
//! Only messages from the parent window are taken, and the app answers only the origin that sent
//! the first of them. Any page may frame the app, so `run` takes only the interface's commands
//! (`ui.*`: panes, the language, the theme); everything else changes the project, which the page
//! sends whole and the app only opens. A change is posted once the project has stayed the same for [`SETTLE`], so a
//! pan sends one message, not one a frame; the page's own project is never echoed back.

use serde_json::{Value, json};
use web_time::{Duration, Instant};

/// How long the project must stay unchanged before the change is posted.
pub const SETTLE: Duration = Duration::from_millis(600);

/// A message from the embedding page.
#[derive(Debug, PartialEq)]
pub enum Msg {
    /// Replace the project with this one.
    Project(String),
    /// Run a command; its outcome is posted back under `id`.
    Run { id: Value, command: String, params: Value },
}

/// Reads a message from the page; anything else (another script's messages) is `None`.
pub fn parse(v: &Value) -> Option<Msg> {
    match v.get("rustgis")?.as_str()? {
        "project" => Some(Msg::Project(v.get("text")?.as_str()?.to_string())),
        "run" => Some(Msg::Run {
            id: v.get("id").cloned().unwrap_or(Value::Null),
            command: v.get("command")?.as_str()?.to_string(),
            params: v.get("params").cloned().filter(|p| !p.is_null()).unwrap_or_else(|| json!({})),
        }),
        _ => None,
    }
}

/// Interface commands an embedding page may run: panes, tabs, the ribbon, look and language,
/// and harmless view or chrome toggles. Commands that fetch web addresses, open or export files,
/// print, open pages, touch the clipboard or change stored tasks and settings outside the
/// embedded session stay out.
const UI_EMBED: &[&str] = &[
    "ui.about",
    "ui.animationPlay",
    "ui.backstage",
    "ui.catalogView",
    "ui.commandSearch",
    "ui.coordinateFormat",
    "ui.gp.open",
    "ui.identify",
    "ui.itemPopups",
    "ui.language",
    "ui.layoutSelect",
    "ui.layoutTool",
    "ui.linkCursors",
    "ui.magnifier",
    "ui.map.gpu",
    "ui.measurementUnits",
    "ui.moveSharedVertices",
    "ui.pane.bookmarks",
    "ui.pane.catalog",
    "ui.pane.contents",
    "ui.pane.design",
    "ui.pane.details",
    "ui.pane.diagnosticLog",
    "ui.pane.diagnostics",
    "ui.pane.editing",
    "ui.pane.format",
    "ui.pane.geoprocessing",
    "ui.pane.history",
    "ui.pane.infographics",
    "ui.pane.jobs",
    "ui.pane.labeling",
    "ui.pane.layerProperties",
    "ui.pane.notebook",
    "ui.pane.presentation",
    "ui.pane.profile",
    "ui.pane.report",
    "ui.pane.sources",
    "ui.pane.symbology",
    "ui.pane.tasks",
    "ui.pane.timeSeries",
    "ui.pane.workflows",
    "ui.paneSet",
    "ui.ribbon",
    "ui.scale",
    "ui.shortcuts",
    "ui.splitView",
    "ui.tab",
    "ui.theme",
    "ui.themeFile",
    "ui.timePlay",
    "ui.timeSlider",
    "ui.traceEdges",
];

/// Whether the embedding page may run `command`: safe interface commands only.
pub fn runnable(command: &str) -> bool {
    UI_EMBED.contains(&command)
}

/// Whether startup query links (`?project=`, `?data=`, `?stac=`, `?style=`, `?tiles=`, `?zarr=`,
/// `?registry=`) run on page load: never in `?embed`, which starts blank and receives its
/// project through `postMessage`.
pub const fn startup_links_allowed(embedded: bool) -> bool {
    !embedded
}

pub fn ready() -> Value {
    json!({"rustgis": "ready"})
}

pub fn project(text: &str) -> Value {
    json!({"rustgis": "project", "text": text})
}

pub fn result(id: Value, outcome: Result<Value, String>) -> Value {
    match outcome {
        Ok(value) => json!({"rustgis": "result", "id": id, "ok": true, "value": value}),
        Err(error) => json!({"rustgis": "result", "id": id, "ok": false, "error": error}),
    }
}

/// What the page has seen of the project: its revision, active map and view. A view change
/// isn't an edit, so the session's revision alone would miss a pan.
pub type Key = (u64, usize, [u64; 3]);

pub fn key(s: &rustgis_engine::Session) -> Key {
    let c = &s.project.map().camera;
    (s.revision, s.project.active_map, [c.center.x.to_bits(), c.center.y.to_bits(), c.res.to_bits()])
}

/// Decides when a change is posted.
#[derive(Default)]
pub struct Changes {
    /// The project the page has (posted by the app or sent by the page).
    seen: Option<Key>,
    /// A change not yet posted, and when it was last seen to change.
    pending: Option<(Key, Instant)>,
}

impl Changes {
    /// The page has this project (it just sent it, or the app posted it).
    pub fn seen(&mut self, key: Key) {
        self.seen = Some(key);
        self.pending = None;
    }

    /// Whether the project, now `key`, should be posted. The first project is the page's to send,
    /// so it never is. `Some(wait)` without posting asks to be called again after `wait`.
    pub fn due(&mut self, key: Key, now: Instant) -> (bool, Option<Duration>) {
        let Some(seen) = self.seen else {
            self.seen = Some(key);
            return (false, None);
        };
        if key == seen {
            self.pending = None;
            return (false, None);
        }
        match self.pending {
            Some((k, since)) if k == key => {
                let waited = now.saturating_duration_since(since);
                if waited >= SETTLE {
                    self.seen(key);
                    (true, None)
                } else {
                    (false, Some(SETTLE - waited))
                }
            }
            _ => {
                self.pending = Some((key, now));
                (false, Some(SETTLE))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_page_messages_and_ignores_others() {
        assert_eq!(parse(&json!({"rustgis": "project", "text": "{}"})), Some(Msg::Project("{}".into())));
        assert_eq!(
            parse(&json!({"rustgis": "run", "id": 3, "command": "ui.language", "params": {"code": "de"}})),
            Some(Msg::Run { id: json!(3), command: "ui.language".into(), params: json!({"code": "de"}) })
        );
        assert_eq!(
            parse(&json!({"rustgis": "run", "command": "edit.undo", "params": null})),
            Some(Msg::Run { id: Value::Null, command: "edit.undo".into(), params: json!({}) })
        );
        for other in
            [json!("hello"), json!({"type": "project"}), json!({"rustgis": "project"}), json!({"rustgis": 1}), json!({"rustgis": "run"})]
        {
            assert_eq!(parse(&other), None, "{other}");
        }
    }

    #[test]
    fn the_page_runs_only_safe_interface_commands() {
        for safe in [
            "ui.language",
            "ui.theme",
            "ui.themeFile",
            "ui.scale",
            "ui.ribbon",
            "ui.tab",
            "ui.backstage",
            "ui.pane.contents",
            "ui.pane.symbology",
            "ui.pane.geoprocessing",
            "ui.map.gpu",
            "ui.coordinateFormat",
            "ui.measurementUnits",
            "ui.timeSlider",
            "ui.magnifier",
            "ui.identify",
        ] {
            assert!(runnable(safe), "{safe}");
        }
        for blocked in [
            "ui.",
            "ui.addUrl",
            "ui.openUrl",
            "ui.stacLink",
            "ui.stac",
            "ui.stac.sentinel2",
            "ui.addZarr",
            "ui.plugins",
            "ui.extensions",
            "ui.installPlugin",
            "ui.searchOpenData",
            "ui.connection",
            "ui.s3Connections",
            "ui.importService",
            "ui.refreshService",
            "ui.addService",
            "ui.exportMap",
            "ui.exportLayer",
            "ui.exportRaster",
            "ui.exportLayout",
            "ui.exportReport",
            "ui.exportPresentation",
            "ui.exportAnimation",
            "ui.timeExport",
            "ui.printMap",
            "ui.printLayout",
            "ui.save",
            "ui.saveAs",
            "ui.saveLayout",
            "ui.saveItem",
            "ui.share",
            "ui.open",
            "ui.openItem",
            "ui.addFile",
            "ui.addData",
            "ui.learn",
            "ui.copy",
            "ui.cut",
            "ui.copyPath",
            "ui.captureMap",
            "ui.captureLayout",
            "ui.taskRecord",
            "ui.recover",
            "ui.autosave",
            "ui.startup",
            "ui.mcpRelay",
            "ui.dialog",
            "ui.pane.assistant",
            "ui.pane.directions",
            "ui.pane.sql",
            "ui.pane.webData",
            "ui.pane.plugin",
            "ui.pluginForm",
            "layer.addData",
            "project.open",
            "share.exportMap",
            "gp.run",
            "uix.language",
            "",
        ] {
            assert!(!runnable(blocked), "{blocked}");
        }
        assert!(startup_links_allowed(false));
        assert!(!startup_links_allowed(true));
    }

    #[test]
    fn a_change_is_posted_once_it_settles_and_never_echoed() {
        let t = Instant::now();
        let k = |rev| (rev, 0, [0, 0, 0]);
        let mut c = Changes::default();
        // The first project is the page's to send.
        assert_eq!(c.due(k(1), t), (false, None));
        // The page sends its project: not echoed back.
        c.seen(k(2));
        assert_eq!(c.due(k(2), t), (false, None));
        // A pan: frames of changing views post nothing until the view stays put.
        assert_eq!(c.due(k(3), t), (false, Some(SETTLE)));
        assert_eq!(c.due(k(4), t + Duration::from_millis(100)), (false, Some(SETTLE)));
        assert_eq!(c.due(k(4), t + Duration::from_millis(400)), (false, Some(Duration::from_millis(300))));
        assert_eq!(c.due(k(4), t + Duration::from_millis(700)), (true, None));
        assert_eq!(c.due(k(4), t + Duration::from_millis(800)), (false, None));
        // A change undone before it settles posts nothing.
        assert_eq!(c.due(k(5), t), (false, Some(SETTLE)));
        assert_eq!(c.due(k(4), t + SETTLE), (false, None));
    }
}
