//! Recommendation engine (spec section 11): ranks cleanup candidates by
//! safety, regeneratability, staleness, and reclaimable size -- explicitly
//! NOT by raw size alone. A 2 GB Downloads folder you touched yesterday
//! should rank below a 500 MB build-cache directory nobody has touched in a
//! year.

use crate::classification::ClassificationEngine;
use crate::model::{
    CleanupCandidate, DirectoryAggregate, Recommendation, SafetyLevel,
};
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

pub struct RecommendationEngine;

impl RecommendationEngine {
    pub fn new() -> Self {
        Self
    }

    /// `classified` is (aggregate, category_id, safety) triples, typically
    /// from `StorageDatabase::all_categorized`. Candidates sharing a category
    /// are merged into one, since the user thinks in terms of "all my Xcode
    /// caches", not one candidate per project.
    pub fn recommend(
        &self,
        classified: &[(DirectoryAggregate, String, SafetyLevel)],
        classifier: &ClassificationEngine,
    ) -> Vec<Recommendation> {
        let mut by_category: HashMap<String, CleanupCandidate> = HashMap::new();

        for (agg, category_id, safety) in classified {
            // NeverAutoDelete items are never surfaced as a recommendation --
            // the user can still find them via the storage explorer, but this
            // engine's job is "safe to suggest", not "everything that exists".
            if *safety == SafetyLevel::NeverAutoDelete {
                continue;
            }
            let display_name = classifier
                .category_by_id(category_id)
                .map(|c| c.display_name.clone())
                .unwrap_or_else(|| category_id.clone());

            let entry = by_category.entry(category_id.clone()).or_insert_with(|| CleanupCandidate {
                id: Uuid::new_v4(),
                category_id: category_id.clone(),
                display_name,
                paths: Vec::new(),
                total_size: 0,
                safety: *safety,
            });
            entry.paths.push(agg.path.clone());
            entry.total_size += agg.total_size;
        }

        let mut recommendations: Vec<Recommendation> = by_category
            .into_values()
            .map(|candidate| {
                let category = classifier.category_by_id(&candidate.category_id);
                let (score, rationale) = score_candidate(&candidate, category, classified);
                Recommendation { candidate, score, rationale }
            })
            .collect();

        recommendations.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        recommendations
    }
}

impl Default for RecommendationEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn score_candidate(
    candidate: &CleanupCandidate,
    category: Option<&crate::model::StorageCategoryDef>,
    classified: &[(DirectoryAggregate, String, SafetyLevel)],
) -> (f64, String) {
    let safety_weight = match candidate.safety {
        SafetyLevel::Safe => 1.0,
        SafetyLevel::Review => 0.55,
        SafetyLevel::Advanced => 0.25,
        SafetyLevel::NeverAutoDelete => 0.0,
    };

    let regeneratable = category.map(|c| c.regeneratable).unwrap_or(false);
    let regen_multiplier = if regeneratable { 1.15 } else { 0.75 };

    // Staleness: average days-since-modified across this category's matched
    // directories, capped at one year for a stable scale.
    let now = Utc::now();
    let staleness_days: Vec<f64> = classified
        .iter()
        .filter(|(_, cat_id, _)| *cat_id == candidate.category_id)
        .filter_map(|(agg, _, _)| agg.latest_modified)
        .map(|dt| (now - dt).num_days().max(0) as f64)
        .collect();
    let avg_staleness_days = if staleness_days.is_empty() {
        365.0 // unknown activity -- treat conservatively as "old"
    } else {
        staleness_days.iter().sum::<f64>() / staleness_days.len() as f64
    };
    let staleness_factor = (avg_staleness_days / 365.0).min(1.0);

    // Size: log-scaled so a 50 GB candidate doesn't dwarf every other signal.
    let size_factor = ((candidate.total_size as f64 + 1.0).log10() / 11.0).min(1.0); // ~100GB -> 1.0

    let score = safety_weight * regen_multiplier + 0.25 * staleness_factor + 0.15 * size_factor;

    let rationale = format!(
        "{} across {} location(s), {}, last touched ~{:.0} days ago on average.",
        human_size(candidate.total_size),
        candidate.paths.len(),
        if regeneratable { "regenerates automatically" } else { "does not regenerate automatically" },
        avg_staleness_days,
    );

    (score, rationale)
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", size, UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn agg(path: &str, size: u64) -> DirectoryAggregate {
        DirectoryAggregate { path: PathBuf::from(path), total_size: size, allocated_size: size, file_count: 1, dir_count: 0, latest_modified: None }
    }

    #[test]
    fn never_auto_delete_is_excluded_from_recommendations() {
        let classifier = ClassificationEngine::new();
        let classified = vec![(agg("/a/pagefile.sys", 1_000_000), "pagefile".to_string(), SafetyLevel::NeverAutoDelete)];
        let recs = RecommendationEngine::new().recommend(&classified, &classifier);
        assert!(recs.is_empty());
    }

    #[test]
    fn safe_regeneratable_category_outranks_review_category_of_similar_size() {
        let classifier = ClassificationEngine::new();
        let classified = vec![
            (agg("/p1/node_modules", 500_000_000), "node-modules".to_string(), SafetyLevel::Safe),
            (agg("/Downloads", 500_000_000), "downloads".to_string(), SafetyLevel::Review),
        ];
        let recs = RecommendationEngine::new().recommend(&classified, &classifier);
        assert_eq!(recs[0].candidate.category_id, "node-modules");
    }
}
