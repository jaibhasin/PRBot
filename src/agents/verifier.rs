use super::{parse_json, prompts};
use crate::config::ReviewConfig;
use crate::llm::{AgentCall, LlmClient};
use crate::repository::{execute_bounded, tool_definitions, RepositoryTools};
use crate::types::{CandidateFinding, Priority, ReviewManifest};
use anyhow::Result;
use serde::Deserialize;
use std::sync::Arc;

pub(super) async fn verify_findings(
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
            AgentCall {
                model: &config.verification_model,
                system: prompts::verifier_system(),
                user: &prompt,
                tools: tool_definitions(),
                max_steps: 8,
                label: "verifier",
            },
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

#[derive(Deserialize)]
struct VerificationResponse {
    accepted_indices: Vec<usize>,
}
