//! Help › Check for Updates: asks GitHub for the latest release and compares it with this build.
//! Nothing is sent until the user asks.

use egui::{Align2, vec2};
use serde_json::Value;

use crate::{RustGisApp, Slot};

/// The latest release, as JSON naming its tag and page.
const LATEST: &str = "https://api.github.com/repos/opengeos/RustGIS/releases/latest";
/// Every release, for when the check can't reach GitHub.
const RELEASES: &str = "https://github.com/opengeos/RustGIS/releases";
/// The release JSON is a few kilobytes; anything far larger isn't it.
const MAX_BYTES: usize = 1024 * 1024;

/// The Check for Updates window.
#[derive(Default)]
pub struct UpdateState {
    pub open: bool,
    /// Start a check on the next frame (commands have no egui context to repaint with).
    start: bool,
    pending: Option<Slot<Result<Vec<u8>, String>>>,
    result: Option<Result<Release, String>>,
}

impl UpdateState {
    /// Opens the window and checks again.
    pub fn check(&mut self) {
        self.open = true;
        self.start = true;
        self.result = None;
    }
}

/// A published release: its version (the tag without its `v`) and its page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
}

/// Reads GitHub's latest-release JSON.
pub fn parse_release(body: &[u8]) -> Result<Release, String> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "GitHub's reply isn't a release".to_string())?;
    let tag =
        v.get("tag_name").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or("GitHub's reply names no release")?;
    let url = v.get("html_url").and_then(Value::as_str).filter(|u| u.starts_with("https://")).unwrap_or(RELEASES);
    Ok(Release { version: tag.trim_start_matches(['v', 'V']).to_string(), url: url.to_string() })
}

/// Whether version `latest` is newer than `current`. The dotted numbers are compared in order
/// (missing ones count as 0), and a pre-release (`0.2.0-beta.1`) comes before its release.
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(v: &str) -> ([u64; 3], bool) {
        let v = v.trim().trim_start_matches(['v', 'V']);
        let (core, pre) = match v.split_once(['-', '+']) {
            Some((c, rest)) => (c, v[c.len()..].starts_with('-') && !rest.is_empty()),
            None => (v, false),
        };
        let mut n = [0u64; 3];
        for (slot, part) in n.iter_mut().zip(core.split('.')) {
            *slot = part.trim().parse().unwrap_or(0);
        }
        (n, pre)
    }
    let ((a, a_pre), (b, b_pre)) = (parts(latest), parts(current));
    a > b || (a == b && b_pre && !a_pre)
}

/// Draws the window, starting and collecting the check.
pub fn show(app: &mut RustGisApp, ctx: &egui::Context) {
    if !app.update.open {
        return;
    }
    if std::mem::take(&mut app.update.start) && !cfg!(target_arch = "wasm32") {
        app.update.pending = Some(crate::pluginsui::fetch(app, LATEST, MAX_BYTES, Some(ctx.clone())));
    }
    if let Some(r) = app.update.pending.as_ref().and_then(|s| s.lock().ok().and_then(|mut s| s.take())) {
        app.update.pending = None;
        app.update.result = Some(r.and_then(|body| parse_release(&body)));
    }
    let current = env!("CARGO_PKG_VERSION");
    let mut open = true;
    let mut again = false;
    let mut visit: Option<String> = None;
    egui::Window::new(tl!("Check for Updates"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            if cfg!(target_arch = "wasm32") {
                ui.label(tl!("The web app always runs the latest release: reload the page to get it."));
                return;
            }
            match &app.update.result {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tl!("Checking for updates…"));
                    });
                }
                Some(Ok(r)) if is_newer(&r.version, current) => {
                    ui.label(egui::RichText::new(crate::i18n::fmt("RustGIS {latest} is available.", &[("latest", &r.version)])).strong());
                    ui.label(crate::i18n::fmt("You have version {version}.", &[("version", current)]));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(tl!("Installed with Homebrew, winget, the AUR or COPR? Update it there.")).small().weak());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(tl!("Open the Release Page")).clicked() {
                            visit = Some(r.url.clone());
                        }
                        if ui.button(tl!("Check Again")).clicked() {
                            again = true;
                        }
                    });
                }
                Some(Ok(_)) => {
                    ui.label(crate::i18n::fmt("RustGIS {version} is the latest version.", &[("version", current)]));
                    ui.add_space(6.0);
                    if ui.button(tl!("Check Again")).clicked() {
                        again = true;
                    }
                }
                Some(Err(e)) => {
                    ui.label(crate::i18n::fmt("Couldn't check for updates: {error}", &[("error", e)]));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(tl!("Open the Release Page")).clicked() {
                            visit = Some(RELEASES.to_string());
                        }
                        if ui.button(tl!("Check Again")).clicked() {
                            again = true;
                        }
                    });
                }
            }
        });
    if again {
        app.update.check();
    }
    if visit.is_some() {
        app.ui.open_url = visit;
    }
    if !open {
        app.update.open = false;
        app.update.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_with_pre_releases_first() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("junk", "0.1.0"));
        assert!(!is_newer("", ""));
    }

    #[test]
    fn release_json_is_read_and_checked() {
        let r = parse_release(br#"{"tag_name":"v0.2.0","html_url":"https://github.com/opengeos/RustGIS/releases/tag/v0.2.0"}"#).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert!(r.url.ends_with("/v0.2.0"));
        // A page that isn't HTTPS falls back to the releases list.
        assert_eq!(parse_release(br#"{"tag_name":"0.2.0","html_url":"javascript:alert(1)"}"#).unwrap().url, RELEASES);
        assert!(parse_release(b"<html>rate limited</html>").is_err());
        assert!(parse_release(br#"{"message":"Not Found"}"#).is_err());
        assert!(parse_release(br#"{"tag_name":"  "}"#).is_err());
    }
}
