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

/// Mirrors apps/desktop/src-tauri/src/lib.rs's ChannelSink + run_writer
/// pipeline exactly: scanner thread sends batches over a bounded channel,
/// a dedicated writer thread folds aggregates and does buffered batched DB
/// inserts. This replaces the earlier single-thread ProductionSink, which
/// (correctly) measured that the old inline design serialized traversal
/// behind DB writes -- this benchmark now measures the fixed pipeline.
use std::sync::mpsc::sync_channel;

struct ChannelSink {
    tx: std::sync::mpsc::SyncSender<Vec<FileEntry>>,
    send_wait: std::time::Duration,
}
impl ScanProgressSink for ChannelSink {
    fn on_entries(&mut self, entries: Vec<FileEntry>) {
        let t0 = Instant::now();
        let _ = self.tx.send(entries);
        self.send_wait += t0.elapsed();
    }
    fn on_complete(&mut self, _stats: &ScanStats) {}
}

fn run_production_mode(root: &Path, db_batch_size: usize, channel_capacity: usize) -> anyhow::Result<()> {
    let mut sys = System::new();
    let pid = Pid::from_u32(std::process::id());
    let start_mb = current_memory_mb(&mut sys, pid);

    let db_path = std::env::temp_dir().join(format!("spacewise-bench-production-{db_batch_size}.db"));
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    let db = std::sync::Arc::new(std::sync::Mutex::new(StorageDatabase::open(&db_path)?));

    let (tx, rx) = sync_channel::<Vec<FileEntry>>(channel_capacity);
    let overall_start = Instant::now();

    let writer_db = db.clone();
    let writer_root = root.to_path_buf();
    let writer_handle = std::thread::spawn(move || {
        let mut aggregates: HashMap<PathBuf, DirectoryAggregate> = HashMap::new();
        let mut pending: Vec<FileEntry> = Vec::with_capacity(db_batch_size);
        let mut sys = System::new();
        let pid = Pid::from_u32(std::process::id());
        let mut peak_mb: f64 = 0.0;
        let mut first_batch_at: Option<Instant> = None;
        let mut insert_time = std::time::Duration::ZERO;
        let mut fold_time = std::time::Duration::ZERO;

        for batch in rx {
            if first_batch_at.is_none() {
                first_batch_at = Some(Instant::now());
            }
            let t0 = Instant::now();
            fold_into(&mut aggregates, &writer_root, &batch);
            fold_time += t0.elapsed();

            pending.extend(batch);
            if pending.len() >= db_batch_size {
                let t1 = Instant::now();
                if let Ok(mut db) = writer_db.lock() {
                    let _ = db.insert_entries(&pending);
                }
                insert_time += t1.elapsed();
                pending.clear();
            }
            let mb = current_memory_mb(&mut sys, pid);
            if mb > peak_mb {
                peak_mb = mb;
            }
        }
        if !pending.is_empty() {
            let t1 = Instant::now();
            if let Ok(mut db) = writer_db.lock() {
                let _ = db.insert_entries(&pending);
            }
            insert_time += t1.elapsed();
        }
        (aggregates, peak_mb, first_batch_at, insert_time, fold_time)
    });

    let mut sink = ChannelSink { tx, send_wait: std::time::Duration::ZERO };
    let stats = Scanner::new().scan(&ScanOptions::new(root), &mut sink)?;
    let scanner_duration = overall_start.elapsed();
    let send_wait = sink.send_wait;
    drop(sink);

    let (aggregates, peak_mb, first_batch_at, insert_time, fold_time) =
        writer_handle.join().map_err(|_| anyhow::anyhow!("writer thread panicked"))?;
    let total_duration = overall_start.elapsed();
    let time_to_first_batch_ms = first_batch_at.map(|t| t.duration_since(overall_start).as_millis());
    let unique_dirs = aggregates.len();

    let end_mb = current_memory_mb(&mut System::new_all(), pid);
    let db_size_before_checkpoint_mb = std::fs::metadata(&db_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);
    let wal_size_before_checkpoint_mb =
        std::fs::metadata(db_path.with_extension("db-wal")).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);

    // Explicit checkpoint + report before/after, per the 870MB investigation:
    // does the on-disk footprint reflect retained data, or un-checkpointed WAL?
    {
        let db = db.lock().unwrap();
        db.checkpoint_wal().ok();
    }
    let db_size_after_checkpoint_mb = std::fs::metadata(&db_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);
    let wal_size_after_checkpoint_mb =
        std::fs::metadata(db_path.with_extension("db-wal")).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);

    println!("== Production-path mode (real pipeline: scanner thread -> bounded channel -> writer thread) ==");
    println!("  db_batch_size:               {db_batch_size}");
    println!("  channel_capacity (batches):  {channel_capacity}");
    println!("  files_scanned:               {}", stats.files_scanned);
    println!("  dirs_scanned:                {}", stats.dirs_scanned);
    println!("  unique directories folded:   {unique_dirs}");
    println!("  scanner thread wall time:    {:?}", scanner_duration);
    println!("  scanner time blocked on send (backpressure): {:?}", send_wait);
    println!("  writer fold time:            {:?}", fold_time);
    println!("  writer db insert time:       {:?}", insert_time);
    println!("  total wall time (scan+write): {:?}", total_duration);
    println!("  files/sec (combined):        {:.0}", stats.files_scanned as f64 / total_duration.as_secs_f64().max(0.001));
    println!("  time to first batch:         {:?} ms", time_to_first_batch_ms);
    println!("  RSS start:                   {:.1} MB", start_mb);
    println!("  RSS peak:                    {:.1} MB", peak_mb);
    println!("  RSS end (post-scan):         {:.1} MB", end_mb);
    println!("  db file size before checkpoint: {:.1} MB", db_size_before_checkpoint_mb);
    println!("  wal file size before checkpoint: {:.1} MB", wal_size_before_checkpoint_mb);
    println!("  db file size after checkpoint:  {:.1} MB", db_size_after_checkpoint_mb);
    println!("  wal file size after checkpoint: {:.1} MB", wal_size_after_checkpoint_mb);

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
    let mut db_batch_size: usize = 10_000;
    let mut channel_capacity: usize = 8;
    let mut keep_tree = false;
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
            "--db-batch" => {
                db_batch_size = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
                i += 2;
            }
            "--channel-capacity" => {
                channel_capacity = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(8);
                i += 2;
            }
            "--production" => {
                production_mode = true;
                i += 1;
            }
            "--keep-tree" => {
                keep_tree = true;
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
        run_production_mode(&root, db_batch_size, channel_capacity)?;
        if let Some(dir) = _tempdir_guard {
            if !keep_tree {
                let _ = std::fs::remove_dir_all(dir);
            } else {
                println!("(kept synthetic tree at {} -- pass --dir to reuse it, remove manually when done)", dir.display());
            }
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
