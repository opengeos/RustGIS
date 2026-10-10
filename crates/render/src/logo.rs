//! The RustGIS logo, drawn in code: a rust-coloured cog framing a globe split into four
//! segments by white gaps, with a tilted orbit. The same design is published as SVG in
//! rustgis-assets; this keeps the app free of image files.

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LinearGradient, Mask, Paint, PathBuilder, Pixmap, Point, SpreadMode, Stroke, Transform,
};

const C: f32 = 256.0;

fn gradient(a: (u8, u8, u8), b: (u8, u8, u8), x0: f32, y0: f32, x1: f32, y1: f32) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    let stops =
        vec![GradientStop::new(0.0, Color::from_rgba8(a.0, a.1, a.2, 255)), GradientStop::new(1.0, Color::from_rgba8(b.0, b.1, b.2, 255))];
    match LinearGradient::new(Point::from_xy(x0, y0), Point::from_xy(x1, y1), stops, SpreadMode::Pad, Transform::identity()) {
        Some(sh) => p.shader = sh,
        None => p.set_color_rgba8(a.0, a.1, a.2, 255),
    }
    p
}

fn solid(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    p.set_color_rgba8(r, g, b, a);
    p
}

const RUST: ((u8, u8, u8), (u8, u8, u8)) = ((0xE4, 0x78, 0x3D), (0xA8, 0x3F, 0x24));

