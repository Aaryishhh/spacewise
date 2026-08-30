//! CleanupPlanner and CleanupExecutor (spec sections 9, 10, 28).
//!
//! Required flow: Recommendation -> CleanupPlan -> USER APPROVAL -> Executor.
//! A recommendation must never call the executor directly -- there is no path
//! from `recommend.rs` to this module's `execute`; only explicit UI-driven
//! candidate selection builds a plan, and only an explicit `execute` call
//! (one per user click) performs a deletion.
//!
//! Every path is revalidated immediately before deletion: re-canonicalized,
//! re-checked against the platform's protected-root allowlist, and confirmed
//! to still exist, because a scan can be minutes old by the time the user
//! approves the plan.

use crate::adapter::PlatformAdapter;
use crate::model::{CleanupAction, CleanupCandidate, SafetyLevel};
use chrono::Utc;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub struct CleanupPlan {
    pub candidates: Vec<CleanupCandidate>,
}

impl CleanupPlan {
    pub fn total_size(&self) -> u64 {
        self.candidates.iter().map(|c| c.total_size).sum()
    }
}

pub struct CleanupPlanner;

impl CleanupPlanner {
    pub fn new() -> Self {
        Self
    }

    /// Builds a plan from candidates the user has already selected in the
    /// cleanup basket UI. Never executes anything.
    pub fn plan(&self, selected: Vec<CleanupCandidate>) -> CleanupPlan {
        CleanupPlan { candidates: selected }
    }
}

impl Default for CleanupPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error("refusing to delete protected system path: {0}")]
    ProtectedPath(PathBuf),
    #[error("refusing to auto-delete a NeverAutoDelete item: {0}")]
    NeverAutoDelete(PathBuf),
    #[error("path no longer exists -- scan is stale: {0}")]
    StaleTarget(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("trash error: {0}")]
    Trash(String),
}

pub struct CleanupExecutor<'a> {
    adapter: &'a dyn PlatformAdapter,
}

impl<'a> CleanupExecutor<'a> {
    pub fn new(adapter: &'a dyn PlatformAdapter) -> Self {
        Self { adapter }
    }

    pub fn execute(&self, candidate: &CleanupCandidate) -> Vec<Result<CleanupAction, CleanupError>> {
        candidate.paths.iter().map(|path| self.execute_one(candidate, path)).collect()
    }

    fn execute_one(&self, candidate: &CleanupCandidate, path: &Path) -> Result<CleanupAction, CleanupError> {
        if candidate.safety == SafetyLevel::NeverAutoDelete {
            return Err(CleanupError::NeverAutoDelete(path.to_path_buf()));
        }
        if !path.exists() {
            return Err(CleanupError::StaleTarget(path.to_path_buf()));
        }
        // canonicalize() resolves symlinks and `..` traversal, so a crafted
        // or stale path can not point somewhere it looked like it did not.
        let canonical = path.canonicalize().map_err(CleanupError::Io)?;
        if self.adapter.is_protected_root(&canonical) {
            return Err(CleanupError::ProtectedPath(canonical));
        }

        let bytes_freed = path_size(&canonical);
        self.adapter
            .move_to_trash(&canonical)
            .map_err(|e| CleanupError::Trash(e.to_string()))?;

        Ok(CleanupAction {
            id: Uuid::new_v4(),
            performed_at: Utc::now(),
            category_id: candidate.category_id.clone(),
            paths: vec![canonical],
            bytes_freed,
            undoable: true, // Trash/Recycle Bin restore is available (spec section 10)
        })
    }
}

fn path_size(path: &Path) -> u64 {
    if path.is_file() {
        return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct FakeAdapter {
        protected: Vec<PathBuf>,
    }
    impl PlatformAdapter for FakeAdapter {
        fn enrich_metadata(&self, _entry: &mut crate::model::FileEntry) -> anyhow::Result<()> {
            Ok(())
        }
        fn move_to_trash(&self, _path: &Path) -> anyhow::Result<()> {
            Ok(())
        }
        fn is_protected_root(&self, path: &Path) -> bool {
            self.protected.iter().any(|p| path.starts_with(p))
        }
        fn list_installed_apps(&self) -> anyhow::Result<Vec<crate::model::InstalledApp>> {
            Ok(Vec::new())
        }
    }

    fn candidate(paths: Vec<PathBuf>, safety: SafetyLevel) -> CleanupCandidate {
        CleanupCandidate {
            id: Uuid::new_v4(),
            category_id: "test".into(),
            display_name: "Test".into(),
            total_size: 0,
            paths,
            safety,
        }
    }

    #[test]
    fn refuses_to_delete_never_auto_delete_items() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FakeAdapter { protected: vec![] };
        let executor = CleanupExecutor::new(&adapter);
        let c = candidate(vec![dir.path().to_path_buf()], SafetyLevel::NeverAutoDelete);
        let results = executor.execute(&c);
        assert!(matches!(results[0], Err(CleanupError::NeverAutoDelete(_))));
    }

    #[test]
    fn refuses_to_delete_protected_roots_even_if_marked_safe() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FakeAdapter { protected: vec![dir.path().canonicalize().unwrap()] };
        let executor = CleanupExecutor::new(&adapter);
        let c = candidate(vec![dir.path().to_path_buf()], SafetyLevel::Safe);
        let results = executor.execute(&c);
        assert!(matches!(results[0], Err(CleanupError::ProtectedPath(_))));
    }

    #[test]
    fn refuses_stale_targets_that_no_longer_exist() {
        let adapter = FakeAdapter { protected: vec![] };
        let executor = CleanupExecutor::new(&adapter);
        let c = candidate(vec![PathBuf::from("/definitely/does/not/exist/anywhere")], SafetyLevel::Safe);
        let results = executor.execute(&c);
        assert!(matches!(results[0], Err(CleanupError::StaleTarget(_))));
    }
}
