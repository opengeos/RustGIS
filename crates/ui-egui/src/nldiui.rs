//! Web Data › United States › USGS NLDI: like GeoLibre's NLDI panel, a flowline direction and a
//! point (a map click, the map's centre or typed), then the basin from its hydrolocation, and
//! navigation up- or downstream: load the sources a navigation can list, pick one (flowlines,
//! stream gages, wells…) and plot it, as many as wanted. Each result is a layer (`nldi.*`).
//! Requests go off the frame: the flowtrace is a POST, the rest the tile cache's downloads.

use egui::{Pos2, Rect, Stroke};
use rustgis_engine::geom::Coord;
use rustgis_engine::render::rustgis_tiles::{cache, nldi};
use serde_json::{Value, json};

use crate::{RustGisApp, Slot};

type Answer = Slot<Result<Vec<u8>, String>>;

/// What is being fetched.
enum Asked {
    /// The flowtrace at a point.
    Trace {
        lon: f64,
        lat: f64,
        direction: &'static str,
    },
    /// The hydrolocation, when the flowtrace failed.
    Hydro {
        lon: f64,
        lat: f64,
    },
    /// The COMID's flowline, after the hydrolocation.
    Line {
        lon: f64,
        lat: f64,
        snapped: Option<[f64; 2]>,
    },
    Basin {
        comid: u64,
    },
    /// A navigation's sources; then the source picked is plotted.
    Sources {
        comid: u64,
        mode: &'static str,
    },
    Navigate {
        comid: u64,
        mode: &'static str,
        source: String,
        distance: f64,
    },
}

/// The pane's choices, the result and the request under way.
pub struct State {
    pub direction: &'static str,
    pub lon: String,
    pub lat: String,
    /// A map click traces the flowline there.
    pub picking: bool,
    /// The point traced, and its flowline's COMID.
    pub at: Option<(f64, f64)>,
    pub comid: Option<u64>,
    /// The navigation method (`UM`…).
    pub mode: &'static str,
    pub distance: f64,
    /// The sources loaded, for which COMID and method.
    sources: Vec<nldi::Source>,
    sources_for: Option<(u64, &'static str)>,
    pub source: String,
    plotted: bool,
    /// The layers this result added.
    layers: Vec<u32>,
    waiting: Option<(Asked, Answer)>,
    pub error: Option<String>,
    pub info: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            direction: "none",
            lon: String::new(),
            lat: String::new(),
            picking: true,
            at: None,
            comid: None,
            mode: "UM",
            distance: 500.0,
            sources: Vec::new(),
            sources_for: None,
            source: "flowlines".into(),
            plotted: false,
            layers: Vec::new(),
            waiting: None,
            error: None,
            info: None,
        }
    }
}

