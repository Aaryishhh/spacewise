//! Rolls up per-file StorageModel entries into per-directory totals. Every
//! file's size is added to each of its ancestor directories, so a treemap or
//! directory-explorer view can render totals at any depth without re-walking
//! the filesystem.

use crate::model::{DirectoryAggregate, FileEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn aggregate_directories(root: &Path, entries: &[FileEntry]) -> HashMap<PathBuf, DirectoryAggregate> {
    let mut aggregates: HashMap<PathBuf, DirectoryAggregate> = HashMap::new();

    for entry in entries {
        if entry.is_dir {
            aggregates
                .entry(entry.path.clone())
                .or_insert_with(|| DirectoryAggregate {
                    path: entry.path.clone(),
                    ..Default::default()
                });
            continue;
        }

        // Attribute this file's size to every ancestor directory, not just
        // its immediate parent, so a top-level category total is correct
        // without a second pass.
        let mut ancestor: Option<&Path> = entry.parent.as_deref();
        while let Some(dir) = ancestor {
            let agg = aggregates
                .entry(dir.to_path_buf())
                .or_insert_with(|| DirectoryAggregate {
                    path: dir.to_path_buf(),
                    ..Default::default()
                });
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

    // dir_count: number of immediate subdirectories, needed by the UI to
    // show "N items" without a second query.
    let dir_paths: Vec<PathBuf> = entries
        .iter()
        .filter(|e| e.is_dir)
        .map(|e| e.path.clone())
        .collect();
    for dir_path in &dir_paths {
        if let Some(parent) = dir_path.parent() {
            if let Some(agg) = aggregates.get_mut(parent) {
                agg.dir_count += 1;
            }
        }
    }

    aggregates
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
}
