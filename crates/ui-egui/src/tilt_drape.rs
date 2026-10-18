//! The moving tilted map. While a tilted map pans, zooms, turns or tilts, its ground drawn flat
//! once in bands (`render::render_tilt_drape`) is laid over the tilted ground as textured meshes,
//! so a frame only moves vertices, as the globe's drape does; the sky above the horizon is a
//! gradient. Extruded layers stand up over it in perspective as MapLibre's fill extrusions do:
//! tessellated once, then projected, lit and ordered far to near every frame. The exact frame is
//! drawn again once the map settles.

use egui::epaint::Vertex;
use egui::{Color32, Mesh, Painter, Rect, Shape, TextureFilter, TextureHandle, TextureOptions, pos2};
use rustgis_engine::geom::{Coord, Extent};
use rustgis_engine::map::camera::{Band, FAR};
use rustgis_engine::map::{Camera, Map};
use rustgis_engine::render::{self, ExtrudedShape, View};

use crate::globe_drape::DrapeKey;
use crate::theme::c32;

/// How far past its band each picture reaches on every side, as a fraction of its size: the map
/// can move this far before the drape is drawn again.
const MARGIN: f64 = 0.3;
/// Zooming in past this factor of the drape's scale draws it again, sharper.
const MAX_ZOOM_IN: f64 = 2.0;
/// Seconds between drawing the drape again for tiles that have arrived meanwhile.
const TILE_REFRESH: f64 = 0.5;
/// Mesh cells along each side of a band: fine enough that the texture's straight-line
/// interpolation across a cell doesn't show the perspective.
const CELLS: usize = 16;

struct Drape {
    key: DrapeKey,
    ppp: f32,
    /// The camera's resolution when it was drawn.
    res: f64,
    tiles: u64,
    built: f64,
    /// Each band and its picture.
    bands: Vec<(Band, TextureHandle)>,
    /// The extruded features, tessellated.
    shapes: Vec<ExtrudedShape>,
}

/// The drape of the moving tilted map, kept between frames while the map doesn't change.
#[derive(Default)]
pub(crate) struct TiltDrape {
    drape: Option<Drape>,
}

impl TiltDrape {
    /// Lets go of the textures (the map no longer tilts).
    pub fn clear(&mut self) {
        self.drape = None;
    }

