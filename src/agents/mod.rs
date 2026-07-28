mod cluster;
mod depth;
#[cfg(test)]
mod integration_tests;
mod prompts;
mod verifier;
mod walkthrough;

pub use walkthrough::generate_walkthrough;

use crate::config::ReviewConfig;
use crate::llm::{AgentCall, LlmClient};
use crate::repository::{
    execute_bounded_for_reviewer, is_agent_instructions, render_repo_map, tool_definitions,
    RepositoryTools,
};
use crate::types::{
    AgentRun, AgentStatus, CandidateFinding, ReviewAgent, ReviewBundle, ReviewManifest,
};
use anyhow::{Context, Result};
use futures::future::join_all;
use serde::Deserialize;
use std::sync::Arc;

pub struct AgentReviewResult {
    pub findings: Vec<CandidateFinding>,
    pub failed_bundles: Vec<String>,
    pub agent_runs: Vec<AgentRun>,
}

pub async fn review_manifest(
    client: &LlmClient,
    tools: Arc<RepositoryTools>,
    manifest: &ReviewManifest,
    config: &ReviewConfig,
) -> AgentReviewResult {
    review_bundles(client, tools, manifest, &manifest.bundles, config).await
}

pub async fn review_bundles(
    client: &LlmClient,
    tools: Arc<RepositoryTools>,
    manifest: &ReviewManifest,
    bundles: &[ReviewBundle],
    config: &ReviewConfig,
) -> AgentReviewResult {
    if bundles.is_empty() {
        return empty_result();
    }

    let risk = depth::max_bundle_risk(bundles);
    let plan = depth::depth_for(risk, config);
    let pass_plans = prompts::pass_plans(plan.primary_passes);
    let bundle_ids = bundles
        .iter()
        .map(|bundle| bundle.id.clone())
        .collect::<Vec<_>>();
    let mut run = AgentRun {
        agent: ReviewAgent::Primary,
        status: AgentStatus::Completed,
        bundle_ids,
        rationale: format!(
            "Precision-first review across every selected bundle using {} pass(es) at {:?} risk.",
            pass_plans.len(),
            risk
        ),
        candidate_findings: 0,
        accepted_findings: 0,
    };
    crate::progress::step(format!(
        "primary: reviewing {} bundle(s) passes={} steps={} risk={risk:?} model={}",
        bundles.len(),
        pass_plans.len(),
        plan.primary_max_steps,
        config.review_model
    ));

    let repo_map = render_repo_map(manifest);
    let mut pass_futures = Vec::new();
    for pass in &pass_plans {
        if client_budget_too_low(client).await {
            crate::progress::step(format!(
                "primary: skipping pass {} due to remaining budget",
                pass.index + 1
            ));
            break;
        }
        let prompt = prompts::review_prompt(bundles, &manifest.files, &repo_map, config, *pass);
        let tool_runner = Arc::clone(&tools);
        let model = config.review_model.clone();
        let label = format!("primary-pass-{}", pass.index + 1);
        let temperature = pass.temperature;
        let max_steps = plan.primary_max_steps;
        let client = client.clone();
        pass_futures.push(async move {
            client
                .run_agent(
                    AgentCall {
                        model: &model,
                        system: prompts::reviewer_system(),
                        user: &prompt,
                        tools: tool_definitions(),
                        max_steps,
                        temperature,
                        label: &label,
                    },
                    move |name, arguments| {
                        let tools = Arc::clone(&tool_runner);
                        async move { execute_bounded_for_reviewer(tools, name, arguments).await }
                    },
                )
                .await
                .and_then(|raw| parse_findings(&raw))
        });
    }

    let pass_results = join_all(pass_futures).await;
    let mut pass_findings = Vec::new();
    let mut any_success = false;
    for (index, result) in pass_results.into_iter().enumerate() {
        match result {
            Ok(findings) => {
                any_success = true;
                crate::progress::step(format!(
                    "primary: pass {} produced {} candidate finding(s)",
                    index + 1,
                    findings.len()
                ));
                pass_findings.push(findings);
            }
            Err(error) => {
                eprintln!("primary reviewer pass {} failed: {error:#}", index + 1);
                pass_findings.push(Vec::new());
            }
        }
    }

    let mut failed_bundles = Vec::new();
    if !any_success {
        run.status = AgentStatus::Failed;
        failed_bundles.push("primary-reviewer".to_owned());
    }
    let merged = cluster::merge_pass_findings(
        &pass_findings,
        config.majority_k,
        config.keep_high_confidence_singleton,
        config.max_comments.saturating_mul(3).max(12),
    );
    run.candidate_findings = merged.len();
    crate::progress::step(format!(
        "primary: merged {} candidate finding(s) from {} pass(es)",
        merged.len(),
        pass_findings.len()
    ));

    crate::progress::step(format!("verifier: start candidates={}", merged.len()));
    let verified = match verifier::verify_findings(
        client,
        tools,
        manifest,
        &merged,
        config,
        plan.verifier_max_steps,
    )
    .await
    {
        Ok(value) => {
            crate::progress::step(format!("verifier: accepted {} finding(s)", value.len()));
            value
        }
        Err(error) => {
            eprintln!("independent verification failed: {error:#}");
            failed_bundles.push("independent-verifier".to_owned());
            Vec::new()
        }
    };
    for finding in &verified {
        debug_assert_eq!(finding.agent, ReviewAgent::Primary);
        run.accepted_findings += 1;
    }

    AgentReviewResult {
        findings: verified,
        failed_bundles,
        agent_runs: vec![run],
    }
}

