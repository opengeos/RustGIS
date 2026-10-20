//! The moving globe. While the globe turns or zooms, the map drawn flat once (in Web Mercator)
//! is laid over the sphere as a textured mesh, so a frame only moves vertices, as MapLibre's
//! globe does; the atmosphere and lighting are sprites scaled with the disc. The exact frame is
//! drawn again once the globe settles. Space around it is starry (`render::starfield`).

use egui::epaint::Vertex;
use egui::{Color32, Mesh, Painter, Rect, Shape, TextureFilter, TextureHandle, TextureOptions, TextureWrapMode, pos2, vec2};
use rustgis_engine::geom::{Coord, Extent, extent_to_merc, merc_to_lonlat};
use rustgis_engine::map::{Globe, Map, Rgba};
use rustgis_engine::render::{self, GLOBE_GLOW, STAR_TILE, View};

use crate::theme::c32;

/// How far past the view the drape reaches on each side, as a fraction of its size: the globe
/// can turn this far before the drape is drawn again.
const MARGIN: f64 = 0.25;
/// Zooming out past the drape draws it this much coarser and with this margin instead, so it
/// keeps covering the view while the zoom goes on, for about the same work.
const ZOOM_OUT_COARSER: f64 = 2.0;
const ZOOM_OUT_MARGIN: f64 = 1.0;
/// Zooming in past this factor of the drape's scale draws it again, sharper.
const MAX_ZOOM_IN: f64 = 2.0;
/// Seconds between drawing the drape again for tiles that have arrived meanwhile.
const TILE_REFRESH: f64 = 0.5;
/// The sprites' disc radius in pixels: drawn once at this size and scaled with the disc (they
/// are smooth gradients, so scaling doesn't show).
const SPRITE_R: f32 = 512.0;

/// The view a drape of `boxes` is drawn with: the camera's scale, at up to `ppp` pixels per
/// point and no finer than `detail` (metres per point at `ppp`).
fn drape_view(map: &Map, w: f64, h: f64, ppp: f32, detail: f64, boxes: &[Extent]) -> View {
    // A whole-world drape can otherwise grow to tens of millions of pixels as the camera
    // zooms in, despite showing only a screen's worth of detail. Bound the combined area,
    // including both antimeridian patches, as well as each side. Its margins and Web
    // Mercator's stretched high latitudes take a few screens' worth of pixels to show the view
    // near screen density; more sharpens it little and stalls the frames that draw it.
    let area: f64 = boxes.iter().map(extent_to_merc).map(|b| b.width() * b.height()).sum();
    let p = f64::from(ppp);
    let pixels = (2.0 * w * h * p * p).clamp(131_072.0, 2_097_152.0);
    let sample_res = detail.max((area / pixels).sqrt() * p);
    // Coarsen the pixels, never the camera's scale: symbols and strokes are sized in points,
    // so drawing at a coarser scale would bake them in larger than they are.
    View { width: w, height: h, pixels_per_point: p * map.camera.res / sample_res, camera: map.camera, sky: true }
}

/// What a drape shows, besides the view: a change draws it again.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DrapeKey {
    /// The session's revision.
    pub rev: u64,
    /// Streamed layers' reads finished so far.
    pub sources: u64,
}

struct Drape {
    key: DrapeKey,
    /// Pixels per point it was drawn at.
    ppp: f32,
    /// Its detail, in Web Mercator metres per point.
    res: f64,
    /// The camera's resolution when it was drawn.
    cam_res: f64,
    /// The tile cache's generation when it was drawn, and when (seconds).
    tiles: u64,
    built: f64,
    /// Tiles were still on their way when it was drawn: it is drawn again as they arrive.
    pending: bool,
    /// Each picture: its longitude/latitude box, its Web Mercator box and its texture.
    patches: Vec<(Extent, Extent, TextureHandle)>,
}

/// What a drape is drawn for (see [`Drape`]), known before its pictures are.
struct Meta {
    key: DrapeKey,
    ppp: f32,
    res: f64,
    cam_res: f64,
    tiles: u64,
    built: f64,
}

/// A drape's pictures drawn on a worker, with the copy of the map they were drawn from.
#[cfg(not(target_arch = "wasm32"))]
type Job = (Meta, std::sync::mpsc::Receiver<(Vec<render::DrapePatch>, Map)>);

