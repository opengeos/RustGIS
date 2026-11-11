//! Stopping a frame no longer wanted (#1750). The desktop app draws a settled flat map's frame on a
//! worker; when the map moves again before it is done, the frame is out of date. The worker
//! installs a flag on its thread ([`install`]) and sets it to stop: the renderer looks at it between
//! layers and every [`EVERY`] features of a layer, and a frame stopped that way comes back as
//! `None`. With none installed (an export, the CLI, the browser) frames always finish.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How many features of a layer are drawn between looks at the flag.
pub(crate) const EVERY: usize = 2048;

thread_local! {
    static FLAG: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Makes `flag` stop the frames drawn on this thread until the guard is dropped, which puts back
/// the one before.
pub fn install(flag: Arc<AtomicBool>) -> Installed {
    Installed(FLAG.with(|f| f.replace(Some(flag))))
}

/// Puts back the flag before when dropped (see [`install`]).
pub struct Installed(Option<Arc<AtomicBool>>);

impl Drop for Installed {
    fn drop(&mut self) {
        let before = self.0.take();
        let _ = FLAG.try_with(|f| f.replace(before));
    }
}

/// Whether the frame being drawn on this thread was asked to stop.
pub(crate) fn requested() -> bool {
    FLAG.with(|f| f.borrow().as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_stops_frames_on_its_thread_only_while_installed() {
        let flag = Arc::new(AtomicBool::new(false));
        assert!(!requested());
        {
            let _guard = install(flag.clone());
            assert!(!requested());
            flag.store(true, Ordering::Relaxed);
            assert!(requested());
            // Another thread draws on.
            assert!(!std::thread::spawn(requested).join().unwrap_or(true));
        }
        assert!(!requested());
    }

    #[test]
    fn a_stopped_frame_is_no_frame() {
        let mut map = rustgis_map::Map::new("m");
        if let Some(d) = crate::world_dataset("Countries", crate::sample_data::COUNTRIES) {
            map.add_dataset(d);
        }
        let view = crate::View { width: 64.0, height: 48.0, pixels_per_point: 1.0, camera: rustgis_map::Camera::world(), sky: false };
        let flag = Arc::new(AtomicBool::new(false));
        let _guard = install(flag.clone());
        assert!(crate::render(&mut map, &view).is_some());
        flag.store(true, Ordering::Relaxed);
        assert!(crate::render(&mut map, &view).is_none());
    }
}
