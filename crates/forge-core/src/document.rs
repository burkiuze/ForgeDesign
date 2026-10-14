//! Documents: layer stack + selection + undo history + change tracking.

use std::path::PathBuf;

use crate::buffer::TiledBuffer;
use crate::color::Color;
use crate::composite;
use crate::geom::{union_opt, IRect};
use crate::history::History;
use crate::layer::{Layer, LayerId};
use crate::selection::Mask;

/// Everything that undo/redo restores. Cheap to clone (copy-on-write tiles).
#[derive(Clone, Debug)]
pub struct DocState {
    pub width: u32,
    pub height: u32,
    /// Bottom-most layer first.
    pub layers: Vec<Layer>,
    pub active: usize,
    pub selection: Option<Mask>,
    pub next_id: LayerId,
    /// When true, painting tools target the active layer's mask.
    pub editing_mask: bool,
}

impl DocState {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, layers: Vec::new(), active: 0, selection: None, next_id: 1, editing_mask: false }
    }

    pub fn alloc_id(&mut self) -> LayerId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn new_layer(&mut self, name: impl Into<String>) -> Layer {
        let id = self.alloc_id();
        Layer::new(id, name, self.width, self.height)
    }

    pub fn active_layer(&self) -> Option<&Layer> {
        self.layers.get(self.active)
    }

    pub fn active_layer_mut(&mut self) -> Option<&mut Layer> {
        self.layers.get_mut(self.active)
    }

    pub fn layer_index(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    pub fn bounds(&self) -> IRect {
        IRect::from_size(self.width, self.height)
    }

    /// Picks a unique "Layer N" name.
    pub fn next_layer_name(&self) -> String {
        let mut n = self.layers.len() + 1;
        loop {
            let name = format!("Layer {n}");
            if !self.layers.iter().any(|l| l.name == name) {
                return name;
            }
            n += 1;
        }
    }
}

pub struct Document {
    pub name: String,
    pub path: Option<PathBuf>,
    pub state: DocState,
    history: History,
    dirty: Option<IRect>,
    selection_changed: bool,
    saved_serial: Option<u64>,
}

impl Document {
    /// New document with a "Background" layer filled with `background`
    /// (`None` = transparent).
    pub fn new(name: impl Into<String>, width: u32, height: u32, background: Option<Color>) -> Self {
        let mut state = DocState::new(width, height);
        let mut bg = state.new_layer("Background");
        if let Some(c) = background {
            bg.pixels = TiledBuffer::solid(width, height, c.to_rgba8());
        }
        state.layers.push(bg);
        Self::from_state(name, state, None)
    }

    pub fn from_state(name: impl Into<String>, state: DocState, path: Option<PathBuf>) -> Self {
        let bounds = state.bounds();
        Self {
            name: name.into(),
            path,
            history: History::new("Open", state.clone()),
            state,
            dirty: Some(bounds),
            selection_changed: true,
            saved_serial: Some(0),
        }
    }

    pub fn width(&self) -> u32 {
        self.state.width
    }

    pub fn height(&self) -> u32 {
        self.state.height
    }

    // ---------------------------------------------------------------- history