struct Sprites {
    key: (Rgba, bool),
    under: TextureHandle,
    over: TextureHandle,
}

/// The drape and sprites of the moving globe, kept between drags while the map doesn't change.
#[derive(Default)]
pub(crate) struct GlobeDrape {
    drape: Option<Drape>,
    sprites: Option<Sprites>,
    /// The starfield tile and the pixels per point it was drawn at.
    stars: Option<(f32, TextureHandle)>,
    /// The drape being drawn again on a worker.
    #[cfg(not(target_arch = "wasm32"))]
    job: Option<Job>,
}

impl GlobeDrape {
    /// Lets go of the textures (the map is no longer a globe).
    pub fn clear(&mut self) {
        self.drape = None;
        self.sprites = None;
        self.stars = None;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.job = None;
        }
    }

    /// Makes sure the drape covers the `w × h` view, drawing it again when the view has turned
    /// or zoomed past it or the map changed. False when there is nothing to drape.
    ///
    /// Natively, a drape is drawn again on a worker while the one drawn before stays on the
    /// globe (each picture is laid over its own box, so an old one still shows its places where
    /// they are now, if blurrier or short of the view's edge): drawn on the UI thread, every
    /// redraw stalled the moving globe for tens of milliseconds. The first drape, and every one
    /// in the browser (no threads), is drawn at once.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, ctx: &egui::Context, map: &mut Map, key: DrapeKey, tiles: u64, w: f64, h: f64, ppp: f32, time: f64) -> bool {
        self.finish(ctx, map);
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
                // Tiles arriving matter only to a drape that was waiting for some.
                && (!d.pending || d.tiles == tiles || time - d.built < TILE_REFRESH)
                && g.drape_covers(w, h, &boxes)
        });
        if fresh || self.busy() {
            return self.drape.is_some();
        }
        // Zooming out: coarser and wider, so the next steps out are covered too.
        let zooming_out = self.drape.as_ref().is_some_and(|d| res > d.cam_res * 1.01);
        let (detail, margin) = if zooming_out { (res * ZOOM_OUT_COARSER, ZOOM_OUT_MARGIN) } else { (res, MARGIN) };
        let boxes = g.drape_extents(w, h, margin);
        let max_side = ctx.input(|i| i.max_texture_side).clamp(256, 4096) as u32;
        let view = drape_view(map, w, h, drape_ppp, detail, &boxes);
        let meta = Meta { key, ppp: drape_ppp, res: detail, cam_res: res, tiles, built: time };
        #[cfg(not(target_arch = "wasm32"))]
        if self.drape.is_some() {
            let (tx, rx) = std::sync::mpsc::channel();
            let (mut copy, ctx2, boxes) = (map.clone(), ctx.clone(), boxes.clone());
            let work = std::panic::AssertUnwindSafe(move || {
                let pictures = render::render_globe_drape(&mut copy, &view, &boxes, max_side);
                (pictures, copy)
            });
            let spawned = std::thread::Builder::new().name("rustgis-globe-drape".into()).spawn(move || {
                // A panic sends nothing: `finish` finds the channel closed and keeps the old drape.
                if let Ok(done) = std::panic::catch_unwind(work) {
                    let _ = tx.send(done);
                }
                ctx2.request_repaint();
            });
            if spawned.is_ok() {
                self.job = Some((meta, rx));
                return true;
            }
        }
        let pictures = render::render_globe_drape(map, &view, &boxes, max_side);
        self.install(ctx, meta, pictures)
    }

    /// Whether a drape is being drawn on a worker.
    fn busy(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.job.is_some();
        #[cfg(target_arch = "wasm32")]
        return false;
    }

    /// Puts the drape the worker finished on the globe, if it has, keeping the caches its copy
    /// of `map` filled.
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    fn finish(&mut self, ctx: &egui::Context, map: &mut Map) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some((_, rx)) = &self.job else { return };
            match rx.try_recv() {
                Ok((pictures, copy)) => {
                    map.adopt_caches(&copy);
                    if let Some((meta, _)) = self.job.take() {
                        self.install(ctx, meta, pictures);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                // The worker ended without a drape: the old one stays, and the next frame tries again.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.job = None,
            }
        }
    }

    /// Uploads `pictures` as the drape; false (and no drape) when there are none.
    fn install(&mut self, ctx: &egui::Context, meta: Meta, pictures: Vec<render::DrapePatch>) -> bool {
        if pictures.is_empty() {
            self.drape = None;
            return false;
        }
        let pending = pictures.iter().any(|p| p.pending);
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
        let Meta { key, ppp, res, cam_res, tiles, built } = meta;
        self.drape = Some(Drape { key, ppp, res, cam_res, tiles, built, pending, patches });
        true
    }

    /// Makes sure the atmosphere and lighting are drawn, once for the background and the body
    /// (no atmosphere off the Earth).
    fn prepare_sprites(&mut self, ctx: &egui::Context, map: &Map) {
        let skey = (map.palette().0, map.body().is_earth());
        if self.sprites.as_ref().is_none_or(|s| s.key != skey)
            && let Some((under, over)) = render::globe_sprites(skey.0, skey.1, SPRITE_R)
        {
            let tex = |name: &str, pm: &render::Pixmap| {
                let img = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
                ctx.load_texture(name, img, TextureOptions::LINEAR)
            };
            self.sprites = Some(Sprites { key: skey, under: tex("globe_under", &under), over: tex("globe_over", &over) });
        }
    }

    /// Paints the space around the globe in `rect`, with the stars of the map's view effects
    /// where the settled frame draws them (laid edge to edge from the top left corner), at `ppp`
    /// pixels per point.
    pub fn paint_space(&mut self, ctx: &egui::Context, painter: &Painter, rect: Rect, map: &Map, ppp: f32) {
        painter.rect_filled(rect, 0.0, c32(render::globe_space(true)));
        if !map.view.effects.stars {
            return;
        }
        if self.stars.as_ref().is_none_or(|s| s.0 != ppp)
            && let Some(pm) = render::starfield(ppp)
        {
            let img = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
            let options = TextureOptions { wrap_mode: TextureWrapMode::Repeat, ..TextureOptions::NEAREST };
            self.stars = Some((ppp, ctx.load_texture("globe_stars", img, options)));
        }
        if let Some((_, tex)) = &self.stars {
            // One rectangle, its texture repeated across it.
            let uv = Rect::from_min_size(pos2(0.0, 0.0), rect.size() / STAR_TILE);
            painter.image(tex.id(), rect, uv, Color32::WHITE);
        }
    }

    /// Paints the globe in `rect`: space, the atmosphere and ocean, the draped map and the
    /// lighting.
    pub fn paint(&mut self, ctx: &egui::Context, painter: &Painter, rect: Rect, map: &Map, ppp: f32) {
        let (w, h) = (rect.width() as f64, rect.height() as f64);
        let g = Globe::new(&map.camera, w, h);
        self.paint_space(ctx, painter, rect, map, ppp);
        let (cx, cy) = g.center();
        let centre = pos2(rect.min.x + cx as f32, rect.min.y + cy as f32);
        let disc = g.disc_radius() as f32;
        self.prepare_sprites(ctx, map);
        let sprites = self.sprites.as_ref().filter(|_| g.disc_radius() < 3.0 * w.hypot(h));
        // Stretched up and down to a tilted globe's ellipse.
        let sprite_rect = Rect::from_center_size(centre, vec2(2.0 * disc, 2.0 * g.disc_height() as f32) * GLOBE_GLOW);
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        match sprites {
            Some(s) => {
                painter.image(s.under.id(), sprite_rect, full, Color32::WHITE);
            }
            None => {
                painter.rect_filled(rect, 0.0, c32(map.palette().0));
            }
        }
        for mesh in self.meshes(rect, &g) {
            painter.add(Shape::mesh(mesh));
        }
        if let Some(s) = sprites {
            painter.image(s.over.id(), sprite_rect, full, Color32::WHITE);
        }
    }

    /// The drape's meshes for the globe `g` in `rect`. A picture wholly behind the horizon has
    /// no triangles and is left out: the GPU can't draw an empty mesh (egui-wgpu panics on an
    /// empty buffer).
    fn meshes(&self, rect: Rect, g: &Globe) -> Vec<Mesh> {
        let Some(d) = &self.drape else { return Vec::new() };
        d.patches
            .iter()
            .filter_map(|(_, merc, tex)| {
                let m = g.drape_mesh(merc);
                if m.indices.is_empty() || !m.pos.iter().flatten().all(|v| v.is_finite()) {
                    return None;
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
                Some(mesh)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rustgis_engine::geom::lonlat_to_merc;
    use rustgis_engine::map::{Camera, Projection};

    use super::*;

    fn globe(lon: f64, res: f64) -> Map {
        let mut m = Map::new("m");
        m.projection = Projection::Globe;
        m.camera = Camera { center: lonlat_to_merc(Coord::new(lon, 0.0)), res, bearing: 0.0, pitch: 0.0 };
        m
    }

    /// Prepares the drape and waits for a redraw on the worker to be put on the globe.
    fn prepare_now(d: &mut GlobeDrape, ctx: &egui::Context, m: &mut Map, key: DrapeKey, tiles: u64, (w, h): (f64, f64), time: f64) -> bool {
        let shown = d.prepare(ctx, m, key, tiles, w, h, 1.0, time);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while d.busy() {
            assert!(std::time::Instant::now() < end, "the drape never came");
            std::thread::sleep(std::time::Duration::from_millis(2));
            d.finish(ctx, m);
        }
        shown
    }

    #[test]
    fn a_picture_behind_the_horizon_gives_no_empty_mesh() {
        // Zoomed out, facing 70°E: the drape's margin runs past the antimeridian, so its second
        // picture (180°–170°W) is all on the far side. Its empty mesh crashed egui-wgpu.
        let ctx = egui::Context::default();
        let mut m = globe(70.0, 20_000.0);
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut d = GlobeDrape::default();
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.0));
        assert_eq!(d.drape.as_ref().map(|d| d.patches.len()), Some(2));
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let meshes = d.meshes(rect, &Globe::new(&m.camera, 800.0, 600.0));
        assert_eq!(meshes.len(), 1);
        assert!(meshes.iter().all(|mesh| !mesh.indices.is_empty() && mesh.is_valid()));
    }

    #[test]
    fn zooming_out_draws_a_wider_drape_and_keeps_it() {
        let ctx = egui::Context::default();
        let mut m = globe(10.0, 500.0);
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut d = GlobeDrape::default();
        assert!(prepare_now(&mut d, &ctx, &mut m, key, 0, (800.0, 600.0), 0.0));
        // Out past the margin: drawn again, coarser and wider ...
        m.camera.res = 900.0;
        assert!(prepare_now(&mut d, &ctx, &mut m, key, 0, (800.0, 600.0), 0.1));
        assert_eq!(d.drape.as_ref().map(|d| d.res), Some(1800.0));
        // ... so a further step out is still covered, and nothing is drawn.
        m.camera.res = 1_300.0;
        let built = d.drape.as_ref().map(|d| d.built);
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.2));
        assert!(!d.busy());
        assert_eq!(d.drape.as_ref().map(|d| d.built), built);
    }

    #[test]
    fn a_drape_keeps_the_camera_scale() {
        // Zoomed out over the whole globe, and zooming out further: the drape is coarse, but
        // drawn at the camera's scale, or its point symbols would grow on the moving globe.
        let m = globe(15.0, 10_000.0);
        let (w, h) = (1100.0, 800.0);
        let g = Globe::new(&m.camera, w, h);
        for (detail, margin) in [(10_000.0, MARGIN), (10_000.0 * ZOOM_OUT_COARSER, ZOOM_OUT_MARGIN)] {
            let boxes = g.drape_extents(w, h, margin);
            let view = drape_view(&m, w, h, 1.0, detail, &boxes);
            assert_eq!(view.camera, m.camera);
            assert!(view.pixels_per_point < 1.0, "a whole-globe drape is coarser than the screen");
            let pixels: f64 = boxes.iter().map(extent_to_merc).map(|b| b.width() * b.height()).sum::<f64>()
                * (view.pixels_per_point / m.camera.res).powi(2);
            assert!(pixels <= 2_097_152.0 * 1.001, "{pixels} pixels");
        }
    }

    #[test]
    fn navigating_a_planet_reuses_the_drape() {
        for lat in [0.0, 25.0, 70.0] {
            let ctx = egui::Context::default();
            let mut m = globe(15.0, 20_000.0);
            m.body = "moon".into();
            m.camera.center = lonlat_to_merc(Coord::new(15.0, lat));
            let key = DrapeKey { rev: 0, sources: 0 };
            let mut d = GlobeDrape::default();
            assert!(prepare_now(&mut d, &ctx, &mut m, key, 0, (1100.0, 800.0), 0.0));
            let mut rebuilt = 0;
            for i in 1..=60 {
                m.camera.center = lonlat_to_merc(Coord::new(15.0 + i as f64 * 0.1, lat));
                m.camera.res = 20_000.0 * (1.0 - i as f64 * 0.001);
                let built = d.drape.as_ref().map(|d| d.built);
                assert!(prepare_now(&mut d, &ctx, &mut m, key, 0, (1100.0, 800.0), i as f64 / 60.0));
                let pixels: usize =
                    d.drape.as_ref().into_iter().flat_map(|d| &d.patches).map(|p| p.2.size().iter().product::<usize>()).sum();
                assert!(pixels <= 2_097_152, "navigation textures grew to {pixels} pixels");
                rebuilt += usize::from(d.drape.as_ref().map(|d| d.built) != built);
                let g = Globe::new(&m.camera, 1100.0, 800.0);
                let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 800.0));
                assert!(!d.meshes(rect, &g).is_empty());
            }
            assert!(rebuilt <= 2, "a small pan/zoom rebuilt the drape {rebuilt} times at {lat}°");
        }
    }

    #[test]
    fn tiles_arriving_elsewhere_keep_a_finished_drape() {
        // Fetches finishing for tiles a drape didn't wait for drew it again every half second
        // while the globe moved, stalling it each time.
        let ctx = egui::Context::default();
        let mut m = globe(10.0, 5_000.0);
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut d = GlobeDrape::default();
        assert!(prepare_now(&mut d, &ctx, &mut m, key, 1, (800.0, 600.0), 0.0));
        assert!(d.drape.as_ref().is_some_and(|d| !d.pending), "a map without tiles has none to wait for");
        let built = d.drape.as_ref().map(|d| d.built);
        for i in 2..10 {
            assert!(d.prepare(&ctx, &mut m, key, i, 800.0, 600.0, 1.0, i as f64));
            assert!(!d.busy(), "drawn again for tiles it didn't wait for");
        }
        assert_eq!(d.drape.as_ref().map(|d| d.built), built);
        // One drawn while tiles were on their way is drawn again as they arrive.
        if let Some(dr) = d.drape.as_mut() {
            dr.pending = true;
        }
        assert!(prepare_now(&mut d, &ctx, &mut m, key, 11, (800.0, 600.0), 20.0));
        assert_ne!(d.drape.as_ref().map(|d| d.built), built);
    }

    #[test]
    fn a_redraw_keeps_the_old_drape_on_the_globe_meanwhile() {
        let ctx = egui::Context::default();
        let mut m = globe(10.0, 5_000.0);
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut d = GlobeDrape::default();
        // The first is drawn at once: there is nothing else to show.
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.0));
        assert!(d.drape.is_some() && !d.busy());
        // Turned past it: drawn again on a worker, the old one still shown.
        m.camera.center = lonlat_to_merc(Coord::new(80.0, 0.0));
        let built = d.drape.as_ref().map(|d| d.built);
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 1.0));
        assert!(d.busy());
        assert!(d.drape.is_some());
        assert_eq!(d.drape.as_ref().map(|d| d.built), built);
        // Once done, the new one replaces it and covers the view.
        assert!(prepare_now(&mut d, &ctx, &mut m, key, 0, (800.0, 600.0), 1.1));
        assert_eq!(d.drape.as_ref().map(|d| d.built), Some(1.0));
        let boxes: Vec<Extent> = d.drape.iter().flat_map(|d| &d.patches).map(|p| p.0).collect();
        assert!(Globe::new(&m.camera, 800.0, 600.0).drape_covers(800.0, 600.0, &boxes));
    }

    #[test]
    fn the_sprites_are_drawn_once() {
        let ctx = egui::Context::default();
        let m = globe(10.0, 5_000.0);
        let mut d = GlobeDrape::default();
        d.prepare_sprites(&ctx, &m);
        let id = d.sprites.as_ref().map(|s| s.under.id());
        d.prepare_sprites(&ctx, &m);
        assert_eq!(d.sprites.as_ref().map(|s| s.under.id()), id);
        assert!(id.is_some());
    }
}
