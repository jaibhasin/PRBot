mod prompts;

use crate::config::ReviewConfig;
use crate::llm::LlmClient;
use crate::repository::{execute_bounded, render_repo_map, tool_definitions, RepositoryTools};
use crate::types::{CandidateFinding, Priority, ReviewBundle, ReviewManifest, RiskLevel};
use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use std::sync::Arc;

pub struct AgentReviewResult {
    pub findings: Vec<CandidateFinding>,
    pub failed_bundles: Vec<String>,
}

/// Reviews all bundles in a manifest and returns the accepted findings and failed bundle identifiers.
///
/// # Examples
///
/// ```no_run
/// let result = review_manifest(&client, tools, &manifest, &config).await;
/// println!("{} findings", result.findings.len());
/// ```
pub async fn review_manifest(
    client: &LlmClient,
    tools: Arc<RepositoryTools>,
    manifest: &ReviewManifest,
    config: &ReviewConfig,
) -> AgentReviewResult {
    review_bundles(client, tools, manifest, &manifest.bundles, config).await
}

/// Reviews the specified bundles and returns verified findings together with any failed review stages.
///
/// Review tasks are run concurrently, followed by a cross-bundle audit when multiple bundles
/// are available and independent verification of the collected findings.
///
/// # Examples
///
/// ```ignore
/// let result = review_bundles(&client, tools, &manifest, &bundles, &config).await;
/// assert!(result.failed_bundles.is_empty());
/// ```
pub async fn review_bundles(
pub async fn review_bundles(
    client: &LlmClient,
    tools: Arc<RepositoryTools>,
    manifest: &ReviewManifest,
    bundles: &[ReviewBundle],
    config: &ReviewConfig,
) -> AgentReviewResult {
    if bundles.is_empty() {
        return AgentReviewResult {
            findings: Vec::new(),
            failed_bundles: Vec::new(),
        };
    }
    let repo_map = Arc::new(render_repo_map(manifest));
    let files = Arc::new(manifest.files.clone());
    let config = Arc::new(config.clone());
    let tasks = bundles
        .iter()
        .flat_map(|bundle| {
            review_roles(bundle)
                .into_iter()
                .map(|role| (bundle.clone(), role))
        })
        .collect::<Vec<_>>();
    let results = stream::iter(tasks)
        .map(|(bundle, role)| {
            let client = client.clone();
            let tools = Arc::clone(&tools);
            let repo_map = Arc::clone(&repo_map);
            let files = Arc::clone(&files);
            let config = Arc::clone(&config);
            async move {
                let prompt = prompts::bundle_prompt(&bundle, role, &files, &repo_map, &config);
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
                            async move { execute_bounded(tools, name, arguments).await }
                        },
                    )
                    .await;
                (
                    format!("{}:{role}", bundle.id),
                    response.and_then(|raw| parse_findings(&raw)),
                )
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

    if bundles.len() > 1 || manifest.bundles.len() > 1 {
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

/// Selects review roles based on a bundle's risk level and paths.
///
/// # Examples
///
/// ```
/// let bundle = ReviewBundle {
///     id: "auth".to_string(),
///     paths: vec!["src/auth.rs".to_string()],
///     risk: RiskLevel::Critical,
/// };
///
/// let roles = review_roles(&bundle);
/// assert!(roles.iter().any(|role| role.contains("security")));
/// ```
fn review_roles(bundle: &ReviewBundle) -> Vec<&'static str> {
    let mut roles = vec!["correctness and reliability"];
    let joined = bundle.paths.join(" ").to_ascii_lowercase();
    match bundle.risk {
        RiskLevel::Critical => {
            roles.push("security and authorization boundaries");
            roles.push("api contracts and compatibility");
        }
        RiskLevel::High => {
            roles.push("concurrency, state transitions, and compatibility");
            if joined.contains("perf")
                || joined.contains("cache")
                || joined.contains("bench")
                || joined.contains("hot")
            {
                roles.push("performance and resource usage");
            }
        }
        RiskLevel::Medium => {
            if joined.contains("api")
                || joined.contains("schema")
                || joined.contains("proto")
                || joined.contains("openapi")
            {
                roles.push("api contracts and compatibility");
            }
            if joined.contains("async")
                || joined.contains("thread")
                || joined.contains("mutex")
                || joined.contains("channel")
            {
                roles.push("concurrency and shared-state hazards");
            }
        }
        RiskLevel::Low => {}
    }
    roles.sort_unstable();
    roles.dedup();
    roles
}

/// Audits relationships and consistency across multiple review bundles.
///
/// # Errors
///
/// Returns an error if the audit agent fails or produces invalid findings.
///
/// # Examples
///
/// ```ignore
/// let findings = run_cross_bundle_audit(&client, tools, &manifest, &config).await?;
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// Returns the findings produced by the cross-bundle audit.
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
                async move { execute_bounded(tools, name, arguments).await }
            },
        )
        .await?;
    parse_findings(&raw)
}

/// Independently verifies candidate findings and returns the accepted findings in priority order.
///
/// A finding is accepted only when its index is selected by the verifier, its priority is above `P3`, and its confidence is at least `0.8`.
///
/// # Examples
///
/// ```rust,no_run
/// # async fn example(
/// #     client: &LlmClient,
/// #     tools: std::sync::Arc<RepositoryTools>,
/// #     manifest: &ReviewManifest,
/// #     config: &ReviewConfig,
/// # ) -> Result<()> {
/// let accepted = verify_findings(client, tools, manifest, &[], config).await?;
/// assert!(accepted.is_empty());
/// # Ok(())
/// # }
/// ```
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

/// Parses agent output into findings and discards entries with empty required fields.
///
/// # Examples
///
/// ```
/// let findings = parse_findings(r#"{"findings":[]}"#).unwrap();
/// assert!(findings.is_empty());
/// ```
///
/// Returns the valid findings parsed from the response.
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

/// Parses agent output as JSON, including responses wrapped in a `json` code fence.
///
/// # Examples
///
/// ```
/// let value: serde_json::Value = parse_json(r#"{"accepted": true}"#).unwrap();
/// assert_eq!(value["accepted"], true);
/// ```
///
/// Returns an error with context when the input is not valid JSON.
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

    #[test]
    fn rejects_empty_finding_fields() {
        let raw = r#"{"findings":[{"path":"","side":"RIGHT","anchor":"x","priority":"P1","category":"correctness","title":"Bug","body":"Impact","evidence":[],"confidence":0.9}]}"#;
        assert!(parse_findings(raw).expect("parse").is_empty());
    }

    #[test]
    fn critical_bundles_get_security_and_api_specialists() {
        let bundle = ReviewBundle {
            id: "bundle-1".to_owned(),
            paths: vec!["src/auth/api.rs".to_owned()],
            hunk_count: 1,
            risk: RiskLevel::Critical,
            related_files: Vec::new(),
        };
        let roles = review_roles(&bundle);
        assert!(roles.iter().any(|role| role.contains("security")));
        assert!(roles.iter().any(|role| role.contains("api")));
    }
}
