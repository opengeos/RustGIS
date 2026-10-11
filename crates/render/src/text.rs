//! Text for exported images: label rasterisation (with halos) and the scale bar.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{Paint, Pixmap, Rect, Transform};

use crate::{PlacedLabel, View};
use rustgis_map::Rgba;

fn font(bold: bool) -> Option<&'static FontRef<'static>> {
    static R: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    static B: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    let f = crate::fonts::ui_fonts();
    if bold {
        B.get_or_init(|| FontRef::try_from_slice(&f.semibold).ok()).as_ref()
    } else {
        R.get_or_init(|| FontRef::try_from_slice(&f.regular).ok()).as_ref()
    }
}

/// Text width in points at `size` (font px ≈ size × 1.33 for point sizes).
pub fn measure(text: &str, size: f32, bold: bool) -> f32 {
    let Some(f) = font(bold) else { return text.chars().count() as f32 * size * 0.6 };
    let sf = f.as_scaled(PxScale::from(size * 1.33));
    let mut w = 0.0;
    let mut prev = None;
    for ch in text.chars().take(500) {
        let id = f.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

fn blend(pm: &mut Pixmap, x: i32, y: i32, c: Rgba, cov: f32) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    if x < 0 || y < 0 || x >= w || y >= h || cov <= 0.0 {
        return;
    }
    let idx = (y * w + x) as usize;
    let data = pm.pixels_mut();
    let Some(px) = data.get_mut(idx) else { return };
    let a = (c.a() as f32 / 255.0) * cov.min(1.0);
    let inv = 1.0 - a;
    let r = (c.r() as f32 * a + px.red() as f32 * inv).round() as u8;
    let g = (c.g() as f32 * a + px.green() as f32 * inv).round() as u8;
    let b = (c.b() as f32 * a + px.blue() as f32 * inv).round() as u8;
    let al = ((a * 255.0) + px.alpha() as f32 * inv).round().min(255.0) as u8;
    if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(r.min(al), g.min(al), b.min(al), al) {
        *px = p;
    }
}

fn draw_text(pm: &mut Pixmap, text: &str, cx: f32, cy: f32, px_size: f32, color: Rgba, bold: bool, offset: (f32, f32)) {
    let Some(f) = font(bold) else { return };
    let sf = f.as_scaled(PxScale::from(px_size));
    let width = {
        let mut w = 0.0;
        for ch in text.chars().take(500) {
            w += sf.h_advance(f.glyph_id(ch));
        }
        w
    };
    let mut x = cx - width / 2.0 + offset.0;
    let base = cy + (sf.ascent() + sf.descent()) / 2.0 + offset.1;
    for ch in text.chars().take(500) {
        let id = f.glyph_id(ch);
        let g = id.with_scale_and_position(sf.scale(), ab_glyph::point(x, base));
        x += sf.h_advance(id);
        if let Some(og) = f.outline_glyph(g) {
            let bb = og.px_bounds();
            og.draw(|gx, gy, cov| blend(pm, bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32, color, cov));
        }
    }
}

/// Draws a placed label (logical coordinates) with its halo.
pub fn draw(pm: &mut Pixmap, l: &PlacedLabel, ppp: f32) {
    let px = l.size * 1.33 * ppp;
    let (cx, cy) = (l.x * ppp, l.y * ppp);
    if let Some(h) = l.halo {
        let r = 1.2 * ppp;
        for (dx, dy) in
            [(-r, 0.0), (r, 0.0), (0.0, -r), (0.0, r), (-r * 0.7, -r * 0.7), (r * 0.7, r * 0.7), (-r * 0.7, r * 0.7), (r * 0.7, -r * 0.7)]
        {
            draw_text(pm, &l.text, cx, cy, px, h, l.bold, (dx, dy));
        }
    }
    draw_text(pm, &l.text, cx, cy, px, l.color, l.bold, (0.0, 0.0));
}

/// A two-tone scale bar in the lower-left corner.
/// `ground_per_pt` is ground metres per logical pixel at the centre of view.
pub fn scale_bar(pm: &mut Pixmap, view: &View, ground_per_pt: f64) {
    let ppp = view.pixels_per_point as f32;
    let target = rustgis_geom::nice_distance(ground_per_pt * 120.0);
    if target <= 0.0 {
        return;
    }
    let len = (target / ground_per_pt) as f32;
    let (x0, y0) = (16.0 * ppp, pm.height() as f32 - 22.0 * ppp);
    let mut bg = Paint::default();
    bg.set_color_rgba8(255, 255, 255, 210);
    if let Some(r) = Rect::from_xywh(x0 - 6.0 * ppp, y0 - 18.0 * ppp, len * ppp + 12.0 * ppp, 30.0 * ppp) {
        pm.fill_rect(r, &bg, Transform::identity(), None);
    }
    for i in 0..4 {
        let mut p = Paint::default();
        if i % 2 == 0 {
            p.set_color_rgba8(40, 40, 40, 255);
        } else {
            p.set_color_rgba8(250, 250, 250, 255);
        }
        if let Some(r) = Rect::from_xywh(x0 + len * ppp * i as f32 / 4.0, y0, len * ppp / 4.0, 5.0 * ppp) {
            pm.fill_rect(r, &p, Transform::identity(), None);
        }
    }
    let label = PlacedLabel {
        text: rustgis_geom::format_distance(target),
        x: (x0 / ppp) + len / 2.0,
        y: y0 / ppp - 8.0,
        size: 8.0,
        color: Rgba::rgb(30, 30, 30),
        halo: None,
        bold: false,
    };
    draw(pm, &label, ppp);
}

/// Horizontal alignment for [`text_at`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
}

/// Draws one line of text at pixel position (`x`, `y`) (`y` is the line's middle), `px_size`
/// pixels tall, optionally with a white halo.
pub fn text_at(pm: &mut Pixmap, text: &str, x: f32, y: f32, px_size: f32, color: Rgba, bold: bool, align: Align, halo: bool) {
    if !(x.is_finite() && y.is_finite() && px_size.is_finite()) || px_size <= 0.0 {
        return;
    }
    let cx = match align {
        Align::Center => x,
        Align::Left => x + measure(text, px_size / 1.33, bold) / 2.0,
    };
    if halo {
        let r = (px_size * 0.09).max(1.0);
        for (dx, dy) in [(-r, 0.0), (r, 0.0), (0.0, -r), (0.0, r)] {
            draw_text(pm, text, cx, y, px_size, Rgba::new(255, 255, 255, 230), bold, (dx, dy));
        }
    }
    draw_text(pm, text, cx, y, px_size, color, bold, (0.0, 0.0));
}
