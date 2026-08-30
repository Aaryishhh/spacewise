//! Scanner/DB/aggregation benchmark harness (spec: measure filesystem
//! traversal, SQLite insertion, and aggregation independently, so we know
//! which component actually limits performance).
//!
//! Usage:
//!   cargo run --release -p spacewise-core --example scan_bench -- --files 100000
//!   cargo run --release -p spacewise-core --example scan_bench -- --dir "C:\some\real\folder"
//!
//! Duplicate hashing is intentionally not part of this benchmark (spec:
//! "Do NOT run duplicate hashing as part of these benchmarks").

use spacewise_core::aggregate::aggregate_directories;
use spacewise_core::db::StorageDatabase;
use spacewise_core::scanner::{CollectingSink, ScanOptions, ScanProgressSink, ScanStats, Scanner};
use std::path::{Path, PathBuf};
use std::time::Instant;
use sysinfo::{Pid, System};

struct TimeToFirstResult {
    start: Instant,
    first_batch_ms: Option<u64>,
}
impl ScanProgressSink for TimeToFirstResult {
    fn on_entries(&mut self, _entries: Vec<spacewise_core::model::FileEntry>) {
        if self.first_batch_ms.is_none() {
            self.first_batch_ms = Some(self.start.elapsed().as_millis() as u64);
        }
    }
    fn on_complete(&mut self, _stats: &ScanStats) {}
}

fn current_memory_mb(sys: &mut System, pid: Pid) -> f64 {
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]));
    sys.process(pid).map(|p| p.memory() as f64 / 1_000_000.0).unwrap_or(0.0)
}

/// Builds a synthetic tree of roughly `target_files` files spread across a
/// realistic directory shape (nested, not one flat directory of a million
/// files, which no real scan ever looks like).
fn generate_tree(root: &Path, target_files: usize) -> anyhow::Result<()> {
    let dirs_per_level = 20;
    let mut remaining = target_files;
    let mut dir_index = 0usize;
    std::fs::create_dir_all(root)?;
    while remaining > 0 {
        let d1 = dir_index / (dirs_per_level * dirs_per_level);
        let d2 = (dir_index / dirs_per_level) % dirs_per_level;
        let d3 = dir_index % dirs_per_level;
        let dir = root.join(format!("g{d1}")).join(format!("s{d2}")).join(format!("l{d3}"));
        std::fs::create_dir_all(&dir)?;
        let files_here = remaining.min(50);
        for i in 0..files_here {
            std::fs::write(dir.join(format!("f{i}.dat")), vec![0u8; 128])?;
        }
        remaining -= files_here;
        dir_index += 1;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut target_files: Option<usize> = None;
    let mut existing_dir: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--files" => {
                target_files = args.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "--dir" => {
                existing_dir = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            _ => i += 1,
        }
    }

    let (root, _tempdir_guard) = match existing_dir {
        Some(dir) => (dir, None),
        None => {
            let files = target_files.unwrap_or(10_000);
            let dir = std::env::temp_dir().join(format!("spacewise-bench-{files}"));
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            println!("Generating synthetic tree of ~{files} files at {}...", dir.display());
            let gen_start = Instant::now();
            generate_tree(&dir, files)?;
            println!("  generated in {:?}", gen_start.elapsed());
            (dir.clone(), Some(dir))
        }
    };

    let mut sys = System::new();
    let pid = Pid::from_u32(std::process::id());
    let mem_before = current_memory_mb(&mut sys, pid);

    println!("\n== Phase 1: filesystem traversal (no DB) ==");
    let mut ttf_sink = TimeToFirstResult { start: Instant::now(), first_batch_ms: None };
    let stats = Scanner::new().scan(&ScanOptions::new(&root), &mut ttf_sink)?;
    println!("  files_scanned:      {}", stats.files_scanned);
    println!("  dirs_scanned:       {}", stats.dirs_scanned);
    println!("  total_logical_size: {} bytes", stats.total_logical_size);
    println!("  skipped:            {}", stats.skipped_total);
    println!("  duration:           {} ms", stats.duration_ms);
    println!(
        "  files/sec:          {:.0}",
        stats.files_scanned as f64 / (stats.duration_ms as f64 / 1000.0).max(0.001)
    );
    println!("  time to first batch (proxy for time-to-first-useful-result): {:?} ms", ttf_sink.first_batch_ms);

    println!("\n== Collecting entries for phases 2-3 (not timed as part of them) ==");
    let mut collecting = CollectingSink::default();
    Scanner::new().scan(&ScanOptions::new(&root), &mut collecting)?;
    let entries = collecting.entries;
    let mem_after_scan = current_memory_mb(&mut sys, pid);
    println!("  in-memory entries: {} (~{:.1} MB resident, delta from baseline: {:.1} MB)", entries.len(), mem_after_scan, mem_after_scan - mem_before);

    println!("\n== Phase 2: SQLite insertion (batched, {} entries) ==", entries.len());
    let db_path = std::env::temp_dir().join(format!("spacewise-bench-{}.db", entries.len()));
    let _ = std::fs::remove_file(&db_path);
    let mut db = StorageDatabase::open(&db_path)?;
    let insert_start = Instant::now();
    for chunk in entries.chunks(1024) {
        db.insert_entries(chunk)?;
    }
    let insert_duration = insert_start.elapsed();
    let db_size_mb = std::fs::metadata(&db_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);
    println!("  duration:   {:?}", insert_duration);
    println!("  inserts/sec: {:.0}", entries.len() as f64 / insert_duration.as_secs_f64().max(0.001));
    println!("  db file size: {:.1} MB", db_size_mb);

    println!("\n== Phase 3: directory aggregation (in-memory rollup) ==");
    let agg_start = Instant::now();
    let aggregates = aggregate_directories(&root, &entries);
    let agg_duration = agg_start.elapsed();
    println!("  duration:        {:?}", agg_duration);
    println!("  unique directories: {}", aggregates.len());

    let mem_peak = current_memory_mb(&mut sys, pid).max(mem_after_scan);
    println!("\n== Summary ==");
    println!("  peak resident memory observed: {:.1} MB (baseline {:.1} MB)", mem_peak, mem_before);
    println!("  db file size:                  {:.1} MB", db_size_mb);

    let _ = std::fs::remove_file(&db_path);
    if let Some(dir) = _tempdir_guard {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(())
}
