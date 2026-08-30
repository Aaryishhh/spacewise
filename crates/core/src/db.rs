//! Embedded SQLite storage database (Scan, Volume, FileEntry, DirectoryAggregate,
//! StorageCategory, CleanupCandidate, CleanupAction, ApplicationAssociation,
//! DuplicateGroup, HistoricalSnapshot, Recommendation). Phase 3.

pub struct StorageDatabase {
    #[allow(dead_code)]
    conn: rusqlite::Connection,
}

impl StorageDatabase {
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open_in_memory()?;
        Ok(Self { conn })
    }
}
