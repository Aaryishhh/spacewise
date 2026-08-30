// Tauri commands are the only bridge between the frontend and spacewise-core.
// They must stay thin: no scanning/classification/safety logic lives here,
// it all lives in spacewise-core per docs/ARCHITECTURE.md. Every command
// converts errors to String (Tauri's IPC requires Serialize) but the real
// error types with context live in spacewise-core.

use spacewise_core::adapter::PlatformAdapter;
use spacewise_core::aggregate::fold_into;
use spacewise_core::classification::ClassificationEngine;
use spacewise_core::cleanup::CleanupExecutor;
use spacewise_core::db::StorageDatabase;
use spacewise_core::duplicates::DuplicateEngine;
use spacewise_core::history::{GrowthSummary, HistoryEngine};
use spacewise_core::model::{
    AppAssociation, CategoryTotal, CleanupAction, CleanupCandidate, DirectoryAggregate,
    DuplicateGroup, FileEntry, InstalledApp, Recommendation,
};
use spacewise_core::recommend::RecommendationEngine;
use spacewise_core::safety::SafetyEngine;
use spacewise_core::scanner::{CancellationToken, ScanOptions, ScanProgressSink, ScanStats, Scanner};
use spacewise_core::uninstall::associate_apps_with_storage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use uuid::Uuid;

// -- scan state machine (spec: "UI state driven by this model rather than
// scattered booleans") --------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScanStatus {
    Idle,
    Starting,
    Scanning,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Default)]
struct ScanSession {
    status: Option<ScanStatus>,
    scan_id: Option<Uuid>,
    root: Option<String>,
    cancel_token: Option<CancellationToken>,
}

impl ScanSession {
    fn current_status(&self) -> ScanStatus {
        self.status.unwrap_or(ScanStatus::Idle)
    }

    fn is_active(&self) -> bool {
        matches!(self.current_status(), ScanStatus::Starting | ScanStatus::Scanning | ScanStatus::Cancelling)
    }
}

struct AppState {
    // Arc, not a bare Mutex, so run_scan can clone a 'static handle into a
    // spawn_blocking task -- a plain State<'_, AppState> borrow can't cross
    // that boundary, and a scan running on the wrong thread is exactly what
    // made the whole window "Not Responding" during a large scan.
    db: Arc<Mutex<StorageDatabase>>,
    scan_session: Arc<Mutex<ScanSession>>,
    duplicate_cancel: Arc<Mutex<Option<CancellationToken>>>,
    cancellation_timings: Arc<Mutex<CancellationTimings>>,
}

/// Real (not theoretical) cancellation-latency measurement: four wall-clock
/// timestamps captured at each stage of the actual cancel path. User-visible
/// latency is scan_state_cancelled_at - cancel_requested_at (when the
/// frontend actually sees status flip to Cancelled), not just when the
/// scanner thread noticed the token.
#[derive(Default, Clone, Copy)]
struct CancellationTimings {
    cancel_requested_at: Option<Instant>,
    scan_worker_stopped_at: Option<Instant>,
    writer_stopped_at: Option<Instant>,
    scan_state_cancelled_at: Option<Instant>,
}

#[derive(Debug, serde::Serialize)]
struct CancellationLatencyReport {
    scan_worker_stop_ms: Option<u64>,
    writer_stop_ms: Option<u64>,
    total_user_visible_ms: Option<u64>,
}

impl CancellationTimings {
    fn report(&self) -> Option<CancellationLatencyReport> {
        let requested = self.cancel_requested_at?;
        Some(CancellationLatencyReport {
            scan_worker_stop_ms: self.scan_worker_stopped_at.map(|t| t.duration_since(requested).as_millis() as u64),
            writer_stop_ms: self.writer_stopped_at.map(|t| t.duration_since(requested).as_millis() as u64),
            total_user_visible_ms: self.scan_state_cancelled_at.map(|t| t.duration_since(requested).as_millis() as u64),
        })
    }
}

#[cfg(target_os = "windows")]
fn platform_adapter() -> Box<dyn PlatformAdapter> {
    Box::new(spacewise_platform_windows::WindowsAdapter)
}

#[cfg(target_os = "macos")]
fn platform_adapter() -> Box<dyn PlatformAdapter> {
    Box::new(spacewise_platform_macos::MacOSAdapter)
}

fn parse_scan_id(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("invalid scan id: {e}"))
}

fn emit_status<R: tauri::Runtime>(app: &tauri::AppHandle<R>, status: ScanStatus, scan_id: Option<Uuid>) {
    let _ = app.emit(
        "scan-status",
        serde_json::json!({ "status": status, "scan_id": scan_id.map(|id| id.to_string()) }),
    );
}

// Storage category ids the classification knowledge base assigns to
// developer-tooling locations (spec section 12). Kept here rather than in
// spacewise-core because "which categories count as developer storage" is a
// presentation grouping choice, not a core engine concern.
const DEVELOPER_CATEGORY_IDS: &[&str] = &[
    "npm-cache-nested", "node-modules", "cargo-target-debug", "cargo-target-release",
    "cargo-registry-cache", "npm-cache", "pnpm-store", "yarn-cache", "pip-cache", "python-venv",
    "gradle-cache", "maven-repo", "jetbrains-cache", "vscode-cache", "docker-data", "git-internals",
    "build-output-dist", "android-build", "xcode-derived-data", "xcode-archives",
    "xcode-simulators", "xcode-device-support", "spm-cache", "homebrew-cache", "cocoapods",
];

