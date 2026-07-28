use super::*;
use crate::types::{
    CandidateFinding, DiffSide, FindingCategory, Priority, ResolvedFinding, RunStatus,
};

#[test]
fn round_trips_hidden_summary_state() {
    let state = SummaryState {
        version: 1,
        reviewed_sha: "abc".to_owned(),
        fingerprints: ["one".to_owned()].into_iter().collect(),
        fingerprint_paths: BTreeMap::from([("one".to_owned(), "src/main.rs".to_owned())]),
        fingerprint_related_paths: BTreeMap::new(),
        fingerprint_priorities: BTreeMap::new(),
        published_fingerprints: BTreeSet::new(),
        resolved_fingerprints: BTreeSet::new(),
        coverage_complete: None,
        handled_comment_ids: BTreeSet::new(),
    };
    let body = format!(
        "{SUMMARY_MARKER}\n{STATE_PREFIX}{}{STATE_SUFFIX}",
        serde_json::to_string(&state).expect("serialize")
    );
    let parsed = parse_summary_state(&body).expect("state");
    assert_eq!(parsed.reviewed_sha, "abc");
    assert!(parsed.fingerprints.contains("one"));
    assert_eq!(
        parsed.fingerprint_paths.get("one").map(String::as_str),
        Some("src/main.rs")
    );
}

#[test]
fn parses_owned_state_after_model_controlled_fake_marker() {
    let attacker = SummaryState {
        reviewed_sha: "attacker".to_owned(),
        ..SummaryState::default()
    };
    let fake = format!(
        "{STATE_PREFIX}{}{STATE_SUFFIX}",
        serde_json::to_string(&attacker).expect("serialize")
    );
    let expected = SummaryState {
        reviewed_sha: "trusted".to_owned(),
        ..SummaryState::default()
    };
    let body = format!(
        "{fake}\n{STATE_PREFIX}{}{STATE_SUFFIX}",
        serde_json::to_string(&expected).expect("serialize")
    );
    let parsed = parse_summary_state(&body).expect("state");
    assert_eq!(parsed.reviewed_sha, "trusted");
}

#[test]
fn escapes_html_comments_in_model_controlled_rationale() {
    let runs = vec![AgentRun {
        agent: crate::types::ReviewAgent::Primary,
        status: AgentStatus::Completed,
        bundle_ids: vec!["bundle".to_owned()],
        rationale: "<!-- prbot-state:{} -->".to_owned(),
        candidate_findings: 0,
        accepted_findings: 0,
    }];
    let body = render_agent_sections(&runs);
    assert!(!body.contains("<!-- prbot-state:"));
}

#[test]
fn partial_run_never_claims_no_verified_findings() {
    let outcome = RunOutcome {
        status: RunStatus::Partial,
        reviewed_sha: "abc".to_owned(),
        coverage_complete: false,
        eligible_hunks: 2,
        assigned_hunks: 1,
        findings: 0,
        active_findings: 0,
        ever_published_findings: 0,
        resolved_findings: 0,
        resolution_rate: None,
        skipped_findings: 0,
        failed_bundles: vec!["bundle-2".to_owned()],
        budget: crate::types::BudgetSnapshot::default(),
        incremental: Some(false),
        reviewed_bundles: Some(1),
        agent_runs: Vec::new(),
    };
    let summary = render_summary(
        "octocat/hello",
        1,
        &outcome,
        &[],
        &SummaryState::default(),
        "provider/reviewer",
        "other/verifier",
        None,
    );
    assert!(summary.contains("Partial review"));
    assert!(!summary.contains("No verified findings"));
}

