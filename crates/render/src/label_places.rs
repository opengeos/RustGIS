//! Deferred curved placements: transformations are retained until collision placement needs them.
use super::*;
use std::sync::Arc;

type Projection = Arc<dyn Fn(f32, f32) -> Option<(f32, f32)> + Send + Sync>;

#[derive(Clone)]
enum Change {
    Project(Projection),
    Shift(f32, f32),
}

#[derive(Debug)]
pub(super) struct CurveSpec {
    pub line: Arc<line_labels::Line>,
    pub chars: Vec<String>,
    pub advances: Vec<f32>,
    pub gap: f32,
    pub overrun: f32,
}

#[derive(Clone)]
pub(super) struct Curves {
    pub spec: Arc<CurveSpec>,
    pub positions: Vec<f32>,
    pub locked: Option<usize>,
    changes: Vec<Change>,
}

impl std::fmt::Debug for Curves {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Curves").field("positions", &self.positions).field("locked", &self.locked).finish_non_exhaustive()
    }
}

impl Curves {
    pub fn new(spec: Arc<CurveSpec>, positions: Vec<f32>, locked: Option<usize>) -> Self {
        Self { spec, positions, locked, changes: Vec::new() }
    }

    pub fn project(&mut self, at: impl Fn(f32, f32) -> Option<(f32, f32)> + Send + Sync + 'static) {
        self.changes.push(Change::Project(Arc::new(at)));
    }

    pub fn shift(&mut self, dx: f32, dy: f32) {
        self.changes.push(Change::Shift(dx, dy));
    }

    /// The first in-sight anchor (middle first) for a tilted view's label; no character layout.
    pub fn anchor_in(&self, at: &impl Fn(f32, f32) -> Option<(f32, f32)>) -> Option<(f32, f32)> {
        for (i, d) in self.positions.iter().enumerate() {
            if self.locked.is_some_and(|won| won != i) {
                continue;
            }
            let Some((x, y)) = self.spec.line.anchor(*d) else { continue };
            if let Some(p) = at(x, y) {
                return Some(p);
            }
        }
        None
    }

    pub fn place(&self, i: usize, template: &mut PlacedLabel) -> Option<LabelPlace> {
        if self.locked.is_some_and(|won| won != i) {
            return None;
        }
        let at = *self.positions.get(i)?;
        #[cfg(test)]
        LAYOUTS.with(|n| n.set(n.get() + 1));
        let ((x, y), turns) = self.spec.line.curved(&self.spec.advances, self.spec.gap, self.spec.overrun, Some(at))?;
        let glyphs =
            self.spec.chars.iter().zip(turns).map(|(text, (dx, dy, angle))| LabelGlyph { text: text.clone(), dx, dy, angle }).collect();
        let mut p = LabelPlace { x, y, angle: None, glyphs };
        for change in &self.changes {
            match change {
                Change::Project(at) => {
                    if !p.transform(template, &|x, y| at(x, y)) {
                        return None;
                    }
                }
                Change::Shift(dx, dy) => (p.x, p.y) = (p.x + dx, p.y + dy),
            }
        }
        Some(p)
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static LAYOUTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
