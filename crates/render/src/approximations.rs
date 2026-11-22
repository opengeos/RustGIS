//! What frames drew otherwise than their layers ask (Never silent): a dashed line drawn solid,
//! a fill left off a block diagram's walls, ground with no height taken as 0 under a block.
//!
//! The drawing code has no session to tell, and a frame comes many times a second, so a frame
//! keeps what it approximated on the thread drawing it, one notice per layer and kind, each once
//! however many frames say it again ([`note`]). Whoever draws for someone reads them:
//! [`crate::shadows_said`] gives them among its notices, which every export, page and preview
//! the engine or the interface draws goes through; [`crate::render_placed`] among its gaps; and
//! the map view takes what its own frames said ([`take`]) after each, saying each once a
//! project. The texts name the layer and are the same from frame to frame (no count that a pan
//! or a zoom would change), so whoever says them once says each kind once.

use std::cell::{Cell, RefCell};

/// The most notices a thread keeps unread: a thread whose frames nobody reads (a pass drawn and
/// thrown away) never holds more.
const KEPT: usize = 64;

thread_local! {
    /// What frames drawn on this thread approximated, each once, since [`take`].
    static SAID: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The map layer a frame on this thread is drawing ([`layer`]), by name.
    static LAYER: RefCell<Option<String>> = const { RefCell::new(None) };
    /// A dashed line of [`LAYER`] was drawn solid ([`dashes_drawn_solid`]).
    static SOLID_DASHES: Cell<bool> = const { Cell::new(false) };
}

/// Keeps `text`, unless this thread keeps it already or keeps [`KEPT`].
pub(crate) fn note(text: String) {
    SAID.with(|s| {
        let mut s = s.borrow_mut();
        if s.len() < KEPT && !s.contains(&text) {
            s.push(text);
        }
    });
}

/// What frames drawn on this thread approximated since the last call, each once, and forgets it.
pub fn take() -> Vec<String> {
    SAID.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// Puts `kept` back in front of what frames said since: what a caller took to read on its own
/// ([`crate::render_placed`]) leaves the rest for whoever reads after it.
pub(crate) fn restore(kept: Vec<String>) {
    let since = take();
    for t in kept.into_iter().chain(since) {
        note(t);
    }
}

/// Starts drawing the map layer `name` (`None`: between layers): what the layer before it
/// approximated (see [`dashes_drawn_solid`]) is noted under its name.
pub(crate) fn layer(name: Option<&str>) {
    let solid = SOLID_DASHES.with(|c| c.replace(false));
    LAYER.with(|l| {
        let mut l = l.borrow_mut();
        if solid && let Some(was) = l.as_deref() {
            note(solid_dashes_text(was));
        }
        *l = name.map(str::to_string);
    });
}

/// A dashed line of the layer being drawn ([`layer`]) is drawn solid: a round of its pattern is
/// under a pixel, or it would cut the line into too many dashes. Cheap: called for every stroke.
pub(crate) fn dashes_drawn_solid() {
    SOLID_DASHES.with(|c| c.set(true));
}

/// What [`dashes_drawn_solid`] says of layer `name`.
fn solid_dashes_text(name: &str) -> String {
    format!(
        "{name}: dashed lines are drawn solid at this scale, as a round of the dash pattern is under a pixel or the line would take more than {} dashes",
        crate::MAX_DASHES as u64
    )
}

/// What a block diagram's walls drew of walls layer `name`: `n` of the polygons along them fill
/// with a gradient or shapeburst, which walls don't draw.
pub(crate) fn walls_left_plain(name: &str, n: usize) {
    note(format!(
        "{name}: gradient and shapeburst fills aren't drawn on the block diagram's walls, so {n} of its polygons along them show there in their solid fill or the walls' colour"
    ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_kept_once_named_by_its_layer_and_taken_once() {
        take();
        // Dashes drawn solid twice in Roads, never in Rivers: one notice, Roads'.
        layer(Some("Roads"));
        dashes_drawn_solid();
        dashes_drawn_solid();
        layer(Some("Rivers"));
        layer(None);
        layer(Some("Roads"));
        dashes_drawn_solid();
        layer(None);
        let said = take();
        assert_eq!(said, vec![solid_dashes_text("Roads")]);
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
        note(ground_without_height_text(0.004));
        let kept = take();
        walls_left_plain("Geology", 3);
        restore(kept.clone());
        let said = take();
        assert_eq!(said.len(), 2);
        assert_eq!(said.first(), kept.first());
        assert!(said.first().is_some_and(|s| s.contains("under 1%")), "{said:?}");
    }
}
