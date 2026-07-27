#[cfg(test)]
mod integration_tests;
mod prompts;
mod verifier;

use crate::config::ReviewConfig;
use crate::llm::LlmClient;
use crate::repository::{
    execute_bounded_for_reviewer, is_agent_instructions, render_repo_map, tool_definitions,
    RepositoryTools,
};
use crate::types::{
    AgentRun, AgentStatus, CandidateFinding, ReviewAgent, ReviewBundle, ReviewManifest,
};
use anyhow::{Context, Result};
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

    let bundle_ids = bundles
        .iter()
        .map(|bundle| bundle.id.clone())
        .collect::<Vec<_>>();
    let mut run = AgentRun {
        agent: ReviewAgent::Primary,
        status: AgentStatus::Completed,
        bundle_ids,
        rationale: "One precision-first review across every selected bundle.".to_owned(),
        candidate_findings: 0,
        accepted_findings: 0,
    };
    let prompt =
        prompts::review_prompt(bundles, &manifest.files, &render_repo_map(manifest), config);
    let tool_runner = Arc::clone(&tools);
    let result = client
        .run_agent(
            &config.review_model,
            prompts::reviewer_system(),
            &prompt,
            tool_definitions(),
            12,
            move |name, arguments| {
                let tools = Arc::clone(&tool_runner);
                async move { execute_bounded_for_reviewer(tools, name, arguments).await }
            },
        )
        .await
        .and_then(|raw| parse_findings(&raw));
    let (findings, mut failed_bundles) = match result {
        Ok(findings) => {
            run.candidate_findings = findings.len();
            (findings, Vec::new())
        }
        Err(error) => {
            eprintln!("primary reviewer failed: {error:#}");
            run.status = AgentStatus::Failed;
            (Vec::new(), vec!["primary-reviewer".to_owned()])
        }
    };

    let verified = match verifier::verify_findings(client, tools, manifest, &findings, config).await
    {
        Ok(value) => value,
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
