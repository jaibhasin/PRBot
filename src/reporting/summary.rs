use crate::types::{AgentRun, AgentStatus, Priority, ResolvedFinding, RunOutcome, RunStatus};
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
    pub fingerprint_related_paths: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub fingerprint_priorities: BTreeMap<String, Priority>,
    #[serde(default)]
    pub coverage_complete: Option<bool>,
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
    ///     fingerprint_related_paths: BTreeMap::new(),
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
        let mut stale = self
            .fingerprint_paths
            .iter()
            .filter(|(_, path)| paths.contains(*path))
            .map(|(fingerprint, _)| fingerprint.clone())
            .collect::<BTreeSet<_>>();
        stale.extend(
            self.fingerprint_related_paths
                .iter()
                .filter(|(_, related)| !related.is_disjoint(paths))
                .map(|(fingerprint, _)| fingerprint.clone()),
        );
        for fingerprint in stale {
            self.fingerprints.remove(&fingerprint);
            self.fingerprint_paths.remove(&fingerprint);
            self.fingerprint_related_paths.remove(&fingerprint);
            self.fingerprint_priorities.remove(&fingerprint);
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
        self.fingerprint_related_paths.insert(
            finding.fingerprint.clone(),
            finding
                .candidate
                .evidence
                .iter()
                .map(|span| span.path.clone())
                .collect(),
        );
        self.fingerprint_priorities
            .insert(finding.fingerprint.clone(), finding.candidate.priority);
    }

    pub fn blocking_findings(&self) -> usize {
        self.fingerprints
            .iter()
            .filter(|fingerprint| {
                !matches!(
                    self.fingerprint_priorities.get(*fingerprint),
                    Some(Priority::P3)
                )
            })
            .count()
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
    let agent_sections = render_agent_sections(&outcome.agent_runs, outcome.router_fallback);
    format!(
        "{SUMMARY_MARKER}\n\
**PRBot contextual review: {status}**\n\n\
Repository: `{repository}#{pr_number}`  \n\
Reviewed head: `{}`  \n\
Coverage: `{}/{}` eligible hunks assigned  \n\
Reviewed bundles: `{reviewed_bundles}`  \n\
Incremental: `{incremental}`  \n\
Published findings: `{}`  \n\
Active unresolved findings: `{}`  \n\
Rejected or unanchored findings: `{}`  \n\
Failed stages: `{failures}`  \n\
Models: `{review_model}` reviewer, `{verification_model}` verifier  \n\
Budget: `{}` input tokens, `{}` output tokens, `${:.4}` estimated, `{}s`\n\n\
{agent_sections}\n\
{STATE_PREFIX}{encoded}{STATE_SUFFIX}\n",
        outcome.reviewed_sha,
        outcome.assigned_hunks,
        outcome.eligible_hunks,
        findings.len(),
        outcome.active_findings,
        outcome.skipped_findings,
        outcome.budget.input_tokens,
        outcome.budget.output_tokens,
        outcome.budget.estimated_cost_usd,
        outcome.budget.elapsed_seconds
    )
}

/// Renders the combined formal review body with one section per review agent.
pub fn render_review_body(agent_runs: &[AgentRun], router_fallback: bool) -> String {
    format!(
        "PRBot independently verified the inline findings below.\n\n{}",
        render_agent_sections(agent_runs, router_fallback)
    )
}

/// Renders stable, ordered status sections for all review agents.
pub fn render_agent_sections(agent_runs: &[AgentRun], router_fallback: bool) -> String {
    let fallback = if router_fallback {
        "> Router fallback: routing failed, so every specialist ran.\n\n"
    } else {
        ""
    };
    let sections = agent_runs
        .iter()
        .map(|run| {
            let status = match run.status {
                AgentStatus::Skipped => "Skipped".to_owned(),
                AgentStatus::Completed => format!(
                    "Completed - {} verified finding(s) from {} candidate(s)",
                    run.accepted_findings, run.candidate_findings
                ),
                AgentStatus::Failed => format!(
                    "Failed - {} verified finding(s) from completed tasks",
                    run.accepted_findings
                ),
            };
            let bundles = if run.bundle_ids.is_empty() {
                "none".to_owned()
            } else {
                run.bundle_ids.join(", ")
            };
            format!(
                "### {}\n\n{status}. Bundles: `{bundles}`. {}\n",
                run.agent.title(),
                sanitize_model_text(&run.rationale)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("## Agent review\n\n{fallback}{sections}")
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
    body.match_indices(STATE_PREFIX)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(|(start, _)| {
            let rest = &body[start + STATE_PREFIX.len()..];
            let end = rest.find(STATE_SUFFIX)?;
            serde_json::from_str(&rest[..end]).ok()
        })
}

fn sanitize_model_text(value: &str) -> String {
    value.replace("<!--", "&lt;!--").replace("-->", "--&gt;")
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
        "<!-- prbot:finding:{} -->\n**{} - {:?} - {}**\n\n{}{}{}",
        finding.fingerprint,
        finding.candidate.agent.title(),
        finding.candidate.priority,
        finding.candidate.title.trim(),
        finding.candidate.body.trim(),
        evidence,
        location
    )
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;
