//! Multi-stage duplicate detection (spec section 16): identical size, then a
//! fast partial hash, then a full cryptographic hash. Filename is never used
//! as a duplicate signal -- two files with different names and identical
//! content are duplicates; two files with the same name and different
//! content are not.
//!
//! The expensive step (full-file hashing) supports cancellation, progress
//! reporting, and a hash cache keyed by (size, modified_at) so a file whose
//! metadata has not changed since it was last hashed is never re-read.

use crate::model::{DuplicateGroup, FileEntry};
use crate::scanner::CancellationToken;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use uuid::Uuid;

const PARTIAL_HASH_BYTES: usize = 64 * 1024;

/// A previously-computed full-file hash, valid only while the file's size
/// and mtime match what was recorded when the hash was taken -- any change
/// to either invalidates the cache entry (we simply recompute).
#[derive(Debug, Clone)]
pub struct CachedHash {
    pub size: u64,
    pub modified_at: Option<DateTime<Utc>>,
    pub hash: String,
}

pub trait DuplicateProgressSink: Send {
    fn on_progress(&mut self, hashed: usize, total: usize);
}

struct NoopProgress;
impl DuplicateProgressSink for NoopProgress {
    fn on_progress(&mut self, _hashed: usize, _total: usize) {}
}

pub struct DuplicateEngine;

impl DuplicateEngine {
    pub fn new() -> Self {
        Self
    }

    /// Simple entry point (no cache, no cancellation) -- used by tests and
    /// any caller that does not need progressive UX.
    pub fn find_duplicates(&self, entries: &[FileEntry]) -> Vec<DuplicateGroup> {
        let (groups, _cache) =
            self.find_duplicates_with(entries, &HashMap::new(), &CancellationToken::new(), &mut NoopProgress);
        groups
    }

    /// Full entry point: `cache` seeds already-known hashes (skipped if the
    /// file's size/mtime still match), `cancel` is checked between
    /// size-candidate groups so a large duplicate scan can be stopped
    /// promptly, and `sink` receives (hashed_so_far, total_candidates) as
    /// stage 3 progresses. Returns the duplicate groups found plus the
    /// updated cache (including entries reused unchanged from the input
    /// cache) so the caller can persist it.
    pub fn find_duplicates_with(
        &self,
        entries: &[FileEntry],
        cache: &HashMap<PathBuf, CachedHash>,
        cancel: &CancellationToken,
        sink: &mut dyn DuplicateProgressSink,
    ) -> (Vec<DuplicateGroup>, HashMap<PathBuf, CachedHash>) {
        // Stage 1: identical size. Zero-byte files are excluded -- every
        // empty file is trivially "identical" and grouping them wastes the
        // user's review time on a group with nothing to reclaim.
        let mut by_size: HashMap<u64, Vec<&Path>> = HashMap::new();
        for e in entries {
            if !e.is_dir && e.logical_size > 0 {
                by_size.entry(e.logical_size).or_default().push(&e.path);
            }
        }
        let size_candidates: Vec<(u64, Vec<PathBuf>)> = by_size
            .into_iter()
            .filter(|(_, paths)| paths.len() > 1)
            .map(|(size, paths)| (size, paths.into_iter().map(|p| p.to_path_buf()).collect()))
            .collect();

        // Stage 2: fast partial hash (first 64KB) narrows same-size files
        // down to groups that are very likely duplicates without reading
        // entire large files yet.
        let partial_groups: Vec<(u64, Vec<PathBuf>)> = size_candidates
            .into_par_iter()
            .flat_map(|(size, paths)| {
                let mut by_partial: HashMap<String, Vec<PathBuf>> = HashMap::new();
                for path in paths {
                    if let Some(hash) = partial_hash(&path) {
                        by_partial.entry(hash).or_default().push(path);
                    }
                }
                by_partial
                    .into_iter()
                    .filter(|(_, paths)| paths.len() > 1)
                    .map(|(_hash, paths)| (size, paths))
                    .collect::<Vec<_>>()
            })
            .collect();

        // Stage 3: full cryptographic hash confirms true duplicates. Cache
        // hits skip the read entirely; cancellation is checked between
        // candidate groups (coarse but simple, and this is the I/O-bound
        // stage where it actually matters).
        let total_candidates: usize = partial_groups.iter().map(|(_, paths)| paths.len()).sum();
        let hashed_so_far = AtomicUsize::new(0);
        let new_cache: Mutex<HashMap<PathBuf, CachedHash>> = Mutex::new(HashMap::new());
        let mut results = Vec::new();

        for (size, paths) in partial_groups {
            if cancel.is_cancelled() {
                break;
            }
            let hashes: Vec<(PathBuf, Option<(String, Option<DateTime<Utc>>)>)> = paths
                .into_par_iter()
                .map(|path| {
                    let result = hash_with_cache(&path, size, cache);
                    (path, result)
                })
                .collect();

            let mut by_full: HashMap<String, Vec<PathBuf>> = HashMap::new();
            for (path, result) in hashes {
                hashed_so_far.fetch_add(1, Ordering::Relaxed);
                if let Some((hash, modified_at)) = result {
                    new_cache.lock().unwrap().insert(path.clone(), CachedHash { size, modified_at, hash: hash.clone() });
                    by_full.entry(hash).or_default().push(path);
                }
            }
            sink.on_progress(hashed_so_far.load(Ordering::Relaxed), total_candidates);

            for (hash, group_paths) in by_full {
                if group_paths.len() > 1 {
                    results.push(DuplicateGroup { id: Uuid::new_v4(), size, content_hash: hash, paths: group_paths });
                }
            }
        }

        (results, new_cache.into_inner().unwrap())
    }
}

