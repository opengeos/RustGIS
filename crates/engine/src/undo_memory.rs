//! Budget for feature allocations retained by undo and redo snapshots.
use crate::{Project, Step, notice_text::NoticeText};
use rustgis_data::{Coord, Feature, Features, Geometry, Value};
use std::{
    collections::{HashMap, HashSet},
    mem::size_of,
    sync::Weak,
};

type Key = Weak<Vec<Feature>>;

/// The allocation identities in a snapshot. The project keeps the corresponding rows alive.
pub(crate) type Allocations = Vec<Key>;

pub(crate) struct Budget {
    pub(crate) limit: usize,
    // Command dispatch also runs post-command maintenance, which can record a second step.
    pub(crate) deferred: bool,
    // Dropping steps is said once a project: past the budget every further edit drops one, and
    // a message each time would only repeat the first. Another project says it again.
    said: bool,
    /// The computer's memory the check before an edit plans with ([`rustgis_data::memory::physical`];
    /// tests pass fixed amounts). `None`: not known, as in the browser, and nothing is checked.
    pub(crate) memory: Option<u64>,
    // Dropping steps to make room for an edit is said once a project, as `said` is.
    room_said: bool,
    // Keeping the weak key prevents address reuse while its estimate is cached, without keeping
    // the rows themselves alive. Copy-on-write gives changed rows a different key.
    estimates: HashMap<usize, (Key, usize)>,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            limit: rustgis_data::memory::undo_history(),
            deferred: false,
            said: false,
            memory: rustgis_data::memory::physical(),
            room_said: false,
            estimates: HashMap::new(),
        }
    }
}

impl Budget {
    pub(crate) fn clear(&mut self) {
        self.estimates.clear();
        self.said = false;
        self.room_said = false;
    }

    /// Keep a contiguous history around the current state. Count a shared allocation only at its
    /// highest-priority owner, so the estimate falls only once its last snapshot is evicted.
    pub(crate) fn trim(&mut self, live: &Project, undo: &mut Vec<Step>, redo: &mut Vec<Step>) -> Option<NoticeText> {
        let mut seen: HashSet<_> = rows(live).map(|features| features.allocation_key().as_ptr() as usize).collect();
        let mut used = 0usize;
        let undo_protected = usize::from(!undo.is_empty());
        let mut account = |step: &Step| {
            used = used.saturating_add(self.private_bytes(&step.8, &mut seen));
            used <= self.limit
        };

        // Work in reverse eviction order. Keep the newest undo even if it alone exceeds the
        // limit, then the nearest redos, then as many older undos as will fit.
        if let Some(newest) = undo.last() {
            account(newest);
        }
        let redo_keep = redo.iter().rev().take_while(|step| account(step)).count();
        let undo_keep = if redo_keep == redo.len() {
            undo_protected + undo.iter().rev().skip(undo_protected).take_while(|step| account(step)).count()
        } else {
            undo_protected
        };

        let undo_drop = undo.len() - undo_keep;
        let redo_drop = redo.len() - redo_keep;
        undo.drain(..undo_drop);
        redo.drain(..redo_drop);
        self.estimates.retain(|id, (key, _)| seen.contains(id) && key.strong_count() > 0);
        if undo_drop + redo_drop == 0 || std::mem::replace(&mut self.said, true) {
            return None;
        }
        let template = "Undo history is limited by memory: older steps are dropped as you edit, keeping Undo and Redo to about {limit}. The newest step is always kept.";
        Some(NoticeText::format(template, [("limit", size_said(self.limit as u64).into())]))
    }

    /// What the session holds (#2431 fix 3): the rows of the open project and those only Undo and
    /// Redo keep, each allocation counted once however many snapshots share it, and the pixels
    /// and embedded files of their rasters likewise. Rows are estimated as for the budget, from a
    /// cache, so this visits allocations, not rows.
    pub(crate) fn held(&mut self, live: &Project, undo: &[Step], redo: &[Step]) -> u64 {
        let mut seen = HashSet::new();
        let mut bytes = self.private_bytes(&allocations(live), &mut seen);
        for step in undo.iter().chain(redo) {
            bytes = bytes.saturating_add(self.private_bytes(&step.8, &mut seen));
        }
        let projects = std::iter::once(live).chain(undo.iter().chain(redo).map(|step| &step.0));
        (bytes as u64).saturating_add(raster_bytes(projects))
    }

