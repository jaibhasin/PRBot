use crate::types::{RelatedFile, ReviewBundle, ReviewManifest};
use std::collections::BTreeSet;
use std::path::Path;

/// Select bundles whose changed paths (or strong related files) intersect `changed_paths`.
/// When `changed_paths` is empty, returns the full bundle list.
pub fn select_bundles_for_paths(
    manifest: &ReviewManifest,
    changed_paths: &BTreeSet<String>,
) -> Vec<ReviewBundle> {
    if changed_paths.is_empty() {
        return manifest.bundles.clone();
    }
    manifest
        .bundles
        .iter()
        .filter(|bundle| bundle_touches(bundle, changed_paths))
        .cloned()
        .collect()
}

fn bundle_touches(bundle: &ReviewBundle, changed_paths: &BTreeSet<String>) -> bool {
    bundle
        .paths
        .iter()
        .any(|path| path_matches(path, changed_paths))
        || bundle
            .related_files
            .iter()
            .any(|related| related.score >= 8 && path_matches(&related.path, changed_paths))
}

fn path_matches(path: &str, changed_paths: &BTreeSet<String>) -> bool {
    if changed_paths.contains(path) {
        return true;
    }
    let stem = Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    !stem.is_empty()
        && changed_paths.iter().any(|changed| {
            Path::new(changed)
                .file_stem()
                .and_then(|value| value.to_str())
                == Some(stem)
        })
}

pub fn related_paths_for_bundles(bundles: &[ReviewBundle]) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for bundle in bundles {
        paths.extend(bundle.paths.iter().cloned());
        for RelatedFile { path, .. } in &bundle.related_files {
            paths.insert(path.clone());
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RelatedFile, RiskLevel};

    #[test]
    fn selects_only_bundles_touching_changed_paths() {
        let manifest = ReviewManifest {
            bundles: vec![
                ReviewBundle {
                    id: "bundle-1".to_owned(),
                    paths: vec!["src/a.rs".to_owned()],
                    hunk_count: 1,
                    risk: RiskLevel::Low,
                    related_files: Vec::new(),
                },
                ReviewBundle {
                    id: "bundle-2".to_owned(),
                    paths: vec!["src/b.rs".to_owned()],
                    hunk_count: 1,
                    risk: RiskLevel::Low,
                    related_files: vec![RelatedFile {
                        path: "src/a.rs".to_owned(),
                        score: 10,
                        reasons: vec!["import".to_owned()],
                    }],
                },
            ],
            ..ReviewManifest::default()
        };
        let selected =
            select_bundles_for_paths(&manifest, &["src/a.rs".to_owned()].into_iter().collect());
        assert_eq!(selected.len(), 2);
        let selected =
            select_bundles_for_paths(&manifest, &["src/c.rs".to_owned()].into_iter().collect());
        assert!(selected.is_empty());
    }
}