    /// Records the current state as a new undo step.
    pub fn commit(&mut self, label: impl Into<String>) {
        self.history.push(label, self.state.clone());
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn undo(&mut self) -> bool {
        if !self.history.can_undo() {
            return false;
        }
        self.jump_history(self.history.cursor() - 1);
        true
    }

    pub fn redo(&mut self) -> bool {
        if !self.history.can_redo() {
            return false;
        }
        self.jump_history(self.history.cursor() + 1);
        true
    }

    pub fn jump_history(&mut self, index: usize) {
        if let Some(s) = self.history.jump(index) {
            let s = s.clone();
            self.restore(s);
        }
    }

    /// Throws away uncommitted changes (e.g. a cancelled filter preview).
    pub fn revert_to_current(&mut self) {
        let s = self.history.current().state.clone();
        self.restore(s);
    }

    /// The last committed state (the "before" image of in-progress edits).
    pub fn committed_state(&self) -> &DocState {
        &self.history.current().state
    }

    fn restore(&mut self, new: DocState) {
        let old = std::mem::replace(&mut self.state, new);
        let new = self.state.clone();
        let structural = old.width != new.width
            || old.height != new.height
            || old.layers.len() != new.layers.len()
            || old.layers.iter().zip(&new.layers).any(|(a, b)| !a.same_props(b));
        if structural {
            self.mark_all_dirty();
        } else {
            for (a, b) in old.layers.iter().zip(&new.layers) {
                self.dirty = union_opt(self.dirty, a.pixels.diff_rect(&b.pixels));
                if let (Some(ma), Some(mb)) = (&a.mask, &b.mask) {
                    self.dirty = union_opt(self.dirty, ma.diff_rect(mb));
                }
            }
        }
        let sel_changed = match (&old.selection, &new.selection) {
            (None, None) => false,
            (Some(a), Some(b)) => a.diff_rect(b).is_some(),
            _ => true,
        };
        self.selection_changed |= sel_changed || structural;
    }

    pub fn is_modified(&self) -> bool {
        self.saved_serial != Some(self.history.current().serial)
    }

    pub fn mark_saved(&mut self) {
        self.saved_serial = Some(self.history.current().serial);
    }

    // ---------------------------------------------------------- change tracking

    pub fn mark_dirty(&mut self, r: IRect) {
        let r = r.clamp_to(self.width(), self.height());
        if !r.is_empty() {
            self.dirty = union_opt(self.dirty, Some(r));
        }
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty = Some(self.state.bounds());
    }

    /// Region of the composite that needs re-rendering since the last call.
    pub fn take_dirty(&mut self) -> Option<IRect> {
        self.dirty.take()
    }

    pub fn set_selection(&mut self, sel: Option<Mask>) {
        self.state.selection = sel;
        self.selection_changed = true;
    }

    pub fn take_selection_changed(&mut self) -> bool {
        std::mem::take(&mut self.selection_changed)
    }

    pub fn selection(&self) -> Option<&Mask> {
        self.state.selection.as_ref()
    }

    // ------------------------------------------------------------ layer ops

    /// Inserts a layer above the active one, makes it active and commits.
    pub fn add_layer(&mut self, layer: Layer, label: &str) {
        let idx = if self.state.layers.is_empty() { 0 } else { self.state.active + 1 };
        self.state.layers.insert(idx, layer);
        self.state.active = idx;
        self.state.editing_mask = false;
        self.mark_all_dirty();
        self.commit(label);
    }

    pub fn new_layer(&mut self) {
        let name = self.state.next_layer_name();
        let layer = self.state.new_layer(name);
        self.add_layer(layer, "New Layer");
    }

    pub fn duplicate_layer(&mut self) {
        let Some(src) = self.state.active_layer().cloned() else { return };
        let mut l = src;
        l.id = self.state.alloc_id();
        l.name = format!("{} copy", l.name);
        self.add_layer(l, "Duplicate Layer");
    }

    pub fn delete_layer(&mut self) {
        if self.state.layers.len() <= 1 {
            return;
        }
        self.state.layers.remove(self.state.active);
        self.state.active = self.state.active.min(self.state.layers.len() - 1);
        self.state.editing_mask = false;
        self.mark_all_dirty();
        self.commit("Delete Layer");
    }

    pub fn select_layer(&mut self, index: usize) {
        if index < self.state.layers.len() && index != self.state.active {
            self.state.active = index;
            self.state.editing_mask = false;
        }
    }

    /// Moves the active layer `delta` steps up (+) or down (-) the stack.
    pub fn move_layer(&mut self, delta: i32) {
        let from = self.state.active;
        let to = (from as i32 + delta).clamp(0, self.state.layers.len() as i32 - 1) as usize;
        if from == to {
            return;
        }
        let l = self.state.layers.remove(from);
        self.state.layers.insert(to, l);
        self.state.active = to;
        self.mark_all_dirty();
        self.commit("Move Layer");
    }

    /// Reorders a layer from one index to another (drag & drop in the panel).
    pub fn reorder_layer(&mut self, from: usize, to: usize) {
        let n = self.state.layers.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let l = self.state.layers.remove(from);
        self.state.layers.insert(to, l);
        self.state.active = to;
        self.mark_all_dirty();
        self.commit("Reorder Layers");
    }

    /// Merges the active layer into the one below it.
    pub fn merge_down(&mut self) {
        let i = self.state.active;
        if i == 0 || i >= self.state.layers.len() {
            return;
        }
        let (w, h) = (self.width(), self.height());
        let below = &self.state.layers[i - 1];
        let above = &self.state.layers[i];
        if !below.visible {
            return;
        }
        let mut base = below.clone();
        base.opacity = 1.0;
        base.blend = crate::blend::BlendMode::Normal;
        let merged = composite::flatten_layers(&[&base, above], w, h);
        let below = &mut self.state.layers[i - 1];
        below.pixels = merged;
        below.mask = None;
        self.state.layers.remove(i);
        self.state.active = i - 1;
        self.state.editing_mask = false;
        self.mark_all_dirty();
        self.commit("Merge Down");
    }

    /// Merges all visible layers into a new layer on top (Ctrl+Shift+Alt+E).
    pub fn stamp_visible(&mut self) {
        let pixels = composite::merged(&self.state);
        let id = self.state.alloc_id();
        let l = Layer::with_pixels(id, "Merged", pixels);
        self.state.active = self.state.layers.len() - 1;
        self.add_layer(l, "Stamp Visible");
    }

    /// Flattens the document into one opaque "Background" layer.
    pub fn flatten(&mut self, background: Color) {
        let (w, h) = (self.width(), self.height());
        let layers: Vec<&Layer> = self.state.layers.iter().collect();
        let data = composite::composite_layers(
            &layers,
            w,
            h,
            IRect::from_size(w, h),
            composite::Background::Solid(background.to_rgba8()),
        );
        let id = self.state.alloc_id();
        let l = Layer::with_pixels(id, "Background", TiledBuffer::from_dense(w, h, &data));
        self.state.layers = vec![l];
        self.state.active = 0;
        self.state.editing_mask = false;
        self.mark_all_dirty();
        self.commit("Flatten Image");
    }

    pub fn add_layer_mask(&mut self, from_selection: bool) {
        let sel = self.state.selection.clone();
        let Some(l) = self.state.active_layer_mut() else { return };
        if l.mask.is_some() {
            return;
        }
        l.add_mask();
        if from_selection {
            if let (Some(sel), Some(m)) = (sel, l.mask.as_mut()) {
                *m = sel;
            }
        }
        self.state.editing_mask = true;
        self.mark_all_dirty();
        self.commit("Add Layer Mask");
    }

    pub fn delete_layer_mask(&mut self, apply: bool) {
        let Some(l) = self.state.active_layer_mut() else { return };
        if l.mask.is_none() {
            return;
        }
        if apply {
            l.apply_mask();
        } else {
            l.mask = None;
        }
        self.state.editing_mask = false;
        self.mark_all_dirty();
        self.commit(if apply { "Apply Layer Mask" } else { "Delete Layer Mask" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_and_dirty() {
        let mut d = Document::new("t", 100, 100, Some(Color::WHITE));
        assert!(!d.is_modified());
        d.take_dirty();
        d.new_layer();
        assert_eq!(d.state.layers.len(), 2);
        assert!(d.is_modified());
        d.state.layers[1].pixels.set(80, 80, [1, 2, 3, 255]);
        d.commit("Paint");
        d.take_dirty();
        assert!(d.undo());
        assert_eq!(d.state.layers[1].pixels.get(80, 80), [0, 0, 0, 0]);
        assert_eq!(d.take_dirty(), Some(IRect::new(64, 64, 100, 100)), "only the touched tile is dirty");
        assert!(d.redo());
        assert_eq!(d.state.layers[1].pixels.get(80, 80), [1, 2, 3, 255]);
        assert!(d.undo() && d.undo());
        assert_eq!(d.state.layers.len(), 1);
        assert!(!d.is_modified());
    }

    #[test]
    fn merge_and_flatten() {
        let mut d = Document::new("t", 10, 10, Some(Color::WHITE));
        d.new_layer();
        d.state.layers[1].pixels.set(1, 1, [0, 0, 0, 255]);
        d.state.layers[1].opacity = 0.5;
        d.merge_down();
        assert_eq!(d.state.layers.len(), 1);
        assert_eq!(d.state.layers[0].pixels.get(1, 1), [128, 128, 128, 255]);
        d.new_layer();
        d.flatten(Color::WHITE);
        assert_eq!(d.state.layers.len(), 1);
    }
}