/// The logo as a premultiplied RGBA pixmap of `size`×`size` pixels (16..=1024).
pub fn pixmap(size: u32) -> Option<Pixmap> {
    let size = size.clamp(16, 1024);
    let mut pm = Pixmap::new(size, size)?;
    let t = Transform::from_scale(size as f32 / 512.0, size as f32 / 512.0);
    let rust = gradient(RUST.0, RUST.1, 0.0, 0.0, 512.0, 512.0);

    // Cog.
    let mut pb = PathBuilder::new();
    let teeth = 12;
    let step = std::f32::consts::TAU / teeth as f32;
    let mut first = true;
    for i in 0..teeth {
        let a = i as f32 * step - std::f32::consts::FRAC_PI_2;
        for (frac, r) in [(-0.5, 212.0), (-0.31, 212.0), (-0.21, 246.0), (0.21, 246.0), (0.31, 212.0)] {
            let ang = a + frac * step;
            let (x, y) = (C + r * ang.cos(), C + r * ang.sin());
            if first {
                pb.move_to(x, y);
                first = false;
            } else {
                pb.line_to(x, y);
            }
        }
    }
    pb.close();
    pm.fill_path(&pb.finish()?, &rust, FillRule::Winding, t, None);
    let white = solid(255, 255, 255, 255);
    pm.fill_path(&PathBuilder::from_circle(C, C, 184.0)?, &white, FillRule::Winding, t, None);

    // Globe segments, clipped to a circle.
    let mut mask = Mask::new(size, size)?;
    mask.fill_path(&PathBuilder::from_circle(C, C, 166.0)?, FillRule::Winding, true, t);
    let seg = |cmds: &dyn Fn(&mut PathBuilder)| -> Option<tiny_skia::Path> {
        let mut pb = PathBuilder::new();
        cmds(&mut pb);
        pb.close();
        pb.finish()
    };
    let blue = seg(&|pb| {
        pb.move_to(60.0, 60.0);
        pb.line_to(250.0, 60.0);
        pb.cubic_to(215.0, 150.0, 205.0, 230.0, 214.0, 262.0);
        pb.cubic_to(150.0, 250.0, 100.0, 262.0, 60.0, 282.0);
    })?;
    let teal = seg(&|pb| {
        pb.move_to(250.0, 60.0);
        pb.line_to(452.0, 60.0);
        pb.line_to(452.0, 226.0);
        pb.cubic_to(380.0, 236.0, 300.0, 250.0, 214.0, 262.0);
        pb.cubic_to(205.0, 230.0, 215.0, 150.0, 250.0, 60.0);
    })?;
    let green = seg(&|pb| {
        pb.move_to(60.0, 282.0);
        pb.cubic_to(100.0, 262.0, 150.0, 250.0, 214.0, 262.0);
        pb.cubic_to(222.0, 330.0, 250.0, 400.0, 280.0, 452.0);
        pb.line_to(60.0, 452.0);
    })?;
    let gold = seg(&|pb| {
        pb.move_to(214.0, 262.0);
        pb.cubic_to(300.0, 250.0, 380.0, 236.0, 452.0, 226.0);
        pb.line_to(452.0, 452.0);
        pb.line_to(280.0, 452.0);
        pb.cubic_to(250.0, 400.0, 222.0, 330.0, 214.0, 262.0);
    })?;
    for (path, (a, b), (x0, y0, x1, y1)) in [
        (&blue, ((0x3B, 0x8E, 0xDB), (0x1F, 0x5F, 0xA8)), (60.0, 60.0, 250.0, 282.0)),
        (&teal, ((0x22, 0xB5, 0xA2), (0x0E, 0x85, 0x76)), (214.0, 60.0, 452.0, 262.0)),
        (&green, ((0x8C, 0xC8, 0x4B), (0x4E, 0x9A, 0x2E)), (60.0, 250.0, 280.0, 452.0)),
        (&gold, ((0xF8, 0xC1, 0x50), (0xE3, 0x9A, 0x22)), (214.0, 226.0, 452.0, 452.0)),
    ] {
        pm.fill_path(path, &gradient(a, b, x0, y0, x1, y1), FillRule::Winding, t, Some(&mask));
    }
    let gap = Stroke { width: 14.0, line_cap: LineCap::Round, ..Stroke::default() };
    let mut pb = PathBuilder::new();
    pb.move_to(250.0, 60.0);
    pb.cubic_to(215.0, 150.0, 205.0, 230.0, 214.0, 262.0);
    pb.cubic_to(222.0, 330.0, 250.0, 400.0, 280.0, 452.0);
    pb.move_to(60.0, 282.0);
    pb.cubic_to(100.0, 262.0, 150.0, 250.0, 214.0, 262.0);
    pb.cubic_to(300.0, 250.0, 380.0, 236.0, 452.0, 226.0);
    pm.stroke_path(&pb.finish()?, &white, &gap, t, Some(&mask));
    let mut pb = PathBuilder::new();
    pb.move_to(70.0, 170.0);
    pb.cubic_to(170.0, 135.0, 330.0, 125.0, 440.0, 150.0);
    pb.move_to(75.0, 360.0);
    pb.cubic_to(180.0, 345.0, 330.0, 335.0, 440.0, 345.0);
    pm.stroke_path(&pb.finish()?, &solid(255, 255, 255, 140), &Stroke { width: 5.0, ..Stroke::default() }, t, Some(&mask));

    // Orbit: the front half of an ellipse tilted by -28°, ending in a node.
    let (rx, ry, tilt) = (226.0f32, 70.0f32, (-28.0f32).to_radians());
    let mut pb = PathBuilder::new();
    for i in 0..=64 {
        let th = std::f32::consts::PI * (1.0 - i as f32 / 64.0);
        let (ex, ey) = (rx * th.cos(), ry * th.sin());
        let (x, y) = (C + ex * tilt.cos() - ey * tilt.sin(), C + ex * tilt.sin() + ey * tilt.cos());
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    let orbit = pb.finish()?;
    pm.stroke_path(&orbit, &white, &Stroke { width: 24.0, line_cap: LineCap::Round, ..Stroke::default() }, t, None);
    pm.stroke_path(&orbit, &rust, &Stroke { width: 11.0, line_cap: LineCap::Round, ..Stroke::default() }, t, None);
    let (nx, ny) = (C + rx * tilt.cos(), C + rx * tilt.sin());
    pm.fill_path(&PathBuilder::from_circle(nx, ny, 22.0)?, &white, FillRule::Winding, t, None);
    pm.fill_path(&PathBuilder::from_circle(nx, ny, 14.0)?, &rust, FillRule::Winding, t, None);
    Some(pm)
}

/// The logo as straight (unpremultiplied) RGBA bytes, for window icons.
pub fn rgba(size: u32) -> Option<(u32, Vec<u8>)> {
    let pm = pixmap(size)?;
    let mut out = Vec::with_capacity(pm.data().len());
    for p in pm.pixels() {
        let c = p.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some((pm.width(), out))
}

#[cfg(test)]
mod tests {
    #[test]
    fn draws_at_every_size() {
        for s in [0, 16, 32, 256, 5000] {
            let (w, px) = super::rgba(s).unwrap();
            assert_eq!(px.len() as u32, w * w * 4);
            assert!(px.chunks(4).any(|c| c[3] == 255), "logo has opaque pixels");
        }
    }
}
