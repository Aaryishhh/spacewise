//! Rolls up per-file StorageModel entries into per-directory totals. Every
//! file's size is added to each of its ancestor directories, so a treemap or
//! directory-explorer view can render totals at any depth without re-walking
//! the filesystem.
//!
//! `fold_into` is the streaming-safe entry point: it mutates an existing
//! aggregate map with one batch of entries at a time, so the caller never
//! needs to hold the full FileEntry list in memory -- the map itself is
//! O(unique directories seen), not O(total files), which is what actually
//! keeps a multi-million-file scan's memory bounded.

use crate::model::{DirectoryAggregate, FileEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn aggregate_directories(root: &Path, entries: &[FileEntry]) -> HashMap<PathBuf, DirectoryAggregate> {
    let mut aggregates = HashMap::new();
    fold_into(&mut aggregates, root, entries);
    aggregates
}

pub fn fold_into(aggregates: &mut HashMap<PathBuf, DirectoryAggregate>, root: &Path, entries: &[FileEntry]) {
    for entry in entries {
        if entry.is_dir {
            aggregates
                .entry(entry.path.clone())
                .or_insert_with(|| DirectoryAggregate { path: entry.path.clone(), ..Default::default() });
            // dir_count is incremented on the parent as soon as we see this
            // directory itself -- one pass, no second sweep over all entries.
            if let Some(parent) = entry.parent.as_deref() {
                let parent_agg = aggregates
                    .entry(parent.to_path_buf())
                    .or_insert_with(|| DirectoryAggregate { path: parent.to_path_buf(), ..Default::default() });
                parent_agg.dir_count += 1;
            }
            continue;
        }

        // Attribute this file's size to every ancestor directory, not just
        // its immediate parent, so a top-level category total is correct
        // without a second pass.
        let mut ancestor: Option<&Path> = entry.parent.as_deref();
        while let Some(dir) = ancestor {
            let agg = aggregates
                .entry(dir.to_path_buf())
                .or_insert_with(|| DirectoryAggregate { path: dir.to_path_buf(), ..Default::default() });
            agg.total_size += entry.logical_size;
            agg.allocated_size += entry.allocated_size;
            agg.file_count += 1;
            if agg.latest_modified < entry.modified_at {
                agg.latest_modified = entry.modified_at;
            }
            // Stop at the scan root -- do not roll sizes up into directories
            // outside the tree the user actually asked to scan.
            if dir == root {
                break;
            }
            ancestor = dir.parent();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileEntry;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn file(path: &str, parent: &str, size: u64) -> FileEntry {
        FileEntry {
            id: Uuid::new_v4(),
            scan_id: Uuid::new_v4(),
            path: PathBuf::from(path),
            parent: Some(PathBuf::from(parent)),
            logical_size: size,
            allocated_size: size,
            extension: None,
            created_at: None,
            modified_at: None,
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
    fn rolls_up_nested_sizes_to_every_ancestor() {
        let entries = vec![
            file("/root/a/b/f1.bin", "/root/a/b", 100),
            file("/root/a/f2.bin", "/root/a", 50),
        ];
        let agg = aggregate_directories(Path::new("/root"), &entries);

        assert_eq!(agg[&PathBuf::from("/root/a/b")].total_size, 100);
        assert_eq!(agg[&PathBuf::from("/root/a")].total_size, 150);
        assert_eq!(agg[&PathBuf::from("/root")].total_size, 150);
        // file_count is recursive (like total_size), not just direct children:
        // "/root/a" contains f2.bin directly and f1.bin via "/root/a/b".
        assert_eq!(agg[&PathBuf::from("/root/a")].file_count, 2);
        assert_eq!(agg[&PathBuf::from("/root/a/b")].file_count, 1);
    }

    #[test]
    fn folding_multiple_batches_matches_folding_all_at_once() {
        let batch1 = vec![file("/root/a/f1.bin", "/root/a", 10)];
        let batch2 = vec![file("/root/a/f2.bin", "/root/a", 20)];

        let mut streamed = HashMap::new();
        fold_into(&mut streamed, Path::new("/root"), &batch1);
        fold_into(&mut streamed, Path::new("/root"), &batch2);

        let all_at_once = aggregate_directories(Path::new("/root"), &[batch1, batch2].concat());

        assert_eq!(streamed[&PathBuf::from("/root/a")].total_size, all_at_once[&PathBuf::from("/root/a")].total_size);
        assert_eq!(streamed[&PathBuf::from("/root/a")].file_count, all_at_once[&PathBuf::from("/root/a")].file_count);
    }
}
