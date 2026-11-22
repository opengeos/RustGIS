//! What frames drew otherwise than their layers ask (Never silent): a dashed line drawn solid,
//! a fill left off a block diagram's walls, ground with no height taken as 0 under a block.
//!
//! The drawing code has no session to tell, and a frame comes many times a second, so a frame
//! keeps what it approximated on the thread drawing it, one notice per layer and kind (its key),
//! each once however many frames say it again ([`note`]): the first text a kind is noted with
//! stays, with the count it had then. Whoever draws for someone reads them: [`crate::shadows_said`]
//! gives them among its notices, which every export, page and preview the engine or the
//! interface draws goes through; [`crate::render_placed`] among its gaps; and the map view takes
//! what its frames said ([`take_keyed`]) after each frame it shows, saying each kind once a
//! project. A map view's frame drawn on a worker thread hands what it approximated over
//! ([`relay`]) for the map view to take with its own.

use std::cell::{Cell, RefCell};

/// The most notices a thread (or the relay) keeps unread: a thread whose frames nobody reads (a
/// pass drawn and thrown away) never holds more.
const KEPT: usize = 64;

/// One thing approximated: its kind (the layer and what), and what to say of it.
type Entry = (String, String);

thread_local! {
    /// What frames drawn on this thread approximated, each kind once, since [`take`].
    static SAID: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    /// The map layer a frame on this thread is drawing ([`layer`]), by name.
    static LAYER: RefCell<Option<String>> = const { RefCell::new(None) };
    /// How many dashed lines of [`LAYER`] were drawn solid ([`dashes_drawn_solid`]).
    static SOLID_DASHES: Cell<u32> = const { Cell::new(0) };
}

/// What map view frames drawn on worker threads approximated ([`relay`]), each kind once.
static RELAYED: std::sync::Mutex<Vec<Entry>> = std::sync::Mutex::new(Vec::new());

/// Adds `entry` to `to`, unless its kind is there already or `to` holds [`KEPT`].
fn add(to: &mut Vec<Entry>, entry: Entry) {
    if to.len() < KEPT && !to.iter().any(|(k, _)| *k == entry.0) {
        to.push(entry);
    }
}

/// Keeps `text` under kind `key`, unless this thread keeps that kind already or keeps [`KEPT`].
pub(crate) fn note(key: String, text: String) {
    SAID.with(|s| add(&mut s.borrow_mut(), (key, text)));
}

/// What frames drawn on this thread approximated since the last call, each kind once, and forgets
/// it.
pub(crate) fn take() -> Vec<String> {
    take_entries().into_iter().map(|(_, t)| t).collect()
}

