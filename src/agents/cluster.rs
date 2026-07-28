use crate::types::{CandidateFinding, Priority};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Builds a cross-pass cluster key that ignores title/priority wording drift.
pub fn cluster_key(finding: &CandidateFinding) -> String {
    let normalized = format!(
        "{}|{:?}|{:?}|{}|{}",
        finding.path,
        finding.side,
        finding.category,
        collapse_ws(&finding.anchor),
        collapse_ws(finding.end_anchor.as_deref().unwrap_or(""))
    );
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

/// Merges multipass candidates with majority voting and high-confidence singleton keep.
pub fn merge_pass_findings(
    passes: &[Vec<CandidateFinding>],
    majority_k: usize,
    keep_high_confidence_singleton: f32,
    max_candidates: usize,
) -> Vec<CandidateFinding> {
    let mut clusters: BTreeMap<String, Cluster> = BTreeMap::new();
    for (pass_index, findings) in passes.iter().enumerate() {
        for finding in findings {
            let key = cluster_key(finding);
            let entry = clusters.entry(key).or_default();
            entry.support.insert(pass_index);
            entry.candidates.push(finding.clone());
        }
    }

    let majority_k = majority_k.max(1);
    let mut merged = clusters
        .into_values()
        .filter(|cluster| {
            let support = cluster.support.len();
            if support >= majority_k {
                return true;
            }
            if support != 1 {
                return false;
            }
            cluster.candidates.iter().any(|finding| {
                finding.confidence >= keep_high_confidence_singleton
                    && matches!(finding.priority, Priority::P0 | Priority::P1)
            })
        })
        .filter_map(|mut cluster| {
            cluster.candidates.sort_by(|left, right| {
                right
                    .confidence
                    .partial_cmp(&left.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.priority.cmp(&right.priority))
                    .then_with(|| right.body.len().cmp(&left.body.len()))
            });
            let mut best = cluster.candidates.into_iter().next()?;
            best.confidence = best
                .confidence
                .max(cluster.support.len() as f32 / passes.len().max(1) as f32);
            Some(best)
        })
        .collect::<Vec<_>>();

    merged.sort_by_key(|finding| finding.priority);
    if merged.len() > max_candidates {
        merged.truncate(max_candidates);
    }
    merged
}

#[derive(Default)]
struct Cluster {
    support: std::collections::BTreeSet<usize>,
    candidates: Vec<CandidateFinding>,
}

fn collapse_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DiffSide, FindingCategory, ReviewAgent};

    fn finding(
        path: &str,
        anchor: &str,
        title: &str,
        priority: Priority,
        confidence: f32,
    ) -> CandidateFinding {
        CandidateFinding {
            agent: ReviewAgent::Primary,
            path: path.to_owned(),
            side: DiffSide::Right,
            anchor: anchor.to_owned(),
            end_anchor: None,
            priority,
            category: FindingCategory::Correctness,
            title: title.to_owned(),
            body: "impact".to_owned(),
            evidence: Vec::new(),
            confidence,
        }
    }

    #[test]
    fn cluster_key_ignores_title_and_priority() {
        let left = finding("a.rs", "x = 1", "One", Priority::P0, 0.9);
        let right = finding("a.rs", "x = 1", "Two", Priority::P2, 0.8);
        assert_eq!(cluster_key(&left), cluster_key(&right));
    }

    #[test]
    fn majority_keeps_overlapping_findings() {
        let pass0 = vec![finding("a.rs", "x", "A", Priority::P1, 0.9)];
        let pass1 = vec![finding("a.rs", "x", "B", Priority::P1, 0.95)];
        let pass2 = vec![finding("b.rs", "y", "C", Priority::P1, 0.9)];
        let merged = merge_pass_findings(&[pass0, pass1, pass2], 2, 0.92, 12);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].path, "a.rs");
        assert!((merged[0].confidence - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn high_confidence_p0_singleton_is_kept() {
        let pass0 = vec![finding("a.rs", "x", "A", Priority::P0, 0.95)];
        let pass1 = Vec::new();
        let merged = merge_pass_findings(&[pass0, pass1], 2, 0.92, 12);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn low_confidence_singleton_is_dropped() {
        let pass0 = vec![finding("a.rs", "x", "A", Priority::P2, 0.99)];
        let pass1 = Vec::new();
        let merged = merge_pass_findings(&[pass0, pass1], 2, 0.92, 12);
        assert!(merged.is_empty());
    }
}
