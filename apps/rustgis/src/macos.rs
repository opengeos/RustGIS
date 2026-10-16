//! macOS: the native title bar the header is drawn into.
//!
//! The window is created with a transparent title bar over a full-size content view, so the
//! window buttons (close, minimise, zoom) float over our header. This reads where they are from
//! AppKit, with safe bindings only: the window comes from `NSApplication`, not from a raw handle.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSWindowButton, NSWindowStyleMask};
use rustgis_ui_egui::TitlebarMetrics;

/// Space between the zoom button and the first thing in the header.
const GAP: f32 = 12.0;

/// The title bar geometry now, or `None` in full screen (no title bar, no buttons).
pub fn titlebar() -> Option<TitlebarMetrics> {
    let fallback = TitlebarMetrics::FALLBACK;
    let Some(mtm) = MainThreadMarker::new() else { return Some(fallback) };
    let app = NSApplication::sharedApplication(mtm);
    // The main window, not the key one: a file dialog or the About panel can be key.
    let Some(window) = app.mainWindow().or_else(|| app.keyWindow()) else { return Some(fallback) };
    if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return None;
    }
    let height = (window.frame().size.height - window.contentLayoutRect().size.height) as f32;
    // The button frame is in its title bar container, whose origin is the window's left edge. Until
    // the window has laid out, it is empty: keep the fallback rather than drawing under the buttons.
    let left_inset = window.standardWindowButton(NSWindowButton::ZoomButton).and_then(|b| {
        let f = b.frame();
        (f.size.width > 0.0 && f.origin.x > 0.0).then_some((f.origin.x + f.size.width) as f32 + GAP)
    });
    Some(TitlebarMetrics {
        left_inset: left_inset.unwrap_or(fallback.left_inset),
        height: if height > 0.0 { height } else { fallback.height },
    })
}
