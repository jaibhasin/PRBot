mod prompts;
mod router;

use crate::config::ReviewConfig;
use crate::llm::LlmClient;
use crate::repository::{execute_bounded, render_repo_map, tool_definitions, RepositoryTools};
use crate::types::{
    AgentRun, AgentStatus, CandidateFinding, Priority, ReviewAgent, ReviewBundle, ReviewManifest,
};
use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use std::collections::BTreeMap;
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
    let mut runs = initial_agent_runs(bundles, &routing.assignments);
    let tasks = build_tasks(bundles, &routing.assignments);
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
                let prompt =
                    prompts::review_prompt(task.agent, &task.bundles, &files, &repo_map, &config);
                let tool_runner = Arc::clone(&tools);
                let response = client
                    .run_agent(
                        &config.review_model,
                        prompts::reviewer_system(task.agent),
                        &prompt,
                        tool_definitions(),
                        12,
                        move |name, arguments| {
                            let tools = Arc::clone(&tool_runner);
                            async move { execute_bounded(tools, name, arguments).await }
                        },
                    )
                    .await
                    .and_then(|raw| parse_findings(&raw, task.agent));
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

    let verified = match verify_findings(client, tools, manifest, &findings, &config).await {
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

fn empty_result() -> AgentReviewResult {
    AgentReviewResult {
        findings: Vec::new(),
        failed_bundles: Vec::new(),
        agent_runs: ReviewAgent::REVIEWERS
            .into_iter()
            .map(|agent| AgentRun {
                agent,
                status: if agent == ReviewAgent::Correctness {
                    AgentStatus::Completed
                } else {
                    AgentStatus::Skipped
                },
                bundle_ids: Vec::new(),
                rationale: "No review bundles were selected.".to_owned(),
                candidate_findings: 0,
                accepted_findings: 0,
            })
            .collect(),
        router_fallback: false,
    }
}

fn initial_agent_runs(
    bundles: &[ReviewBundle],
    assignments: &[router::RoutingAssignment],
) -> BTreeMap<ReviewAgent, AgentRun> {
    let all_bundle_ids = bundles
        .iter()
        .map(|bundle| bundle.id.clone())
        .collect::<Vec<_>>();
    ReviewAgent::REVIEWERS
        .into_iter()
        .map(|agent| {
            let assignment = assignments
                .iter()
                .find(|assignment| assignment.agent == agent);
            let run = if agent == ReviewAgent::Correctness {
                AgentRun {
                    agent,
                    status: AgentStatus::Completed,
                    bundle_ids: all_bundle_ids.clone(),
                    rationale: "Always-on review for every selected bundle.".to_owned(),
                    candidate_findings: 0,
                    accepted_findings: 0,
                }
            } else if let Some(assignment) = assignment {
                AgentRun {
                    agent,
                    status: AgentStatus::Completed,
                    bundle_ids: assignment.bundle_ids.clone(),
                    rationale: assignment.rationale.clone(),
                    candidate_findings: 0,
                    accepted_findings: 0,
                }
            } else {
                AgentRun {
                    agent,
                    status: AgentStatus::Skipped,
                    bundle_ids: Vec::new(),
                    rationale: "Not selected by the routing agent.".to_owned(),
                    candidate_findings: 0,
                    accepted_findings: 0,
                }
            };
            (agent, run)
        })
        .collect()
}

fn build_tasks(
    bundles: &[ReviewBundle],
    assignments: &[router::RoutingAssignment],
) -> Vec<ReviewTask> {
    let by_id = bundles
        .iter()
        .map(|bundle| (bundle.id.as_str(), bundle))
        .collect::<BTreeMap<_, _>>();
    let mut tasks = bundles
        .iter()
        .cloned()
        .map(|bundle| ReviewTask {
            agent: ReviewAgent::Correctness,
            label: format!("{}:correctness", bundle.id),
            bundles: vec![bundle],
        })
        .collect::<Vec<_>>();
    for assignment in assignments {
        let selected = assignment
            .bundle_ids
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).map(|bundle| (*bundle).clone()))
            .collect::<Vec<_>>();
        match assignment.agent {
            ReviewAgent::Security | ReviewAgent::Performance => {
                tasks.extend(selected.into_iter().map(|bundle| ReviewTask {
                    agent: assignment.agent,
                    label: format!("{}:{}", bundle.id, assignment.agent),
                    bundles: vec![bundle],
                }));
            }
            ReviewAgent::Architecture | ReviewAgent::Documentation => {
                tasks.push(ReviewTask {
                    agent: assignment.agent,
                    label: assignment.agent.to_string(),
                    bundles: selected,
                });
            }
            ReviewAgent::Correctness => {}
        }
    }
    tasks
}

#[derive(Debug)]
struct ReviewTask {
    agent: ReviewAgent,
    label: String,
    bundles: Vec<ReviewBundle>,
}

async fn verify_findings(
    client: &LlmClient,
    tools: Arc<RepositoryTools>,
    manifest: &ReviewManifest,
    findings: &[CandidateFinding],
    config: &ReviewConfig,
) -> Result<Vec<CandidateFinding>> {
    if findings.is_empty() {
        return Ok(Vec::new());
    }
    let prompt = prompts::verification_prompt(manifest, findings)?;
    let tool_runner = Arc::clone(&tools);
    let raw = client
        .run_agent(
            &config.verification_model,
            prompts::verifier_system(),
            &prompt,
            tool_definitions(),
            8,
            move |name, arguments| {
                let tools = Arc::clone(&tool_runner);
                async move { execute_bounded(tools, name, arguments).await }
            },
        )
        .await?;
    let response: VerificationResponse = parse_json(&raw)?;
    let mut accepted = response
        .accepted_indices
        .into_iter()
        .filter_map(|index| findings.get(index).cloned())
        .filter(|finding| finding.priority != Priority::P3 && finding.confidence >= 0.8)
        .collect::<Vec<_>>();
    accepted.sort_by_key(|finding| finding.priority);
    Ok(accepted)
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

#[derive(Deserialize)]
struct VerificationResponse {
    accepted_indices: Vec<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RiskLevel;

    fn bundle(id: &str) -> ReviewBundle {
        ReviewBundle {
            id: id.to_owned(),
            paths: vec![format!("src/{id}.rs")],
            hunk_count: 1,
            risk: RiskLevel::High,
            related_files: Vec::new(),
        }
    }

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
    fn builds_correctness_per_bundle_and_groups_pr_wide_specialists() {
        let bundles = vec![bundle("api"), bundle("config")];
        let assignments = vec![
            router::RoutingAssignment {
                agent: ReviewAgent::Architecture,
                bundle_ids: vec!["api".to_owned(), "config".to_owned()],
                rationale: "cross-file contract".to_owned(),
            },
            router::RoutingAssignment {
                agent: ReviewAgent::Security,
                bundle_ids: vec!["api".to_owned(), "config".to_owned()],
                rationale: "input boundary".to_owned(),
            },
        ];
        let tasks = build_tasks(&bundles, &assignments);
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.agent == ReviewAgent::Correctness)
                .count(),
            2
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.agent == ReviewAgent::Architecture)
                .count(),
            1
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.agent == ReviewAgent::Security)
                .count(),
            2
        );
    }
}
