mod commands;
mod contextual;
mod event;
mod incremental;
mod legacy;
mod review_context;
#[cfg(test)]
mod tests;

use crate::config::{ReviewConfig, ReviewEngine};
use crate::github::GitHubClient;
use crate::llm::Budget;
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
    #[arg(long, env = "GITHUB_API_URL", hide = true)]
    pub github_api_url: Option<String>,
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
    #[arg(long, env = "PRBOT_ENGINE", default_value = "legacy")]
    pub engine: String,
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

/// Runs a pull-request review or interactive PRBot command for the configured event.
///
/// Resolves the repository, pull request, invocation, authorization, review configuration,
/// and execution mode before dispatching the selected command. In dry-run mode, prints the
/// resolved review metadata and repository manifest without making model calls.
///
/// # Examples
///
/// ```no_run
/// # async fn example() -> anyhow::Result<()> {
/// let args = ReviewArgs::try_parse_from(["prbot", "--dry-run"])?;
/// run(args).await?;
/// # Ok(())
/// # }
/// ```
pub async fn run(args: ReviewArgs) -> Result<()> {
    let repository = required(args.repository.as_deref(), "GITHUB_REPOSITORY")?.to_owned();
    let token = required(args.github_token.as_deref(), "GITHUB_TOKEN")?.to_owned();
    let event = event::read_event_payload()?;
    let pr_number = event::resolve_pr_number(args.pr_number.as_deref(), event.as_ref())?;
    let github = if let Some(base_url) = &args.github_api_url {
        GitHubClient::with_base_url(&token, &repository, base_url)?
    } else {
        GitHubClient::new(&token, &repository)?
    };
    let pull_request = github.get_pull_request(pr_number).await?;
    let invocation = event::resolve_invocation(event.as_ref());
    let automatic = matches!(&invocation, event::Invocation::Automatic);

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

    let mut config = config_from_args(&args)?;
    let dry_run = args.dry_run || env_flag("PRBOT_DRY_RUN");
    if !dry_run && args.openrouter_api_key.as_deref().unwrap_or("").is_empty() {
        bail!("missing --openrouter-api-key or OPENROUTER_API_KEY");
    }
    let budget = (!dry_run).then(|| {
        Arc::new(Budget::new(
            config.max_review_minutes,
            config.max_input_tokens,
            config.max_cost_usd,
        ))
    });

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

    let prepare = review_context::prepare_snapshot(
        &github,
        &repository,
        &token,
        pr_number,
        &pull_request,
        &mut config,
        true,
    );
    let mut snapshot = if let Some(budget) = &budget {
        tokio::time::timeout(budget.remaining_time()?, prepare)
            .await
            .context("repository context preparation exceeded review deadline")??
    } else {
        prepare.await?
    };
    if automatic && !config.auto_review_owner_authored {
        println!("PRBot automatic review is disabled by trusted .prbot.toml");
        return Ok(());
    }
    if dry_run {
        let output = DryRunOutput {
            repository: &repository,
            pr_number,
            actor: &actor,
            command: command.name(),
            base_sha: snapshot.repository.base_sha(),
            head_sha: snapshot.repository.head_sha(),
            run: crate::types::ReviewRun {
                trigger: if automatic {
                    "automatic".to_owned()
                } else {
                    format!("command:{}", command.name())
                },
                actor: actor.clone(),
                repository: repository.clone(),
                pr_number,
                base_sha: snapshot.repository.base_sha().to_owned(),
                head_sha: snapshot.repository.head_sha().to_owned(),
                previous_head_sha: None,
                incremental: false,
            },
            manifest: &snapshot.manifest,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let api_key = args
        .openrouter_api_key
        .as_deref()
        .context("OpenRouter API key disappeared after validation")?;
    match command {
        Command::Review => {
            let budget = budget.context("review budget missing outside dry run")?;
            let mut reviewed_pull_request = pull_request;
            for attempt in 0..=1 {
                match contextual::run_review(
                    &github,
                    api_key,
                    &repository,
                    pr_number,
                    &reviewed_pull_request,
                    Arc::clone(&snapshot.repository),
                    &snapshot.manifest,
                    &snapshot.pr_context,
                    &comments,
                    comment_id,
                    &config,
                    Arc::clone(&budget),
                )
                .await?
                {
                    contextual::ReviewResult::Complete => return Ok(()),
                    contextual::ReviewResult::Stale(updated) if attempt == 0 => {
                        println!(
                            "PR head changed during review; retrying once at {}",
                            updated.head.sha
                        );
                        reviewed_pull_request = updated;
                        let prepare = review_context::prepare_snapshot(
                            &github,
                            &repository,
                            &token,
                            pr_number,
                            &reviewed_pull_request,
                            &mut config,
                            false,
                        );
                        snapshot = tokio::time::timeout(budget.remaining_time()?, prepare)
                            .await
                            .context("stale-head retry preparation exceeded review deadline")??;
                    }
                    contextual::ReviewResult::Stale(updated) => {
                        bail!(
                            "PR head changed again during retry to {}; no stale findings were published",
                            updated.head.sha
                        );
                    }
                }
            }
            unreachable!("review retry loop always returns")
        }
        Command::Ask(question) => {
            commands::answer_command(
                &github,
                api_key,
                pr_number,
                snapshot.repository,
                snapshot.pr_context,
                &comments,
                comment_id.context("interactive command missing comment id")?,
                &question,
                &config,
                budget.context("command budget missing outside dry run")?,
            )
            .await
        }
        Command::Explain(target) => {
            commands::answer_command(
                &github,
                api_key,
                pr_number,
                snapshot.repository,
                snapshot.pr_context,
                &comments,
                comment_id.context("interactive command missing comment id")?,
                &format!("Explain this PRBot finding in detail: {target}"),
                &config,
                budget.context("command budget missing outside dry run")?,
            )
            .await
        }
    }
}

/// Builds the review configuration from command-line arguments and validates its model settings.
///
/// # Examples
///
/// ```
/// use clap::Parser;
///
/// let args = ReviewArgs::parse_from(["prbot"]);
/// let config = config_from_args(&args).expect("default configuration is valid");
/// assert!(config.max_concurrency >= 1);
/// ```
///
/// # Errors
///
/// Returns an error if the engine is invalid, or if the review and verification
/// models use the same model ID or provider family.
///
/// # Returns
///
/// The validated review configuration.
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
    let review_provider = config.review_model.split('/').next().unwrap_or_default();
    let verifier_provider = config
        .verification_model
        .split('/')
        .next()
        .unwrap_or_default();
    if review_provider == verifier_provider {
        bail!("review_model and verification_model must use different provider families");
    }
    Ok(config)
}

/// Extracts a non-blank string from an optional value.
///
/// # Examples
///
/// ```
/// assert_eq!(required(Some("configured"), "setting").unwrap(), "configured");
/// assert!(required(Some("  "), "setting").is_err());
/// ```
///
/// # Arguments
///
/// * `value` - The optional string to validate.
/// * `name` - The setting name included in the error when the value is missing or blank.
///
/// # Returns
///
/// The original string when it contains non-whitespace characters.
fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str> {
    value
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("missing required {name}"))
}

/// Interprets recognized environment variable values as an enabled flag.
///
/// # Examples
///
/// ```
/// std::env::set_var("PRBOT_EXAMPLE_FLAG", "yes");
/// assert!(env_flag("PRBOT_EXAMPLE_FLAG"));
/// std::env::remove_var("PRBOT_EXAMPLE_FLAG");
/// ```
///
/// # Returns
///
/// `true` if the variable is set to `1`, `true`, `TRUE`, `yes`, or `YES`; `false` otherwise.
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
    run: crate::types::ReviewRun,
    manifest: &'a crate::types::ReviewManifest,
}
