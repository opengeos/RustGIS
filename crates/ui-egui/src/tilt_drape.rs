//! The moving tilted map. While a tilted map pans, zooms, turns or tilts, its ground drawn flat
//! once in bands (`render::render_tilt_drape`) is laid over the tilted ground as textured meshes,
//! so a frame only moves vertices, as the globe's drape does; the sky above the horizon is a
//! gradient. The exact frame is drawn again once the map settles.

use egui::epaint::Vertex;
use egui::{Color32, Mesh, Painter, Rect, Shape, TextureFilter, TextureHandle, TextureOptions, pos2};
use rustgis_engine::geom::{Coord, Extent};
use rustgis_engine::map::camera::Band;
use rustgis_engine::map::{Camera, Map};
use rustgis_engine::render::{self, View};

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
        self.drape = Some(Drape { key, ppp: drape_ppp, res: cam.res, tiles, built: time, bands });
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
                        let s = cam.try_to_screen(m, w, h).filter(|s| s.0.is_finite() && s.1.is_finite());
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