    /// Makes room for an edit that copies `copy` bytes of rows (#2431 fix 3): when what the session
    /// holds and the copy pass the memory it plans to use ([`rustgis_data::memory::usable`]),
    /// history goes until they fit: Redo first, farthest first, as the edit clears it anyway, then
    /// Undo, oldest first. The history is left as it is, and `Err` gives what is held and the memory
    /// planned for, when the copy wouldn't fit even with the history gone.
    pub(crate) fn make_room(
        &mut self,
        live: &Project,
        undo: &mut Vec<Step>,
        redo: &mut Vec<Step>,
        copy: u64,
    ) -> Result<Option<usize>, (u64, u64)> {
        let Some(usable) = rustgis_data::memory::usable(self.memory) else { return Ok(None) };
        let fits = |held: u64| held.saturating_add(copy) <= usable;
        if fits(self.held(live, undo, redo)) {
            return Ok(None);
        }
        let open = self.held(live, &[], &[]);
        if !fits(open) {
            return Err((open, usable));
        }
        let mut dropped = 0;
        while !fits(self.held(live, undo, redo)) {
            if !redo.is_empty() {
                redo.remove(0);
            } else if undo.is_empty() {
                break;
            } else {
                undo.remove(0);
            }
            dropped += 1;
        }
        Ok((dropped > 0 && !std::mem::replace(&mut self.room_said, true)).then_some(dropped))
    }

    /// The estimated size of `features`' rows: what the first write to them copies while a
    /// snapshot shares them.
    pub(crate) fn rows_bytes(&mut self, features: &Features) -> u64 {
        self.private_bytes(&vec![features.allocation_key()], &mut HashSet::new()) as u64
    }

    /// Measure only history-owned allocations with no cached estimate. The other work visits
    /// cached allocation identities, independent of the number of rows.
    fn private_bytes(&mut self, allocations: &Allocations, seen: &mut HashSet<usize>) -> usize {
        allocations.iter().fold(0, |sum, key| {
            let id = key.as_ptr() as usize;
            if !seen.insert(id) {
                return sum;
            }
            let (_, bytes) = self.estimates.entry(id).or_insert_with(|| {
                let bytes = key.upgrade().as_deref().map_or(0, estimate);
                (key.clone(), bytes)
            });
            sum.saturating_add(*bytes)
        })
    }
}

/// The pixels and embedded files of the rasters in `projects`, each shared allocation once.
fn raster_bytes<'a>(projects: impl Iterator<Item = &'a Project>) -> u64 {
    let mut seen = HashSet::new();
    let mut bytes = 0u64;
    let rasters = projects
        .flat_map(|project| project.maps.iter().chain(project.offline_areas.iter().map(|area| &area.data)))
        .flat_map(|map| map.layers.iter())
        .filter_map(|layer| layer.raster.as_ref());
    for raster in rasters {
        if let Some(data) = raster.data.as_ref().filter(|data| seen.insert(std::sync::Arc::as_ptr(data) as usize)) {
            let pixels = data.bands.iter().fold(0usize, |n, band| n.saturating_add(band.data.capacity().saturating_mul(size_of::<f32>())));
            bytes = bytes.saturating_add(pixels as u64);
        }
        if let Some(file) = raster.embedded.as_ref().filter(|file| seen.insert(std::sync::Arc::as_ptr(file) as usize)) {
            bytes = bytes.saturating_add(file.capacity() as u64);
        }
    }
    bytes
}