fn take_entries() -> Vec<Entry> {
    SAID.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// [`take`] with each notice's kind, and what frames on worker threads handed over ([`relay`]):
/// for the map view, which says each kind once a project.
pub(crate) fn take_keyed() -> Vec<Entry> {
    let mut out = take_entries();
    if let Ok(mut relayed) = RELAYED.lock() {
        for e in relayed.drain(..) {
            add(&mut out, e);
        }
    }
    out
}

/// Hands what frames drawn on this thread approximated over to the map view ([`take_keyed`]):
/// called by a worker thread drawing the map view's frames, which the map view reads on its own.
pub(crate) fn relay() {
    let mine = take_entries();
    if mine.is_empty() {
        return;
    }
    if let Ok(mut relayed) = RELAYED.lock() {
        for e in mine {
            add(&mut relayed, e);
        }
    }
}

/// Puts `kept` (from [`take`]'s kinds, see [`keep`]) back in front of what frames said since:
/// what a caller took to read on its own ([`crate::render_placed`]) leaves the rest for whoever
/// reads after it.
pub(crate) fn restore(kept: Vec<Entry>) {
    let since = take_entries();
    SAID.with(|s| {
        let mut s = s.borrow_mut();
        for e in kept.into_iter().chain(since) {
            add(&mut s, e);
        }
    });
}

/// What frames drawn on this thread approximated so far, with their kinds, taken for [`restore`].
pub(crate) fn keep() -> Vec<Entry> {
    take_entries()
}

/// Starts drawing the map layer `name` (`None`: between layers): what the layer before it
/// approximated (see [`dashes_drawn_solid`]) is noted under its name.
pub(crate) fn layer(name: Option<&str>) {
    let solid = SOLID_DASHES.with(|c| c.replace(0));
    LAYER.with(|l| {
        let mut l = l.borrow_mut();
        if solid > 0
            && let Some(was) = l.as_deref()
        {
            note(format!("{was}\0dashes"), solid_dashes_text(was, solid));
        }
        match name {
            Some(n) if l.as_deref() != Some(n) => *l = Some(n.to_string()),
            Some(_) => {}
            None => *l = None,
        }
    });
}

/// A dashed line of the layer being drawn ([`layer`]) is drawn solid: a round of its pattern is
/// under a pixel, or it would cut the line into too many dashes. Cheap: called for every stroke.
pub(crate) fn dashes_drawn_solid() {
    SOLID_DASHES.with(|c| c.set(c.get().saturating_add(1)));
}

/// What [`dashes_drawn_solid`] says of layer `name`, `n` of whose lines were drawn solid.
fn solid_dashes_text(name: &str, n: u32) -> String {
    let max = crate::MAX_DASHES as u64;
    format!(
        "{name}: {n} dashed line(s) drawn solid, as a round of the dash pattern was under a pixel at the scale drawn, or a line would take more than {},{:03} dashes",
        max / 1000,
        max % 1000
    )
}

/// What a block diagram's walls drew of walls layer `name`: `n` of the polygons along them fill
/// with a gradient or shapeburst, which walls don't draw.
pub(crate) fn walls_left_plain(name: &str, n: usize) {
    note(
        format!("{name}\0walls"),
        format!(
            "{name}: gradient and shapeburst fills aren't drawn on the block diagram's walls, so {n} of its polygons along them show there in their solid fill or the walls' colour"
        ),
    );
}

/// What to say of a block diagram whose `share` (0 to 1) of the ground it read for its base
/// had no height (off the DEM, the DEM's nodata, an elevation tile that failed), taken as 0
/// ([`crate::terrain::Block::without_height_notice`]).
pub(crate) fn ground_without_height_text(share: f64) -> String {
    let pct = share * 100.0;
    let said = if pct < 1.0 { "under 1%".to_string() } else { format!("about {:.0}%", pct.round()) };
    format!(
        "Block diagram: {said} of the ground in it has no height (off the DEM, the DEM's nodata, or elevation tiles that failed), so it is taken as 0 m and the base goes below that"
    )
}

/// Notes [`ground_without_height_text`].
pub(crate) fn ground_without_height(text: String) {
    note("\0block ground".to_string(), text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_kept_once_named_by_its_layer_with_its_first_count_and_taken_once() {
        take();
        // Two lines drawn solid in Roads, none in Rivers: one notice, Roads', counting two.
        layer(Some("Roads"));
        dashes_drawn_solid();
        dashes_drawn_solid();
        layer(Some("Rivers"));
        layer(None);
        // Another frame, another count: the same kind, said once with the first.
        layer(Some("Roads"));
        dashes_drawn_solid();
        layer(None);
        let said = take();
        assert_eq!(said, vec![solid_dashes_text("Roads", 2)]);
        assert!(said[0].contains("2 dashed line(s)") && said[0].contains("100,000"), "{said:?}");
        assert!(take().is_empty(), "taken once");
        // Nothing approximated, nothing said.
        layer(Some("Roads"));
        layer(None);
        assert!(take().is_empty());
    }

    #[test]
    fn a_thread_nobody_reads_keeps_a_bounded_number() {
        take();
        for i in 0..KEPT * 2 {
            walls_left_plain(&format!("Layer {i}"), 1);
        }
        assert_eq!(take().len(), KEPT);
    }

    #[test]
    fn restoring_keeps_what_came_before_and_since() {
        take();
        ground_without_height(ground_without_height_text(0.004));
        let kept = keep();
        walls_left_plain("Geology", 3);
        restore(kept.clone());
        let said = take();
        assert_eq!(said.len(), 2);
        assert!(said.first().is_some_and(|s| s.contains("under 1%")), "{said:?}");
    }

    #[test]
    fn a_workers_frames_are_handed_to_the_map_view() {
        take_keyed();
        std::thread::scope(|s| {
            s.spawn(|| {
                walls_left_plain("Geology", 3);
                relay();
            });
        });
        let said = take_keyed();
        assert!(said.len() == 1 && said[0].0 == "Geology\0walls", "{said:?}");
        assert!(take_keyed().is_empty(), "taken once");
    }
}
