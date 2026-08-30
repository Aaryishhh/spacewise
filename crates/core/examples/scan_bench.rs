//! Scanner/DB/aggregation benchmark harness.
//!
//! Two modes:
//!   --files N            Phased mode: traversal, DB insertion, and
//!                         aggregation measured as three separate phases
//!                         (useful for isolating which component is slow,
//!                         but its "in-memory entries" number reflects
//!                         CollectingSink -- NOT what production does).
//!   --files N --production
//!                         Production-path mode: mimics the real desktop
//!                         app's StreamingSink exactly -- per-batch buffered
//!                         DB insert (DB_INSERT_BATCH_SIZE=10000) and
//!                         incremental aggregate folding, no FileEntry
//!                         buffering. This is the number that matters for
//!                         "does memory actually stay bounded".
//!
//! Synthetic tree generation time is always reported separately from scan
//! timing -- never folded into a "total scan duration" figure.
//!
//! Duplicate hashing is intentionally not part of this benchmark.

use spacewise_core::aggregate::{aggregate_directories, fold_into};
use spacewise_core::db::StorageDatabase;
use spacewise_core::model::{DirectoryAggregate, FileEntry};
use spacewise_core::scanner::{CollectingSink, ScanOptions, ScanProgressSink, ScanStats, Scanner};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use sysinfo::{Pid, System};

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

const DB_INSERT_BATCH_SIZE: usize = 10_000;

/// Mirrors apps/desktop/src-tauri/src/lib.rs's StreamingSink exactly:
/// fold_into for aggregates (O(directories) memory), buffered batched DB
/// insert, no FileEntry accumulation.
struct ProductionSink<'a> {
    db: &'a mut StorageDatabase,
    root: PathBuf,
    aggregates: HashMap<PathBuf, DirectoryAggregate>,
    pending_for_db: Vec<FileEntry>,
    sys: System,
    pid: Pid,
    peak_mb: f64,
    first_batch_at: Option<Instant>,
    start: Instant,
}

impl<'a> ProductionSink<'a> {
    fn flush(&mut self) {
        if self.pending_for_db.is_empty() {
            return;
        }
        let _ = self.db.insert_entries(&self.pending_for_db);
        self.pending_for_db.clear();
    }
}

impl<'a> ScanProgressSink for ProductionSink<'a> {
    fn on_entries(&mut self, entries: Vec<FileEntry>) {
        if self.first_batch_at.is_none() {
            self.first_batch_at = Some(Instant::now());
        }
        fold_into(&mut self.aggregates, &self.root, &entries);
        self.pending_for_db.extend(entries);
        if self.pending_for_db.len() >= DB_INSERT_BATCH_SIZE {
            self.flush();
        }
        let mb = current_memory_mb(&mut self.sys, self.pid);
        if mb > self.peak_mb {
            self.peak_mb = mb;
        }
    }
    fn on_complete(&mut self, _stats: &ScanStats) {
        self.flush();
    }
}