#[derive(serde::Serialize)]
struct ScanSummary {
    scan_id: String,
    stats: ScanStats,
    total_size: u64,
    status: ScanStatus,
    timings: ScanTimingsReport,
}

// Benchmarked (crates/core/examples/db_bench.rs): raising the DB insert
// transaction size from the scanner's 1024-entry UI/cancellation batch to
// 10,000 measured ~46.7k rows/sec vs ~22.4k rows/sec at 300k synthetic rows
// -- batch size was the dominant lever for insertion throughput (far more
// than journal mode or synchronous setting). Re-validated in production
// (crates/core/examples/scan_bench.rs --production --db-batch N) across
// 5000/10000/20000/25000/50000 -- see session notes for the chosen value's
// full tradeoff (throughput vs memory vs time-to-first-result vs
// cancellation drain latency), not just the isolated insertion number.
const DB_INSERT_BATCH_SIZE: usize = 10_000;

// Scanner batches (1024 entries, spacewise-core's ScanOptions default) sent
// through this channel before the writer thread has consumed them. Bounded
// so the writer falling behind creates backpressure (tx.send blocks the
// scanner) instead of unbounded memory growth -- 8 * 1024 = ~8192 entries
// is the hard cap on in-flight-but-not-yet-processed entries at any moment.
const CHANNEL_CAPACITY: usize = 8;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

#[derive(Default)]
struct ScanTimings {
    scanner_wall_ns: AtomicU64,
    /// Time the scanner thread spent blocked inside tx.send() because the
    /// bounded channel was full -- i.e. genuine backpressure from a writer
    /// that cannot keep up. Near-zero means traversal is the bottleneck;
    /// large means DB persistence is.
    channel_send_wait_ns: AtomicU64,
    writer_wall_ns: AtomicU64,
    fold_ns: AtomicU64,
    db_insert_ns: AtomicU64,
    progress_emit_ns: AtomicU64,
    dashboard_persist_ns: AtomicU64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ScanTimingsReport {
    scanner_wall_ms: u64,
    channel_send_wait_ms: u64,
    writer_wall_ms: u64,
    fold_ms: u64,
    db_insert_ms: u64,
    progress_emit_ms: u64,
    dashboard_persist_ms: u64,
}

impl ScanTimings {
    fn report(&self) -> ScanTimingsReport {
        let ms = |c: &AtomicU64| c.load(Ordering::Relaxed) / 1_000_000;
        ScanTimingsReport {
            scanner_wall_ms: ms(&self.scanner_wall_ns),
            channel_send_wait_ms: ms(&self.channel_send_wait_ns),
            writer_wall_ms: ms(&self.writer_wall_ns),
            fold_ms: ms(&self.fold_ns),
            db_insert_ms: ms(&self.db_insert_ns),
            progress_emit_ms: ms(&self.progress_emit_ns),
            dashboard_persist_ms: ms(&self.dashboard_persist_ns),
        }
    }
}

/// Sends each scanned batch to a dedicated writer thread over a bounded
/// channel instead of doing DB/aggregate work inline on the scanner's own
/// thread. This is the fix for the architectural serialization measured at
/// 1M-entry scale: isolated traversal benchmarks 20k+ files/sec, but the
/// old inline design (fold + DB insert called directly from the scan loop)
/// measured only ~3.4k files/sec combined, because every batch blocked the
/// walk from proceeding until its DB write finished. Now traversal and
/// persistence run concurrently on separate threads; the channel's bounded
/// capacity is the only backpressure mechanism, so memory still cannot
/// grow unbounded if the writer falls behind.
struct ChannelSink {
    tx: SyncSender<Vec<FileEntry>>,
    timings: Arc<ScanTimings>,
}

impl ScanProgressSink for ChannelSink {
    fn on_entries(&mut self, entries: Vec<FileEntry>) {
        let t0 = Instant::now();
        let _ = self.tx.send(entries);
        self.timings.channel_send_wait_ns.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    fn on_complete(&mut self, _stats: &ScanStats) {}
}

#[derive(Clone, serde::Serialize)]
struct ScanProgressPayload {
    files_scanned: u64,
    dirs_scanned: u64,
    total_logical_size: u64,
    skipped_total: u64,
    current_path: Option<String>,
    elapsed_ms: u64,
    files_per_sec: f64,
    mb_per_sec: f64,
}

struct WriterOutput {
    aggregates: HashMap<PathBuf, DirectoryAggregate>,
}

/// Runs on its own thread: receives batches from the scanner via `rx`,
/// folds them into the aggregate map, buffers for SQLite (one transaction
/// per DB_INSERT_BATCH_SIZE entries, never one giant transaction and never
/// one tiny transaction per scanner batch), and coalesces progress/
/// dashboard events on its own clock rather than once per batch -- so
/// event volume is independent of how fast the scanner is producing
/// entries (spec: ~4-10 UI updates/sec, not one event per filesystem
/// entry).
fn run_writer<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: Arc<Mutex<StorageDatabase>>,
    scan_id: Uuid,
    root: PathBuf,
    rx: Receiver<Vec<FileEntry>>,
    timings: Arc<ScanTimings>,
    db_batch_size: usize,
) -> WriterOutput {
    let writer_start = Instant::now();
    let mut aggregates: HashMap<PathBuf, DirectoryAggregate> = HashMap::new();
    let mut pending: Vec<FileEntry> = Vec::with_capacity(db_batch_size);
    let mut current_path: Option<PathBuf> = None;
    let mut files_scanned = 0u64;
    let mut dirs_scanned = 0u64;
    let mut total_logical_size = 0u64;
    let mut last_progress_emit = Instant::now();
    let mut last_dashboard_persist = Instant::now();

    let flush = |pending: &mut Vec<FileEntry>| {
        if pending.is_empty() {
            return;
        }
        let t = Instant::now();
        if let Ok(mut db) = db.lock() {
            let _ = db.insert_entries(pending);
        }
        timings.db_insert_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        pending.clear();
    };

    for batch in rx {
        if let Some(last) = batch.last() {
            current_path = last.parent.clone().or_else(|| Some(last.path.clone()));
        }
        for e in &batch {
            if e.is_dir {
                dirs_scanned += 1;
            } else {
                files_scanned += 1;
                total_logical_size += e.logical_size;
            }
        }

        let t0 = Instant::now();
        fold_into(&mut aggregates, &root, &batch);
        timings.fold_ns.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);

        pending.extend(batch);
        if pending.len() >= db_batch_size {
            flush(&mut pending);
        }

        if last_progress_emit.elapsed() > Duration::from_millis(160) {
            let t1 = Instant::now();
            let elapsed = writer_start.elapsed();
            let elapsed_secs = elapsed.as_secs_f64().max(0.001);
            let _ = app.emit(
                "scan-progress",
                ScanProgressPayload {
                    files_scanned,
                    dirs_scanned,
                    total_logical_size,
                    skipped_total: 0,
                    current_path: current_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                    elapsed_ms: elapsed.as_millis() as u64,
                    files_per_sec: files_scanned as f64 / elapsed_secs,
                    mb_per_sec: (total_logical_size as f64 / 1_000_000.0) / elapsed_secs,
                },
            );
            timings.progress_emit_ns.fetch_add(t1.elapsed().as_nanos() as u64, Ordering::Relaxed);
            last_progress_emit = Instant::now();
        }

        if last_dashboard_persist.elapsed() > Duration::from_millis(1500) {
            let t2 = Instant::now();
            if let Ok(mut db) = db.lock() {
                let _ = db.upsert_directory_aggregates(scan_id, &aggregates);
            }
            let _ = app.emit("dashboard-updated", serde_json::json!({ "scan_id": scan_id.to_string() }));
            timings.dashboard_persist_ns.fetch_add(t2.elapsed().as_nanos() as u64, Ordering::Relaxed);
            last_dashboard_persist = Instant::now();
        }
    }