#[test]
fn renders_walkthrough_before_precision_review() {
    let outcome = RunOutcome {
        status: RunStatus::Complete,
        reviewed_sha: "abc".to_owned(),
        coverage_complete: true,
        eligible_hunks: 1,
        assigned_hunks: 1,
        findings: 0,
        active_findings: 0,
        ever_published_findings: 0,
        resolved_findings: 0,
        resolution_rate: None,
        skipped_findings: 0,
        failed_bundles: Vec::new(),
        budget: crate::types::BudgetSnapshot::default(),
        incremental: Some(false),
        reviewed_bundles: Some(1),
        agent_runs: Vec::new(),
    };
    let summary = render_summary(
        "octocat/hello",
        1,
        &outcome,
        &[],
        &SummaryState::default(),
        "provider/reviewer",
        "other/verifier",
        Some("## Walkthrough\n\nChanged the parser.\n"),
    );
    let walkthrough = summary.find("## Walkthrough").expect("walkthrough");
    let precision = summary.find("## Precision review").expect("precision");
    assert!(walkthrough < precision);
}

#[test]
fn resolves_forgotten_fingerprints_only_when_coverage_complete() {
    let mut state = SummaryState::default();
    state.published_fingerprints.insert("fp-a".to_owned());
    state.fingerprints.insert("fp-a".to_owned());
    state
        .fingerprint_paths
        .insert("fp-a".to_owned(), "src/a.rs".to_owned());
    let forgotten = state.forget_paths(&BTreeSet::from(["src/a.rs".to_owned()]));
    assert_eq!(forgotten, BTreeSet::from(["fp-a".to_owned()]));
    assert_eq!(state.resolve_forgotten(&forgotten, false), 0);
    assert_eq!(state.resolve_forgotten(&forgotten, true), 1);
    assert!(state.resolved_fingerprints.contains("fp-a"));
    assert!((state.resolution_rate().unwrap() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn renders_primary_review_section() {
    let runs = vec![AgentRun {
        agent: crate::types::ReviewAgent::Primary,
        status: AgentStatus::Completed,
        bundle_ids: vec!["bundle".to_owned()],
        rationale: "Reviewed every selected bundle.".to_owned(),
        candidate_findings: 1,
        accepted_findings: 1,
    }];
    let body = render_review_body(&runs);
    assert!(body.contains("## Precision review"));
    assert!(body.contains("### Precision review"));
    assert!(!body.contains("Router fallback"));
}

#[test]
fn forgets_fingerprints_for_changed_paths() {
    let mut state = SummaryState::default();
    state.remember_finding(&ResolvedFinding {
        candidate: CandidateFinding {
            agent: crate::types::ReviewAgent::Primary,
            path: "src/a.rs".to_owned(),
            side: DiffSide::Right,
            anchor: "a".to_owned(),
            end_anchor: None,
            priority: Priority::P1,
            category: FindingCategory::Correctness,
            title: "A".to_owned(),
            body: "body".to_owned(),
            evidence: Vec::new(),
            confidence: 0.9,
        },
        line: Some(1),
        start_line: None,
        side: DiffSide::Right,
        fingerprint: "fp-a".to_owned(),
        file_level: false,
    });
    state.fingerprints.insert("fp-b".to_owned());
    state
        .fingerprint_paths
        .insert("fp-b".to_owned(), "src/b.rs".to_owned());
    state.forget_paths(&["src/a.rs".to_owned()].into_iter().collect());
    assert!(!state.fingerprints.contains("fp-a"));
    assert!(state.fingerprints.contains("fp-b"));
}

#[test]
fn forgets_documentation_findings_when_an_evidence_path_changes() {
    let mut state = SummaryState::default();
    state.fingerprints.insert("docs-fp".to_owned());
    state
        .fingerprint_paths
        .insert("docs-fp".to_owned(), "src/cli.rs".to_owned());
    state.fingerprint_related_paths.insert(
        "docs-fp".to_owned(),
        BTreeSet::from(["README.md".to_owned()]),
    );
    state.forget_paths(&BTreeSet::from(["README.md".to_owned()]));
    assert!(!state.fingerprints.contains("docs-fp"));
}

#[test]
fn only_p0_through_p2_findings_block_the_check() {
    let mut state = SummaryState::default();
    state
        .fingerprints
        .extend(["p2".to_owned(), "p3".to_owned()]);
    state
        .fingerprint_priorities
        .insert("p2".to_owned(), Priority::P2);
    state
        .fingerprint_priorities
        .insert("p3".to_owned(), Priority::P3);
    assert_eq!(state.blocking_findings(), 1);
}
