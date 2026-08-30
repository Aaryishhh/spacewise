//! HistoryEngine (spec sections 17-18): persists lightweight per-scan
//! category snapshots and diffs them to answer "why is my disk full?" --
//! total growth plus the categories that grew the most, never raw file
//! listings (spec section 22: minimize retained personal content).

use crate::db::StorageDatabase;
use crate::model::HistoricalSnapshot;
use chrono::{Duration, Utc};
use std::collections::HashMap;
use uuid::Uuid;

pub struct HistoryEngine;

impl HistoryEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn record_snapshot(&self, db: &StorageDatabase, scan_id: Uuid, total_size: u64) -> anyhow::Result<HistoricalSnapshot> {
        let category_totals = db.category_totals(scan_id)?;
        let snapshot = HistoricalSnapshot {
            id: Uuid::new_v4(),
            scan_id,
            taken_at: Utc::now(),
            total_size,
            category_totals,
        };
        db.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    /// Compares the oldest snapshot within `lookback_days` against the most
    /// recent one. Returns None if there is not yet enough history.
    pub fn growth_summary(&self, db: &StorageDatabase, lookback_days: i64) -> anyhow::Result<Option<GrowthSummary>> {
        let since = Utc::now() - Duration::days(lookback_days);
        let snapshots = db.snapshots_since(since)?;
        let (Some(first), Some(last)) = (snapshots.first(), snapshots.last()) else {
            return Ok(None);
        };
        if first.id == last.id {
            return Ok(None);
        }

        let growth_bytes = last.total_size as i64 - first.total_size as i64;

        let mut before: HashMap<String, u64> = HashMap::new();
        for c in &first.category_totals {
            before.insert(c.category_id.clone(), c.total_size);
        }

        let mut deltas: Vec<CategoryDelta> = last
            .category_totals
            .iter()
            .map(|c| {
                let prev = before.get(&c.category_id).copied().unwrap_or(0);
                CategoryDelta { category_id: c.category_id.clone(), delta_bytes: c.total_size as i64 - prev as i64 }
            })
            .collect();
        deltas.sort_by(|a, b| b.delta_bytes.cmp(&a.delta_bytes));
        deltas.truncate(5);

        Ok(Some(GrowthSummary { growth_bytes, period_days: lookback_days, top_contributors: deltas }))
    }
}

impl Default for HistoryEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CategoryDelta {
    pub category_id: String,
    pub delta_bytes: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GrowthSummary {
    pub growth_bytes: i64,
    pub period_days: i64,
    pub top_contributors: Vec<CategoryDelta>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CategoryTotal;

    #[test]
    fn growth_summary_is_none_with_only_one_snapshot() {
        let db = StorageDatabase::open_in_memory().unwrap();
        let engine = HistoryEngine::new();
        let scan_id = Uuid::new_v4();
        engine.record_snapshot(&db, scan_id, 1000).unwrap();
        assert!(engine.growth_summary(&db, 30).unwrap().is_none());
    }

    #[test]
    fn growth_summary_ranks_biggest_category_deltas_first() {
        let db = StorageDatabase::open_in_memory().unwrap();
        let scan_a = Uuid::new_v4();
        let scan_b = Uuid::new_v4();

        db.save_snapshot(&HistoricalSnapshot {
            id: Uuid::new_v4(),
            scan_id: scan_a,
            taken_at: Utc::now() - Duration::days(10),
            total_size: 1_000,
            category_totals: vec![CategoryTotal { category_id: "docker-data".into(), total_size: 100, item_count: 1 }],
        })
        .unwrap();
        db.save_snapshot(&HistoricalSnapshot {
            id: Uuid::new_v4(),
            scan_id: scan_b,
            taken_at: Utc::now(),
            total_size: 5_000,
            category_totals: vec![CategoryTotal { category_id: "docker-data".into(), total_size: 4_100, item_count: 3 }],
        })
        .unwrap();

        let summary = HistoryEngine::new().growth_summary(&db, 30).unwrap().unwrap();
        assert_eq!(summary.growth_bytes, 4_000);
        assert_eq!(summary.top_contributors[0].category_id, "docker-data");
        assert_eq!(summary.top_contributors[0].delta_bytes, 4_000);
    }
}
