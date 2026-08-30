//! Traversal-only benchmark: nothing but filesystem walking + the minimum
//! metadata Spacewise genuinely needs. No SQLite, no classification, no
//! aggregation, no UI events, no duplicate hashing -- isolates whether the
//! walk itself (as opposed to anything Spacewise's pipeline adds on top) is
//! what is slow, and whether cold-vs-warm OS cache state matters.
//!
//! Usage:
//!   cargo run --release -p spacewise-core --example traversal_bench -- --dir "C:\path"
//!
//! Runs three passes over the same tree (cold, second, third) and reports
//! each separately, plus a "metadata-light" pass (just readdir + file-type,
//! no stat() at all) vs the production-equivalent full-metadata pass, to
//! see whether a specific syscall is the expensive part.

use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

fn full_metadata_pass(root: &Path) -> (u64, u64, std::time::Duration) {
    let start = Instant::now();
    let mut files = 0u64;
    let mut dirs = 0u64;
    for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
        // Mirrors build_entry_checked in scanner.rs: symlink_metadata (one
        // stat call), read is_dir/len/timestamps/attributes from it.
        if let Ok(meta) = std::fs::symlink_metadata(entry.path()) {
            if meta.is_dir() {
                dirs += 1;
            } else {
                files += 1;
            }
            let _ = meta.len();
            let _ = meta.modified();
        }
    }
    (files, dirs, start.elapsed())
}

fn metadata_light_pass(root: &Path) -> (u64, u64, std::time::Duration) {
    let start = Instant::now();
    let mut files = 0u64;
    let mut dirs = 0u64;
    // walkdir's own DirEntry::file_type() is often free on Windows/most
    // platforms (comes from the readdir/FindNextFile call itself, no extra
    // stat syscall) -- this measures readdir-only cost with no explicit
    // stat() at all.
    for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
        if entry.file_type().is_dir() {
            dirs += 1;
        } else {
            files += 1;
        }
    }
    (files, dirs, start.elapsed())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let root = args
        .iter()
        .position(|a| a == "--dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .expect("usage: traversal_bench --dir <path>");

    println!("Traversal-only benchmark against {}\n", root.display());

    println!("== Full metadata pass (production-equivalent: symlink_metadata per entry) ==");
    for pass in 1..=3 {
        let (files, dirs, dur) = full_metadata_pass(&root);
        println!(
            "  pass {pass}: {files} files, {dirs} dirs, {:?} ({:.0} files/sec)",
            dur,
            files as f64 / dur.as_secs_f64().max(0.001)
        );
    }

    println!("\n== Metadata-light pass (readdir file-type only, no stat()) ==");
    for pass in 1..=3 {
        let (files, dirs, dur) = metadata_light_pass(&root);
        println!(
            "  pass {pass}: {files} files, {dirs} dirs, {:?} ({:.0} files/sec)",
            dur,
            files as f64 / dur.as_secs_f64().max(0.001)
        );
    }

    Ok(())
}
