//! Snapshot-based undo history.
//!
//! Each entry stores a complete [`DocState`]. Because all pixel data lives in
//! copy-on-write tiles, a snapshot only owns the tiles that changed since the
//! previous one, so "store everything" is both simple and memory-efficient.

use crate::document::DocState;

#[derive(Clone)]
pub struct HistoryEntry {
    pub label: String,
    pub state: DocState,
    /// Unique, monotonically increasing serial (used for "modified" tracking).
    pub serial: u64,
}

#[derive(Clone)]
pub struct History {
    entries: Vec<HistoryEntry>,
    cursor: usize,
    next_serial: u64,
    pub max_entries: usize,
}

impl History {
    pub fn new(label: impl Into<String>, state: DocState) -> Self {
        Self {
            entries: vec![HistoryEntry { label: label.into(), state, serial: 0 }],
            cursor: 0,
            next_serial: 1,
            max_entries: 100,
        }
    }

    pub fn push(&mut self, label: impl Into<String>, state: DocState) {
        self.entries.truncate(self.cursor + 1);
        self.entries.push(HistoryEntry { label: label.into(), state, serial: self.next_serial });
        self.next_serial += 1;
        if self.entries.len() > self.max_entries {
            let excess = self.entries.len() - self.max_entries;
            self.entries.drain(..excess);
        }
        self.cursor = self.entries.len() - 1;
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    /// Moves the cursor and returns the state to restore.
    pub fn jump(&mut self, index: usize) -> Option<&DocState> {
        if index >= self.entries.len() {
            return None;
        }
        self.cursor = index;
        Some(&self.entries[index].state)
    }

    pub fn current(&self) -> &HistoryEntry {
        &self.entries[self.cursor]
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }
}
