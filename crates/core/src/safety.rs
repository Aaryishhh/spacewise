//! Safety engine: deterministic, rule-based SafetyLevel assignment. AI is never
//! solely responsible for a deletion-safety decision (spec section 8). Phase 7.

use crate::model::SafetyLevel;
use std::path::Path;

pub struct SafetyEngine;

impl SafetyEngine {
    /// Deterministic classification only -- no ML/LLM inference here.
    pub fn classify(&self, _path: &Path) -> SafetyLevel {
        SafetyLevel::NeverAutoDelete
    }
}