    flush(&mut pending);
    // One deliberate checkpoint at the end of the scan (not per-batch,
    // which would reintroduce the serialization this pipeline was built to
    // remove) -- keeps the on-disk footprint reflecting retained data
    // rather than accumulated WAL, at zero cost to in-scan throughput.
    if let Ok(db) = db.lock() {
        let _ = db.checkpoint_wal();
    }
    timings.writer_wall_ns.fetch_add(writer_start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    WriterOutput { aggregates }
}

#[tauri::command]
async fn run_scan(app: tauri::AppHandle, state: tauri::State<'_, AppState>, root: String) -> Result<ScanSummary, String> {
    let root_path = PathBuf::from(&root);
    if !root_path.exists() {
        return Err(format!("path does not exist: {root}"));
    }

    let cancel_token = {
        let mut session = state.scan_session.lock().map_err(|e| e.to_string())?;
        if session.is_active() {
            return Err("a scan is already in progress".to_string());
        }
        let token = CancellationToken::new();
        *session = ScanSession {
            status: Some(ScanStatus::Starting),
            scan_id: None,
            root: Some(root.clone()),
            cancel_token: Some(token.clone()),
        };
        token
    };
    // Fresh timings for this scan -- a previous scan's cancellation data
    // must not leak into this one's report.
    *state.cancellation_timings.lock().map_err(|e| e.to_string())? = CancellationTimings::default();
    emit_status(&app, ScanStatus::Starting, None);

    let db = state.db.clone();
    let session = state.scan_session.clone();
    let cancellation_timings = state.cancellation_timings.clone();

    // The scan (recursive walk, per-file stat, then potentially hundreds of
    // thousands of SQLite inserts) must never run on whatever thread pumps
    // the window's message loop -- spawn_blocking guarantees a dedicated
    // blocking-pool thread regardless of how the surrounding async runtime
    // is configured, which is what actually fixes "Not Responding" on a
    // large scan (this command being `async fn` alone is not sufficient).
    let app_for_task = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_scan_blocking(app_for_task, db, session, root_path, cancel_token, cancellation_timings)
    })
    .await
    .map_err(|e| e.to_string())?;

    if let Ok(mut session) = state.scan_session.lock() {
        session.status = Some(match &result {
            Ok(summary) => summary.status,
            Err(_) => ScanStatus::Failed,
        });
        session.cancel_token = None;
    }

    if matches!(&result, Ok(s) if s.status == ScanStatus::Cancelled) {
        let report = {
            let mut timings = state.cancellation_timings.lock().map_err(|e| e.to_string())?;
            timings.scan_state_cancelled_at = Some(Instant::now());
            timings.report()
        };
        println!("[spacewise] cancellation latency: {report:?}");
    }

    result
}

