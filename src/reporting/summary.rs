use crate::types::{ResolvedFinding, RunOutcome, RunStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SUMMARY_MARKER: &str = "<!-- prbot-contextual-review -->";
const STATE_PREFIX: &str = "<!-- prbot-state:";
const STATE_SUFFIX: &str = " -->";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SummaryState {
    pub version: u32,
    pub reviewed_sha: String,
    pub fingerprints: BTreeSet<String>,
    #[serde(default)]
    pub fingerprint_paths: BTreeMap<String, String>,
    #[serde(default)]
    pub handled_comment_ids: BTreeSet<u64>,
}

impl SummaryState {
    /// Removes remembered findings associated with the specified file paths.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{BTreeMap, BTreeSet};
    ///
    /// let mut state = SummaryState {
    ///     version: 1,
    ///     reviewed_sha: String::new(),
    ///     fingerprints: BTreeSet::from(["fp-a".to_string(), "fp-b".to_string()]),
    ///     fingerprint_paths: BTreeMap::from([
    ///         ("fp-a".to_string(), "src/a.rs".to_string()),
    ///         ("fp-b".to_string(), "src/b.rs".to_string()),
    ///     ]),
    ///     handled_comment_ids: BTreeSet::new(),
    /// };
    /// let paths = BTreeSet::from(["src/a.rs".to_string()]);
    ///
    /// state.forget_paths(&paths);
    ///
    /// assert!(!state.fingerprints.contains("fp-a"));
    /// assert!(state.fingerprints.contains("fp-b"));
    /// ```
    pub fn forget_paths(&mut self, paths: &BTreeSet<String>) {
        let stale = self
            .fingerprint_paths
            .iter()
            .filter(|(_, path)| paths.contains(*path))
            .map(|(fingerprint, _)| fingerprint.clone())
            .collect::<Vec<_>>();
        for fingerprint in stale {
            self.fingerprints.remove(&fingerprint);
            self.fingerprint_paths.remove(&fingerprint);
        }
    }

    /// Records a finding's fingerprint and associates it with the finding's path.
    ///
    /// # Examples
    ///
    /// ```
    /// state.remember_finding(&finding);
    /// assert!(state.fingerprints.contains(&finding.fingerprint));
    /// assert_eq!(
    ///     state.fingerprint_paths.get(&finding.fingerprint),
    ///     Some(&finding.candidate.path)
    /// );
    /// ```
    pub fn remember_finding(&mut self, finding: &ResolvedFinding) {
        self.fingerprints.insert(finding.fingerprint.clone());
        self.fingerprint_paths
            .insert(finding.fingerprint.clone(), finding.candidate.path.clone());
    }
}

/// Renders a Markdown summary of a pull request review, including review status, coverage, findings, budget metrics, and serialized contextual state.
///
/// # Examples
///
/// ```ignore
/// let summary = render_summary(
///     "owner/repository",
///     42,
///     &outcome,
///     &findings,
///     &state,
///     "review-model",
///     "verification-model",
/// );
/// assert!(summary.contains("PRBot contextual review"));
/// ```
///
/// # Parameters
///
/// * `repository` - Repository identifier.
/// * `pr_number` - Pull request number.
/// * `outcome` - Results and metrics from the review run.
/// * `findings` - Findings published or evaluated during the review.
/// * `state` - Contextual review state to embed in the summary.
/// * `review_model` - Name of the model used for review.
/// * `verification_model` - Name of the model used for verification.
///
/// # Returns
///
/// A Markdown-formatted review summary containing serialized contextual state.
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
        RunStatus::Skipped => "Review skipped",
        RunStatus::Failed => "Review failed",
    };
    let failures = if outcome.failed_bundles.is_empty() {
        "None".to_owned()
    } else {
        outcome.failed_bundles.join(", ")
    };
    let incremental = match outcome.incremental {
        Some(true) => "yes",
        Some(false) => "no",
        None => "n/a",
    };
    let reviewed_bundles = outcome
        .reviewed_bundles
        .map(|count| count.to_string())
        .unwrap_or_else(|| "all".to_owned());
    let encoded = serde_json::to_string(state).unwrap_or_else(|_| "{}".to_owned());
    format!(
        "{SUMMARY_MARKER}\n\
**PRBot contextual review: {status}**\n\n\
Repository: `{repository}#{pr_number}`  \n\
Reviewed head: `{}`  \n\
Coverage: `{}/{}` eligible hunks assigned  \n\
Reviewed bundles: `{reviewed_bundles}`  \n\
Incremental: `{incremental}`  \n\
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

/// Extracts the persisted review state embedded in a summary body.
///
/// Returns `Some` when the body contains valid serialized state between the
/// summary state markers, or `None` when the markers are missing or the state
/// cannot be deserialized.
///
/// # Examples
///
/// ```
/// assert!(parse_summary_state("not a summary").is_none());
/// ```
pub fn parse_summary_state(body: &str) -> Option<SummaryState> {
    let start = body.find(STATE_PREFIX)? + STATE_PREFIX.len();
    let rest = &body[start..];
    let end = rest.find(STATE_SUFFIX)?;
    serde_json::from_str(&rest[..end]).ok()
}

/// Formats a resolved finding as a Markdown comment body with its fingerprint, title, description, and optional evidence.
///
/// File-level findings also include a note explaining that the anchor could not be uniquely resolved.
///
/// # Examples
///
/// ```
/// # fn example(finding: &ResolvedFinding) {
/// let body = finding_body(finding);
/// assert!(body.starts_with("<!-- prbot:finding:"));
/// # }
/// ```
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
    let location = if finding.file_level {
        "\n\n_Anchor could not be uniquely resolved; posted as a file-level comment._"
    } else {
        ""
    };
    format!(
        "<!-- prbot:finding:{} -->\n**{:?} - {}**\n\n{}{}{}",
        finding.fingerprint,
        finding.candidate.priority,
        finding.candidate.title.trim(),
        finding.candidate.body.trim(),
        evidence,
        location
    )
}

#[cfg(test)]
mod tests {
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
            incremental: Some(false),
            reviewed_bundles: Some(1),
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

    #[test]
    fn forgets_fingerprints_for_changed_paths() {
        let mut state = SummaryState::default();
        state.remember_finding(&ResolvedFinding {
            candidate: CandidateFinding {
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
}
