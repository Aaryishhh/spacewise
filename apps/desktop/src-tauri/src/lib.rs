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

fn emit_status(app: &tauri::AppHandle, status: ScanStatus, scan_id: Option<Uuid>) {
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
}

/// Streams a running scan straight into SQLite one batch at a time (one
/// transaction per batch, never one giant transaction for the whole scan)
/// and folds each batch into a rolling directory-aggregate map instead of
/// buffering FileEntry records -- memory stays O(unique directories seen),
/// not O(total files), which is what actually keeps a multi-million-file
/// scan's memory bounded. Periodically persists the partial aggregate map
/// and emits events so the UI shows real results within seconds, not only
/// after the entire tree finishes.
struct StreamingSink {
    app: tauri::AppHandle,
    db: Arc<Mutex<StorageDatabase>>,
    scan_id: Uuid,
    root: PathBuf,
    aggregates: HashMap<PathBuf, DirectoryAggregate>,
    start: Instant,
    last_progress_emit: Instant,
    last_dashboard_persist: Instant,
    current_path: Option<PathBuf>,
    time_to_first_result_ms: Option<u64>,
}

impl StreamingSink {
    fn new(app: tauri::AppHandle, db: Arc<Mutex<StorageDatabase>>, scan_id: Uuid, root: PathBuf) -> Self {
        let now = Instant::now();
        Self {
            app,
            db,
            scan_id,
            root,
            aggregates: HashMap::new(),
            start: now,
            last_progress_emit: now,
            last_dashboard_persist: now,
            current_path: None,
            time_to_first_result_ms: None,
        }
    }
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

impl ScanProgressSink for StreamingSink {
    fn on_entries(&mut self, entries: Vec<FileEntry>) {
        if let Some(last) = entries.last() {
            self.current_path = last.parent.clone().or_else(|| Some(last.path.clone()));
        }

        // One transaction per batch (StorageDatabase::insert_entries wraps
        // its own call in a transaction) -- never one transaction for the
        // whole scan, and never held open longer than a single batch.
        if let Ok(mut db) = self.db.lock() {
            let _ = db.insert_entries(&entries);
        }

        fold_into(&mut self.aggregates, &self.root, &entries);

        if self.last_dashboard_persist.elapsed() > Duration::from_millis(1500) {
            if let Ok(mut db) = self.db.lock() {
                let _ = db.upsert_directory_aggregates(self.scan_id, &self.aggregates);
            }
            if self.time_to_first_result_ms.is_none() {
                self.time_to_first_result_ms = Some(self.start.elapsed().as_millis() as u64);
            }
            let _ = self.app.emit("dashboard-updated", serde_json::json!({ "scan_id": self.scan_id.to_string() }));
            self.last_dashboard_persist = Instant::now();
        }
    }

    fn on_progress(&mut self, stats: &ScanStats) {
        if self.last_progress_emit.elapsed() <= Duration::from_millis(250) {
            return;
        }
        let elapsed = self.start.elapsed();
        let elapsed_secs = elapsed.as_secs_f64().max(0.001);
        let _ = self.app.emit(
            "scan-progress",
            ScanProgressPayload {
                files_scanned: stats.files_scanned,
                dirs_scanned: stats.dirs_scanned,
                total_logical_size: stats.total_logical_size,
                skipped_total: stats.skipped_total,
                current_path: self.current_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                elapsed_ms: elapsed.as_millis() as u64,
                files_per_sec: stats.files_scanned as f64 / elapsed_secs,
                mb_per_sec: (stats.total_logical_size as f64 / 1_000_000.0) / elapsed_secs,
            },
        );
        self.last_progress_emit = Instant::now();
    }

    fn on_complete(&mut self, _stats: &ScanStats) {}
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
    emit_status(&app, ScanStatus::Starting, None);

    let db = state.db.clone();
    let session = state.scan_session.clone();

    // The scan (recursive walk, per-file stat, then potentially hundreds of
    // thousands of SQLite inserts) must never run on whatever thread pumps
    // the window's message loop -- spawn_blocking guarantees a dedicated
    // blocking-pool thread regardless of how the surrounding async runtime
    // is configured, which is what actually fixes "Not Responding" on a
    // large scan (this command being `async fn` alone is not sufficient).
    let app_for_task = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_scan_blocking(app_for_task, db, session, root_path, cancel_token)
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

    result
}

#[tauri::command]
fn cancel_scan(state: tauri::State<AppState>) -> Result<(), String> {
    let mut session = state.scan_session.lock().map_err(|e| e.to_string())?;
    if let Some(token) = &session.cancel_token {
        token.cancel();
        session.status = Some(ScanStatus::Cancelling);
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

fn run_scan_blocking(
    app: tauri::AppHandle,
    db: Arc<Mutex<StorageDatabase>>,
    session: Arc<Mutex<ScanSession>>,
    root_path: PathBuf,
    cancel_token: CancellationToken,
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

    let mut sink = StreamingSink::new(app.clone(), db.clone(), scan_id, root_path.clone());
    let options = ScanOptions::with_scan_id(&root_path, scan_id).with_cancel_token(cancel_token);
    let stats = Scanner::new().scan(&options, &mut sink).map_err(|e| e.to_string())?;
    let aggregates = sink.aggregates;

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

        Ok(ScanSummary { scan_id: scan_id.to_string(), stats, total_size, status })
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

#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
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
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            run_scan,
            cancel_scan,
            get_scan_status,
            get_dashboard,
            get_directory_children,
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
            reveal_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