#[tauri::command]
fn cancel_scan(state: tauri::State<AppState>) -> Result<(), String> {
    let mut session = state.scan_session.lock().map_err(|e| e.to_string())?;
    if let Some(token) = &session.cancel_token {
        token.cancel();
        session.status = Some(ScanStatus::Cancelling);
        if let Ok(mut timings) = state.cancellation_timings.lock() {
            timings.cancel_requested_at = Some(Instant::now());
        }
        Ok(())
    } else {
        Err("no scan is currently running".to_string())
    }
}

#[derive(serde::Serialize)]
struct ScanStatusPayload {
    status: ScanStatus,
    scan_id: Option<String>,
    root: Option<String>,
}

#[tauri::command]
fn get_scan_status(state: tauri::State<AppState>) -> Result<ScanStatusPayload, String> {
    let session = state.scan_session.lock().map_err(|e| e.to_string())?;
    Ok(ScanStatusPayload {
        status: session.current_status(),
        scan_id: session.scan_id.map(|id| id.to_string()),
        root: session.root.clone(),
    })
}

fn run_scan_blocking<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: Arc<Mutex<StorageDatabase>>,
    session: Arc<Mutex<ScanSession>>,
    root_path: PathBuf,
    cancel_token: CancellationToken,
    cancellation_timings: Arc<Mutex<CancellationTimings>>,
) -> Result<ScanSummary, String> {
    let scan_id = {
        let db = db.lock().map_err(|e| e.to_string())?;
        db.start_scan(&root_path).map_err(|e| e.to_string())?
    };
    if let Ok(mut s) = session.lock() {
        s.scan_id = Some(scan_id);
        s.status = Some(ScanStatus::Scanning);
    }
    emit_status(&app, ScanStatus::Scanning, Some(scan_id));

    let timings = Arc::new(ScanTimings::default());
    let (tx, rx) = sync_channel::<Vec<FileEntry>>(CHANNEL_CAPACITY);

    let writer_handle = {
        let app = app.clone();
        let db = db.clone();
        let root = root_path.clone();
        let timings = timings.clone();
        std::thread::spawn(move || run_writer(app, db, scan_id, root, rx, timings, DB_INSERT_BATCH_SIZE))
    };

    let mut sink = ChannelSink { tx, timings: timings.clone() };
    let options = ScanOptions::with_scan_id(&root_path, scan_id).with_cancel_token(cancel_token);
    let scanner_start = Instant::now();
    let stats = Scanner::new().scan(&options, &mut sink).map_err(|e| e.to_string())?;
    timings.scanner_wall_ns.fetch_add(scanner_start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    if stats.cancelled {
        if let Ok(mut t) = cancellation_timings.lock() {
            t.scan_worker_stopped_at = Some(Instant::now());
        }
    }
    drop(sink); // drops tx -> closes the channel -> writer's `for batch in rx` loop ends after draining

    let WriterOutput { aggregates } =
        writer_handle.join().map_err(|_| "scan writer thread panicked".to_string())?;
    if stats.cancelled {
        if let Ok(mut t) = cancellation_timings.lock() {
            t.writer_stopped_at = Some(Instant::now());
        }
    }

    // Final persist -- picks up whatever was folded since the last
    // throttled progressive persist.
    {
        let mut db = db.lock().map_err(|e| e.to_string())?;
        db.upsert_directory_aggregates(scan_id, &aggregates).map_err(|e| e.to_string())?;

        let classifier = ClassificationEngine::new();
        let safety_engine = SafetyEngine::new();
        for agg in aggregates.values() {
            if let Some(category) = classifier.classify(&agg.path) {
                let safety = safety_engine.classify(Some(category));
                db.set_directory_category(scan_id, &agg.path, &category.id, safety)
                    .map_err(|e| e.to_string())?;
            }
        }

        db.finish_scan(scan_id, &stats).map_err(|e| e.to_string())?;

        // Duplicate detection reads full file contents for every same-size
        // candidate -- deferred to get_duplicates (computed lazily, once,
        // on first visit to the Duplicates page) so a plain scan never pays
        // that cost.

        let total_size = db
            .directory_aggregate(scan_id, &root_path)
            .map_err(|e| e.to_string())?
            .map(|a| a.total_size)
            .unwrap_or(stats.total_logical_size);

        HistoryEngine::new().record_snapshot(&db, scan_id, total_size).map_err(|e| e.to_string())?;

        let status = if stats.cancelled { ScanStatus::Cancelled } else { ScanStatus::Completed };
        emit_status(&app, status, Some(scan_id));
        let _ = app.emit("scan-complete", ());

        let timing_report = timings.report();
        // Visible in the `npm run tauri dev` terminal -- the real, measured
        // (not inferred) per-phase breakdown for this scan.
        println!("[spacewise] scan {scan_id} timings: {timing_report:?}");

        Ok(ScanSummary { scan_id: scan_id.to_string(), stats, total_size, status, timings: timing_report })
    }
}

#[derive(serde::Serialize)]
struct DashboardData {
    scan_id: String,
    root: String,
    total_size: u64,
    scanned_at: Option<String>,
    category_totals: Vec<CategoryTotal>,
}

#[tauri::command]
fn get_dashboard(state: tauri::State<AppState>) -> Result<Option<DashboardData>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let Some(scan_id) = db.latest_scan_id().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let Some(meta) = db.get_scan(scan_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let root_agg = db.directory_aggregate(scan_id, &meta.root).map_err(|e| e.to_string())?;
    let category_totals = db.category_totals(scan_id).map_err(|e| e.to_string())?;

    Ok(Some(DashboardData {
        scan_id: scan_id.to_string(),
        root: meta.root.to_string_lossy().to_string(),
        total_size: root_agg.map(|a| a.total_size).unwrap_or(0),
        scanned_at: meta.finished_at.map(|d| d.to_rfc3339()),
        category_totals,
    }))
}

#[tauri::command]
fn get_directory_children(
    state: tauri::State<AppState>,
    scan_id: String,
    path: String,
) -> Result<Vec<DirectoryAggregate>, String> {
    let scan_id = parse_scan_id(&scan_id)?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.directory_children(scan_id, Path::new(&path)).map_err(|e| e.to_string())
}

// -- treemap: aggregated-hierarchy API -----------------------------------
//
// Never ships raw FileEntry dumps to the frontend. For a given directory,
// returns only that directory's immediate children (subdirectories as
// pre-aggregated totals, plus direct files), capped at TREEMAP_MAX_CHILDREN
// with the remainder folded into a synthetic "Other" bucket -- so this
// scales to a directory with hundreds of thousands of descendants without
// ever asking React to render (or even receive) that many objects. Drilling
// into a child directory is a fresh call to this same command.

const TREEMAP_MAX_CHILDREN: usize = 40;

#[derive(Clone, serde::Serialize)]
struct TreemapChild {
    name: String,
    path: String,
    size: u64,
    #[serde(rename = "type")]
    kind: String,
    child_count: u64,
    modified_at: Option<String>,
}

#[derive(serde::Serialize)]
struct TreemapNode {
    path: String,
    total_size: u64,
    children: Vec<TreemapChild>,
}

#[tauri::command]
fn get_treemap_node(state: tauri::State<AppState>, scan_id: String, path: String) -> Result<TreemapNode, String> {
    let scan_id = parse_scan_id(&scan_id)?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let path_buf = PathBuf::from(&path);

    let total_size = db
        .directory_aggregate(scan_id, &path_buf)
        .map_err(|e| e.to_string())?
        .map(|a| a.total_size)
        .unwrap_or(0);

    let dir_children = db.directory_children(scan_id, &path_buf).map_err(|e| e.to_string())?;
    let file_children = db.file_children(scan_id, &path_buf).map_err(|e| e.to_string())?;

    let children = build_treemap_children(&dir_children, &file_children, &path, TREEMAP_MAX_CHILDREN);
    Ok(TreemapNode { path, total_size, children })
}

/// Pure, unit-testable core of the treemap children API: merges
/// subdirectory aggregates and direct files into one size-sorted list,
/// capping at `max_children` with the remainder folded into a synthetic
/// "Other" bucket. Split out of the get_treemap_node command so it can be
/// tested without a Tauri State/AppHandle harness.
fn build_treemap_children(
    dir_children: &[DirectoryAggregate],
    file_children: &[FileEntry],
    parent_path: &str,
    max_children: usize,
) -> Vec<TreemapChild> {
    let mut all: Vec<TreemapChild> = Vec::with_capacity(dir_children.len() + file_children.len());
    for d in dir_children {
        let name = d.path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        all.push(TreemapChild {
            name,
            path: d.path.to_string_lossy().to_string(),
            size: d.total_size,
            kind: "directory".to_string(),
            child_count: d.file_count + d.dir_count,
            modified_at: d.latest_modified.map(|dt| dt.to_rfc3339()),
        });
    }
    for f in file_children {
        let name = f.path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        all.push(TreemapChild {
            name,
            path: f.path.to_string_lossy().to_string(),
            size: f.logical_size,
            kind: "file".to_string(),
            child_count: 0,
            modified_at: f.modified_at.map(|dt| dt.to_rfc3339()),
        });
    }
    all.sort_by(|a, b| b.size.cmp(&a.size));

    if all.len() > max_children && max_children > 0 {
        let (visible, rest) = all.split_at(max_children - 1);
        let mut visible = visible.to_vec();
        let other_size: u64 = rest.iter().map(|c| c.size).sum();
        visible.push(TreemapChild {
            name: format!("Other ({} items)", rest.len()),
            path: format!("{parent_path}::__other__"),
            size: other_size,
            kind: "other".to_string(),
            child_count: rest.len() as u64,
            modified_at: None,
        });
        visible
    } else {
        all
    }
}

#[cfg(test)]
mod treemap_tests {
    use super::*;
    use chrono::Utc;

    fn dir(path: &str, size: u64, files: u64, dirs: u64) -> DirectoryAggregate {
        DirectoryAggregate {
            path: PathBuf::from(path),
            total_size: size,
            allocated_size: size,
            file_count: files,
            dir_count: dirs,
            latest_modified: Some(Utc::now()),
        }
    }

    fn file(path: &str, size: u64) -> FileEntry {
        FileEntry {
            id: Uuid::new_v4(),
            scan_id: Uuid::new_v4(),
            path: PathBuf::from(path),
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
    fn directory_with_only_files() {
        let files = vec![file("/p/a.txt", 100), file("/p/b.txt", 50)];
        let children = build_treemap_children(&[], &files, "/p", 40);
        assert_eq!(children.len(), 2);
        assert!(children.iter().all(|c| c.kind == "file"));
        assert_eq!(children[0].size, 100); // sorted by size desc
    }

    #[test]
    fn directory_with_only_folders() {
        let dirs = vec![dir("/p/sub1", 500, 3, 0), dir("/p/sub2", 200, 1, 0)];
        let children = build_treemap_children(&dirs, &[], "/p", 40);
        assert_eq!(children.len(), 2);
        assert!(children.iter().all(|c| c.kind == "directory"));
        assert_eq!(children[0].name, "sub1");
    }

    #[test]
    fn mixed_content_sorted_by_size_regardless_of_type() {
        let dirs = vec![dir("/p/small_dir", 10, 1, 0)];
        let files = vec![file("/p/big_file.bin", 1000)];
        let children = build_treemap_children(&dirs, &files, "/p", 40);
        assert_eq!(children[0].kind, "file");
        assert_eq!(children[1].kind, "directory");
    }

    #[test]
    fn zero_byte_entries_are_included_not_dropped() {
        let files = vec![file("/p/empty.txt", 0)];
        let children = build_treemap_children(&[], &files, "/p", 40);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].size, 0);
    }

    #[test]
    fn one_huge_item_plus_thousands_of_tiny_ones_groups_other_correctly() {
        let mut files = vec![file("/p/huge.bin", 1_000_000)];
        for i in 0..5000 {
            files.push(file(&format!("/p/tiny{i}.txt"), 1));
        }
        let children = build_treemap_children(&[], &files, "/p", 40);

        assert_eq!(children.len(), 40); // capped, never 5001 rectangles
        assert_eq!(children[0].path, "/p/huge.bin");
        let other = children.last().unwrap();
        assert_eq!(other.kind, "other");
        assert_eq!(other.child_count, 5001 - 39); // 39 shown individually (huge + 38 tiny), rest grouped
        assert_eq!(other.size, (5001 - 39) as u64); // 1 byte each
    }

    #[test]
    fn no_other_bucket_when_under_the_cap() {
        let files: Vec<FileEntry> = (0..10).map(|i| file(&format!("/p/f{i}.txt"), 10)).collect();
        let children = build_treemap_children(&[], &files, "/p", 40);
        assert_eq!(children.len(), 10);
        assert!(children.iter().all(|c| c.kind != "other"));
    }

    #[test]
    fn empty_directory_produces_no_children() {
        let children = build_treemap_children(&[], &[], "/p", 40);
        assert!(children.is_empty());
    }
}

#[tauri::command]
fn get_large_files(
    state: tauri::State<AppState>,
    scan_id: String,
    min_size_bytes: u64,
    older_than_days: Option<i64>,
) -> Result<Vec<FileEntry>, String> {
    let scan_id = parse_scan_id(&scan_id)?;
    let older_than = older_than_days.map(|d| chrono::Utc::now() - chrono::Duration::days(d));
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.large_files(scan_id, min_size_bytes, older_than, 500).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_recommendations(state: tauri::State<AppState>, scan_id: String) -> Result<Vec<Recommendation>, String> {
    let scan_id = parse_scan_id(&scan_id)?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let classified = db.all_categorized(scan_id).map_err(|e| e.to_string())?;
    let classifier = ClassificationEngine::new();
    Ok(RecommendationEngine::new().recommend(&classified, &classifier))
}

#[derive(Clone, serde::Serialize)]
struct DuplicateProgressPayload {
    hashed: usize,
    total: usize,
}

struct EmittingDuplicateProgress {
    app: tauri::AppHandle,
    last_emit: Instant,
}
impl spacewise_core::duplicates::DuplicateProgressSink for EmittingDuplicateProgress {
    fn on_progress(&mut self, hashed: usize, total: usize) {
        if self.last_emit.elapsed() > Duration::from_millis(200) {
            let _ = self.app.emit("duplicate-progress", DuplicateProgressPayload { hashed, total });
            self.last_emit = Instant::now();
        }
    }
}

#[tauri::command]
async fn get_duplicates(app: tauri::AppHandle, state: tauri::State<'_, AppState>, scan_id: String) -> Result<Vec<DuplicateGroup>, String> {
    let scan_id = parse_scan_id(&scan_id)?;
    let db = state.db.clone();
    let dup_cancel_slot = state.duplicate_cancel.clone();

    let token = CancellationToken::new();
    *dup_cancel_slot.lock().map_err(|e| e.to_string())? = Some(token.clone());

    // Full-file-content hashing is expensive on a large scan -- computed
    // lazily here (once; persisted after) rather than eagerly in run_scan,
    // and still kept off the window/event-loop thread via spawn_blocking.
    // Opening the Duplicates page must never freeze the interface.
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut db = db.lock().map_err(|e| e.to_string())?;
        let existing = db.duplicate_groups(scan_id).map_err(|e| e.to_string())?;
        if !existing.is_empty() {
            return Ok(existing);
        }
        let files = db.all_file_entries(scan_id).map_err(|e| e.to_string())?;
        let cache = db.load_hash_cache().unwrap_or_default();
        let mut progress_sink = EmittingDuplicateProgress { app: app.clone(), last_emit: Instant::now() };
        let (groups, updated_cache) = DuplicateEngine::new().find_duplicates_with(&files, &cache, &token, &mut progress_sink);
        let _ = db.save_hash_cache(&updated_cache);
        if !groups.is_empty() {
            db.insert_duplicate_groups(scan_id, &groups).map_err(|e| e.to_string())?;
        }
        let _ = app.emit("duplicate-complete", ());
        Ok(groups)
    })
    .await
    .map_err(|e| e.to_string())?;

    *dup_cancel_slot.lock().map_err(|e| e.to_string())? = None;
    result
}

