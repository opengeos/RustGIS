//! macOS: the native title bar the header is drawn into.
//!
//! The window is created with a transparent title bar over a full-size content view, so the
//! window buttons (close, minimise, zoom) float over our header. This reads where they are from
//! AppKit, with safe bindings only: the window comes from `NSApplication`, not from a raw handle.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSWindowButton, NSWindowStyleMask};
use rustgis_ui_egui::TitlebarMetrics;

/// What AppKit reports before the window exists: the usual button strip and title bar height.
const FALLBACK: TitlebarMetrics = TitlebarMetrics { left_inset: 80.0, height: 34.0 };
/// Space between the zoom button and the first thing in the header.
const GAP: f32 = 12.0;

/// The title bar geometry now, or `None` in full screen (no title bar, no buttons).
pub fn titlebar() -> Option<TitlebarMetrics> {
    let Some(mtm) = MainThreadMarker::new() else { return Some(FALLBACK) };
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app.keyWindow().or_else(|| app.mainWindow()) else { return Some(FALLBACK) };
    if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return None;
    }
    let height = (window.frame().size.height - window.contentLayoutRect().size.height) as f32;
    let left_inset = window
        .standardWindowButton(NSWindowButton::ZoomButton)
        .map(|b| {
            let f = b.frame();
            (f.origin.x + f.size.width) as f32 + GAP
        })
        .unwrap_or(FALLBACK.left_inset);
    Some(TitlebarMetrics { left_inset, height: if height > 0.0 { height } else { FALLBACK.height } })
}
