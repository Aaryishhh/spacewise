//! Controlled SQLite insertion experiments, isolated from filesystem
//! traversal entirely (synthetic in-memory FileEntry generation, no real
//! file I/O) so each variable -- batch size, journal mode, synchronous
//! mode, indexes -- can be changed one at a time cheaply and repeatably.
//!
//! Usage:
//!   cargo run --release -p spacewise-core --example db_bench -- --rows 200000 --batch 5000
//!   cargo run --release -p spacewise-core --example db_bench -- --rows 200000 --batch 5000 --no-indexes
//!   cargo run --release -p spacewise-core --example db_bench -- --rows 200000 --batch 5000 --journal delete
//!   cargo run --release -p spacewise-core --example db_bench -- --rows 200000 --batch 5000 --synchronous full

use chrono::Utc;
use rusqlite::{params, Connection};
use spacewise_core::model::FileEntry;
use std::path::PathBuf;
use std::time::Instant;
use uuid::Uuid;

const SCHEMA_TABLE: &str = "
CREATE TABLE file_entries (
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
";
// Matches the corrected schema (see crates/core/src/db.rs's index audit
// comment): idx_file_entries_scan and idx_file_entries_modified were
// removed (redundant / never actually usable by the query that motivated
// them); idx_file_entries_parent was added (file_children/treemap hot path
// had no supporting index before that audit).
const SCHEMA_INDEXES: &str = "
CREATE INDEX idx_file_entries_size ON file_entries(scan_id, is_dir, logical_size);
CREATE INDEX idx_file_entries_parent ON file_entries(scan_id, parent, is_dir);
";

fn gen_entries(n: usize) -> Vec<FileEntry> {
    let scan_id = Uuid::new_v4();
    // Realistic path lengths, not the old short "C:/synthetic/gN/fN.dat"
    // placeholders -- matching the actual generated-tree shape used by
    // scan_bench.rs's --production mode (temp-dir root + 3 nested levels),
    // since path/parent string length is what actually drives bytes/row,
    // and the old short synthetic paths understated real-world DB size.
    let root = r"C:\Users\aaryi\AppData\Local\Temp\spacewise-bench-1000000";
    (0..n)
        .map(|i| {
            let d1 = (i / 50) / 400;
            let d2 = ((i / 50) / 20) % 20;
            let d3 = (i / 50) % 20;
            let dir = format!(r"{root}\g{d1}\s{d2}\l{d3}");
            FileEntry {
                id: Uuid::new_v4(),
                scan_id,
                path: PathBuf::from(format!(r"{dir}\f{}.dat", i % 50)),
                parent: Some(PathBuf::from(dir)),
                logical_size: (i % 100_000) as u64,
                allocated_size: (i % 100_000) as u64,
                extension: Some("dat".to_string()),
                created_at: Some(Utc::now()),
                modified_at: Some(Utc::now()),
                accessed_at: Some(Utc::now()),
                is_dir: false,
                is_symlink: false,
                is_hardlink: false,
                is_hidden: false,
                is_system: false,
                // Matches production on Windows post-fix: unset (see
                // scanner.rs's compact_filesystem_id) rather than the old
                // 194-byte same_file::Handle Debug string.
                filesystem_id: None,
            }
        })
        .collect()
}

fn insert_batch(conn: &Connection, entries: &[FileEntry]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO file_entries (id, scan_id, path, parent, logical_size, allocated_size, extension, created_at, modified_at, accessed_at, is_dir, is_symlink, is_hardlink, is_hidden, is_system, filesystem_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        )?;
        for e in entries {
            stmt.execute(params![
                e.id.to_string(),
                e.scan_id.to_string(),
                e.path.to_string_lossy(),
                e.parent.as_ref().map(|p| p.to_string_lossy().to_string()),
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
    tx.commit()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let get_flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let has_flag = |name: &str| args.iter().any(|a| a == name);

    let total_rows: usize = get_flag("--rows").and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let batch_size: usize = get_flag("--batch").and_then(|s| s.parse().ok()).unwrap_or(1024);
    let journal = get_flag("--journal").unwrap_or_else(|| "wal".to_string());
    let synchronous = get_flag("--synchronous").unwrap_or_else(|| "normal".to_string());
    let with_indexes = !has_flag("--no-indexes");
    let report_window = (total_rows / 10).max(batch_size).max(1);

    let db_path = std::env::temp_dir().join(format!(
        "spacewise-dbbench-{total_rows}-{batch_size}-{journal}-{synchronous}-{}.db",
        if with_indexes { "idx" } else { "noidx" }
    ));
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));

    let conn = Connection::open(&db_path)?;
    conn.execute_batch(&format!(
        "PRAGMA journal_mode = {journal}; PRAGMA synchronous = {synchronous}; PRAGMA temp_store = MEMORY; PRAGMA cache_size = -32000;"
    ))?;
    conn.execute_batch(SCHEMA_TABLE)?;
    if with_indexes {
        conn.execute_batch(SCHEMA_INDEXES)?;
    }

    println!(
        "rows={total_rows} batch={batch_size} journal={journal} synchronous={synchronous} indexes={with_indexes}"
    );

    let entries = gen_entries(total_rows);
    let overall_start = Instant::now();
    let mut window_start = Instant::now();
    let mut inserted_since_window = 0usize;

    for (chunk_idx, chunk) in entries.chunks(batch_size).enumerate() {
        insert_batch(&conn, chunk)?;
        inserted_since_window += chunk.len();
        let total_inserted = (chunk_idx + 1) * batch_size.min(chunk.len().max(1));
        if inserted_since_window >= report_window {
            let rate = inserted_since_window as f64 / window_start.elapsed().as_secs_f64().max(0.001);
            println!("  [{:>8} rows]  window rate: {:>8.0} rows/sec", total_inserted.min(total_rows), rate);
            window_start = Instant::now();
            inserted_since_window = 0;
        }
    }
    let overall_duration = overall_start.elapsed();

    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").ok();
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    let freelist_count: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let db_bytes = page_count * page_size;
    let free_bytes = freelist_count * page_size;

    println!("\n== Result ==");
    println!("  total duration:   {:?}", overall_duration);
    println!("  overall rows/sec: {:.0}", total_rows as f64 / overall_duration.as_secs_f64());
    println!("  db file bytes:    {} ({:.1} MB)", db_bytes, db_bytes as f64 / 1_000_000.0);
    println!("  free/unused bytes:{} ({:.1} MB, {:.1}%)", free_bytes, free_bytes as f64 / 1_000_000.0, 100.0 * free_bytes as f64 / db_bytes.max(1) as f64);
    println!("  bytes/row:        {:.1}", db_bytes as f64 / total_rows as f64);

    drop(conn);
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    Ok(())
}
