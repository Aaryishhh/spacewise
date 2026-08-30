//! Embedded SQLite storage database (spec section 27): Scan, FileEntry,
//! DirectoryAggregate, HistoricalSnapshot, CleanupAction, DuplicateGroup.
//!
//! Every write path goes through a transaction; every path column is stored
//! as TEXT (UTF-8 lossy -- non-UTF-8 paths are a known, documented edge case
//! for a later phase) so queries can stay portable SQL instead of per-OS path
//! logic.

use crate::model::{
    CategoryTotal, DirectoryAggregate, DuplicateGroup, FileEntry, HistoricalSnapshot, SafetyLevel,
};
use crate::scanner::ScanStats;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS scans (
    id TEXT PRIMARY KEY,
    root TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    files_scanned INTEGER NOT NULL DEFAULT 0,
    dirs_scanned INTEGER NOT NULL DEFAULT 0,
    total_logical_size INTEGER NOT NULL DEFAULT 0,
    errors INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS file_entries (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL,
    path TEXT NOT NULL,
    parent TEXT,
    logical_size INTEGER NOT NULL,
    allocated_size INTEGER NOT NULL,
    extension TEXT,
    created_at TEXT,
    modified_at TEXT,
    accessed_at TEXT,
    is_dir INTEGER NOT NULL,
    is_symlink INTEGER NOT NULL,
    is_hardlink INTEGER NOT NULL,
    is_hidden INTEGER NOT NULL,
    is_system INTEGER NOT NULL,
    filesystem_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_file_entries_scan ON file_entries(scan_id);
CREATE INDEX IF NOT EXISTS idx_file_entries_size ON file_entries(scan_id, is_dir, logical_size);
CREATE INDEX IF NOT EXISTS idx_file_entries_modified ON file_entries(scan_id, is_dir, modified_at);

CREATE TABLE IF NOT EXISTS directory_aggregates (
    scan_id TEXT NOT NULL,
    path TEXT NOT NULL,
    parent TEXT,
    total_size INTEGER NOT NULL,
    allocated_size INTEGER NOT NULL,
    file_count INTEGER NOT NULL,
    dir_count INTEGER NOT NULL,
    latest_modified TEXT,
    category_id TEXT,
    safety TEXT,
    PRIMARY KEY (scan_id, path)
);
CREATE INDEX IF NOT EXISTS idx_dir_agg_parent ON directory_aggregates(scan_id, parent);
CREATE INDEX IF NOT EXISTS idx_dir_agg_category ON directory_aggregates(scan_id, category_id);

CREATE TABLE IF NOT EXISTS historical_snapshots (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL,
    taken_at TEXT NOT NULL,
    total_size INTEGER NOT NULL,
    category_totals TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cleanup_actions (
    id TEXT PRIMARY KEY,
    performed_at TEXT NOT NULL,
    category_id TEXT NOT NULL,
    paths TEXT NOT NULL,
    bytes_freed INTEGER NOT NULL,
    undoable INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS duplicate_groups (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL,
    size INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    paths TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_duplicate_groups_scan ON duplicate_groups(scan_id);
";

pub struct StorageDatabase {
    conn: Connection,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanMeta {
    pub id: Uuid,
    pub root: PathBuf,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl StorageDatabase {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    // -- scans ----------------------------------------------------------

    pub fn start_scan(&self, root: &Path) -> anyhow::Result<Uuid> {
        let id = Uuid::new_v4();
        self.conn.execute(
            "INSERT INTO scans (id, root, started_at) VALUES (?1, ?2, ?3)",
            params![id.to_string(), path_str(root), Utc::now().to_rfc3339()],
        )?;
        Ok(id)
    }

    pub fn finish_scan(&self, scan_id: Uuid, stats: &ScanStats) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE scans SET finished_at = ?1, files_scanned = ?2, dirs_scanned = ?3, total_logical_size = ?4, errors = ?5 WHERE id = ?6",
            params![
                Utc::now().to_rfc3339(),
                stats.files_scanned,
                stats.dirs_scanned,
                stats.total_logical_size,
                stats.errors,
                scan_id.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn get_scan(&self, scan_id: Uuid) -> anyhow::Result<Option<ScanMeta>> {
        let mut stmt = self.conn.prepare("SELECT id, root, started_at, finished_at FROM scans WHERE id = ?1")?;
        let mut rows = stmt.query_map(params![scan_id.to_string()], |row| {
            let id: String = row.get(0)?;
            let root: String = row.get(1)?;
            let started_at: String = row.get(2)?;
            let finished_at: Option<String> = row.get(3)?;
            Ok(ScanMeta {
                id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
                root: PathBuf::from(root),
                started_at: DateTime::parse_from_rfc3339(&started_at)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                finished_at: finished_at
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc)),
            })
        })?;
        rows.next().transpose().map_err(Into::into)
    }

    pub fn latest_scan_id(&self) -> anyhow::Result<Option<Uuid>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM scans WHERE finished_at IS NOT NULL ORDER BY started_at DESC LIMIT 1")?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            Ok(Uuid::parse_str(&id).ok())
        } else {
            Ok(None)
        }
    }

    // -- file entries -----------------------------------------------------

    pub fn insert_entries(&mut self, entries: &[FileEntry]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO file_entries (id, scan_id, path, parent, logical_size, allocated_size, extension, created_at, modified_at, accessed_at, is_dir, is_symlink, is_hardlink, is_hidden, is_system, filesystem_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            )?;
            for e in entries {
                stmt.execute(params![
                    e.id.to_string(),
                    e.scan_id.to_string(),
                    path_str(&e.path),
                    e.parent.as_deref().map(path_str),
                    e.logical_size,
                    e.allocated_size,
                    e.extension,
                    e.created_at.map(|d| d.to_rfc3339()),
                    e.modified_at.map(|d| d.to_rfc3339()),
                    e.accessed_at.map(|d| d.to_rfc3339()),
                    e.is_dir as i64,
                    e.is_symlink as i64,
                    e.is_hardlink as i64,
                    e.is_hidden as i64,
                    e.is_system as i64,
                    e.filesystem_id,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn large_files(
        &self,
        scan_id: Uuid,
        min_size: u64,
        older_than: Option<DateTime<Utc>>,
        limit: u32,
    ) -> anyhow::Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, scan_id, path, parent, logical_size, allocated_size, extension, created_at, modified_at, accessed_at, is_dir, is_symlink, is_hardlink, is_hidden, is_system, filesystem_id
             FROM file_entries
             WHERE scan_id = ?1 AND is_dir = 0 AND logical_size >= ?2
               AND (?3 IS NULL OR modified_at IS NULL OR modified_at <= ?3)
             ORDER BY logical_size DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                scan_id.to_string(),
                min_size,
                older_than.map(|d| d.to_rfc3339()),
                limit,
            ],
            row_to_file_entry,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Every file (non-directory) entry for a scan -- used to lazily compute
    /// duplicates on first visit to the Duplicates page, rather than paying
    /// full-file-content hashing cost on every scan up front.
    pub fn all_file_entries(&self, scan_id: Uuid) -> anyhow::Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, scan_id, path, parent, logical_size, allocated_size, extension, created_at, modified_at, accessed_at, is_dir, is_symlink, is_hardlink, is_hidden, is_system, filesystem_id
             FROM file_entries WHERE scan_id = ?1 AND is_dir = 0",
        )?;
        let rows = stmt.query_map(params![scan_id.to_string()], row_to_file_entry)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // -- directory aggregates ---------------------------------------------

    pub fn upsert_directory_aggregates(
        &mut self,
        scan_id: Uuid,
        aggregates: &HashMap<PathBuf, DirectoryAggregate>,
    ) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO directory_aggregates (scan_id, path, parent, total_size, allocated_size, file_count, dir_count, latest_modified)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(scan_id, path) DO UPDATE SET
                    parent=excluded.parent,
                    total_size=excluded.total_size,
                    allocated_size=excluded.allocated_size,
                    file_count=excluded.file_count,
                    dir_count=excluded.dir_count,
                    latest_modified=excluded.latest_modified",
            )?;
            for agg in aggregates.values() {
                stmt.execute(params![
                    scan_id.to_string(),
                    path_str(&agg.path),
                    agg.path.parent().map(path_str),
                    agg.total_size,
                    agg.allocated_size,
                    agg.file_count,
                    agg.dir_count,
                    agg.latest_modified.map(|d| d.to_rfc3339()),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_directory_category(
        &self,
        scan_id: Uuid,
        path: &Path,
        category_id: &str,
        safety: SafetyLevel,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE directory_aggregates SET category_id = ?1, safety = ?2 WHERE scan_id = ?3 AND path = ?4",
            params![category_id, safety.as_str(), scan_id.to_string(), path_str(path)],
        )?;
        Ok(())
    }

    pub fn directory_children(&self, scan_id: Uuid, parent: &Path) -> anyhow::Result<Vec<DirectoryAggregate>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, total_size, allocated_size, file_count, dir_count, latest_modified
             FROM directory_aggregates WHERE scan_id = ?1 AND parent = ?2
             ORDER BY total_size DESC",
        )?;
        let rows = stmt.query_map(params![scan_id.to_string(), path_str(parent)], row_to_aggregate)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Every aggregated directory for a scan -- used by app-association
    /// matching, which needs to search all AppData/Library paths, not just
    /// the subset the classification engine happened to recognise.
    pub fn all_aggregates(&self, scan_id: Uuid) -> anyhow::Result<Vec<DirectoryAggregate>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, total_size, allocated_size, file_count, dir_count, latest_modified
             FROM directory_aggregates WHERE scan_id = ?1",
        )?;
        let rows = stmt.query_map(params![scan_id.to_string()], row_to_aggregate)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn directory_aggregate(&self, scan_id: Uuid, path: &Path) -> anyhow::Result<Option<DirectoryAggregate>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, total_size, allocated_size, file_count, dir_count, latest_modified
             FROM directory_aggregates WHERE scan_id = ?1 AND path = ?2",
        )?;
        let mut rows = stmt.query_map(params![scan_id.to_string(), path_str(path)], row_to_aggregate)?;
        rows.next().transpose().map_err(Into::into)
    }

    pub fn all_categorized(&self, scan_id: Uuid) -> anyhow::Result<Vec<(DirectoryAggregate, String, SafetyLevel)>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, total_size, allocated_size, file_count, dir_count, latest_modified, category_id, safety
             FROM directory_aggregates WHERE scan_id = ?1 AND category_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![scan_id.to_string()], |row| {
            let agg = row_to_aggregate(row)?;
            let category_id: String = row.get(6)?;
            let safety: String = row.get(7)?;
            Ok((agg, category_id, SafetyLevel::from_str_or_never(&safety)))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn category_totals(&self, scan_id: Uuid) -> anyhow::Result<Vec<CategoryTotal>> {
        let mut stmt = self.conn.prepare(
            "SELECT category_id, SUM(total_size), COUNT(*) FROM directory_aggregates
             WHERE scan_id = ?1 AND category_id IS NOT NULL GROUP BY category_id ORDER BY 2 DESC",
        )?;
        let rows = stmt.query_map(params![scan_id.to_string()], |row| {
            Ok(CategoryTotal {
                category_id: row.get(0)?,
                total_size: row.get(1)?,
                item_count: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // -- duplicates ---------------------------------------------------------

    pub fn insert_duplicate_groups(&mut self, scan_id: Uuid, groups: &[DuplicateGroup]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO duplicate_groups (id, scan_id, size, content_hash, paths) VALUES (?1,?2,?3,?4,?5)",
            )?;
            for g in groups {
                let paths_json = serde_json::to_string(&g.paths)?;
                stmt.execute(params![g.id.to_string(), scan_id.to_string(), g.size, g.content_hash, paths_json])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn duplicate_groups(&self, scan_id: Uuid) -> anyhow::Result<Vec<DuplicateGroup>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, size, content_hash, paths FROM duplicate_groups WHERE scan_id = ?1 ORDER BY size DESC")?;
        let rows = stmt.query_map(params![scan_id.to_string()], |row| {
            let id: String = row.get(0)?;
            let paths_json: String = row.get(3)?;
            Ok(DuplicateGroup {
                id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
                size: row.get(1)?,
                content_hash: row.get(2)?,
                paths: serde_json::from_str(&paths_json).unwrap_or_default(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // -- cleanup actions (undo history) ------------------------------------

    pub fn record_cleanup_action(&self, action: &crate::model::CleanupAction) -> anyhow::Result<()> {
        let paths_json = serde_json::to_string(&action.paths)?;
        self.conn.execute(
            "INSERT INTO cleanup_actions (id, performed_at, category_id, paths, bytes_freed, undoable) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                action.id.to_string(),
                action.performed_at.to_rfc3339(),
                action.category_id,
                paths_json,
                action.bytes_freed,
                action.undoable as i64,
            ],
        )?;
        Ok(())
    }

    pub fn recent_cleanup_actions(&self, limit: u32) -> anyhow::Result<Vec<crate::model::CleanupAction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, performed_at, category_id, paths, bytes_freed, undoable FROM cleanup_actions ORDER BY performed_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            let id: String = row.get(0)?;
            let performed_at: String = row.get(1)?;
            let paths_json: String = row.get(3)?;
            Ok(crate::model::CleanupAction {
                id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
                performed_at: DateTime::parse_from_rfc3339(&performed_at)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                category_id: row.get(2)?,
                paths: serde_json::from_str(&paths_json).unwrap_or_default(),
                bytes_freed: row.get(4)?,
                undoable: row.get::<_, i64>(5)? != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // -- history --------------------------------------------------------

    pub fn save_snapshot(&self, snapshot: &HistoricalSnapshot) -> anyhow::Result<()> {
        let totals_json = serde_json::to_string(&snapshot.category_totals)?;
        self.conn.execute(
            "INSERT INTO historical_snapshots (id, scan_id, taken_at, total_size, category_totals) VALUES (?1,?2,?3,?4,?5)",
            params![
                snapshot.id.to_string(),
                snapshot.scan_id.to_string(),
                snapshot.taken_at.to_rfc3339(),
                snapshot.total_size,
                totals_json,
            ],
        )?;
        Ok(())
    }

    pub fn snapshots_since(&self, since: DateTime<Utc>) -> anyhow::Result<Vec<HistoricalSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, scan_id, taken_at, total_size, category_totals FROM historical_snapshots WHERE taken_at >= ?1 ORDER BY taken_at ASC",
        )?;
        let rows = stmt.query_map(params![since.to_rfc3339()], |row| {
            let id: String = row.get(0)?;
            let scan_id: String = row.get(1)?;
            let taken_at: String = row.get(2)?;
            let totals_json: String = row.get(4)?;
            Ok(HistoricalSnapshot {
                id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
                scan_id: Uuid::parse_str(&scan_id).unwrap_or_else(|_| Uuid::nil()),
                taken_at: DateTime::parse_from_rfc3339(&taken_at)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                total_size: row.get(3)?,
                category_totals: serde_json::from_str(&totals_json).unwrap_or_default(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn row_to_aggregate(row: &Row) -> rusqlite::Result<DirectoryAggregate> {
    let path: String = row.get(0)?;
    let latest_modified: Option<String> = row.get(5)?;
    Ok(DirectoryAggregate {
        path: PathBuf::from(path),
        total_size: row.get(1)?,
        allocated_size: row.get(2)?,
        file_count: row.get(3)?,
        dir_count: row.get(4)?,
        latest_modified: latest_modified
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
    })
}

fn row_to_file_entry(row: &Row) -> rusqlite::Result<FileEntry> {
    let id: String = row.get(0)?;
    let scan_id: String = row.get(1)?;
    let path: String = row.get(2)?;
    let parent: Option<String> = row.get(3)?;
    let created_at: Option<String> = row.get(7)?;
    let modified_at: Option<String> = row.get(8)?;
    let accessed_at: Option<String> = row.get(9)?;
    Ok(FileEntry {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
        scan_id: Uuid::parse_str(&scan_id).unwrap_or_else(|_| Uuid::nil()),
        path: PathBuf::from(path),
        parent: parent.map(PathBuf::from),
        logical_size: row.get(4)?,
        allocated_size: row.get(5)?,
        extension: row.get(6)?,
        created_at: created_at.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()).map(|d| d.with_timezone(&Utc)),
        modified_at: modified_at.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()).map(|d| d.with_timezone(&Utc)),
        accessed_at: accessed_at.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()).map(|d| d.with_timezone(&Utc)),
        is_dir: row.get::<_, i64>(10)? != 0,
        is_symlink: row.get::<_, i64>(11)? != 0,
        is_hardlink: row.get::<_, i64>(12)? != 0,
        is_hidden: row.get::<_, i64>(13)? != 0,
        is_system: row.get::<_, i64>(14)? != 0,
        filesystem_id: row.get(15)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{CollectingSink, ScanOptions, ScanProgressSink, Scanner};

    #[test]
    fn round_trips_a_real_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), vec![0u8; 42]).unwrap();

        let mut db = StorageDatabase::open_in_memory().unwrap();
        let scan_id = db.start_scan(dir.path()).unwrap();

        let mut sink = CollectingSink::default();
        let stats = Scanner::new()
            .scan(&ScanOptions::with_scan_id(dir.path(), scan_id), &mut sink)
            .unwrap();
        sink.on_complete(&stats);

        db.insert_entries(&sink.entries).unwrap();
        let aggregates = crate::aggregate::aggregate_directories(dir.path(), &sink.entries);
        db.upsert_directory_aggregates(scan_id, &aggregates).unwrap();
        db.finish_scan(scan_id, &stats).unwrap();

        let root_agg = db.directory_aggregate(scan_id, dir.path()).unwrap().unwrap();
        assert_eq!(root_agg.total_size, 42);

        let large = db.large_files(scan_id, 1, None, 10).unwrap();
        assert_eq!(large.len(), 1);
        assert_eq!(large[0].logical_size, 42);
    }
}
