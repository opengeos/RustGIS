//! The browser shell: file picker, downloads, drag-and-drop, the unsaved-changes guard and the
//! eframe web runner.

use std::cell::Cell;
use std::rc::Rc;

use rustgis_ui_egui::rustgis_engine::{Project, Session};
use rustgis_ui_egui::{Inbox, RustGisApp, Services};
use wasm_bindgen::JsCast as _;

const DATA_EXTS: &[&str] = &["geojson", "json", "csv", "txt"];
const CANVAS_ID: &str = "rustgis_canvas";
const LOADING_ID: &str = "rustgis_loading";

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let dirty = Rc::new(Cell::new(false));
        if let Err(e) = guard_unload(dirty.clone()) {
            log::error!("no unsaved-changes guard: {e}");
        }
        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(move |cc| {
                    let inbox: Inbox = Inbox::default();
                    let font_inbox: Inbox = Inbox::default();
                    let app = if query().contains("blank") {
                        RustGisApp::new(Session::new(Project::default()))
                    } else {
                        RustGisApp::with_sample()
                    };
                    let mut app = app.with_services(services(inbox.clone(), font_inbox, cc.egui_ctx.clone()));
                    // `?lang=de` picks the interface language for this visit.
                    let q = query();
                    if let Some(code) = q.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix("lang=")) {
                        app.ui.language = code.to_ascii_lowercase();
                    }
                    Ok(Box::new(WebShell { app, inbox, dirty }))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => {
                    el.set_inner_html(&format!("<p>RustGIS failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>"))
                }
            }
        }
    });
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

/// Reads dropped files asynchronously (browsers can't read them synchronously) into the inbox,
/// and mirrors the unsaved-changes flag for the `beforeunload` guard.
struct WebShell {
    app: RustGisApp,
    inbox: Inbox,
    dirty: Rc<Cell<bool>>,
}

impl eframe::App for WebShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for f in dropped {
            let inbox = self.inbox.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped.geojson".into());
                match f.bytes_async().await {
                    Ok(bytes) => {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => log::error!("couldn't read dropped file {name}: {e}"),
                }
            });
        }
        self.app.logic(ctx);
        self.dirty.set(self.app.session.dirty);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Downloads `url`.
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response =
        wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url)).await.map_err(err)?.dyn_into().map_err(err)?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(err)?).await.map_err(err)?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

fn services(inbox: Inbox, font_inbox: Inbox, ctx: egui::Context) -> Services {
    let open_inbox = inbox.clone();
    let font_ctx = ctx.clone();
    let font_sink = font_inbox.clone();
    Services {
        load_pref: Some(Box::new(|key: &str| storage().and_then(|s| s.get_item(&format!("rustgis.{key}")).ok().flatten()))),
        save_pref: Some(Box::new(|key: &str, value: &str| {
            if let Some(s) = storage() {
                let _ = s.set_item(&format!("rustgis.{key}"), value);
            }
        })),
        fetch_font: Some(Box::new(move |url: &str| {
            let (url, inbox, ctx) = (url.to_string(), font_sink.clone(), font_ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_bytes(&url).await {
                    Ok(bytes) => {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((url, bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => log::error!("couldn't load font {url}: {e}"),
                }
            });
        })),
        font_inbox: Some(font_inbox),
        open_async: Some(Box::new(move |purpose: &str| {
            let inbox = open_inbox.clone();
            let ctx = ctx.clone();
            let dialog = if purpose == "project" {
                rfd::AsyncFileDialog::new().add_filter("RustGIS project", &["rgisx"])
            } else {
                rfd::AsyncFileDialog::new().add_filter("Vector data", DATA_EXTS)
            };
            wasm_bindgen_futures::spawn_local(async move {
                let Some(file) = dialog.pick_file().await else {
                    return;
                };
                let bytes = file.read().await;
                inbox.lock().unwrap_or_else(|e| e.into_inner()).push((file.file_name(), bytes));
                ctx.request_repaint();
            });
        })),
        download: Some(Box::new(|name: &str, bytes: Vec<u8>| {
            if let Err(e) = download(name, &bytes) {
                log::error!("download of {name} failed: {e}");
            }
        })),
        inbox: Some(inbox),
    }
}

/// Saves bytes through a temporary `<a download>` link.
fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array);
    let props = web_sys::BlobPropertyBag::new();
    props.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &props).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "not an anchor".to_string())?;
    a.set_href(&url);
    a.set_download(name);
    // Some browsers only follow links that are in the document.
    let body = document.body().ok_or("no body")?;
    body.append_child(&a).map_err(err)?;
    a.click();
    a.remove();
    // Browsers may start the download after `click` returns, so free the URL a little later.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let window = web_sys::window().ok_or("no window")?;
    window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000).map_err(err)?;
    Ok(())
}

/// Closing, reloading or leaving the tab with unsaved changes asks first.
fn guard_unload(dirty: Rc<Cell<bool>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let on_unload = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(move |e: web_sys::BeforeUnloadEvent| {
        if dirty.get() {
            e.prevent_default();
            e.set_return_value("unsaved");
        }
    });
    window.add_event_listener_with_callback("beforeunload", on_unload.as_ref().unchecked_ref()).map_err(|e| format!("{e:?}"))?;
    on_unload.forget();
    Ok(())
}