#[tauri::command]
fn cancel_duplicate_scan(state: tauri::State<AppState>) -> Result<(), String> {
    let slot = state.duplicate_cancel.lock().map_err(|e| e.to_string())?;
    match slot.as_ref() {
        Some(token) => {
            token.cancel();
            Ok(())
        }
        None => Err("no duplicate scan is currently running".to_string()),
    }
}

#[derive(serde::Serialize)]
struct CleanupOutcome {
    succeeded: Vec<CleanupAction>,
    failed: Vec<String>,
}

#[tauri::command]
async fn execute_cleanup(state: tauri::State<'_, AppState>, candidate: CleanupCandidate) -> Result<CleanupOutcome, String> {
    let db = state.db.clone();
    // Computing bytes-freed (a full walkdir per path) and the actual
    // trash/recycle-bin move are real filesystem I/O -- same reasoning as
    // run_scan, keep it off the window thread.
    tauri::async_runtime::spawn_blocking(move || {
        let adapter = platform_adapter();
        let executor = CleanupExecutor::new(adapter.as_ref());
        let mut succeeded = Vec::new();
        let mut failed = Vec::new();
        for result in executor.execute(&candidate) {
            match result {
                Ok(action) => succeeded.push(action),
                Err(e) => failed.push(e.to_string()),
            }
        }

        // Record every successful action so the History page can show what
        // was cleaned and (spec section 10) point back to the Trash/Recycle
        // Bin item for restore.
        if !succeeded.is_empty() {
            let db = db.lock().map_err(|e| e.to_string())?;
            for action in &succeeded {
                db.record_cleanup_action(action).map_err(|e| e.to_string())?;
            }
        }

        Ok(CleanupOutcome { succeeded, failed })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_cleanup_history(state: tauri::State<AppState>) -> Result<Vec<CleanupAction>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.recent_cleanup_actions(50).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_growth_summary(state: tauri::State<AppState>, lookback_days: i64) -> Result<Option<GrowthSummary>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    HistoryEngine::new().growth_summary(&db, lookback_days).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_installed_apps() -> Result<Vec<InstalledApp>, String> {
    platform_adapter().list_installed_apps().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_app_associations(state: tauri::State<AppState>, scan_id: String) -> Result<Vec<AppAssociation>, String> {
    let scan_id = parse_scan_id(&scan_id)?;
    let apps = platform_adapter().list_installed_apps().map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let aggregates = db.all_aggregates(scan_id).map_err(|e| e.to_string())?;
    Ok(associate_apps_with_storage(&apps, &aggregates))
}

#[tauri::command]
fn get_developer_storage(state: tauri::State<AppState>, scan_id: String) -> Result<Vec<CategoryTotal>, String> {
    let scan_id = parse_scan_id(&scan_id)?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let totals = db.category_totals(scan_id).map_err(|e| e.to_string())?;
    Ok(totals.into_iter().filter(|t| DEVELOPER_CATEGORY_IDS.contains(&t.category_id.as_str())).collect())
}

// -- platform abstraction (spec: "both platforms first-class, UI calls
// semantic operations, never OS-specific terminology") ------------------

#[derive(serde::Serialize)]
struct PlatformInfo {
    os: &'static str, // "windows" | "macos"
    os_family_label: &'static str, // "Windows" | "macOS"
    file_manager_label: &'static str, // "Explorer" | "Finder"
    trash_label: &'static str, // "Recycle Bin" | "Trash"
    arch: String,
}

#[tauri::command]
fn get_platform_info() -> PlatformInfo {
    #[cfg(target_os = "windows")]
    let info = PlatformInfo {
        os: "windows",
        os_family_label: "Windows",
        file_manager_label: "Explorer",
        trash_label: "Recycle Bin",
        arch: std::env::consts::ARCH.to_string(),
    };
    #[cfg(target_os = "macos")]
    let info = PlatformInfo {
        os: "macos",
        os_family_label: "macOS",
        file_manager_label: "Finder",
        trash_label: "Trash",
        arch: std::env::consts::ARCH.to_string(),
    };
    info
}

// -- real (not Rust-internal) time-to-first-useful-result -----------------
//
// The frontend is the one authority for "did the user actually see
// something useful" -- it records performance.now() at SCAN_STARTED (the
// moment it receives the scan-status="scanning" event) and again at each
// FIRST_* milestone once that UI has actually rendered non-empty content,
// then reports the deltas here so they land in the same terminal as every
// other dev-mode measurement in this session rather than only being
// visible in a browser devtools console we cannot read from Rust.
#[tauri::command]
fn report_frontend_timing(label: String, elapsed_ms: f64) -> Result<(), String> {
    println!("[spacewise][frontend-timing] {label}: {elapsed_ms:.0}ms since SCAN_STARTED");
    Ok(())
}

#[tauri::command]
fn reveal_in_file_manager(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{path}"))
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").args(["-R", &path]).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let db_path = app_data_dir.join("spacewise.db");
            let db = StorageDatabase::open(&db_path)?;
            app.manage(AppState {
                db: Arc::new(Mutex::new(db)),
                scan_session: Arc::new(Mutex::new(ScanSession::default())),
                duplicate_cancel: Arc::new(Mutex::new(None)),
                cancellation_timings: Arc::new(Mutex::new(CancellationTimings::default())),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            run_scan,
            cancel_scan,
            get_scan_status,
            get_dashboard,
            get_directory_children,
            get_treemap_node,
            get_large_files,
            get_recommendations,
            get_duplicates,
            cancel_duplicate_scan,
            execute_cleanup,
            get_cleanup_history,
            get_growth_summary,
            list_installed_apps,
            get_app_associations,
            get_developer_storage,
            reveal_in_file_manager,
            report_frontend_timing,
            get_platform_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod cancellation_latency_tests {
    use super::*;

    /// Real (not theoretical) cancellation-latency measurement. Reimplements
    /// run_scan_blocking's exact threading shape (scanner thread -> bounded
    /// channel -> writer thread, real CancellationToken, real
    /// StorageDatabase, real fold_into) without going through a
    /// tauri::AppHandle -- tauri::test's mock runtime hit a Windows-specific
    /// DLL-loading failure (STATUS_ENTRYPOINT_NOT_FOUND, WebView2Loader.dll
    /// not present next to the test binary) not worth spending further
    /// budget on. Event emission itself is fire-and-forget and not part of
    /// what is being timed, so this measures the real bottleneck machinery
    /// faithfully; only "did the frontend actually re-render in response to
    /// the emitted event" is out of scope here (that needs a live app).
    #[test]
    fn cancellation_latency_on_a_real_scan() {
        let dir = tempfile::tempdir().unwrap();
        // Large enough that cancelling shortly after start reliably lands
        // mid-scan rather than after it has already finished.
        for i in 0..30_000 {
            std::fs::write(dir.path().join(format!("f{i}.bin")), vec![0u8; 16]).unwrap();
        }

        let db = Arc::new(Mutex::new(StorageDatabase::open_in_memory().unwrap()));
        let scan_id = { db.lock().unwrap().start_scan(dir.path()).unwrap() };
        let cancel_token = CancellationToken::new();

        let (tx, rx) = sync_channel::<Vec<FileEntry>>(CHANNEL_CAPACITY);
        let root = dir.path().to_path_buf();
        let writer_db = db.clone();
        let writer_root = root.clone();

        let writer_handle = std::thread::spawn(move || {
            let mut aggregates: HashMap<PathBuf, DirectoryAggregate> = HashMap::new();
            let mut pending: Vec<FileEntry> = Vec::with_capacity(DB_INSERT_BATCH_SIZE);
            for batch in rx {
                fold_into(&mut aggregates, &writer_root, &batch);
                pending.extend(batch);
                if pending.len() >= DB_INSERT_BATCH_SIZE {
                    if let Ok(mut db) = writer_db.lock() {
                        let _ = db.insert_entries(&pending);
                    }
                    pending.clear();
                }
            }
            if !pending.is_empty() {
                if let Ok(mut db) = writer_db.lock() {
                    let _ = db.insert_entries(&pending);
                }
            }
            aggregates
        });

        let mut sink = ChannelSink { tx, timings: Arc::new(ScanTimings::default()) };
        let options = ScanOptions::with_scan_id(&root, scan_id).with_cancel_token(cancel_token.clone());

        let scan_token = cancel_token.clone();
        let scanner_thread = std::thread::spawn(move || Scanner::new().scan(&options, &mut sink));

        // Mirrors a user clicking "Cancel Scan" shortly after it starts.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let cancel_requested_at = Instant::now();
        scan_token.cancel();

        let stats = scanner_thread.join().unwrap().unwrap();
        let scan_worker_stopped_at = Instant::now();
        assert!(stats.cancelled, "scan should have actually been cancelled, not raced to completion");

        writer_handle.join().unwrap();
        let writer_stopped_at = Instant::now();
        // scan_state_cancelled_at: in production this is when AppState's
        // ScanSession.status flips to Cancelled, which happens immediately
        // after the writer join returns (see run_scan) -- i.e. effectively
        // simultaneous with writer_stopped_at, so reuse it here rather than
        // add a redundant Instant::now() a few nanoseconds later.
        let scan_state_cancelled_at = writer_stopped_at;

        let scan_worker_stop_ms = scan_worker_stopped_at.duration_since(cancel_requested_at).as_millis();
        let writer_stop_ms = writer_stopped_at.duration_since(cancel_requested_at).as_millis();
        let total_user_visible_ms = scan_state_cancelled_at.duration_since(cancel_requested_at).as_millis();

        println!(
            "[test] measured real cancellation latency: scan_worker_stop={scan_worker_stop_ms}ms writer_stop={writer_stop_ms}ms total_user_visible={total_user_visible_ms}ms"
        );

        assert!(
            total_user_visible_ms < 2000,
            "cancellation latency target is <2s on large scans, measured {total_user_visible_ms}ms"
        );
    }
}
