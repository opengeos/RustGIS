//! The moving globe. While the globe turns or zooms, the map drawn flat once (in Web Mercator)
//! is laid over the sphere as a textured mesh, so a frame only moves vertices, as MapLibre's
//! globe does; the atmosphere and lighting are sprites scaled with the disc. The exact frame is
//! drawn again once the globe settles.

use egui::epaint::Vertex;
use egui::{Color32, Mesh, Painter, Rect, Shape, TextureFilter, TextureHandle, TextureOptions, pos2, vec2};
use rustgis_engine::geom::{Coord, Extent, merc_to_lonlat};
use rustgis_engine::map::{Basemap, Globe, Map};
use rustgis_engine::render::{self, GLOBE_GLOW, View};

use crate::theme::c32;

/// How far past the view the drape reaches on each side, as a fraction of its size: the globe
/// can turn this far before the drape is drawn again.
const MARGIN: f64 = 0.25;
/// Zooming in past this factor of the drape's scale draws it again, sharper.
const MAX_ZOOM_IN: f64 = 2.0;
/// Seconds between drawing the drape again for tiles that have arrived meanwhile.
const TILE_REFRESH: f64 = 0.5;
/// Largest sprite radius in pixels; a bigger disc scales it up.
const SPRITE_MAX: f32 = 1024.0;

/// What a drape shows, besides the view: a change draws it again.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DrapeKey {
    /// The session's revision.
    pub rev: u64,
    pub dark: bool,
    /// Streamed layers' reads finished so far.
    pub sources: u64,
}

struct Drape {
    key: DrapeKey,
    /// Pixels per point it was drawn at.
    ppp: f32,
    /// Web Mercator metres per point at the centre when it was drawn.
    res: f64,
    /// The tile cache's generation when it was drawn, and when (seconds).
    tiles: u64,
    built: f64,
    /// Each picture: its longitude/latitude box, its Web Mercator box and its texture.
    patches: Vec<(Extent, Extent, TextureHandle)>,
}

struct Sprites {
    key: (Basemap, bool),
    /// Disc radius in sprite pixels.
    r: f32,
    under: TextureHandle,
    over: TextureHandle,
}

/// The drape and sprites of the moving globe, kept between drags while the map doesn't change.
#[derive(Default)]
pub(crate) struct GlobeDrape {
    drape: Option<Drape>,
    sprites: Option<Sprites>,
}

impl GlobeDrape {
    /// Lets go of the textures (the map is no longer a globe).
    pub fn clear(&mut self) {
        self.drape = None;
        self.sprites = None;
    }

