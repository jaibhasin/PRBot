mod prompts;

use crate::config::ReviewConfig;
use crate::llm::LlmClient;
use crate::repository::{render_repo_map, tool_definitions, RepositoryTools};
use crate::types::{CandidateFinding, Priority, ReviewManifest};
use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use std::sync::Arc;

pub struct AgentReviewResult {
    pub findings: Vec<CandidateFinding>,
    pub failed_bundles: Vec<String>,
}

pub async fn review_manifest(
    client: &LlmClient,
    tools: Arc<RepositoryTools>,
    manifest: &ReviewManifest,
    config: &ReviewConfig,
) -> AgentReviewResult {
    let repo_map = Arc::new(render_repo_map(manifest));
    let files = Arc::new(manifest.files.clone());
    let config = Arc::new(config.clone());
    let results = stream::iter(manifest.bundles.clone())
        .map(|bundle| {
            let client = client.clone();
            let tools = Arc::clone(&tools);
            let repo_map = Arc::clone(&repo_map);
            let files = Arc::clone(&files);
            let config = Arc::clone(&config);
            async move {
                let prompt = prompts::bundle_prompt(&bundle, &files, &repo_map, &config);
                let system = prompts::reviewer_system();
                let tool_runner = Arc::clone(&tools);
                let response = client
                    .run_agent(
                        &config.review_model,
                        system,
                        &prompt,
                        tool_definitions(),
                        12,
                        move |name, arguments| {
                            let tools = Arc::clone(&tool_runner);
                            async move { tools.execute(&name, &arguments) }
                        },
                    )
                    .await;
                (bundle.id, response.and_then(|raw| parse_findings(&raw)))
            }
        })
        .buffer_unordered(config.max_concurrency)
        .collect::<Vec<_>>()
        .await;

    let mut findings = Vec::new();
    let mut failed_bundles = Vec::new();
    for (bundle, result) in results {
        match result {
            Ok(mut bundle_findings) => findings.append(&mut bundle_findings),
            Err(error) => {
                eprintln!("review bundle {bundle} failed: {error:#}");
                failed_bundles.push(bundle);
            }
        }
    }

    if manifest.bundles.len() > 1 {
        match run_cross_bundle_audit(client, Arc::clone(&tools), manifest, &config).await {
            Ok(mut audit_findings) => findings.append(&mut audit_findings),
            Err(error) => {
                eprintln!("cross-bundle audit failed: {error:#}");
                failed_bundles.push("cross-bundle-audit".to_owned());
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
    AgentReviewResult {
        findings: verified,
        failed_bundles,
    }
}

async fn run_cross_bundle_audit(
    client: &LlmClient,
    tools: Arc<RepositoryTools>,
    manifest: &ReviewManifest,
    config: &ReviewConfig,
) -> Result<Vec<CandidateFinding>> {
    let prompt = prompts::audit_prompt(manifest);
    let tool_runner = Arc::clone(&tools);
    let raw = client
        .run_agent(
            &config.review_model,
            prompts::auditor_system(),
            &prompt,
            tool_definitions(),
            10,
            move |name, arguments| {
                let tools = Arc::clone(&tool_runner);
                async move { tools.execute(&name, &arguments) }
            },
        )
        .await?;
    parse_findings(&raw)
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
                async move { tools.execute(&name, &arguments) }
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

    #[test]
    fn rejects_empty_finding_fields() {
        let raw = r#"{"findings":[{"path":"","side":"RIGHT","anchor":"x","priority":"P1","category":"correctness","title":"Bug","body":"Impact","evidence":[],"confidence":0.9}]}"#;
        assert!(parse_findings(raw).expect("parse").is_empty());
    }
}