fn run_production_mode(root: &Path) -> anyhow::Result<()> {
    let mut sys = System::new();
    let pid = Pid::from_u32(std::process::id());
    let start_mb = current_memory_mb(&mut sys, pid);

    let db_path = std::env::temp_dir().join("spacewise-bench-production.db");
    let _ = std::fs::remove_file(&db_path);
    let mut db = StorageDatabase::open(&db_path)?;

    let overall_start = Instant::now();
    let mut sink = ProductionSink {
        db: &mut db,
        root: root.to_path_buf(),
        aggregates: HashMap::new(),
        pending_for_db: Vec::with_capacity(DB_INSERT_BATCH_SIZE),
        sys,
        pid,
        peak_mb: start_mb,
        first_batch_at: None,
        start: overall_start,
    };
    let stats = Scanner::new().scan(&ScanOptions::new(root), &mut sink)?;
    let scan_plus_insert_duration = overall_start.elapsed();
    let time_to_first_batch_ms = sink.first_batch_at.map(|t| t.duration_since(sink.start).as_millis());
    let peak_mb = sink.peak_mb;
    let unique_dirs = sink.aggregates.len();

    // Aggregates are already folded incrementally throughout the scan (that
    // is the point being measured) -- no separate final-persist step is
    // needed for this benchmark's purpose.
    let end_mb = current_memory_mb(&mut System::new_all(), pid);
    let db_size_mb = std::fs::metadata(&db_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);

    println!("== Production-path mode (StreamingSink-equivalent) ==");
    println!("  files_scanned:              {}", stats.files_scanned);
    println!("  dirs_scanned:                {}", stats.dirs_scanned);
    println!("  unique directories folded:   {}", unique_dirs);
    println!("  scan + insert + fold duration: {:?}", scan_plus_insert_duration);
    println!("  files/sec (combined):        {:.0}", stats.files_scanned as f64 / scan_plus_insert_duration.as_secs_f64().max(0.001));
    println!("  time to first batch:         {:?} ms", time_to_first_batch_ms);
    println!("  RSS start:                   {:.1} MB", start_mb);
    println!("  RSS peak:                    {:.1} MB", peak_mb);
    println!("  RSS end (post-scan):         {:.1} MB", end_mb);
    println!("  db file size:                {:.1} MB", db_size_mb);

    drop(db);
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut target_files: Option<usize> = None;
    let mut existing_dir: Option<PathBuf> = None;
    let mut production_mode = false;
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
            "--production" => {
                production_mode = true;
                i += 1;
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
            println!("Generating synthetic tree of ~{files} files at {} (this generation time is NOT scan time)...", dir.display());
            let gen_start = Instant::now();
            generate_tree(&dir, files)?;
            println!("  generated in {:?}\n", gen_start.elapsed());
            (dir.clone(), Some(dir))
        }
    };

    if production_mode {
        run_production_mode(&root)?;
        if let Some(dir) = _tempdir_guard {
            let _ = std::fs::remove_dir_all(dir);
        }
        return Ok(());
    }

    let mut sys = System::new();
    let pid = Pid::from_u32(std::process::id());
    let mem_before = current_memory_mb(&mut sys, pid);

    println!("== Phase 1: filesystem traversal (no DB) ==");
    struct TimeToFirst {
        start: Instant,
        first_batch_ms: Option<u64>,
    }
    impl ScanProgressSink for TimeToFirst {
        fn on_entries(&mut self, _entries: Vec<FileEntry>) {
            if self.first_batch_ms.is_none() {
                self.first_batch_ms = Some(self.start.elapsed().as_millis() as u64);
            }
        }
        fn on_complete(&mut self, _stats: &ScanStats) {}
    }
    let mut ttf_sink = TimeToFirst { start: Instant::now(), first_batch_ms: None };
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

    println!("\n== Collecting entries for phases 2-3 (NOT what production does -- see --production mode) ==");
    let mut collecting = CollectingSink::default();
    Scanner::new().scan(&ScanOptions::new(&root), &mut collecting)?;
    let entries = collecting.entries;
    let mem_after_scan = current_memory_mb(&mut sys, pid);
    println!("  in-memory entries: {} (~{:.1} MB resident, delta from baseline: {:.1} MB)", entries.len(), mem_after_scan, mem_after_scan - mem_before);

    println!("\n== Phase 2: SQLite insertion (batched at {DB_INSERT_BATCH_SIZE}, matching production, {} entries) ==", entries.len());
    let db_path = std::env::temp_dir().join(format!("spacewise-bench-{}.db", entries.len()));
    let _ = std::fs::remove_file(&db_path);
    let mut db = StorageDatabase::open(&db_path)?;
    let insert_start = Instant::now();
    for chunk in entries.chunks(DB_INSERT_BATCH_SIZE) {
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
    println!("  peak resident memory observed (phased/CollectingSink mode): {:.1} MB (baseline {:.1} MB)", mem_peak, mem_before);
    println!("  db file size:                  {:.1} MB", db_size_mb);

    let _ = std::fs::remove_file(&db_path);
    if let Some(dir) = _tempdir_guard {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(())
}
