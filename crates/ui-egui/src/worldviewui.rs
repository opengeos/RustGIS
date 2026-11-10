//! The worldview: whose borders the built-in countries and the basemap's world outline show
//! (Settings › Language › Worldview). The default is built in; every other one is downloaded when
//! it is chosen (or, for China, when the system calls for it), off the frame through the tile
//! cache's downloads (a thread natively, fetch on the web), and kept in the cache folder.

use rustgis_engine::sample::{self, Worldview};

use crate::{RustGisApp, Slot, i18n};

/// A worldview's name in the interface language.
pub fn name(w: Worldview) -> &'static str {
    match w.name() {
        "Default" => tl!("Default"),
        "Argentina" => tl!("Argentina"),
        "Bangladesh" => tl!("Bangladesh"),
        "Brazil" => tl!("Brazil"),
        "China" => tl!("China"),
        "Egypt" => tl!("Egypt"),
        "France" => tl!("France"),
        "Germany" => tl!("Germany"),
        "Greece" => tl!("Greece"),
        "India" => tl!("India"),
        "Indonesia" => tl!("Indonesia"),
        "Israel" => tl!("Israel"),
        "Italy" => tl!("Italy"),
        "Japan" => tl!("Japan"),
        "Morocco" => tl!("Morocco"),
        "Nepal" => tl!("Nepal"),
        "Netherlands" => tl!("Netherlands"),
        "Pakistan" => tl!("Pakistan"),
        "Palestine" => tl!("Palestine"),
        "Poland" => tl!("Poland"),
        "Portugal" => tl!("Portugal"),
        "Russia" => tl!("Russia"),
        "Saudi Arabia" => tl!("Saudi Arabia"),
        "South Korea" => tl!("South Korea"),
        "Spain" => tl!("Spain"),
        "Sweden" => tl!("Sweden"),
        "Taiwan" => tl!("Taiwan"),
        "Turkey" => tl!("Turkey"),
        "Ukraine" => tl!("Ukraine"),
        "United Kingdom" => tl!("United Kingdom"),
        "United States" => tl!("United States"),
        "Vietnam" => tl!("Vietnam"),
        other => other,
    }
}

/// The download under way: the worldview and where its answer lands.
pub struct Fetch {
    pub wanted: Worldview,
    slot: Slot<Result<Vec<u8>, String>>,
}

/// The worldview preference, with `auto` as the default one in tests (they don't depend on the
/// machine's time zone and locale).
fn pref(app: &RustGisApp) -> &str {
    if cfg!(test) && app.ui.worldview == i18n::AUTO { "default" } else { app.ui.worldview.as_str() }
}

/// Applies the worldview preference: shows it if its countries are at hand, else downloads them
/// and keeps the current borders until they arrive. An untouched sample project's countries
/// follow.
pub fn apply(app: &mut RustGisApp) {
    let before = sample::worldview();
    let wanted = sample::apply_worldview(pref(app));
    let after = sample::worldview();
    if after != before {
        sample::swap_countries(&mut app.session, before, after);
        app.map.invalidate();
    }
    if after == wanted {
        app.ui.worldview_fetch = None;
    } else if app.ui.worldview_fetch.as_ref().is_none_or(|f| f.wanted != wanted) {
        start(app, wanted);
    }
}

