//! Classification engine: maps scanned paths to StorageCategory + a plain-English
//! explanation (what/why/regeneratable/reversible) via the versioned storage
//! knowledge base described in docs/ARCHITECTURE.md. Phase 6.

use crate::model::StorageCategory;

pub trait ClassificationRule: Send + Sync {
    fn matches(&self, path: &std::path::Path) -> bool;
    fn category(&self) -> StorageCategory;
}

pub struct ClassificationEngine {
    rules: Vec<Box<dyn ClassificationRule>>,
}

impl ClassificationEngine {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }
}

impl Default for ClassificationEngine {
    fn default() -> Self {
        Self::new()
    }
}
