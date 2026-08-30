//! Associates installed applications with their supporting storage (spec
//! section 13). Confidence reflects how specifically an aggregated
//! directory's name matches the application's name -- an exact normalized
//! match is high confidence, a substring match is medium, and anything
//! weaker is not surfaced at all. Never delete on a guess.

use crate::model::{AppAssociation, DirectoryAggregate, InstalledApp};

pub fn associate_apps_with_storage(
    apps: &[InstalledApp],
    aggregates: &[DirectoryAggregate],
) -> Vec<AppAssociation> {
    apps.iter().filter_map(|app| associate_one(app, aggregates)).collect()
}

fn associate_one(app: &InstalledApp, aggregates: &[DirectoryAggregate]) -> Option<AppAssociation> {
    let normalized_app = normalize(&app.name);
    if normalized_app.is_empty() {
        return None;
    }

    let mut associated_paths = Vec::new();
    let mut associated_size = 0u64;
    let mut best_confidence: f32 = 0.0;

    for agg in aggregates {
        let path_name = agg
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .map(normalize)
            .unwrap_or_default();
        if path_name.is_empty() {
            continue;
        }
        let confidence = if path_name == normalized_app {
            1.0
        } else if path_name.contains(&normalized_app) || normalized_app.contains(&path_name) {
            0.6
        } else {
            0.0
        };
        if confidence > 0.0 {
            associated_paths.push(agg.path.clone());
            associated_size += agg.total_size;
            best_confidence = best_confidence.max(confidence);
        }
    }

    if associated_paths.is_empty() {
        return None;
    }

    Some(AppAssociation {
        app: app.clone(),
        associated_paths,
        associated_size,
        confidence: best_confidence,
    })
}

fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn app(name: &str) -> InstalledApp {
        InstalledApp { name: name.into(), publisher: None, install_location: None, estimated_size_bytes: None, uninstall_command: None }
    }

    fn agg(path: &str, size: u64) -> DirectoryAggregate {
        DirectoryAggregate { path: PathBuf::from(path), total_size: size, allocated_size: size, file_count: 1, dir_count: 0, latest_modified: None }
    }

    #[test]
    fn exact_name_match_gets_high_confidence() {
        let apps = vec![app("Slack")];
        let aggregates = vec![agg(r"C:\Users\x\AppData\Roaming\Slack", 500)];
        let assoc = associate_apps_with_storage(&apps, &aggregates);
        assert_eq!(assoc.len(), 1);
        assert_eq!(assoc[0].confidence, 1.0);
        assert_eq!(assoc[0].associated_size, 500);
    }

    #[test]
    fn unrelated_app_gets_no_association() {
        let apps = vec![app("Totally Unrelated App")];
        let aggregates = vec![agg(r"C:\Users\x\AppData\Roaming\Slack", 500)];
        assert!(associate_apps_with_storage(&apps, &aggregates).is_empty());
    }
}
