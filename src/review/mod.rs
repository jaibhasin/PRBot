mod contextual;
mod event;
mod legacy;

use crate::config::{PathRule, ReviewConfig, ReviewEngine};
use crate::github::{CheckRun, GitHubClient, Issue, PullRequest};
use crate::repository::{build_context, build_manifest, GitRepository};
use anyhow::{bail, Context, Result};
use clap::Parser;
use event::Command;
use serde::Serialize;
use std::env;
use std::sync::Arc;

#[derive(Debug, Parser)]
pub struct ReviewArgs {
    #[arg(long, env = "GITHUB_REPOSITORY")]
    pub repository: Option<String>,
    #[arg(long, env = "PRBOT_PR_NUMBER")]
    pub pr_number: Option<String>,
    #[arg(long, env = "OPENROUTER_API_KEY", hide_env_values = true)]
    pub openrouter_api_key: Option<String>,
    #[arg(long, env = "GITHUB_TOKEN", hide_env_values = true)]
    pub github_token: Option<String>,
    #[arg(long, env = "PRBOT_REVIEW_MODEL")]
    pub review_model: Option<String>,
    #[arg(long, env = "PRBOT_VERIFICATION_MODEL")]
    pub verification_model: Option<String>,
    #[arg(long, env = "PRBOT_MAX_REVIEW_MINUTES", default_value_t = 15)]
    pub max_review_minutes: u64,
    #[arg(long, env = "PRBOT_MAX_INPUT_TOKENS", default_value_t = 500_000)]
    pub max_input_tokens: u64,
    #[arg(long, env = "PRBOT_MAX_COST_USD", default_value_t = 3.0)]
    pub max_cost_usd: f64,
    #[arg(long, env = "PRBOT_MAX_CONCURRENCY", default_value_t = 8)]
    pub max_concurrency: usize,
    #[arg(long, env = "PRBOT_MAX_COMMENTS", default_value_t = 12)]
    pub max_comments: usize,
    #[arg(long, env = "PRBOT_ENGINE", default_value = "contextual")]
    pub engine: String,
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

pub async fn run(args: ReviewArgs) -> Result<()> {
    let repository = required(args.repository.as_deref(), "GITHUB_REPOSITORY")?.to_owned();
    let token = required(args.github_token.as_deref(), "GITHUB_TOKEN")?.to_owned();
    let event = event::read_event_payload()?;
    let pr_number = event::resolve_pr_number(args.pr_number.as_deref(), event.as_ref())?;
    let github = GitHubClient::new(&token, &repository)?;
    let pull_request = github.get_pull_request(pr_number).await?;
    let invocation = event::resolve_invocation(event.as_ref());

    let (actor, command, comment_id) = match invocation {
        event::Invocation::Ignored(reason) => {
            println!("PRBot skipped event: {reason}");
            return Ok(());
        }
        event::Invocation::Automatic => (pull_request.user.login.clone(), Command::Review, None),
        event::Invocation::Command {
            actor,
            command,
            comment_id,
        } => (actor, command, Some(comment_id)),
    };
    if !github.is_repository_admin(&actor).await? {
        if let Some(comment_id) = comment_id {
            github
                .create_issue_comment(
                    pr_number,
                    &format!(
                        "<!-- prbot-command:{comment_id} -->\nOnly repository owners can run `/prbot` commands."
                    ),
                )
                .await?;
        }
        println!("PRBot skipped unauthorized actor @{actor} before any model call");
        return Ok(());
    }

    let comments = github.list_issue_comments(pr_number).await?;
    if let Some(comment_id) = comment_id {
        if comments.iter().any(|item| {
            item.body
                .contains(&format!("<!-- prbot-command:{comment_id} -->"))
        }) {
            println!("PRBot skipped duplicate command event #{comment_id}");
            return Ok(());
        }
    }

    let mut config = config_from_args(&args)?;
    let dry_run = args.dry_run || env_flag("PRBOT_DRY_RUN");
    if !dry_run && args.openrouter_api_key.as_deref().unwrap_or("").is_empty() {
        bail!("missing --openrouter-api-key or OPENROUTER_API_KEY");
    }

    let repo_for_fetch = repository.clone();
    let base_ref = pull_request.base.ref_name.clone();
    let expected_head = pull_request.head.sha.clone();
    let fetch_token = token.clone();
    let git = tokio::task::spawn_blocking(move || {
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
    apply_trusted_repository_config(&git, &mut config)?;
    let filter = config.path_filter()?;
    let mut manifest = build_manifest(&git, &filter)?;
    build_context(&git, &mut manifest)?;
    let checks = github
        .list_check_runs(&pull_request.head.sha)
        .await
        .unwrap_or_default();
    let linked_issue = fetch_linked_issue(&github, &pull_request).await;
    let pr_context = render_pr_context(&pull_request, &checks, linked_issue.as_ref(), &config);

    if dry_run {
        let output = DryRunOutput {
            repository: &repository,
            pr_number,
            actor: &actor,
            command: command.name(),
            base_sha: git.base_sha(),
            head_sha: git.head_sha(),
            manifest: &manifest,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let api_key = args
        .openrouter_api_key
        .as_deref()
        .context("OpenRouter API key disappeared after validation")?;
    let git = Arc::new(git);
    match command {
        Command::Review => {
            contextual::run_review(
                &github,
                api_key,
                &repository,
                pr_number,
                &pull_request,
                git,
                manifest,
                pr_context,
                &comments,
                comment_id,
                &config,
            )
            .await
        }
        Command::Ask(question) | Command::Explain(question) => {
            contextual::answer_command(
                &github,
                api_key,
                pr_number,
                git,
                pr_context,
                &comments,
                comment_id.context("interactive command missing comment id")?,
                &question,
                &config,
            )
            .await
        }
    }
}

fn config_from_args(args: &ReviewArgs) -> Result<ReviewConfig> {
    let mut config = ReviewConfig {
        max_review_minutes: args.max_review_minutes,
        max_input_tokens: args.max_input_tokens,
        max_cost_usd: args.max_cost_usd,
        max_concurrency: args.max_concurrency.max(1),
        max_comments: args.max_comments,
        engine: ReviewEngine::parse(&args.engine)?,
        ..ReviewConfig::default()
    };
    if let Some(model) = args.review_model.as_ref().filter(|value| !value.is_empty()) {
        config.review_model = model.clone();
    }
    if let Some(model) = args
        .verification_model
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        config.verification_model = model.clone();
    }
    if config.review_model == config.verification_model {
        bail!("review_model and verification_model must use different model IDs");
    }
    Ok(config)
}

fn apply_trusted_repository_config(
    repository: &GitRepository,
    config: &mut ReviewConfig,
) -> Result<()> {
    if let Ok(source) = repository.read_file("base", ".prbot.toml", 100_000) {
        config.apply_repository_toml(&source)?;
    }
    for path in repository
        .list_tree("base")?
        .into_iter()
        .filter(|path| path == "AGENTS.md" || path.ends_with("/AGENTS.md"))
        .take(50)
    {
        let source = repository.read_file("base", &path, 20_000)?;
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

async fn fetch_linked_issue(github: &GitHubClient, pull_request: &PullRequest) -> Option<Issue> {
    let body = pull_request.body.as_deref()?;
    let number = body
        .split_whitespace()
        .filter_map(|word| {
            word.trim_matches(|c: char| !c.is_ascii_digit() && c != '#')
                .strip_prefix('#')
        })
        .filter_map(|value| value.parse::<u64>().ok())
        .find(|number| *number != pull_request.number)?;
    github.get_issue(number).await.ok()
}

fn render_pr_context(
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
                issue.body.as_deref().unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "(none)".to_owned());
    format!(
        "PR #{}: {}\nDescription:\n{}\n\nExisting checks:\n{}\n\nLinked issue:\n{}\n\nTrusted global instructions:\n{}",
        pull_request.number,
        pull_request.title,
        pull_request.body.as_deref().unwrap_or_default(),
        if checks.is_empty() { "(none)" } else { &checks },
        issue,
        config.instructions.join("\n")
    )
}

fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str> {
    value
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("missing required {name}"))
}

fn env_flag(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

#[derive(Serialize)]
struct DryRunOutput<'a> {
    repository: &'a str,
    pr_number: u64,
    actor: &'a str,
    command: &'a str,
    base_sha: &'a str,
    head_sha: &'a str,
    manifest: &'a crate::types::ReviewManifest,
}
