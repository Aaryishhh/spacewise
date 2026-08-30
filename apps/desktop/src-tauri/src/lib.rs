// Tauri commands are the only bridge between the frontend and spacewise-core.
// They must stay thin: no scanning/classification/safety logic lives here,
// it all lives in spacewise-core per docs/ARCHITECTURE.md. Every command
// converts errors to String (Tauri's IPC requires Serialize) but the real
// error types with context live in spacewise-core.

use spacewise_core::adapter::PlatformAdapter;
use spacewise_core::aggregate::aggregate_directories;
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
use spacewise_core::scanner::{ScanOptions, ScanStats, Scanner};
use spacewise_core::uninstall::associate_apps_with_storage;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use uuid::Uuid;

struct AppState {
    // Arc, not a bare Mutex, so run_scan can clone a 'static handle into a
    // spawn_blocking task -- a plain State<'_, AppState> borrow can't cross
    // that boundary, and a scan running on the wrong thread is exactly what
    // made the whole window "Not Responding" during a large scan.
    db: Arc<Mutex<StorageDatabase>>,
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
}

/// Streams progress to the frontend as `scan-progress` events (files/dirs/
/// bytes seen so far) and still buffers everything for the aggregate step,
/// same as CollectingSink. Emitted at most every 250ms so a huge scan does
/// not flood IPC with an event per 1024-entry batch.
struct EmittingSink {
    app: tauri::AppHandle,
    entries: Vec<FileEntry>,
    last_emit: std::time::Instant,
}

#[derive(Clone, serde::Serialize)]
struct ScanProgressPayload {
    files_scanned: usize,
    dirs_scanned: usize,
    total_logical_size: u64,
}

impl spacewise_core::scanner::ScanProgressSink for EmittingSink {
    fn on_entries(&mut self, mut entries: Vec<FileEntry>) {
        self.entries.append(&mut entries);
        if self.last_emit.elapsed() > std::time::Duration::from_millis(250) {
            let files_scanned = self.entries.iter().filter(|e| !e.is_dir).count();
            let dirs_scanned = self.entries.len() - files_scanned;
            let total_logical_size = self.entries.iter().map(|e| e.logical_size).sum();
            let _ = self.app.emit(
                "scan-progress",
                ScanProgressPayload { files_scanned, dirs_scanned, total_logical_size },
            );
            self.last_emit = std::time::Instant::now();
        }
    }
    fn on_complete(&mut self, _stats: &ScanStats) {}
}

#[tauri::command]
async fn run_scan(app: tauri::AppHandle, state: tauri::State<'_, AppState>, root: String) -> Result<ScanSummary, String> {
    let db = state.db.clone();
    let root_path = PathBuf::from(&root);
    if !root_path.exists() {
        return Err(format!("path does not exist: {root}"));
    }

    // The scan (recursive walk, per-file stat, then potentially hundreds of
    // thousands of SQLite inserts) must never run on whatever thread pumps
    // the window's message loop -- spawn_blocking guarantees a dedicated
    // blocking-pool thread regardless of how the surrounding async runtime
    // is configured, which is what actually fixes "Not Responding" on a
    // large scan (this command being `async fn` alone is not sufficient).
    tauri::async_runtime::spawn_blocking(move || run_scan_blocking(app, db, root_path))
        .await
        .map_err(|e| e.to_string())?
}

fn run_scan_blocking(app: tauri::AppHandle, db: Arc<Mutex<StorageDatabase>>, root_path: PathBuf) -> Result<ScanSummary, String> {
    let mut db = db.lock().map_err(|e| e.to_string())?;
    let scan_id = db.start_scan(&root_path).map_err(|e| e.to_string())?;

    let mut sink = EmittingSink { app: app.clone(), entries: Vec::new(), last_emit: std::time::Instant::now() };
    let options = ScanOptions::with_scan_id(&root_path, scan_id);
    let stats = Scanner::new().scan(&options, &mut sink).map_err(|e| e.to_string())?;
    let entries = sink.entries;

    db.insert_entries(&entries).map_err(|e| e.to_string())?;

    let aggregates = aggregate_directories(&root_path, &entries);
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
    // candidate -- on a large scan that is minutes of I/O by itself.
    // Deferred to get_duplicates (computed lazily, once, on first visit to
    // the Duplicates page) so a plain "scan a folder" never pays that cost.

    let total_size = db
        .directory_aggregate(scan_id, &root_path)
        .map_err(|e| e.to_string())?
        .map(|a| a.total_size)
        .unwrap_or(stats.total_logical_size);

    HistoryEngine::new().record_snapshot(&db, scan_id, total_size).map_err(|e| e.to_string())?;

    let _ = app.emit("scan-complete", ());

    Ok(ScanSummary { scan_id: scan_id.to_string(), stats, total_size })
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

#[tauri::command]
async fn get_duplicates(state: tauri::State<'_, AppState>, scan_id: String) -> Result<Vec<DuplicateGroup>, String> {
    let scan_id = parse_scan_id(&scan_id)?;
    let db = state.db.clone();
    // Full-file-content hashing is expensive on a large scan -- computed
    // lazily here (once; persisted after) rather than eagerly in run_scan,
    // and still kept off the window/event-loop thread via spawn_blocking.
    tauri::async_runtime::spawn_blocking(move || {
        let mut db = db.lock().map_err(|e| e.to_string())?;
        let existing = db.duplicate_groups(scan_id).map_err(|e| e.to_string())?;
        if !existing.is_empty() {
            return Ok(existing);
        }
        let files = db.all_file_entries(scan_id).map_err(|e| e.to_string())?;
        let groups = DuplicateEngine::new().find_duplicates(&files);
        if !groups.is_empty() {
            db.insert_duplicate_groups(scan_id, &groups).map_err(|e| e.to_string())?;
        }
        Ok(groups)
    })
    .await
    .map_err(|e| e.to_string())?
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
            app.manage(AppState { db: Arc::new(Mutex::new(db)) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            run_scan,
            get_dashboard,
            get_directory_children,
            get_large_files,
            get_recommendations,
            get_duplicates,
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