/// A size as notices say it: GiB or MiB to one decimal, or bytes.
pub(crate) fn size_said(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GiB", bytes as f64 / (1_u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.1} MiB", bytes as f64 / (1_u64 << 20) as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Records identities once when the snapshot enters history or has a reservation removed.
pub(crate) fn allocations(project: &Project) -> Allocations {
    rows(project).map(Features::allocation_key).collect()
}

fn rows(project: &Project) -> impl Iterator<Item = &Features> {
    project
        .maps
        .iter()
        .chain(project.offline_areas.iter().map(|area| &area.data))
        .flat_map(|map| map.layers.iter().map(|layer| &layer.dataset.features))
        .chain(project.tables.iter().map(|table| &table.dataset.features))
        .filter(|features| features.capacity() > 0)
}

/// Count the row vector's capacity and extrapolate the heap storage of up to 64 evenly spaced
/// rows, each heap allocation as a block of the system allocator ([`block`]).
fn estimate(features: &Vec<Feature>) -> usize {
    let stride = features.len().div_ceil(64).max(1);
    let (sum, count) =
        features.iter().step_by(stride).fold((0usize, 0usize), |(sum, count), row| (sum.saturating_add(row_bytes(row)), count + 1));
    block(features.capacity().saturating_mul(size_of::<Feature>()))
        .saturating_add(sum.div_ceil(count.max(1)).saturating_mul(features.len()))
}

/// A row's heap storage, block by block: its attribute list, each text, and its geometry's parts.
fn row_bytes(row: &Feature) -> usize {
    let attrs = row.attrs.iter().fold(block(row.attrs.capacity().saturating_mul(size_of::<Value>())), |n, v| {
        n.saturating_add(match v {
            Value::Text(s) => block(s.capacity()),
            _ => 0,
        })
    });
    let ring = |r: &Vec<Coord>| block(r.capacity().saturating_mul(size_of::<Coord>()));
    let parts =
        |p: &Vec<Vec<Coord>>| p.iter().fold(block(p.capacity().saturating_mul(size_of::<Vec<Coord>>())), |n, r| n.saturating_add(ring(r)));
    let geometry = match &row.geometry {
        None | Some(Geometry::Point(_)) => 0,
        Some(Geometry::MultiPoint(p)) => ring(p),
        Some(Geometry::Line(p)) => parts(p),
        Some(Geometry::Polygon(p)) => {
            p.iter().fold(block(p.capacity().saturating_mul(size_of::<Vec<Vec<Coord>>>())), |n, r| n.saturating_add(parts(r)))
        }
    };
    attrs.saturating_add(geometry)
}

/// What an allocation of `bytes` takes from the system allocator: a header and 16-byte rounding,
/// 32 bytes at the least (glibc's malloc; others are alike). It is most of a short text's cost:
/// a layer of short texts measured about 0.74 GB a copy where its bytes alone came to 0.54
/// (#2431). Nothing is allocated for none.
fn block(bytes: usize) -> usize {
    if bytes == 0 { 0 } else { bytes.saturating_add(8 + 15).max(32) & !15 }
}

#[cfg(test)]
mod tests {
    use crate::Session;
    use rustgis_data::{Dataset, Field, FieldType, GeometryType, Value};
    use serde_json::json;

    fn session(text_len: usize) -> Session {
        let mut session = Session::new(crate::Project::default());
        let mut data = Dataset::new("rows", GeometryType::Point, vec![Field::new("text", FieldType::Text)]);
        data.push(Some(rustgis_data::Geometry::Point(rustgis_geom::Coord::new(0.0, 0.0))), vec![Value::Text("a".repeat(text_len))]);
        session.map_mut().add_layer(rustgis_map::Layer::new(0, data));
        session.undo_memory.limit = 2 << 10;
        session
    }

    fn edit(session: &mut Session, text: &str) {
        session.run("edit.setValue", &json!({"layer": "rows", "row": 0, "field": "text", "value": text})).unwrap();
    }

    fn value(session: &Session) -> &Value {
        &session.map().layer_by_name("rows").unwrap().dataset.features[0].attrs[0]
    }

    #[test]
    fn shared_snapshots_are_counted_once_and_dropping_is_said_once() {
        let mut session = session(1024);
        let mark = session.notice_mark();
        for visible in [false, true] {
            session.run("layer.visibility", &json!({"layer": "rows", "visible": visible})).unwrap();
        }
        let b = "b".repeat(1024);
        let c = "c".repeat(1024);
        edit(&mut session, &b);
        assert_eq!(session.undo_depth(), 3);
        assert!(session.notices_since(mark).is_empty());

        // Both visibility steps and the first edit own the same initial rows. All three have
        // to go before the second edit fits, while the remaining step spans b to c.
        edit(&mut session, &c);
        let notices = session.notices_since(mark);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].text.contains("older steps are dropped"), "{notices:?}");
        assert_eq!(session.undo_depth(), 1);

        let mark = session.notice_mark();
        session.run("edit.undo", &json!({})).unwrap();
        assert_eq!(value(&session), &Value::Text(b.clone()));
        session.run("edit.redo", &json!({})).unwrap();
        assert_eq!(value(&session), &Value::Text(c.clone()));
        assert!(session.notices_since(mark).is_empty());

        // Past the budget each edit drops a step again, without saying it again.
        let mark = session.notice_mark();
        edit(&mut session, &b);
        assert_eq!(session.undo_depth(), 1);
        assert!(session.notices_since(mark).is_empty());

        // Another project's first drop is said again.
        session.clear_history();
        edit(&mut session, &c);
        let mark = session.notice_mark();
        edit(&mut session, &b);
        assert_eq!(session.notices_since(mark).len(), 1);
    }

    #[test]
    fn keeps_the_newest_undo_even_when_its_rows_exceed_the_budget() {
        let mut session = session(4096);
        edit(&mut session, "small");
        assert_eq!(session.undo_depth(), 1);
        assert!(session.notices.is_empty());
        edit(&mut session, "smaller");
        assert_eq!(session.undo_depth(), 1);
        assert_eq!(session.notices.len(), 1);
        assert!(session.notices[0].text.contains("older steps are dropped"));
        session.run("edit.undo", &json!({})).unwrap();
        assert_eq!(value(&session), &Value::Text("small".into()));
    }

    #[test]
    fn cuts_the_farthest_redo_when_undo_moves_large_live_rows_into_history() {
        let mut session = session(1);
        let b = "b".repeat(1200);
        let c = "c".repeat(1200);
        edit(&mut session, &b);
        edit(&mut session, &c);
        let mark = session.notice_mark();
        session.run("edit.undo", &json!({})).unwrap();
        assert_eq!(value(&session), &Value::Text(b.clone()));
        session.run("edit.undo", &json!({})).unwrap();
        assert_eq!(value(&session), &Value::Text("a".into()));
        let notices = session.notices_since(mark);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].text.contains("older steps are dropped"));
        session.run("edit.redo", &json!({})).unwrap();
        assert_eq!(value(&session), &Value::Text(b));
        assert!(!session.can_redo(), "only the nearer redo fits");
    }

    /// Memory planned so the open rows and one copy of them fit with `spare` bytes over,
    /// without the history.
    fn room_for_one_copy(session: &mut Session, spare: u64) -> u64 {
        let rows = session.map().layer_by_name("rows").unwrap().dataset.features.clone();
        let copy = session.undo_memory.rows_bytes(&rows);
        let open = session.undo_memory.held(&session.project, &[], &[]);
        let target = open + copy + spare;
        // `usable` leaves a quarter: the smallest memory whose three quarters reach the target.
        let memory = (target * 4).div_ceil(3) + 4;
        assert!(rustgis_data::memory::usable(Some(memory)).unwrap() >= target);
        memory
    }

    #[test]
    fn an_edit_that_fits_beside_the_history_drops_nothing() {
        let mut session = session(4096);
        session.undo_memory.limit = 1 << 30;
        session.undo_memory.memory = Some(1 << 40);
        let mark = session.notice_mark();
        edit(&mut session, "b");
        edit(&mut session, "c");
        assert_eq!(session.undo_depth(), 2);
        assert!(session.notices_since(mark).is_empty());
    }

    #[test]
    fn older_steps_make_room_for_an_edit_and_that_is_said_once() {
        let mut session = session(4096);
        session.undo_memory.limit = 1 << 30;
        session.undo_memory.memory = None;
        let (b, c) = ("b".repeat(4096), "c".repeat(4096));
        edit(&mut session, &b);
        edit(&mut session, &c);
        assert_eq!(session.undo_depth(), 2);
        let memory = room_for_one_copy(&mut session, 64);
        session.undo_memory.memory = Some(memory);
        let mark = session.notice_mark();
        edit(&mut session, "d");
        let notices = session.notices_since(mark);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].text.contains("dropped to make room for edits (2 before editing rows)"), "{notices:?}");
        assert_eq!(value(&session), &Value::Text("d".into()));
        // The step the edit made undoes it.
        session.run("edit.undo", &json!({})).unwrap();
        assert_eq!(value(&session), &Value::Text(c.clone()));
        session.run("edit.redo", &json!({})).unwrap();

        // Dropping again says nothing more in the same project.
        edit(&mut session, &b);
        let mark = session.notice_mark();
        session.undo_memory.memory = Some(room_for_one_copy(&mut session, 64));
        edit(&mut session, &c);
        assert!(session.notices_since(mark).is_empty());
        assert_eq!(session.undo_depth(), 1, "the older steps went");
        assert_eq!(value(&session), &Value::Text(c));
    }

    #[test]
    fn an_edit_that_cannot_fit_is_refused_and_changes_nothing() {
        let mut session = session(4096);
        session.undo_memory.limit = 1 << 30;
        session.undo_memory.memory = None;
        edit(&mut session, "b");
        let state = session.state;
        session.undo_memory.memory = Some(4);
        let mark = session.notice_mark();
        let err = session.run("edit.setValue", &json!({"layer": "rows", "row": 0, "field": "text", "value": "c"})).unwrap_err();
        assert!(err.contains("not enough memory to edit rows: the edit copies its rows"), "{err}");
        assert_eq!(value(&session), &Value::Text("b".into()));
        assert_eq!(session.undo_depth(), 1, "the history is kept");
        assert_eq!(session.state, state);
        let notices = session.notices_since(mark);
        assert_eq!(notices.len(), 1, "{notices:?}");
        // Commands that don't edit features still run.
        session.run("layer.visibility", &json!({"layer": "rows", "visible": false})).unwrap();
        // Unknown memory (the browser) checks nothing.
        session.undo_memory.memory = None;
        edit(&mut session, "c");
        assert_eq!(value(&session), &Value::Text("c".into()));
    }

    #[test]
    fn redo_goes_before_undo_to_make_room() {
        let mut session = session(4096);
        session.undo_memory.limit = 1 << 30;
        session.undo_memory.memory = None;
        for text in ["b", "c", "d"] {
            edit(&mut session, &text.repeat(4096));
        }
        session.run("edit.undo", &json!({})).unwrap();
        assert_eq!((session.undo_depth(), session.can_redo()), (2, true));
        // Room for the open rows, the Undo steps and one copy: only Redo has to go.
        let rows = session.map().layer_by_name("rows").unwrap().dataset.features.clone();
        let copy = session.undo_memory.rows_bytes(&rows);
        drop(rows);
        let held = session.undo_memory.held(&session.project, &session.undo, &[]);
        session.undo_memory.memory = Some(((held + copy + 64) * 4).div_ceil(3) + 4);
        let mark = session.notice_mark();
        edit(&mut session, "e");
        let notices = session.notices_since(mark);
        assert!(notices.iter().any(|n| n.text.contains("(1 before editing rows)")), "{notices:?}");
        assert_eq!(session.undo_depth(), 3, "every Undo step is kept");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_table_edit_run_in_the_background_is_checked_before_its_work() {
        let mut session = Session::new(crate::Project::default());
        session.run("layer.addDataText", &json!({"fileName": "calc.csv", "text": "n\n1\n2\n"})).unwrap();
        session.undo_memory.memory = Some(4);
        let p = json!({"table": "calc", "row": 0, "field": "n", "value": 3});
        let err = crate::cmd::reads::read_ahead(&mut session, "edit.setValue", &p).err().unwrap();
        assert!(err.contains("not enough memory to edit calc"), "{err}");
        session.undo_memory.memory = Some(1 << 40);
        assert!(crate::cmd::reads::read_ahead(&mut session, "edit.setValue", &p).is_ok());
    }

    #[test]
    fn paste_special_is_not_measured_by_the_active_layer() {
        let mut session = session(4096);
        session.undo_memory.memory = Some(4);
        let id = Some(session.map().layer_by_name("rows").unwrap().id);
        assert!(session.room_for_edit("edit.pasteSpecial", true, "Paste Special", id, None).is_none());
        assert!(session.room_for_edit("edit.paste", true, "Paste", id, None).is_some());
    }
}
