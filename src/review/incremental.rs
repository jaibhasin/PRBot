use crate::types::{RelatedFile, ReviewBundle, ReviewManifest};
use std::collections::BTreeSet;
use std::path::Path;

/// Selects bundles that intersect the changed paths directly or through a strong related-file match.
///
/// When `changed_paths` is empty, all bundles in the manifest are selected. Related files
/// qualify when their score is at least 8.
///
/// # Arguments
///
/// * `manifest` - The review manifest containing the bundles to select.
/// * `changed_paths` - Paths changed in the current review.
///
/// # Returns
///
/// A vector containing the selected bundles.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeSet;
///
/// let manifest = ReviewManifest { bundles: vec![] };
/// let changed_paths = BTreeSet::new();
///
/// let selected = select_bundles_for_paths(&manifest, &changed_paths);
/// assert!(selected.is_empty());
/// ```♀♀♀♀♀♀
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

/// Determines whether a review bundle is affected by any changed path.
///
/// A bundle is affected when one of its paths matches a changed path, or when a
/// related file with a score of at least 8 matches a changed path.
///
/// # Examples
///
/// ```rust,ignore
/// let changed_paths = BTreeSet::from(["src/a.rs".to_owned()]);
/// assert!(bundle_touches(&bundle, &changed_paths));
/// ```
///
/// # Returns
///
/// `true` if the bundle touches a changed path, `false` otherwise.
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

/// Determines whether a path matches any changed path exactly or by file stem.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeSet;
///
/// let changed_paths = BTreeSet::from([String::from("tests/lib.rs")]);
///
/// assert!(path_matches("src/lib.rs", &changed_paths));
/// ```
///
/// # Arguments
///
/// * `path` - The path to compare.
/// * `changed_paths` - The paths changed in the current update.
///
/// # Returns
///
/// `true` if `path` exactly matches a changed path or shares its file stem with one; `false` otherwise.
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

/// Collects all direct and related file paths from the provided review bundles.
///
/// # Examples
///
/// ```
/// let bundles: &[ReviewBundle] = &[];
/// let paths = related_paths_for_bundles(bundles);
///
/// assert!(paths.is_empty());
/// ```
///
/// Returns a sorted set containing each bundle path and related file path.
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
