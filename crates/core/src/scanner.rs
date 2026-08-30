//! Recursive filesystem scanner. Streams `FileEntry` results progressively via
//! a channel so the UI can render before the full scan completes.
//!
//! Must never recurse infinitely through symlinks/junctions/reparse points:
//! every visited (device, inode) or (volume, file_id) pair is tracked in a
//! visited-set before descending.

use crate::model::FileEntry;

pub struct ScanOptions {
    pub root: std::path::PathBuf,
    pub follow_symlinks: bool,
}

pub trait ScanProgressSink: Send {
    fn on_entries(&mut self, entries: &[FileEntry]);
    fn on_complete(&mut self);
}

pub struct Scanner;

impl Scanner {
    pub fn new() -> Self {
        Self
    }

    /// Placeholder: Phase 2 implements the real walker (rayon work-stealing
    /// traversal + platform adapter metadata calls + visited-set cycle guard).
    pub fn scan(&self, _options: &ScanOptions, _sink: &mut dyn ScanProgressSink) -> anyhow::Result<()> {
        unimplemented!("Phase 2: filesystem scanner")
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}
