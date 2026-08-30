//! Recursive filesystem scanner. Walks the tree with `walkdir` (readdir is
//! inherently tree-shaped and hard to parallelize safely across platforms),
//! then stats each batch of paths in parallel with `rayon` (the actually
//! expensive I/O). Streams results to a `ScanProgressSink` in batches so the
//! caller never has to buffer the whole tree, and checks a `CancellationToken`
//! between batches so a scan can be stopped promptly and the caller regains
//! control immediately.
//!
//! Cycle protection: every visited directory's platform file-id (device+inode
//! on Unix, volume+file-index on Windows) is recorded in a visited-set via
//! `filter_entry`, so a symlink loop or an NTFS junction pointing back at an
//! ancestor can not cause infinite recursion or double-counted size.
//!
//! Robustness: a single unreadable/vanished/permission-denied entry never
//! aborts the scan -- it is counted and recorded as a `SkippedItem` and the
//! walk continues. `symlink_metadata` (never `metadata`) is used throughout
//! so a broken symlink, a file that disappears between readdir and stat, or
//! a reparse point can not hang or panic the scan.

use crate::model::FileEntry;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use uuid::Uuid;
use walkdir::WalkDir;

/// Cheap, cloneable, thread-safe cancellation flag. Checked between batches
/// (not per-entry, to avoid an atomic load per file) so cancellation takes
/// effect within one batch (`ScanOptions::batch_size` entries), typically a
/// few hundred milliseconds even on a fast local disk.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub root: PathBuf,
    pub batch_size: usize,
    /// Identity every FileEntry this scan produces will carry. Callers that
    /// persist results (e.g. via StorageDatabase::start_scan) must pass the
    /// same id they used there, or queries by scan_id will silently match
    /// nothing.
    pub scan_id: Uuid,
    pub cancel_token: CancellationToken,
}

impl ScanOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), batch_size: 1024, scan_id: Uuid::new_v4(), cancel_token: CancellationToken::new() }
    }

    pub fn with_scan_id(root: impl Into<PathBuf>, scan_id: Uuid) -> Self {
        Self { root: root.into(), batch_size: 1024, scan_id, cancel_token: CancellationToken::new() }
    }

    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }
}

/// One filesystem entry the walker could not process, with the reason, so
/// the user can review what was skipped instead of being told a scary raw
/// I/O error. Capped in ScanStats to a bounded sample -- `skipped_total`
/// still counts every occurrence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkippedItem {
    pub path: Option<PathBuf>,
    pub reason: String,
}

const MAX_TRACKED_SKIPS: usize = 500;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanStats {
    pub files_scanned: u64,
    pub dirs_scanned: u64,
    pub total_logical_size: u64,
    /// Deprecated alias for skipped_total, kept so any existing caller
    /// reading `.errors` still compiles; new code should use skipped_total.
    pub errors: u64,
    pub skipped_total: u64,
    pub skipped_sample: Vec<SkippedItem>,
    pub duration_ms: u64,
    pub cancelled: bool,
}

pub trait ScanProgressSink: Send {
    fn on_entries(&mut self, entries: Vec<FileEntry>);
    /// Called once per batch with the running totals so a caller can stream
    /// progress without recomputing sums itself. Default no-op keeps
    /// existing sinks (tests, CLI) compiling unchanged.
    fn on_progress(&mut self, _stats: &ScanStats) {}
    fn on_complete(&mut self, stats: &ScanStats);
}

