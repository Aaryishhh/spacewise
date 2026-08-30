//! Core storage data model shared by every engine stage.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: Uuid,
    pub scan_id: Uuid,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectoryAggregate {
    pub path: PathBuf,
    pub total_size: u64,
    pub allocated_size: u64,
    pub file_count: u64,
    pub dir_count: u64,
    pub latest_modified: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafetyLevel {
    Safe,
    Review,
    Advanced,
    NeverAutoDelete,
}

impl SafetyLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            SafetyLevel::Safe => "safe",
            SafetyLevel::Review => "review",
            SafetyLevel::Advanced => "advanced",
            SafetyLevel::NeverAutoDelete => "never_auto_delete",
        }
    }

    pub fn from_str_or_never(s: &str) -> Self {
        match s {
            "safe" => SafetyLevel::Safe,
            "review" => SafetyLevel::Review,
            "advanced" => SafetyLevel::Advanced,
            _ => SafetyLevel::NeverAutoDelete,
        }
    }
}

/// A knowledge-base entry describing one recognised storage location (spec
/// section 32: "the moat"). Static per category id, not per matched path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageCategoryDef {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub what_happens_if_removed: String,
    pub regeneratable: bool,
    pub reversible: bool,
    pub safety: SafetyLevel,
}

/// One scanned path (file or aggregated directory) after the classification
/// + safety stages have run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedPath {
    pub path: PathBuf,
    pub category_id: String,
    pub safety: SafetyLevel,
    pub size: u64,
    pub last_activity: Option<DateTime<Utc>>,
}

/// A group of classified paths the user can choose to clean up as a unit
/// (spec section 9: the cleanup basket holds these, never raw paths).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupCandidate {
    pub id: Uuid,
    pub category_id: String,
    pub display_name: String,
    pub paths: Vec<PathBuf>,
    pub total_size: u64,
    pub safety: SafetyLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub candidate: CleanupCandidate,
    pub score: f64,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryTotal {
    pub category_id: String,
    pub total_size: u64,
    pub item_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalSnapshot {
    pub id: Uuid,
    pub scan_id: Uuid,
    pub taken_at: DateTime<Utc>,
    pub total_size: u64,
    pub category_totals: Vec<CategoryTotal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub id: Uuid,
    pub size: u64,
    pub content_hash: String,
    pub paths: Vec<PathBuf>,
}

/// Outcome of a completed cleanup action, kept so HistoryEngine can offer
/// undo where the underlying mechanism (Trash/Recycle Bin) supports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupAction {
    pub id: Uuid,
    pub performed_at: DateTime<Utc>,
    pub category_id: String,
    pub paths: Vec<PathBuf>,
    pub bytes_freed: u64,
    pub undoable: bool,
}