fn fetch(url: &str) -> Answer {
    let slot: Answer = Slot::default();
    let land = slot.clone();
    cache::download(url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
    slot
}

fn post(url: &str, body: String) -> Answer {
    let slot: Answer = Slot::default();
    let land = slot.clone();
    cache::post_json(url, body, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
    slot
}

/// Whether the pane is open and waiting for a map click.
fn active(app: &RustGisApp) -> bool {
    app.ui.right == Some(crate::RightPane::WebData) && app.ui.web_data == "nldi"
}

/// Whether a map click goes to the NLDI (the crosshair shows).
pub fn wants_click(app: &RustGisApp) -> bool {
    // Other map tools (measuring, selecting, zooming, editing) keep their clicks.
    active(app) && app.ui.nldi.picking && app.ui.nldi.waiting.is_none() && app.session.tool == rustgis_engine::Tool::Explore
}

/// A map click while the pane waits for one: traces the flowline there.
pub fn click(app: &mut RustGisApp, c: Coord) -> bool {
    if !wants_click(app) {
        return false;
    }
    let st = &mut app.ui.nldi;
    st.lon = format!("{:.5}", c.x);
    st.lat = format!("{:.5}", c.y);
    trace(app, c.x, c.y);
    true
}

/// Traces the flowline at a point: the flowtrace, then (if it fails) the hydrolocation.
fn trace(app: &mut RustGisApp, lon: f64, lat: f64) {
    let st = &mut app.ui.nldi;
    st.error = None;
    st.info = None;
    if let Err(e) = nldi::point(lon, lat) {
        st.error = Some(e);
        return;
    }
    st.at = Some((lon, lat));
    st.comid = None;
    st.sources.clear();
    st.sources_for = None;
    st.plotted = false;
    let direction = st.direction;
    st.waiting = Some((Asked::Trace { lon, lat, direction }, post(&nldi::flowtrace_url(), nldi::flowtrace_body(lon, lat, direction))));
}

/// Runs an `nldi.*` command with an answer, keeping the layers it adds.
fn add(app: &mut RustGisApp, id: &str, p: Value) -> Result<Value, String> {
    let v = app.run(id, p)?;
    for k in ["layer", "raindrop"] {
        if let Some(l) = v[k].as_u64().and_then(|l| u32::try_from(l).ok()) {
            app.ui.nldi.layers.push(l);
        }
    }
    Ok(v)
}

/// Lands the answers and moves the steps along.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some((_, slot)) = &app.ui.nldi.waiting else { return };
    let Some(r) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let Some((asked, _)) = app.ui.nldi.waiting.take() else { return };
    let text = |b: Vec<u8>| String::from_utf8(b).map_err(|_| "the NLDI's answer isn't UTF-8 text".to_string());
    let done = match asked {
        Asked::Trace { lon, lat, direction } => {
            // The flowtrace's answer, read first so a failure falls back to the hydrolocation.
            match r.and_then(|b| nldi::parse_flowtrace(&b).map(|_| b)).and_then(text) {
                Ok(t) => {
                    add(app, "nldi.flowline", json!({"lon": lon, "lat": lat, "direction": direction, "answer": t})).map(|v| landed(app, &v))
                }
                Err(e) => {
                    log::info!("NLDI flowtrace failed ({e}); using the hydrolocation");
                    if direction != "none" {
                        app.ui.nldi.info =
                            Some(tl!("The flowtrace didn't answer: the whole flowline nearest the point is shown.").to_string());
                    }
                    app.ui.nldi.waiting = Some((Asked::Hydro { lon, lat }, fetch(&nldi::hydrolocation_url(lon, lat))));
                    Ok(())
                }
            }
        }
        Asked::Hydro { lon, lat } => r.and_then(|b| nldi::parse_hydrolocation(&b)).map(|(comid, snapped)| {
            app.ui.nldi.waiting = Some((Asked::Line { lon, lat, snapped }, fetch(&nldi::comid_url(comid))));
        }),
        Asked::Line { lon, lat, snapped } => r.and_then(text).and_then(|t| {
            add(app, "nldi.flowline", json!({"lon": lon, "lat": lat, "answer": t, "snapped": snapped})).map(|v| landed(app, &v))
        }),
        Asked::Basin { comid } => r.and_then(text).and_then(|t| add(app, "nldi.basin", json!({"comid": comid, "answer": t})).map(|_| ())),
        Asked::Sources { comid, mode } => r.and_then(|b| nldi::parse_sources(&b)).and_then(|list| {
            let st = &mut app.ui.nldi;
            if list.is_empty() {
                return Err(tl!("NLDI returned no plottable navigation source.").to_string());
            }
            if !list.iter().any(|s| s.id == st.source) {
                st.source = list.first().map(|s| s.id.clone()).unwrap_or_default();
            }
            st.sources = list;
            st.sources_for = Some((comid, mode));
            navigate(app);
            Ok(())
        }),
        Asked::Navigate { comid, mode, source, distance } => r.and_then(text).and_then(|t| {
            add(app, "nldi.navigate", json!({"comid": comid, "mode": mode, "source": source, "distance": distance, "answer": t})).map(|v| {
                app.ui.nldi.plotted = true;
                let n = v["features"].as_u64().unwrap_or(0);
                app.ui.nldi.info = Some(crate::i18n::fmt("Plotted {n} {source} features.", &[("n", &n.to_string()), ("source", &source)]));
            })
        }),
    };
    if let Err(e) = done {
        app.ui.nldi.error = Some(nldi::explain(e));
    }
    app.map.invalidate();
}

/// A flowline added: its COMID, for the basin and navigation.
fn landed(app: &mut RustGisApp, v: &Value) {
    app.ui.nldi.comid = v["comid"].as_u64();
    if app.ui.nldi.comid.is_none() {
        app.ui.nldi.error = Some(tl!("No COMID was returned for this point.").to_string());
    }
}

/// Plots the source picked along the navigation, loading the sources first.
fn navigate(app: &mut RustGisApp) {
    let st = &mut app.ui.nldi;
    st.error = None;
    let Some(comid) = st.comid else {
        st.error = Some(tl!("Trace a point first: click the map.").to_string());
        return;
    };
    if st.mode.is_empty() {
        st.error = Some(tl!("Select a navigation method after tracing a point.").to_string());
        return;
    }
    let mode = st.mode;
    if st.sources_for != Some((comid, mode)) {
        st.waiting = Some((Asked::Sources { comid, mode }, fetch(&nldi::sources_url(comid, mode))));
        return;
    }
    let distance = nldi::clamp_distance(st.distance);
    match nldi::navigation_url(comid, mode, &st.source, distance) {
        Ok(url) => st.waiting = Some((Asked::Navigate { comid, mode, source: st.source.clone(), distance }, fetch(&url))),
        Err(e) => st.error = Some(e),
    }
}

/// The NLDI pane.
pub fn pane(app: &mut RustGisApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("USGS NLDI network tools: choose a flowline direction and click the map (or trace the point below). Then get the basin, or choose a navigation method and press Load sources & plot to plot its flowlines, or pick a catalog such as stream gages or wells and plot it too. The NLDI covers the United States' NHDPlus network.")).weak());
    let busy = app.ui.nldi.waiting.is_some();
    let w = ui.available_width();
    let field = (w - 130.0).clamp(160.0, 320.0);
    let mut go = None;
    egui::Grid::new("nldi").num_columns(2).spacing(egui::vec2(10.0, 8.0)).show(ui, |ui| {
        let st = &mut app.ui.nldi;
        let input_label = ui.label(tl!("Flowline"));
        let shown = nldi::DIRECTIONS.iter().find(|(k, _)| *k == st.direction).map(|(_, l)| tl!(*l)).unwrap_or_default();
        egui::ComboBox::from_id_salt("nldi_dir")
            .selected_text(shown)
            .width(field)
            .show_ui(ui, |ui| {
                for (k, l) in nldi::DIRECTIONS {
                    ui.selectable_value(&mut st.direction, k, tl!(*l));
                }
            })
            .response
            .labelled_by(input_label.id);
        ui.end_row();
        ui.label(tl!("Point"));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.lon).hint_text(tl!("lon")).desired_width(70.0));
            ui.add(egui::TextEdit::singleline(&mut st.lat).hint_text(tl!("lat")).desired_width(70.0));
            if ui.selectable_label(st.picking, "⌖").on_hover_text(tl!("Click the map to trace a point")).clicked() {
                st.picking = !st.picking;
            }
        });
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new(tl!("Trace"))).clicked() {
                go = Some(None);
            }
            if ui.add_enabled(!busy, egui::Button::new(tl!("Map centre"))).clicked() {
                go = Some(Some(()));
            }
        });
        ui.end_row();
    });
    if let Some(centre) = go {
        let point = if centre.is_some() {
            rustgis_engine::cmd::catalogs::area(&app.session, &Value::Null).map(|[w, s, e, n]| ((w + e) / 2.0, (s + n) / 2.0))
        } else {
            let st = &app.ui.nldi;
            match (st.lon.trim().parse::<f64>(), st.lat.trim().parse::<f64>()) {
                (Ok(x), Ok(y)) => Ok((x, y)),
                _ => Err(tl!("type a longitude and latitude, or click the map").to_string()),
            }
        };
        match point {
            Ok((x, y)) => {
                app.ui.nldi.lon = format!("{x:.5}");
                app.ui.nldi.lat = format!("{y:.5}");
                trace(app, x, y);
            }
            Err(e) => app.ui.nldi.error = Some(e),
        }
    }
    if let Some(c) = app.ui.nldi.comid {
        ui.label(egui::RichText::new(crate::i18n::fmt("COMID {comid}", &[("comid", &c.to_string())])).small().color(t.text_dim));
    }
    let traced = app.ui.nldi.comid.is_some();
    if ui.add_enabled(traced && !busy, egui::Button::new(tl!("Basin from hydrolocation"))).clicked()
        && let Some(comid) = app.ui.nldi.comid
    {
        app.ui.nldi.error = None;
        app.ui.nldi.waiting = Some((Asked::Basin { comid }, fetch(&nldi::basin_url(comid))));
    }
    ui.separator();
    let mut plot = false;
    egui::Grid::new("nldi_nav").num_columns(2).spacing(egui::vec2(10.0, 8.0)).show(ui, |ui| {
        let st = &mut app.ui.nldi;
        let input_label = ui.label(tl!("Navigation"));
        let shown = nldi::MODES.iter().find(|(c, _, _)| *c == st.mode).map(|(_, _, l)| tl!(*l)).unwrap_or(tl!("Select a direction…"));
        egui::ComboBox::from_id_salt("nldi_mode")
            .selected_text(shown)
            .width(field)
            .show_ui(ui, |ui| {
                for (code, _, label) in nldi::MODES {
                    ui.selectable_value(&mut st.mode, code, tl!(*label));
                }
            })
            .response
            .labelled_by(input_label.id);
        ui.end_row();
        let input_label = ui.label(tl!("Distance (km)"));
        ui.add(egui::DragValue::new(&mut st.distance).range(1.0..=nldi::MAX_DISTANCE).speed(5.0)).labelled_by(input_label.id);
        ui.end_row();
        let input_label = ui.label(tl!("Source"));
        let loaded = st.comid.is_some() && st.sources_for == st.comid.map(|c| (c, st.mode));
        if loaded {
            let shown = st.sources.iter().find(|s| s.id == st.source).map(|s| s.name.clone()).unwrap_or_else(|| st.source.clone());
            egui::ComboBox::from_id_salt("nldi_src")
                .selected_text(shown)
                .width(field)
                .show_ui(ui, |ui| {
                    for s in &st.sources {
                        ui.selectable_value(&mut st.source, s.id.clone(), format!("{} ({})", s.name, s.id));
                    }
                })
                .response
                .labelled_by(input_label.id);
        } else {
            ui.label(egui::RichText::new(tl!("Press Load sources & plot first")).color(t.text_dim));
        }
        ui.end_row();
    });
    let label = if app.ui.nldi.plotted { tl!("Plot another navigation layer") } else { tl!("Load sources & plot") };
    if ui.add_enabled(traced && !busy && !app.ui.nldi.mode.is_empty(), egui::Button::new(label)).clicked() {
        plot = true;
    }
    if plot {
        navigate(app);
    }
    if !app.ui.nldi.layers.is_empty() && ui.add_enabled(!busy, egui::Button::new(tl!("Clear NLDI result"))).clicked() {
        for id in std::mem::take(&mut app.ui.nldi.layers) {
            if app.session.map().layer(id).is_some() {
                let _ = app.run("layer.remove", json!({"layer": id}));
            }
        }
        let st = &mut app.ui.nldi;
        st.at = None;
        st.comid = None;
        st.sources_for = None;
        st.plotted = false;
        st.info = Some(tl!("NLDI result cleared.").to_string());
    }
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Asking the NLDI…"));
        });
    }
    if let Some(e) = &app.ui.nldi.error {
        ui.label(egui::RichText::new(e).color(t.red));
    }
    if let Some(i) = &app.ui.nldi.info {
        ui.label(egui::RichText::new(i).weak());
    }
}

/// The point traced, on the map while the pane is open.
pub fn paint(app: &RustGisApp, p: &egui::Painter, rect: Rect, t: &crate::theme::Tokens, to_screen: impl Fn(Coord) -> Option<Pos2>) {
    if !active(app) {
        return;
    }
    if let Some((x, y)) = app.ui.nldi.at
        && let Some(s) = to_screen(Coord::new(x, y)).filter(|s| rect.contains(*s))
    {
        p.circle_filled(s, 6.0, t.accent);
        p.circle_stroke(s, 6.0, Stroke::new(2.0, egui::Color32::WHITE));
    }
}