    /// Makes sure the drape covers the tilted `w × h` view of `map`, drawing it again when the
    /// view has moved or zoomed past it or the map changed. False when there is nothing to
    /// drape.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, ctx: &egui::Context, map: &mut Map, key: DrapeKey, tiles: u64, w: f64, h: f64, ppp: f32, time: f64) -> bool {
        let cam = map.camera;
        // A screen's worth of detail at most: the settled frame is sharp.
        let drape_ppp = ppp.min(1.0);
        let fresh = self.drape.as_ref().is_some_and(|d| {
            d.key == key
                && d.ppp == drape_ppp
                && cam.res * MAX_ZOOM_IN >= d.res
                && (d.tiles == tiles || time - d.built < TILE_REFRESH)
                && covers(&cam, w, h, &d.bands)
        });
        if fresh {
            return true;
        }
        let max_side = ctx.input(|i| i.max_texture_side).clamp(256, 4096) as u32;
        let view = View { width: w, height: h, pixels_per_point: f64::from(drape_ppp), camera: cam, sky: true };
        let patches = render::render_tilt_drape(map, &view, MARGIN, max_side);
        if patches.is_empty() {
            self.drape = None;
            return false;
        }
        let options = TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR };
        let mut old = self.drape.take().map(|d| d.bands).unwrap_or_default().into_iter().map(|b| b.1);
        let bands = patches
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let img =
                    egui::ColorImage::from_rgba_premultiplied([p.pixmap.width() as usize, p.pixmap.height() as usize], p.pixmap.data());
                let tex = match old.next() {
                    Some(mut t) => {
                        t.set(img, options);
                        t
                    }
                    None => ctx.load_texture(format!("tilt_drape_{i}"), img, options),
                };
                (p.band, tex)
            })
            .collect();
        let shapes = render::extruded_shapes(map, &view);
        self.drape = Some(Drape { key, ppp: drape_ppp, res: cam.res, tiles, built: time, bands, shapes });
        true
    }

    /// Paints the tilted map in `rect`: the sky, and the drape's bands, farthest first.
    pub fn paint(&self, painter: &Painter, rect: Rect, map: &Map) {
        let (w, h) = (rect.width() as f64, rect.height() as f64);
        let cam = map.camera;
        let (sky, fog) = render::tilt_sky(map);
        painter.rect_filled(rect, 0.0, c32(fog));
        if let Some(y) = cam.sky_y(h) {
            let y = rect.min.y + (y as f32).min(rect.height());
            let mut m = Mesh::default();
            let (top, bottom) = (c32(sky), c32(fog));
            for (p, c) in [(rect.left_top(), top), (rect.right_top(), top), (pos2(rect.max.x, y), bottom), (pos2(rect.min.x, y), bottom)] {
                m.colored_vertex(p, c);
            }
            m.add_triangle(0, 1, 2);
            m.add_triangle(0, 2, 3);
            painter.add(Shape::mesh(m));
        }
        for mesh in self.meshes(rect, &cam, w, h) {
            painter.add(Shape::mesh(mesh));
        }
        if let Some(mesh) = self.extrusions(rect, &cam, map.ground_res_at(&cam), fog) {
            painter.add(Shape::mesh(mesh));
        }
    }

    /// The extruded features standing on the drape in perspective, as one mesh: features far to
    /// near, each one's walls (facing the eye, lit by their facing and darker toward their foot,
    /// far first) then its roof; fading into `fog` with distance. `m_per_px` is metres per
    /// logical px of ground at the centre.
    fn extrusions(&self, rect: Rect, cam: &Camera, m_per_px: f64, fog: rustgis_engine::map::Rgba) -> Option<Mesh> {
        let d = self.drape.as_ref()?;
        let (w, h) = (rect.width() as f64, rect.height() as f64);
        let (eu, ev, _) = cam.eye(h);
        let light = render::EXTRUSION_LIGHT;
        let light_len = light.0.hypot(light.1).max(1e-9);
        let at = |u: f64, v: f64, up: f64| {
            cam.project_raised(u, v, up, w, h)
                .filter(|p| p.0.is_finite() && p.1.is_finite())
                .map(|(x, y)| pos2(rect.min.x + x as f32, rect.min.y + y as f32))
        };
        // Far (up the screen) first; past the view's reach is sky.
        let mut order: Vec<(f64, f64, usize)> = d
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let depth = 1.0 / cam.depth_scale(s.origin, h)?.max(1e-9);
                (depth <= FAR).then(|| (cam.ground_px(s.origin).1, depth, i))
            })
            .collect();
        order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut mesh = Mesh::default();
        for (_, depth, i) in order {
            let Some(s) = d.shapes.get(i) else { continue };
            let up = (s.metres / m_per_px.max(1e-9)).clamp(0.0, 1e7);
            let t = ((depth - FAR * 0.5) / (FAR * 0.5)).clamp(0.0, 1.0);
            let base = s.fill.lerp(fog, t * t * (3.0 - 2.0 * t));
            let mut walls: Vec<(f64, [egui::Pos2; 4], f64)> = Vec::new();
            for ring in &s.rings {
                let g: Vec<(f64, f64)> = ring.iter().map(|c| cam.ground_px(*c)).collect();
                let area2: f64 =
                    (0..g.len()).filter_map(|k| Some((g.get(k)?, g.get((k + 1) % g.len())?))).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum();
                for k in 0..g.len() {
                    let (Some(&a), Some(&b)) = (g.get(k), g.get((k + 1) % g.len())) else { continue };
                    let (du, dv) = (b.0 - a.0, b.1 - a.1);
                    let len = du.hypot(dv);
                    if len <= 0.0 {
                        continue;
                    }
                    let n = if area2 > 0.0 { (dv / len, -du / len) } else { (-dv / len, du / len) };
                    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                    if n.0 * (eu - mid.0) + n.1 * (ev - mid.1) <= 0.0 {
                        continue;
                    }
                    let (Some(fa), Some(fb), Some(tb), Some(ta)) =
                        (at(a.0, a.1, 0.0), at(b.0, b.1, 0.0), at(b.0, b.1, up), at(a.0, a.1, up))
                    else {
                        continue;
                    };
                    walls.push((mid.1, [fa, fb, tb, ta], ((n.0 * light.0 + n.1 * light.1) / light_len).max(0.0)));
                }
            }
            walls.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (_, [fa, fb, tb, ta], lit) in walls {
                let top = base.shade(0.6 + 0.4 * lit);
                let (top, foot) = (c32(top), c32(top.shade(0.72)));
                let k = mesh.vertices.len() as u32;
                for (p, c) in [(fa, foot), (fb, foot), (tb, top), (ta, top)] {
                    mesh.colored_vertex(p, c);
                }
                mesh.add_triangle(k, k + 1, k + 2);
                mesh.add_triangle(k, k + 2, k + 3);
            }
            let roof: Option<Vec<egui::Pos2>> = s
                .roof
                .iter()
                .map(|p| {
                    let (u, v) = cam.ground_px(Coord::new(s.origin.x + f64::from(p[0]), s.origin.y + f64::from(p[1])));
                    at(u, v, up)
                })
                .collect();
            if let Some(roof) = roof {
                let k = mesh.vertices.len() as u32;
                let color = c32(base);
                for p in roof {
                    mesh.colored_vertex(p, color);
                }
                for [a, b, c] in s.roof_indices.as_chunks::<3>().0 {
                    mesh.add_triangle(k + a, k + b, k + c);
                }
            }
        }
        (!mesh.indices.is_empty()).then_some(mesh)
    }

    /// The drape's meshes for `cam` in `rect`, farthest band first. A band with nothing in front
    /// of the camera has no mesh: egui-wgpu can't draw an empty one.
    fn meshes(&self, rect: Rect, cam: &Camera, w: f64, h: f64) -> Vec<Mesh> {
        let Some(d) = &self.drape else { return Vec::new() };
        d.bands
            .iter()
            .rev()
            .filter_map(|(b, tex)| {
                let n = CELLS + 1;
                let mut mesh = Mesh::with_texture(tex.id());
                let mut ok = Vec::with_capacity(n * n);
                for j in 0..n {
                    for i in 0..n {
                        let (u, v) = (i as f64 / CELLS as f64, j as f64 / CELLS as f64);
                        let m = b.camera.to_map(u * b.width, v * b.height, b.width, b.height);
                        // Past the view's reach is sky, as in the settled frame.
                        let in_reach = cam.depth_scale(m, h).is_some_and(|k| k >= 1.0 / FAR);
                        let s = cam.try_to_screen(m, w, h).filter(|s| in_reach && s.0.is_finite() && s.1.is_finite());
                        ok.push(s.is_some());
                        let (x, y) = s.unwrap_or((0.0, 0.0));
                        mesh.vertices.push(Vertex {
                            pos: pos2(rect.min.x + x as f32, rect.min.y + y as f32),
                            uv: pos2(u as f32, v as f32),
                            color: Color32::WHITE,
                        });
                    }
                }
                for j in 0..CELLS {
                    for i in 0..CELLS {
                        let k = j * n + i;
                        let quad = [k, k + 1, k + n, k + n + 1];
                        if quad.iter().all(|&q| ok.get(q).copied().unwrap_or(false)) {
                            let [a, b, c, e] = quad.map(|q| q as u32);
                            mesh.add_triangle(a, b, e);
                            mesh.add_triangle(a, e, c);
                        }
                    }
                }
                (!mesh.indices.is_empty()).then_some(mesh)
            })
            .collect()
    }
}

