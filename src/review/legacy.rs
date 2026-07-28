use crate::agents::AgentReviewResult;
use crate::config::ReviewConfig;
use crate::llm::{AgentCall, LlmClient};
use crate::types::{AgentRun, AgentStatus, CandidateFinding, ReviewAgent, ReviewManifest};
use serde::Deserialize;

/// Reviews the changes in a manifest using the legacy precision-first workflow.
///
/// Failed agent calls or response parsing produce an empty result with
/// `"legacy-review"` listed in `failed_bundles`.
///
/// # Examples
///
/// ```no_run
/// # async fn example(
/// #     client: &LlmClient,
/// #     manifest: &ReviewManifest,
/// #     config: &ReviewConfig,
/// # ) {
/// let result = review(client, manifest, config).await;
/// # }
/// ```
///
/// # Returns
///
/// An [`AgentReviewResult`] containing the parsed findings, or an error result
/// when the review cannot be completed.
pub async fn review(
    client: &LlmClient,
    manifest: &ReviewManifest,
    config: &ReviewConfig,
) -> AgentReviewResult {
    let patches = manifest
        .files
        .iter()
        .map(|file| format!("### {}\n```diff\n{}\n```", file.path, file.patch))
        .collect::<Vec<_>>()
        .join("\n\n");
    let prompt = format!(
        "Review only these changes for concrete introduced defects. Return JSON with `findings` using path, side RIGHT or LEFT, exact anchor text without diff prefix, optional end_anchor, priority P0-P3, category, title, body, evidence, and confidence. Return an empty array when there are no findings.\n\n{}",
        patches.chars().take(80_000).collect::<String>()
    );
    let response = client
        .run_agent(
            AgentCall {
                model: &config.review_model,
                system: "You are the legacy precision-first PR reviewer. Return JSON only.",
                user: &prompt,
                tools: Vec::new(),
                max_steps: 1,
                temperature: 0.0,
                label: "legacy",
            },
            |_name, _arguments| async { unreachable!("legacy engine has no tools") },
        )
        .await;
    match response.and_then(|raw| {
        let trimmed = raw.trim();
        let candidate = trimmed
            .strip_prefix("```json")
            .and_then(|value| value.strip_suffix("```"))
            .unwrap_or(trimmed);
        serde_json::from_str::<FindingResponse>(candidate).map_err(Into::into)
    }) {
        Ok(response) => {
            let mut findings = response.findings;
            for finding in &mut findings {
                finding.agent = ReviewAgent::Primary;
            }
            let finding_count = findings.len();
            AgentReviewResult {
                findings,
                failed_bundles: Vec::new(),
                agent_runs: vec![legacy_run(
                    AgentStatus::Completed,
                    finding_count,
                    finding_count,
                )],
            }
        }
        Err(error) => {
            eprintln!("legacy review failed: {error:#}");
            AgentReviewResult {
                findings: Vec::new(),
                failed_bundles: vec!["legacy-review".to_owned()],
                agent_runs: vec![legacy_run(AgentStatus::Failed, 0, 0)],
            }
        }
    }
}

fn legacy_run(
    status: AgentStatus,
    candidate_findings: usize,
    accepted_findings: usize,
) -> AgentRun {
    AgentRun {
        agent: ReviewAgent::Primary,
        status,
        bundle_ids: vec!["legacy-review".to_owned()],
        rationale: "Legacy rollback reviewer.".to_owned(),
        candidate_findings,
        accepted_findings,
    }
}

#[derive(Deserialize)]
struct FindingResponse {
    findings: Vec<CandidateFinding>,
}