pub fn empty_result() -> AgentReviewResult {
    AgentReviewResult {
        findings: Vec::new(),
        failed_bundles: Vec::new(),
        agent_runs: vec![AgentRun {
            agent: ReviewAgent::Primary,
            status: AgentStatus::Skipped,
            bundle_ids: Vec::new(),
            rationale: "No review bundles were selected.".to_owned(),
            candidate_findings: 0,
            accepted_findings: 0,
        }],
    }
}

async fn client_budget_too_low(client: &LlmClient) -> bool {
    // Heuristic: leave room for at least one completion + verifier.
    client.remaining_input_tokens().await < 8_000 || client.remaining_time_secs() < 30
}

fn parse_findings(raw: &str) -> Result<Vec<CandidateFinding>> {
    let response: FindingResponse = parse_json(raw)?;
    Ok(response
        .findings
        .into_iter()
        .filter(|finding| {
            !finding.path.trim().is_empty()
                && !finding.anchor.trim().is_empty()
                && !finding.title.trim().is_empty()
                && !finding.body.trim().is_empty()
                && !is_agent_instructions(&finding.path)
                && !finding
                    .evidence
                    .iter()
                    .any(|span| is_agent_instructions(&span.path))
        })
        .map(|mut finding| {
            finding.agent = ReviewAgent::Primary;
            finding
        })
        .collect())
}

fn parse_json<T: for<'de> Deserialize<'de>>(raw: &str) -> Result<T> {
    let trimmed = raw.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    serde_json::from_str(candidate).context("agent returned invalid structured JSON")
}

#[derive(Deserialize)]
struct FindingResponse {
    findings: Vec<CandidateFinding>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_agent_identity_and_rejects_empty_fields() {
        let raw = r#"{"findings":[
            {"path":"","side":"RIGHT","anchor":"x","priority":"P1","category":"correctness","title":"Bug","body":"Impact","evidence":[],"confidence":0.9},
            {"path":"src/a.rs","side":"RIGHT","anchor":"x","priority":"P1","category":"security","title":"Bug","body":"Impact","evidence":[],"confidence":0.9}
        ]}"#;
        let findings = parse_findings(raw).expect("parse");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].agent, ReviewAgent::Primary);
    }

    #[test]
    fn rejects_findings_that_target_agent_instructions() {
        let raw = r#"{"findings":[
            {"path":"AGENTS.md","side":"RIGHT","anchor":"rule","priority":"P2","category":"documentation","title":"Update instructions","body":"Change AGENTS.md.","evidence":[],"confidence":0.9},
            {"path":"src/a.rs","side":"RIGHT","anchor":"x","priority":"P2","category":"documentation","title":"Update instructions","body":"Change AGENTS.md.","evidence":[{"path":"nested/AGENTS.md","revision":"head","explanation":"target"}],"confidence":0.9}
        ]}"#;
        let findings = parse_findings(raw).expect("parse");
        assert!(findings.is_empty());
    }
}