/// Whether the bands cover the ground a tilted `w × h` view of `cam` shows (sampled on a grid;
/// the sky needs no cover).
fn covers(cam: &Camera, w: f64, h: f64, bands: &[(Band, TextureHandle)]) -> bool {
    let extents: Vec<Extent> = bands.iter().map(|(b, _)| b.camera.extent(b.width, b.height)).collect();
    let n = 6;
    (0..=n).all(|j| {
        (0..=n).all(|i| {
            let (x, y) = (w * f64::from(i) / f64::from(n), h * f64::from(j) / f64::from(n));
            match cam.try_to_map(x, y, w, h) {
                Some(m) => extents.iter().any(|e| e.contains(Coord::new(m.x, m.y))),
                None => true,
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use rustgis_engine::geom::lonlat_to_merc;

    use super::*;

    fn tilted(pitch: f64) -> Map {
        let mut m = Map::new("m");
        m.camera = Camera { center: lonlat_to_merc(Coord::new(10.0, 48.0)), res: 2_000.0, bearing: 20.0, pitch };
        m
    }

    fn fog() -> rustgis_engine::map::Rgba {
        rustgis_engine::map::Rgba::rgb(200, 210, 220)
    }

    #[test]
    fn extrusions_stand_up_on_the_drape() {
        use rustgis_engine::data::{Dataset, Geometry, GeometryType};
        let ctx = egui::Context::default();
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut m = tilted(55.0);
        let mut ds = Dataset::new("blocks", GeometryType::Polygon, vec![]);
        let ring =
            vec![Coord::new(9.0, 47.0), Coord::new(11.0, 47.0), Coord::new(11.0, 49.0), Coord::new(9.0, 49.0), Coord::new(9.0, 47.0)];
        ds.push(Some(Geometry::Polygon(vec![vec![ring]])), vec![]);
        let id = m.add_dataset(ds);
        if let Some(l) = m.layer_mut(id) {
            l.extrusion = Some(rustgis_engine::map::effects::Extrusion {
                field: None,
                height: 100_000.0,
                factor: 1.0,
                face_culling: true,
                lighting: true,
                shadows: false,
            });
        }
        let mut d = TiltDrape::default();
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.0));
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mesh = d.extrusions(rect, &m.camera, m.ground_res_at(&m.camera), fog()).expect("the block stands up");
        assert!(mesh.is_valid() && mesh.indices.len() > 6);
        // Its roof stands above its foot on screen.
        let foot = m.camera.to_screen(rustgis_engine::geom::lonlat_to_merc(Coord::new(10.0, 47.0)), 800.0, 600.0).1 as f32;
        let top = mesh.vertices.iter().map(|v| v.pos.y).fold(f32::MAX, f32::min);
        assert!(top < foot - 20.0, "{top} vs {foot}");
    }

    #[test]
    fn the_drape_covers_the_view_and_is_kept_while_it_does() {
        let ctx = egui::Context::default();
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut m = tilted(60.0);
        let mut d = TiltDrape::default();
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.0));
        let built = d.drape.as_ref().map(|d| d.built);
        assert!(d.drape.as_ref().is_some_and(|d| d.bands.len() >= 2), "near and far bands");
        // A small pan stays inside the margin: nothing is drawn.
        m.camera.pan_px(30.0, 20.0);
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.1));
        assert_eq!(d.drape.as_ref().map(|d| d.built), built);
        // No extruded layer, no extrusion mesh.
        assert!(d.extrusions(Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0)), &m.camera, 2_000.0, fog()).is_none());
        // Every band in front of the camera gives a valid mesh.
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let meshes = d.meshes(rect, &m.camera, 800.0, 600.0);
        assert!(!meshes.is_empty() && meshes.iter().all(|mesh| !mesh.indices.is_empty() && mesh.is_valid()));
        // Zooming in past the drape's detail draws it again.
        m.camera.res /= 3.0;
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.2));
        assert_ne!(d.drape.as_ref().map(|d| d.built), built);
        // So does panning far away.
        let built = d.drape.as_ref().map(|d| d.built);
        m.camera.pan_px(5_000.0, 0.0);
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.3));
        assert_ne!(d.drape.as_ref().map(|d| d.built), built);
    }
}