/// Sink that just accumulates everything in memory. Useful for tests, small
/// scans, and CLI tools; the desktop app streams straight into
/// StorageDatabase per batch instead so memory stays bounded (O(unique
/// directories), not O(total files)) on huge trees.
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
                Err(err) => {
                    record_skip(&mut stats, err.path().map(|p| p.to_path_buf()), skip_reason(&err));
                    continue;
                }
            };
            pending.push(entry.into_path());
            if pending.len() >= batch_size {
                flush_batch(scan_id, &mut pending, &mut stats, sink);
                sink.on_progress(&stats);
                if options.cancel_token.is_cancelled() {
                    stats.cancelled = true;
                    break;
                }
            }
        }
        if !stats.cancelled && !pending.is_empty() {
            if options.cancel_token.is_cancelled() {
                stats.cancelled = true;
            } else {
                flush_batch(scan_id, &mut pending, &mut stats, sink);
            }
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

fn skip_reason(err: &walkdir::Error) -> String {
    if let Some(io_err) = err.io_error() {
        io_err.to_string()
    } else if err.loop_ancestor().is_some() {
        "filesystem loop detected".to_string()
    } else {
        err.to_string()
    }
}

fn record_skip(stats: &mut ScanStats, path: Option<PathBuf>, reason: String) {
    stats.errors += 1;
    stats.skipped_total += 1;
    if stats.skipped_sample.len() < MAX_TRACKED_SKIPS {
        stats.skipped_sample.push(SkippedItem { path, reason });
    }
}

fn flush_batch(
    scan_id: Uuid,
    pending: &mut Vec<PathBuf>,
    stats: &mut ScanStats,
    sink: &mut dyn ScanProgressSink,
) {
    let paths = std::mem::take(pending);
    let mut entries: Vec<FileEntry> = Vec::with_capacity(paths.len());
    let results: Vec<Result<FileEntry, (PathBuf, String)>> = paths.into_par_iter().map(build_entry_checked(scan_id)).collect();
    for r in results {
        match r {
            Ok(e) => entries.push(e),
            Err((path, reason)) => record_skip(stats, Some(path), reason),
        }
    }

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

fn build_entry_checked(scan_id: Uuid) -> impl Fn(PathBuf) -> Result<FileEntry, (PathBuf, String)> {
    move |path: PathBuf| {
        // symlink_metadata: never follows the link, so a broken/looping
        // symlink, a reparse point, or a file that disappeared between
        // readdir and stat can not hang or panic the scan -- it is reported
        // as a skip with the real OS error instead.
        let meta = std::fs::symlink_metadata(&path).map_err(|e| (path.clone(), e.to_string()))?;
        let is_dir = meta.is_dir();
        Ok(FileEntry {
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
            filesystem_id: same_file::Handle::from_path(&path).ok().map(|h| format!("{:?}", h)),
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

    #[test]
    fn cancellation_stops_the_scan_and_reports_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            fs::write(dir.path().join(format!("f{i}.bin")), vec![0u8; 10]).unwrap();
        }

        let token = CancellationToken::new();
        token.cancel(); // pre-cancelled: must stop after at most one batch

        let mut sink = CollectingSink::default();
        let options = ScanOptions::new(dir.path()).with_cancel_token(token);
        let stats = Scanner::new().scan(&options, &mut sink).unwrap();

        assert!(stats.cancelled);
    }

    #[test]
    fn unreadable_file_is_skipped_not_fatal() {
        // A path that never existed must be recorded as a skip, not panic
        // or abort a batch that also contains real entries.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("real.txt"), b"present").unwrap();
        let ghost = dir.path().join("ghost.txt");

        let scan_id = Uuid::new_v4();
        let mut stats = ScanStats::default();
        let mut sink = CollectingSink::default();
        let mut pending = vec![dir.path().join("real.txt"), ghost.clone()];
        flush_batch(scan_id, &mut pending, &mut stats, &mut sink);

        assert_eq!(sink.entries.len(), 1);
        assert_eq!(stats.skipped_total, 1);
        assert_eq!(stats.skipped_sample[0].path.as_deref(), Some(ghost.as_path()));
    }

    #[cfg(windows)]
    #[test]
    fn does_not_follow_ntfs_junction_loop() {
        // Junctions are a distinct NTFS reparse-point type from symlinks and
        // are not created via std::os::windows::fs::symlink_dir -- shell out
        // to mklink /J, which needs no elevated privilege.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let junction = real.join("loop_back");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", junction.to_str().unwrap(), dir.path().to_str().unwrap()])
            .status();
        if status.map(|s| !s.success()).unwrap_or(true) {
            return; // mklink unavailable in this environment -- not a scanner bug
        }

        let mut sink = CollectingSink::default();
        let result = Scanner::new().scan(&ScanOptions::new(dir.path()), &mut sink);
        assert!(result.is_ok(), "junction loop must not hang or error out the whole scan");
    }

    #[cfg(windows)]
    #[test]
    fn does_not_follow_windows_symlink_loop() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = real.join("loop_back");
        // Creating a directory symlink on Windows needs either Developer
        // Mode or an elevated process -- skip gracefully if unavailable
        // rather than failing CI on machines without either.
        if std::os::windows::fs::symlink_dir(dir.path(), &link).is_err() {
            return;
        }

        let mut sink = CollectingSink::default();
        let result = Scanner::new().scan(&ScanOptions::new(dir.path()), &mut sink);
        assert!(result.is_ok(), "symlink loop must not hang or error out the whole scan");
    }

    #[cfg(windows)]
    #[test]
    fn locked_file_is_skipped_not_fatal() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked.bin");
        fs::write(&locked, b"locked contents").unwrap();

        // Open with zero share flags: no other handle (including this scan)
        // can even read the file while this handle is held.
        let _handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .expect("should be able to open exclusively for this test");

        let mut sink = CollectingSink::default();
        let result = Scanner::new().scan(&ScanOptions::new(dir.path()), &mut sink);
        assert!(result.is_ok(), "a locked file must not hang or abort the scan");
        // symlink_metadata (stat) typically still succeeds on a locked file
        // even when open-for-read would fail, so this mostly guards against
        // a hang/panic regression rather than asserting a specific skip.
    }

    #[test]
    fn files_disappearing_mid_scan_do_not_crash_it() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..300 {
            fs::write(dir.path().join(format!("f{i}.bin")), vec![0u8; 16]).unwrap();
        }

        let dir_path = dir.path().to_path_buf();
        let deleter = std::thread::spawn(move || {
            for i in 0..300 {
                let _ = fs::remove_file(dir_path.join(format!("f{i}.bin")));
            }
        });

        let mut sink = CollectingSink::default();
        let result = Scanner::new().scan(&ScanOptions::new(dir.path()), &mut sink);
        deleter.join().unwrap();

        assert!(result.is_ok(), "files vanishing during a scan must not crash it");
    }

    #[test]
    fn very_deep_directory_tree_does_not_overflow_or_hang() {
        let dir = tempfile::tempdir().unwrap();
        let mut path = dir.path().to_path_buf();
        for i in 0..120 {
            path = path.join(format!("d{i}"));
        }
        // Windows MAX_PATH is 260 without the \\?\ prefix; this depth alone
        // already exceeds it on a typical temp dir path, so this also
        // exercises the "long Windows paths" edge case for free.
        let create_result = fs::create_dir_all(&path);
        if create_result.is_err() {
            // Some CI/sandbox environments cap path length below what this
            // test needs; that is an environment limit, not a scanner bug.
            return;
        }
        fs::write(path.join("leaf.txt"), b"deep").ok();

        let mut sink = CollectingSink::default();
        let result = Scanner::new().scan(&ScanOptions::new(dir.path()), &mut sink);
        assert!(result.is_ok());
    }
}