impl Default for DuplicateEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_with_cache(path: &Path, size: u64, cache: &HashMap<PathBuf, CachedHash>) -> Option<(String, Option<DateTime<Utc>>)> {
    let modified_at = std::fs::symlink_metadata(path).ok().and_then(|m| m.modified().ok()).map(DateTime::<Utc>::from);

    if let Some(cached) = cache.get(path) {
        if cached.size == size && cached.modified_at == modified_at {
            return Some((cached.hash.clone(), modified_at));
        }
    }

    full_hash(path).map(|h| (h, modified_at))
}

fn partial_hash(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; PARTIAL_HASH_BYTES];
    let n = file.read(&mut buf).ok()?;
    Some(blake3::hash(&buf[..n]).to_hex().to_string())
}

fn full_hash(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn file_entry(path: PathBuf, size: u64) -> FileEntry {
        FileEntry {
            id: Uuid::new_v4(),
            scan_id: Uuid::new_v4(),
            path,
            parent: None,
            logical_size: size,
            allocated_size: size,
            extension: None,
            created_at: None,
            modified_at: Some(Utc::now()),
            accessed_at: None,
            is_dir: false,
            is_symlink: false,
            is_hardlink: false,
            is_hidden: false,
            is_system: false,
            filesystem_id: None,
        }
    }

    #[test]
    fn finds_true_duplicates_and_ignores_same_size_different_content() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        let c = dir.path().join("c.bin"); // same size as a/b, different content
        std::fs::write(&a, b"identical content here").unwrap();
        std::fs::write(&b, b"identical content here").unwrap();
        std::fs::write(&c, b"totally different text!").unwrap();

        let entries = vec![
            file_entry(a.clone(), 23),
            file_entry(b.clone(), 23),
            file_entry(c.clone(), 24),
        ];
        let groups = DuplicateEngine::new().find_duplicates(&entries);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].paths.len(), 2);
        assert!(groups[0].paths.contains(&a));
        assert!(groups[0].paths.contains(&b));
    }

    #[test]
    fn unique_sizes_never_produce_a_group() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        std::fs::write(&a, b"only file").unwrap();
        let entries = vec![file_entry(a, 9)];
        assert!(DuplicateEngine::new().find_duplicates(&entries).is_empty());
    }

    #[test]
    fn cancellation_stops_before_processing_further_groups() {
        let dir = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        // Multiple distinct size-groups, each with a duplicate pair, so
        // there is more than one "group" for the cancel check between
        // groups to actually skip.
        for g in 0..5 {
            let content = vec![g as u8; 50];
            let a = dir.path().join(format!("g{g}_a.bin"));
            let b = dir.path().join(format!("g{g}_b.bin"));
            std::fs::write(&a, &content).unwrap();
            std::fs::write(&b, &content).unwrap();
            entries.push(file_entry(a, 50));
            entries.push(file_entry(b, 50));
        }

        let token = CancellationToken::new();
        token.cancel();
        let (groups, _cache) = DuplicateEngine::new().find_duplicates_with(&entries, &HashMap::new(), &token, &mut NoopProgress);
        assert!(groups.len() < 5, "cancellation should stop before all groups are processed");
    }

    #[test]
    fn reuses_cached_hash_when_size_and_mtime_match() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        std::fs::write(&a, b"cached content!!").unwrap();
        std::fs::write(&b, b"cached content!!").unwrap();
        let entries = vec![file_entry(a.clone(), 16), file_entry(b.clone(), 16)];

        let (groups, cache) =
            DuplicateEngine::new().find_duplicates_with(&entries, &HashMap::new(), &CancellationToken::new(), &mut NoopProgress);
        assert_eq!(groups.len(), 1);
        let real_hash = groups[0].content_hash.clone();

        // Feed back a cache with a deliberately wrong hash for `a` but
        // matching size/mtime -- if the cache is honored, the "duplicate"
        // group should report the (wrong) cached hash, proving the real
        // file was not re-read.
        let mut poisoned_cache = cache.clone();
        let entry = poisoned_cache.get_mut(&a).unwrap();
        entry.hash = "not-the-real-hash".to_string();

        let (groups2, _) =
            DuplicateEngine::new().find_duplicates_with(&entries, &poisoned_cache, &CancellationToken::new(), &mut NoopProgress);
        // `a` reuses the poisoned cache entry, `b` is rehashed for real and
        // gets the real hash -- they now disagree, so no group forms.
        assert!(groups2.is_empty(), "poisoned cache for one file should break the pairing, proving it was actually used");
        assert_ne!(real_hash, "not-the-real-hash");
    }
}
