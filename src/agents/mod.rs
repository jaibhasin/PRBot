#[cfg(test)]
mod integration_tests;
mod prompts;
mod router;
mod tasks;
mod verifier;

use crate::config::ReviewConfig;
use crate::llm::LlmClient;
use crate::repository::{
    execute_bounded_for_agent, is_agent_instructions, render_repo_map, tool_definitions,
    RepositoryTools,
};
use crate::types::{
    AgentRun, AgentStatus, CandidateFinding, ReviewAgent, ReviewBundle, ReviewManifest,
};
use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use std::sync::Arc;

pub struct AgentReviewResult {
    pub findings: Vec<CandidateFinding>,
    pub failed_bundles: Vec<String>,
    pub agent_runs: Vec<AgentRun>,
    pub router_fallback: bool,
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

    let routing = router::route(client, manifest, bundles, config).await;
    let mut runs = tasks::initial_agent_runs(bundles, &routing.assignments);
    let tasks = tasks::build_tasks(bundles, &routing.assignments);
    let repo_map = Arc::new(render_repo_map(manifest));
    let files = Arc::new(manifest.files.clone());
    let config = Arc::new(config.clone());
    let results = stream::iter(tasks)
        .map(|task| {
            let client = client.clone();
            let tools = Arc::clone(&tools);
            let repo_map = Arc::clone(&repo_map);
            let files = Arc::clone(&files);
            let config = Arc::clone(&config);
            async move {
                let agent = task.agent;
                let prompt =
                    prompts::review_prompt(agent, &task.bundles, &files, &repo_map, &config);
                let tool_runner = Arc::clone(&tools);
                let response =
                    client
                        .run_agent(
                            &config.review_model,
                            prompts::reviewer_system(agent),
                            &prompt,
                            tool_definitions(),
                            12,
                            move |name, arguments| {
                                let tools = Arc::clone(&tool_runner);
                                async move {
                                    execute_bounded_for_agent(tools, agent, name, arguments).await
                                }
                            },
                        )
                        .await
                        .and_then(|raw| parse_findings(&raw, agent));
                (task, response)
            }
        })
        .buffer_unordered(config.max_concurrency)
        .collect::<Vec<_>>()
        .await;

    let mut findings = Vec::new();
    let mut failed_bundles = Vec::new();
    for (task, result) in results {
        let run = runs
            .get_mut(&task.agent)
            .expect("every task has an agent run");
        match result {
            Ok(mut task_findings) => {
                run.candidate_findings += task_findings.len();
                findings.append(&mut task_findings);
            }
            Err(error) => {
                eprintln!("review task {} failed: {error:#}", task.label);
                run.status = AgentStatus::Failed;
                failed_bundles.push(task.label);
            }
        }
    }

    let verified =
        match verifier::verify_findings(client, tools, manifest, &findings, &config).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("independent verification failed: {error:#}");
                failed_bundles.push("independent-verifier".to_owned());
                Vec::new()
            }
        };
    for finding in &verified {
        if let Some(run) = runs.get_mut(&finding.agent) {
            run.accepted_findings += 1;
        }
    }

    AgentReviewResult {
        findings: verified,
        failed_bundles,
        agent_runs: ReviewAgent::REVIEWERS
            .into_iter()
            .filter_map(|agent| runs.remove(&agent))
            .collect(),
        router_fallback: routing.fallback,
    }
}

pub fn empty_result() -> AgentReviewResult {
    AgentReviewResult {
        findings: Vec::new(),
        failed_bundles: Vec::new(),
        agent_runs: tasks::empty_agent_runs(),
        router_fallback: false,
    }
}

fn parse_findings(raw: &str, agent: ReviewAgent) -> Result<Vec<CandidateFinding>> {
    let response: FindingResponse = parse_json(raw)?;
    Ok(response
        .findings
        .into_iter()
        .filter(|finding| {
            !finding.path.trim().is_empty()
                && !finding.anchor.trim().is_empty()
                && !finding.title.trim().is_empty()
                && !finding.body.trim().is_empty()
                && !(agent == ReviewAgent::Documentation
                    && (is_agent_instructions(&finding.path)
                        || finding
                            .evidence
                            .iter()
                            .any(|span| is_agent_instructions(&span.path))))
        })
        .map(|mut finding| {
            finding.agent = agent;
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
        let findings = parse_findings(raw, ReviewAgent::Security).expect("parse");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].agent, ReviewAgent::Security);
    }

    #[test]
    fn rejects_documentation_findings_that_target_agent_instructions() {
        let raw = r#"{"findings":[
            {"path":"AGENTS.md","side":"RIGHT","anchor":"rule","priority":"P2","category":"documentation","title":"Update instructions","body":"Change AGENTS.md.","evidence":[],"confidence":0.9},
            {"path":"src/a.rs","side":"RIGHT","anchor":"x","priority":"P2","category":"documentation","title":"Update instructions","body":"Change AGENTS.md.","evidence":[{"path":"nested/AGENTS.md","revision":"head","explanation":"target"}],"confidence":0.9}
        ]}"#;
        let findings = parse_findings(raw, ReviewAgent::Documentation).expect("parse");
        assert!(findings.is_empty());
    }
}
