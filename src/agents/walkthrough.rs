use super::prompts;
use crate::config::ReviewConfig;
use crate::llm::{Budget, LlmClient};
use crate::repository::render_repo_map;
use crate::types::{ChangedFile, ResolvedFinding, ReviewBundle, ReviewManifest};
use std::sync::Arc;
use std::time::Duration;

/// Generates a soft-fail walkthrough for the summary comment.
#[allow(clippy::too_many_arguments)]
pub async fn generate_walkthrough(
    client: &LlmClient,
    budget: &Arc<Budget>,
    pr_context: &str,
    manifest: &ReviewManifest,
    bundles: &[ReviewBundle],
    files: &[ChangedFile],
    findings: &[ResolvedFinding],
    config: &ReviewConfig,
) -> Option<String> {
    if !config.enable_walkthrough {
        return None;
    }
    if budget.remaining_time().ok()?.as_secs() < 5 {
        return None;
    }
    let prompt = prompts::walkthrough_prompt(
        pr_context,
        bundles,
        files,
        &render_repo_map(manifest),
        findings,
        config,
    );
    let future = client.respond(
        &config.review_model,
        prompts::walkthrough_system(),
        &prompt,
        1_500,
    );
    match tokio::time::timeout(Duration::from_secs(20), future).await {
        Ok(Ok(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else if trimmed.starts_with("## Walkthrough") {
                Some(trimmed.to_owned())
            } else {
                Some(format!("## Walkthrough\n\n{trimmed}"))
            }
        }
        Ok(Err(error)) => {
            eprintln!("walkthrough generation failed: {error:#}");
            None
        }
        Err(_) => {
            eprintln!("walkthrough generation timed out");
            None
        }
    }
}
