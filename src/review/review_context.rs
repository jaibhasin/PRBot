use crate::config::{PathRule, ReviewConfig};
use crate::github::{CheckRun, GitHubClient, Issue, PullRequest};
use crate::repository::{build_context, build_manifest, GitRepository};
use crate::types::ReviewManifest;
use anyhow::{Context, Result};
use std::sync::Arc;

pub struct PreparedSnapshot {
    pub repository: Arc<GitRepository>,
    pub manifest: ReviewManifest,
    pub pr_context: String,
}

pub async fn prepare_snapshot(
    github: &GitHubClient,
    repository_name: &str,
    token: &str,
    pr_number: u64,
    pull_request: &PullRequest,
    config: &mut ReviewConfig,
    load_trusted_config: bool,
) -> Result<PreparedSnapshot> {
    let repo_for_fetch = repository_name.to_owned();
    let base_ref = pull_request.base.ref_name.clone();
    let expected_head = pull_request.head.sha.clone();
    let fetch_token = token.to_owned();
    let repository = tokio::task::spawn_blocking(move || {
        GitRepository::fetch_pull_request(
            &repo_for_fetch,
            pr_number,
            &base_ref,
            &expected_head,
            &fetch_token,
        )
    })
    .await
    .context("repository fetch task failed")??;
    let mut working_config = config.clone();
    let (repository, manifest, working_config) = tokio::task::spawn_blocking(move || {
        if load_trusted_config {
            apply_trusted_repository_config(&repository, &mut working_config)?;
        }
        let filter = working_config.path_filter()?;
        let mut manifest = build_manifest(&repository, &filter)?;
        build_context(&repository, &mut manifest)?;
        Ok::<_, anyhow::Error>((repository, manifest, working_config))
    })
    .await
    .context("repository context task failed")??;
    *config = working_config;
    let checks = github
        .list_check_runs(&pull_request.head.sha)
        .await
        .unwrap_or_default();
    let linked_issue = fetch_linked_issue(github, pull_request).await;
    let pr_context = render_pr_context(pull_request, &checks, linked_issue.as_ref(), config);
    Ok(PreparedSnapshot {
        repository: Arc::new(repository),
        manifest,
        pr_context,
    })
}

pub fn apply_trusted_repository_config(
    repository: &GitRepository,
    config: &mut ReviewConfig,
) -> Result<()> {
    if let Ok(source) = repository.read_file("base", ".prbot.toml", 100_000) {
        config.apply_repository_toml(&source)?;
    }
    let mut instruction_chars = config.instructions.iter().map(String::len).sum::<usize>();
    for path in repository
        .list_tree("base")?
        .into_iter()
        .filter(|path| path == "AGENTS.md" || path.ends_with("/AGENTS.md"))
        .take(50)
    {
        if instruction_chars >= 50_000 {
            break;
        }
        let remaining = 50_000 - instruction_chars;
        let source = repository.read_file("base", &path, remaining.min(20_000))?;
        instruction_chars += source.len();
        if path == "AGENTS.md" {
            config.instructions.push(source);
        } else if let Some(directory) = path.strip_suffix("/AGENTS.md") {
            config.path_rules.push(PathRule {
                glob: format!("{directory}/**"),
                instructions: vec![source],
            });
        }
    }
    Ok(())
}

pub async fn fetch_linked_issue(
    github: &GitHubClient,
    pull_request: &PullRequest,
) -> Option<Issue> {
    let body = pull_request.body.as_deref()?;
    let number = body
        .split_whitespace()
        .filter_map(|word| {
            word.trim_matches(|character: char| !character.is_ascii_digit() && character != '#')
                .strip_prefix('#')
        })
        .filter_map(|value| value.parse::<u64>().ok())
        .find(|number| *number != pull_request.number)?;
    github.get_issue(number).await.ok()
}

pub fn render_pr_context(
    pull_request: &PullRequest,
    checks: &[CheckRun],
    linked_issue: Option<&Issue>,
    config: &ReviewConfig,
) -> String {
    let checks = checks
        .iter()
        .take(50)
        .map(|check| {
            let details = check
                .output
                .as_ref()
                .map(|output| {
                    format!(
                        " - {} {}",
                        output.title.as_deref().unwrap_or_default(),
                        output.summary.as_deref().unwrap_or_default()
                    )
                })
                .unwrap_or_default();
            format!(
                "{}: {} / {}{}",
                check.name,
                check.status,
                check.conclusion.as_deref().unwrap_or("pending"),
                details
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let issue = linked_issue
        .map(|issue| {
            format!(
                "#{} {}\n{}",
                issue.number,
                issue.title,
                truncate(issue.body.as_deref().unwrap_or_default(), 10_000)
            )
        })
        .unwrap_or_else(|| "(none)".to_owned());
    format!(
        "PR #{}: {}\nDescription:\n{}\n\nExisting checks:\n{}\n\nLinked issue:\n{}\n\nTrusted global instructions:\n{}",
        pull_request.number,
        pull_request.title,
        truncate(pull_request.body.as_deref().unwrap_or_default(), 10_000),
        if checks.is_empty() { "(none)" } else { &checks },
        issue,
        truncate(&config.instructions.join("\n"), 50_000)
    )
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
