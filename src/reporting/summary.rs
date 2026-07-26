use crate::types::{ResolvedFinding, RunOutcome, RunStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const SUMMARY_MARKER: &str = "<!-- prbot-contextual-review -->";
const STATE_PREFIX: &str = "<!-- prbot-state:";
const STATE_SUFFIX: &str = " -->";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SummaryState {
    pub version: u32,
    pub reviewed_sha: String,
    pub fingerprints: BTreeSet<String>,
    #[serde(default)]
    pub handled_comment_ids: BTreeSet<u64>,
}

pub fn render_summary(
    repository: &str,
    pr_number: u64,
    outcome: &RunOutcome,
    findings: &[ResolvedFinding],
    state: &SummaryState,
    review_model: &str,
    verification_model: &str,
) -> String {
    let status = match outcome.status {
        RunStatus::Complete if findings.is_empty() => "No verified findings",
        RunStatus::Complete => "Review complete",
        RunStatus::Partial => "Partial review",
    };
    let failures = if outcome.failed_bundles.is_empty() {
        "None".to_owned()
    } else {
        outcome.failed_bundles.join(", ")
    };
    let encoded = serde_json::to_string(state).unwrap_or_else(|_| "{}".to_owned());
    format!(
        "{SUMMARY_MARKER}\n\
**PRBot contextual review: {status}**\n\n\
Repository: `{repository}#{pr_number}`  \n\
Reviewed head: `{}`  \n\
Coverage: `{}/{}` eligible hunks assigned  \n\
Published findings: `{}`  \n\
Rejected or unanchored findings: `{}`  \n\
Failed stages: `{failures}`  \n\
Models: `{review_model}` reviewer, `{verification_model}` verifier  \n\
Budget: `{}` input tokens, `{}` output tokens, `${:.4}` estimated, `{}s`\n\n\
{STATE_PREFIX}{encoded}{STATE_SUFFIX}\n",
        outcome.reviewed_sha,
        outcome.assigned_hunks,
        outcome.eligible_hunks,
        findings.len(),
        outcome.skipped_findings,
        outcome.budget.input_tokens,
        outcome.budget.output_tokens,
        outcome.budget.estimated_cost_usd,
        outcome.budget.elapsed_seconds
    )
}

pub fn parse_summary_state(body: &str) -> Option<SummaryState> {
    let start = body.find(STATE_PREFIX)? + STATE_PREFIX.len();
    let rest = &body[start..];
    let end = rest.find(STATE_SUFFIX)?;
    serde_json::from_str(&rest[..end]).ok()
}

pub fn finding_body(finding: &ResolvedFinding) -> String {
    let evidence = if finding.candidate.evidence.is_empty() {
        String::new()
    } else {
        let list = finding
            .candidate
            .evidence
            .iter()
            .map(|span| {
                format!(
                    "- `{}` ({:?}): {}",
                    span.path, span.revision, span.explanation
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\nEvidence:\n{list}")
    };
    format!(
        "<!-- prbot:finding:{} -->\n**{:?} - {}**\n\n{}{}",
        finding.fingerprint,
        finding.candidate.priority,
        finding.candidate.title.trim(),
        finding.candidate.body.trim(),
        evidence
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_hidden_summary_state() {
        let state = SummaryState {
            version: 1,
            reviewed_sha: "abc".to_owned(),
            fingerprints: ["one".to_owned()].into_iter().collect(),
            handled_comment_ids: BTreeSet::new(),
        };
        let body = format!(
            "{SUMMARY_MARKER}\n{STATE_PREFIX}{}{STATE_SUFFIX}",
            serde_json::to_string(&state).expect("serialize")
        );
        let parsed = parse_summary_state(&body).expect("state");
        assert_eq!(parsed.reviewed_sha, "abc");
        assert!(parsed.fingerprints.contains("one"));
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
            skipped_findings: 0,
            failed_bundles: vec!["bundle-2".to_owned()],
            budget: crate::types::BudgetSnapshot::default(),
        };
        let summary = render_summary(
            "octocat/hello",
            1,
            &outcome,
            &[],
            &SummaryState::default(),
            "provider/reviewer",
            "other/verifier",
        );
        assert!(summary.contains("Partial review"));
        assert!(!summary.contains("No verified findings"));
    }
}
