//! Safety engine: deterministic, rule-based SafetyLevel assignment (spec
//! section 8). AI is never consulted here -- every category's safety level
//! comes from the classification knowledge base, and anything the
//! classification engine did not recognise defaults to the strictest level.

use crate::model::{SafetyLevel, StorageCategoryDef};

pub struct SafetyEngine;

impl SafetyEngine {
    pub fn new() -> Self {
        Self
    }

    /// A classified category's safety is authored directly into its
    /// knowledge-base entry (see classification.rs) -- this stage exists so
    /// the pipeline has one place future per-instance overrides (staleness,
    /// user history) could plug in without touching classification.
    pub fn classify(&self, category: Option<&StorageCategoryDef>) -> SafetyLevel {
        match category {
            Some(c) => c.safety,
            // Unknown/unclassified paths are never auto-deletable: the
            // strictest default, matching spec section 8's NEVER AUTO-DELETE
            // tier for "unknown sensitive areas".
            None => SafetyLevel::NeverAutoDelete,
        }
    }
}

impl Default for SafetyEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification::ClassificationEngine;
    use std::path::Path;

    #[test]
    fn unclassified_paths_default_to_never_delete() {
        let engine = SafetyEngine::new();
        assert_eq!(engine.classify(None), SafetyLevel::NeverAutoDelete);
    }

    #[test]
    fn classified_safe_category_stays_safe() {
        let classifier = ClassificationEngine::new();
        let category = classifier.classify(Path::new("/Users/x/project/node_modules")).unwrap();
        let engine = SafetyEngine::new();
        assert_eq!(engine.classify(Some(category)), SafetyLevel::Safe);
    }
}