    /// Makes sure the drape covers the `w × h` view, drawing it again when the view has turned
    /// or zoomed past it or the map changed. False when there is nothing to drape.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, ctx: &egui::Context, map: &mut Map, key: DrapeKey, tiles: u64, w: f64, h: f64, ppp: f32, time: f64) -> bool {
        let g = Globe::new(&map.camera, w, h);
        // A screen's worth of detail: at most one point per pixel, so a dense screen keeps the
        // drawing cheap (the settled frame is sharp).
        let drape_ppp = ppp.min(1.0);
        let res = map.camera.res;
        let fresh = self.drape.as_ref().is_some_and(|d| {
            let boxes: Vec<Extent> = d.patches.iter().map(|p| p.0).collect();
            d.key == key
                && d.ppp == drape_ppp
                && res * MAX_ZOOM_IN >= d.res
                && (d.tiles == tiles || time - d.built < TILE_REFRESH)
                && g.drape_covers(w, h, &boxes)
        });
        if !fresh {
            let boxes = g.drape_extents(w, h, MARGIN);
            let max_side = ctx.input(|i| i.max_texture_side).clamp(256, 4096) as u32;
            let view = View { width: w, height: h, pixels_per_point: f64::from(drape_ppp), camera: map.camera, dark: key.dark };
            let pictures = render::render_globe_drape(map, &view, &boxes, max_side);
            if pictures.is_empty() {
                self.drape = None;
                return false;
            }
            let options = TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR };
            let mut old = self.drape.take().map(|d| d.patches).unwrap_or_default().into_iter().map(|p| p.2);
            let patches = pictures
                .into_iter()
                .enumerate()
                .map(|(i, p)| {
                    let size = [p.pixmap.width() as usize, p.pixmap.height() as usize];
                    let img = egui::ColorImage::from_rgba_premultiplied(size, p.pixmap.data());
                    let tex = match old.next() {
                        Some(mut t) => {
                            t.set(img, options);
                            t
                        }
                        None => ctx.load_texture(format!("globe_drape_{i}"), img, options),
                    };
                    let ll = Extent::from_points(
                        merc_to_lonlat(Coord::new(p.merc.min_x, p.merc.min_y)),
                        merc_to_lonlat(Coord::new(p.merc.max_x, p.merc.max_y)),
                    );
                    (ll, p.merc, tex)
                })
                .collect();
            self.drape = Some(Drape { key, ppp: drape_ppp, res, tiles, built: time, patches });
        }
        // The atmosphere and lighting, unless the disc is so big that only its middle shows.
        let r = g.disc_radius() as f32 * ppp;
        if g.disc_radius() < 3.0 * w.hypot(h) {
            let want = r.min(SPRITE_MAX);
            let skey = (map.basemap, key.dark);
            let stale = self.sprites.as_ref().is_none_or(|s| s.key != skey || !(0.75..=1.33).contains(&(want / s.r)));
            if stale && let Some((under, over)) = render::globe_sprites(map.basemap, key.dark, want) {
                let tex = |name: &str, pm: &render::Pixmap| {
                    let img = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
                    ctx.load_texture(name, img, TextureOptions::LINEAR)
                };
                self.sprites = Some(Sprites { key: skey, r: want, under: tex("globe_under", &under), over: tex("globe_over", &over) });
            }
        }
        true
    }

    /// Paints the globe in `rect`: space, the atmosphere and ocean, the draped map and the
    /// lighting.
    pub fn paint(&self, painter: &Painter, rect: Rect, map: &Map, dark: bool) {
        let (w, h) = (rect.width() as f64, rect.height() as f64);
        let g = Globe::new(&map.camera, w, h);
        painter.rect_filled(rect, 0.0, c32(render::globe_space(dark, map.basemap)));
        let (cx, cy) = g.center();
        let centre = pos2(rect.min.x + cx as f32, rect.min.y + cy as f32);
        let disc = g.disc_radius() as f32;
        let sprites = self.sprites.as_ref().filter(|_| g.disc_radius() < 3.0 * w.hypot(h));
        let sprite_rect = Rect::from_center_size(centre, vec2(2.0, 2.0) * disc * GLOBE_GLOW);
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        match sprites {
            Some(s) => {
                painter.image(s.under.id(), sprite_rect, full, Color32::WHITE);
            }
            None => {
                painter.rect_filled(rect, 0.0, c32(map.basemap.palette().0));
            }
        }
        if let Some(d) = &self.drape {
            for (_, merc, tex) in &d.patches {
                let m = g.drape_mesh(merc);
                if !m.pos.iter().flatten().all(|v| v.is_finite()) {
                    continue;
                }
                let mut mesh = Mesh::with_texture(tex.id());
                mesh.vertices = m
                    .pos
                    .iter()
                    .zip(&m.uv)
                    .map(|(p, uv)| Vertex {
                        pos: pos2(rect.min.x + p[0], rect.min.y + p[1]),
                        uv: pos2(uv[0], uv[1]),
                        color: Color32::WHITE,
                    })
                    .collect();
                mesh.indices = m.indices;
                painter.add(Shape::mesh(mesh));
            }
        }
        if let Some(s) = sprites {
            painter.image(s.over.id(), sprite_rect, full, Color32::WHITE);
        }
    }
}
