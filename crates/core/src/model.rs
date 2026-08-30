//! Core storage data model shared by every engine stage.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: uuid::Uuid,
    pub scan_id: uuid::Uuid,
    pub path: PathBuf,
    pub parent: Option<PathBuf>,
    pub logical_size: u64,
    pub allocated_size: u64,
    pub extension: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub accessed_at: Option<DateTime<Utc>>,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_hardlink: bool,
    pub is_hidden: bool,
    pub is_system: bool,
    pub filesystem_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryAggregate {
    pub path: PathBuf,
    pub total_size: u64,
    pub allocated_size: u64,
    pub file_count: u64,
    pub dir_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafetyLevel {
    Safe,
    Review,
    Advanced,
    NeverAutoDelete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageCategory {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub total_size: u64,
    pub safety: SafetyLevel,
}