/// Starts downloading a worldview's countries.
fn start(app: &mut RustGisApp, wanted: Worldview) {
    let slot: Slot<Result<Vec<u8>, String>> = Slot::default();
    match (&app.services.fetch, wanted.url()) {
        (Some(fetch), Some(url)) => fetch(&url, slot.clone()),
        _ => {
            let land = slot.clone();
            sample::download(wanted, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
        }
    }
    app.ui.worldview_fetch = Some(Fetch { wanted, slot });
}

/// The worldview being downloaded, if any (the ribbon says so).
pub fn downloading(app: &RustGisApp) -> Option<Worldview> {
    app.ui.worldview_fetch.as_ref().map(|f| f.wanted)
}

/// Lands a download: shows the worldview if it is still the one asked for, or says why it can't.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(fetch) = &app.ui.worldview_fetch else { return };
    let Some(r) = fetch.slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let wanted = fetch.wanted;
    app.ui.worldview_fetch = None;
    match r.and_then(|bytes| sample::accept(wanted, bytes)) {
        Ok(()) => apply(app),
        Err(e) => {
            let text = i18n::fmt(
                "Couldn't download the {view} worldview ({error}); the map keeps the {current} borders",
                &[("view", name(wanted)), ("error", &e), ("current", name(sample::worldview()))],
            );
            app.session.notify(text, true);
        }
    }
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An app whose downloads all end the way `answer` says, at once.
    fn app_fetching(answer: impl Fn(&str) -> Result<Vec<u8>, String> + 'static) -> RustGisApp {
        // Never the computer's own cache folder.
        sample::keep_downloads(false);
        let mut app = RustGisApp::with_sample();
        app.services.fetch = Some(Box::new(move |url: &str, slot: Slot<Result<Vec<u8>, String>>| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(answer(url));
            }
        }));
        app
    }

    /// A worldview is downloaded only when it is chosen, from its own address, and kept as its code.
    /// (The worldview shown is shared by every test in the process, so it isn't looked at here.)
    #[test]
    fn a_chosen_worldview_is_downloaded_then_kept() {
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let log = asked.clone();
        let mut app = app_fetching(move |url| {
            log.borrow_mut().push(url.to_string());
            Ok(sample::COUNTRIES.as_bytes().to_vec())
        });
        let nld = Worldview::from_code("nld").unwrap();
        assert!(asked.borrow().is_empty(), "nothing is downloaded before a worldview is chosen");
        assert!(sample::text(nld).is_none());
        app.run("ui.worldview", json!({"worldview": "Netherlands"})).unwrap();
        assert_eq!(app.ui.worldview, "nld");
        assert_eq!(downloading(&app), Some(nld));
        assert_eq!(*asked.borrow(), ["https://data.source.coop/giswqs/opengeos/rustgis/worldviews/countries_nld.geojson"]);
        let mark = app.session.notice_mark();
        poll(&mut app, &egui::Context::default());
        assert!(app.session.notices_since(mark).is_empty());
        assert_eq!(downloading(&app), None);
        assert!(sample::text(nld).is_some());
        // Choosing it again downloads nothing.
        app.run("ui.worldview", json!({"worldview": "nld"})).unwrap();
        assert_eq!(asked.borrow().len(), 1);
        assert_eq!(downloading(&app), None);
        // Nor does the built-in default.
        app.run("ui.worldview", json!({"worldview": "default"})).unwrap();
        assert_eq!(asked.borrow().len(), 1);
        assert!(app.run("ui.worldview", json!({"worldview": "atlantis"})).is_err());
        assert_eq!(app.ui.worldview, "default");
    }

    /// A download that fails, or isn't countries, says so and keeps the borders shown.
    #[test]
    fn a_failed_worldview_download_is_told() {
        for (code, answer) in [("pol", Err("HTTP 404".to_string())), ("prt", Ok(b"<html>not found</html>".to_vec()))] {
            let mut app = app_fetching(move |_| answer.clone());
            let w = Worldview::from_code(code).unwrap();
            app.run("ui.worldview", json!({"worldview": code})).unwrap();
            let mark = app.session.notice_mark();
            poll(&mut app, &egui::Context::default());
            let told: Vec<_> = app.session.notices_since(mark).iter().map(|n| (n.text.clone(), n.error)).collect();
            assert_eq!(told.len(), 1, "{code}: {told:?}");
            assert!(told[0].1 && told[0].0.contains(w.name()) && told[0].0.contains("worldview"), "{told:?}");
            assert!(sample::text(w).is_none());
            assert_eq!(downloading(&app), None);
        }
    }
}
