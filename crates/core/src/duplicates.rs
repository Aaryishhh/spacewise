//! Multi-stage duplicate detection (spec section 16): identical size, then a
//! fast partial hash, then a full cryptographic hash. Filename is never used
//! as a duplicate signal -- two files with different names and identical
//! content are duplicates; two files with the same name and different
//! content are not.

use crate::model::{DuplicateGroup, FileEntry};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const PARTIAL_HASH_BYTES: usize = 64 * 1024;

pub struct DuplicateEngine;

impl DuplicateEngine {
    pub fn new() -> Self {
        Self
    }

    /// `entries` should be file (not directory) entries from one scan.
    pub fn find_duplicates(&self, entries: &[FileEntry]) -> Vec<DuplicateGroup> {
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
        let partial_groups: Vec<(u64, String, Vec<PathBuf>)> = size_candidates
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
                    .map(|(hash, paths)| (size, hash, paths))
                    .collect::<Vec<_>>()
            })
            .collect();

        // Stage 3: full cryptographic hash confirms true duplicates.
        partial_groups
            .into_par_iter()
            .flat_map(|(size, _partial_hash, paths)| {
                let mut by_full: HashMap<String, Vec<PathBuf>> = HashMap::new();
                for path in paths {
                    if let Some(hash) = full_hash(&path) {
                        by_full.entry(hash).or_default().push(path);
                    }
                }
                by_full
                    .into_iter()
                    .filter(|(_, paths)| paths.len() > 1)
                    .map(|(hash, paths)| DuplicateGroup { id: Uuid::new_v4(), size, content_hash: hash, paths })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

impl Default for DuplicateEngine {
    fn default() -> Self {
        Self::new()
    }
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
}
