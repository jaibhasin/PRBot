use super::*;
use crate::github::CheckRun;
use crate::types::{CandidateFinding, FindingCategory, Priority, ResolvedFinding};

#[test]
fn file_level_comments_use_subject_type() {
    let finding = ResolvedFinding {
        candidate: CandidateFinding {
            agent: crate::types::ReviewAgent::Correctness,
            path: "src/main.rs".to_owned(),
            side: DiffSide::Right,
            anchor: "ambiguous".to_owned(),
            end_anchor: None,
            priority: Priority::P1,
            category: FindingCategory::Correctness,
            title: "Bug".to_owned(),
            body: "Impact".to_owned(),
            evidence: Vec::new(),
            confidence: 0.9,
        },
        line: None,
        start_line: None,
        side: DiffSide::Right,
        fingerprint: "fp".to_owned(),
        file_level: true,
    };
    let comment = review_comment(&finding);
    assert_eq!(comment.subject_type.as_deref(), Some("file"));
    assert!(comment.line.is_none());
}

#[test]
fn unpublished_overflow_findings_never_enter_active_state() {
    let mut first = test_finding();
    first.fingerprint = "published".to_owned();
    let mut second = test_finding();
    second.fingerprint = "overflow".to_owned();
    let mut state = SummaryState::default();
    let (published, overflow) = select_publishable_findings(&mut state, vec![first, second], 1);
    assert_eq!(published.len(), 1);
    assert_eq!(overflow, 1);
    assert!(state.fingerprints.contains("published"));
    assert!(!state.fingerprints.contains("overflow"));
}

#[test]
fn missing_or_incomplete_review_check_requires_recovery() {
    assert!(!has_completed_review_check(&[]));
    assert!(!has_completed_review_check(&[CheckRun {
        name: "PRBot review".to_owned(),
        status: "in_progress".to_owned(),
        conclusion: None,
        output: None,
    }]));
    assert!(has_completed_review_check(&[CheckRun {
        name: "PRBot review".to_owned(),
        status: "completed".to_owned(),
        conclusion: Some("success".to_owned()),
        output: None,
    }]));
}

#[test]
fn incomplete_coverage_fails_even_without_findings() {
    let (conclusion, title) = review_check_result(false, 0);
    assert_eq!(conclusion, CheckConclusion::Failure);
    assert_eq!(title, "PRBot review incomplete");
}

#[test]
fn incomplete_review_reruns_full_coverage_on_the_same_head() {
    let state = SummaryState {
        reviewed_sha: "head".to_owned(),
        coverage_complete: Some(false),
        ..SummaryState::default()
    };
    assert!(requires_full_recovery(&state, false));
    assert!(!requires_full_recovery(&state, true));
}

fn test_finding() -> ResolvedFinding {
    ResolvedFinding {
        candidate: CandidateFinding {
            agent: crate::types::ReviewAgent::Correctness,
            path: "src/main.rs".to_owned(),
            side: DiffSide::Right,
            anchor: "changed".to_owned(),
            end_anchor: None,
            priority: Priority::P1,
            category: FindingCategory::Correctness,
            title: "Bug".to_owned(),
            body: "Impact".to_owned(),
            evidence: Vec::new(),
            confidence: 0.9,
        },
        line: Some(1),
        start_line: None,
        side: DiffSide::Right,
        fingerprint: "fp".to_owned(),
        file_level: false,
    }
}
