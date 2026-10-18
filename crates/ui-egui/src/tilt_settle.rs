//! The settled tilted map's exact frame, drawn off the UI thread. A tilted frame takes a few
//! hundred milliseconds for a large view; drawn on the UI thread it froze the map just as it came
//! to rest. A copy of the map is drawn on a worker instead (the copy shares its features and
//! caches, so it costs pointers), while the drape (see `tilt_drape`) stays on screen until the
//! frame is ready. In the browser there are no threads: the frame is drawn at once, as before.

use rustgis_engine::map::Map;
use rustgis_engine::render::{self, Rendered, View};

/// A finished frame.
pub(crate) struct Done {
    pub rendered: Option<Rendered>,
    /// The view it was drawn for.
    pub view: View,
    /// The copy it was drawn from, holding the caches the drawing filled.
    pub map: Map,
    /// How long it took (ms).
    pub ms: u64,
}

/// The frame being drawn (at most one at a time), and what it is for.
pub(crate) struct TiltSettle<K> {
    #[cfg(not(target_arch = "wasm32"))]
    job: Option<(K, std::sync::mpsc::Receiver<Done>)>,
    #[cfg(target_arch = "wasm32")]
    ready: Option<(K, Done)>,
}

impl<K> Default for TiltSettle<K> {
    fn default() -> Self {
        TiltSettle {
            #[cfg(not(target_arch = "wasm32"))]
            job: None,
            #[cfg(target_arch = "wasm32")]
            ready: None,
        }
    }
}

fn draw(mut map: Map, view: View) -> Done {
    let t0 = web_time::Instant::now();
    let rendered = render::render(&mut map, &view);
    Done { rendered, view, map, ms: t0.elapsed().as_millis() as u64 }
}

impl<K> TiltSettle<K> {
    /// Whether a frame is being drawn.
    pub fn busy(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.job.is_some();
        #[cfg(target_arch = "wasm32")]
        return self.ready.is_some();
    }

    /// Starts drawing `map` (a copy) for `view`, keyed `key`, waking `ctx` when it is done.
    /// Does nothing while a frame is being drawn: the next starts once that one is taken.
    pub fn start(&mut self, ctx: &egui::Context, key: K, map: Map, view: View) {
        if self.busy() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("rustgis-tilt-frame".into()).spawn(move || {
                // A panic in the renderer ends the worker without an answer: `take` lets go of it.
                let _ = tx.send(draw(map, view));
                ctx.request_repaint();
            });
            if spawned.is_ok() {
                self.job = Some((key, rx));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.ready = Some((key, draw(map, view)));
            ctx.request_repaint();
        }
    }

    /// The finished frame and its key, once it is ready.
    pub fn take(&mut self) -> Option<(K, Done)> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (_, rx) = self.job.as_ref()?;
            match rx.try_recv() {
                Ok(done) => self.job.take().map(|(k, _)| (k, done)),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    None
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.ready.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_engine::map::Camera;

    #[test]
    fn draws_a_tilted_frame_and_hands_it_back_once() {
        let ctx = egui::Context::default();
        let mut map = Map::new("m");
        map.camera = Camera { pitch: 60.0, ..Camera::world() };
        let view = View { width: 64.0, height: 48.0, pixels_per_point: 1.0, camera: map.camera, sky: true };
        let mut s = TiltSettle::default();
        s.start(&ctx, 7u8, map.clone(), view);
        assert!(s.busy());
        // A second start while busy is ignored.
        s.start(&ctx, 8u8, map, view);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let done = loop {
            if let Some(d) = s.take() {
                break d;
            }
            assert!(std::time::Instant::now() < end, "the frame never came");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(done.0, 7);
        let r = done.1.rendered.expect("a frame");
        assert_eq!((r.pixmap.width(), r.pixmap.height()), (64, 48));
        assert!(!s.busy());
        assert!(s.take().is_none());
    }
}
