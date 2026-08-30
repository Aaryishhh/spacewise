//! Recursive filesystem scanner. Walks the tree with `walkdir` (readdir is
//! inherently tree-shaped and hard to parallelize safely across platforms),
//! then stats each batch of paths in parallel with `rayon` (the actually
//! expensive I/O). Streams results to a `ScanProgressSink` in batches so the
//! UI can render before the full scan completes.
//!
//! Cycle protection: every visited directory's platform file-id (device+inode
//! on Unix, volume+file-index on Windows) is recorded in a visited-set via
//! `filter_entry`, so a symlink loop or an NTFS junction pointing back at an
//! ancestor can not cause infinite recursion or double-counted size.

use crate::model::FileEntry;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};
use uuid::Uuid;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub root: PathBuf,
    pub batch_size: usize,
    /// Identity every FileEntry this scan produces will carry. Callers that
    /// persist results (e.g. via StorageDatabase::start_scan) must pass the
    /// same id they used there, or queries by scan_id will silently match
    /// nothing.
    pub scan_id: Uuid,
}

impl ScanOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), batch_size: 1024, scan_id: Uuid::new_v4() }
    }

    pub fn with_scan_id(root: impl Into<PathBuf>, scan_id: Uuid) -> Self {
        Self { root: root.into(), batch_size: 1024, scan_id }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanStats {
    pub files_scanned: u64,
    pub dirs_scanned: u64,
    pub total_logical_size: u64,
    pub errors: u64,
    pub duration_ms: u64,
}

pub trait ScanProgressSink: Send {
    fn on_entries(&mut self, entries: Vec<FileEntry>);
    fn on_complete(&mut self, stats: &ScanStats);
}

/// Sink that just accumulates everything in memory. Useful for tests, small
/// scans, and CLI tools; the desktop app uses a sink that streams into
/// StorageDatabase instead so memory stays bounded on huge trees.
#[derive(Default)]
pub struct CollectingSink {
    pub entries: Vec<FileEntry>,
    pub stats: ScanStats,
}

impl ScanProgressSink for CollectingSink {
    fn on_entries(&mut self, mut entries: Vec<FileEntry>) {
        self.entries.append(&mut entries);
    }
    fn on_complete(&mut self, stats: &ScanStats) {
        self.stats = stats.clone();
    }
}

pub struct Scanner;

impl Scanner {
    pub fn new() -> Self {
        Self
    }

    pub fn scan(&self, options: &ScanOptions, sink: &mut dyn ScanProgressSink) -> anyhow::Result<ScanStats> {
        let scan_id = options.scan_id;
        let start = Instant::now();
        let mut stats = ScanStats::default();
        let batch_size = options.batch_size.max(1);
        let mut pending: Vec<PathBuf> = Vec::with_capacity(batch_size);
        let mut visited: HashSet<same_file::Handle> = HashSet::new();

        let walker = WalkDir::new(&options.root)
            .follow_links(false)
            .into_iter()
            .filter_entry(move |entry| {
                if !entry.file_type().is_dir() {
                    return true;
                }
                match same_file::Handle::from_path(entry.path()) {
                    // HashSet::insert returns false if the id was already
                    // present -- i.e. we have been here before via another path.
                    Ok(handle) => visited.insert(handle),
                    Err(_) => true,
                }
            });

        for entry_result in walker {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => {
                    stats.errors += 1;
                    continue;
                }
            };
            pending.push(entry.into_path());
            if pending.len() >= batch_size {
                flush_batch(scan_id, &mut pending, &mut stats, sink);
            }
        }
        if !pending.is_empty() {
            flush_batch(scan_id, &mut pending, &mut stats, sink);
        }

        stats.duration_ms = start.elapsed().as_millis() as u64;
        sink.on_complete(&stats);
        Ok(stats)
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

fn flush_batch(
    scan_id: Uuid,
    pending: &mut Vec<PathBuf>,
    stats: &mut ScanStats,
    sink: &mut dyn ScanProgressSink,
) {
    let paths = std::mem::take(pending);
    let entries: Vec<FileEntry> = paths
        .into_par_iter()
        .filter_map(|path| build_entry(scan_id, path))
        .collect();

    for e in &entries {
        if e.is_dir {
            stats.dirs_scanned += 1;
        } else {
            stats.files_scanned += 1;
        }
        stats.total_logical_size += e.logical_size;
    }
    sink.on_entries(entries);
}

fn build_entry(scan_id: Uuid, path: PathBuf) -> Option<FileEntry> {
    // symlink_metadata: never follows the link, so a broken/looping symlink
    // can not hang the scan -- we report the link's own size/type.
    let meta = std::fs::symlink_metadata(&path).ok()?;
    let is_dir = meta.is_dir();
    Some(FileEntry {
        id: Uuid::new_v4(),
        scan_id,
        parent: path.parent().map(|p| p.to_path_buf()),
        extension: path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase()),
        is_dir,
        is_symlink: meta.file_type().is_symlink(),
        is_hardlink: is_hardlink(&meta),
        is_hidden: is_hidden(&path),
        // Real system-file detection lives in the classification engine
        // (Phase 6), which knows platform-specific protected roots.
        is_system: false,
        filesystem_id: same_file::Handle::from_path(&path)
            .ok()
            .map(|h| format!("{:?}", h)),
        logical_size: meta.len(),
        // Real allocated/on-disk size (accounting for sparse files, APFS
        // clones, NTFS compression) is enriched later via
        // PlatformAdapter::enrich_metadata; this is a safe upper-bound
        // approximation so downstream code always has a value.
        allocated_size: meta.len(),
        created_at: meta.created().ok().map(to_datetime),
        modified_at: meta.modified().ok().map(to_datetime),
        accessed_at: meta.accessed().ok().map(to_datetime),
        path,
    })
}

fn to_datetime(t: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(t)
}

#[cfg(unix)]
fn is_hardlink(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() > 1
}

/// Real Windows hardlink counts require GetFileInformationByHandle, which is
/// PlatformAdapter::enrich_metadata's job (Phase 2 follow-up), not the
/// platform-agnostic scanner's.
#[cfg(not(unix))]
fn is_hardlink(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn is_hidden(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    std::fs::symlink_metadata(path)
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.starts_with('.'))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_flat_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        fs::write(dir.path().join("b.txt"), b"world!").unwrap();

        let mut sink = CollectingSink::default();
        let stats = Scanner::new()
            .scan(&ScanOptions::new(dir.path()), &mut sink)
            .unwrap();

        assert_eq!(stats.files_scanned, 2);
        assert_eq!(stats.total_logical_size, 11);
        assert_eq!(sink.entries.len(), 3); // root dir + 2 files
    }

    #[test]
    fn scans_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub/inner")).unwrap();
        fs::write(dir.path().join("sub/inner/f.bin"), vec![0u8; 100]).unwrap();

        let mut sink = CollectingSink::default();
        let stats = Scanner::new()
            .scan(&ScanOptions::new(dir.path()), &mut sink)
            .unwrap();

        assert_eq!(stats.files_scanned, 1);
        assert_eq!(stats.dirs_scanned, 3); // root + sub + sub/inner
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlink_loop() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(dir.path(), real.join("loop")).unwrap();

        let mut sink = CollectingSink::default();
        let result = Scanner::new().scan(&ScanOptions::new(dir.path()), &mut sink);
        assert!(result.is_ok(), "scan must terminate, not hang or error out");
    }
}
