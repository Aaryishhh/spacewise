//! spacewise-core: platform-agnostic storage intelligence engine.
//!
//! Pipeline (see docs/ARCHITECTURE.md): Scanner -> StorageModel -> ClassificationEngine
//! -> SafetyEngine -> RecommendationEngine -> CleanupPlanner -> user approval -> CleanupExecutor.
//! No stage may skip forward: the scanner never decides what to delete, and a
//! recommendation never executes deletion directly.

pub mod adapter;
pub mod classification;
pub mod cleanup;
pub mod db;
pub mod duplicates;
pub mod history;
pub mod model;
pub mod recommend;
pub mod safety;
pub mod scanner;

/// Minimal proof-of-life for the Tauri <-> core bridge (Phase 1 scope only).
/// Real status (last scan time, engine versions, etc.) lands in later phases.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoreStatus {
    pub version: String,
    pub phase: &'static str,
}

pub fn status() -> CoreStatus {
    CoreStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        phase: "phase-1-architecture",
    }
}
