use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

/// Arguments for the `review` command.
#[derive(Debug, Parser)]
pub struct ReviewArgs {
    /// GitHub repository in `owner/repo` form.
    #[arg(long, env = "GITHUB_REPOSITORY")]
    pub repository: Option<String>,

    /// Pull request number to review.
    #[arg(long, env = "PRBOT_PR_NUMBER")]
    pub pr_number: Option<String>,

    /// OpenRouter (or compatible) API key.
    #[arg(long, env = "OPENROUTER_API_KEY", hide_env_values = true)]
    pub openrouter_api_key: Option<String>,

    /// GitHub token used to read the PR and post comments.
    #[arg(long, env = "GITHUB_TOKEN", hide_env_values = true)]
    pub github_token: Option<String>,

    /// Dry run: gather context and print what would be posted.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct EventPayload {
    number: Option<u64>,
    pull_request: Option<PullRequestRef>,
    issue: Option<IssueRef>,
}

#[derive(Debug, Deserialize)]
struct PullRequestRef {
    number: u64,
}

#[derive(Debug, Deserialize)]
struct IssueRef {
    number: u64,
    pull_request: Option<serde_json::Value>,
}

/// Entry point for PR review. Logic is stubbed for the initial scaffold.
pub async fn run(args: ReviewArgs) -> Result<()> {
    let dry_run = args.dry_run || env_flag("PRBOT_DRY_RUN");

    let repository = args
        .repository
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing --repository or GITHUB_REPOSITORY"))?;

    let pr_number = resolve_pr_number(args.pr_number.as_deref())?;

    if args.github_token.as_deref().unwrap_or("").is_empty() {
        bail!("missing --github-token or GITHUB_TOKEN");
    }

    if args.openrouter_api_key.as_deref().unwrap_or("").is_empty() && !dry_run {
        bail!("missing --openrouter-api-key or OPENROUTER_API_KEY (or pass --dry-run)");
    }

    // Placeholder until agents + GitHub/OpenRouter clients are wired up.
    println!("prbot review: repository={repository} pr=#{pr_number} dry_run={dry_run}");
    println!("Review agents are not implemented yet. Scaffold is ready.");

    Ok(())
}

fn env_flag(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn resolve_pr_number(explicit: Option<&str>) -> Result<u64> {
    if let Some(raw) = explicit {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed
                .parse::<u64>()
                .with_context(|| format!("invalid PR number '{trimmed}'"));
        }
    }

    let event_path = env::var_os("GITHUB_EVENT_PATH").map(PathBuf::from);
    let Some(path) = event_path else {
        bail!("missing --pr-number / PRBOT_PR_NUMBER and GITHUB_EVENT_PATH is unset");
    };

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read GitHub event file {}", path.display()))?;
    let event: EventPayload =
        serde_json::from_str(&raw).context("failed to parse GitHub event JSON")?;

    if let Some(pr) = event.pull_request {
        return Ok(pr.number);
    }
    if let Some(number) = event.number {
        return Ok(number);
    }
    if let Some(issue) = event.issue {
        if issue.pull_request.is_some() {
            return Ok(issue.number);
        }
    }

    bail!("could not determine pull request number from event payload");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_explicit_pr_number() {
        let number = resolve_pr_number(Some("17")).expect("number");
        assert_eq!(number, 17);
    }

    #[test]
    fn parses_pull_request_event_file() {
        let dir = tempfile_dir();
        let path = dir.join("event.json");
        let mut file = fs::File::create(&path).expect("create event");
        write!(file, r#"{{"pull_request":{{"number":99}}}}"#).expect("write event");

        env::set_var("GITHUB_EVENT_PATH", &path);
        let number = resolve_pr_number(None).expect("number from event");
        env::remove_var("GITHUB_EVENT_PATH");
        assert_eq!(number, 99);
    }

    fn tempfile_dir() -> PathBuf {
        let path = env::temp_dir().join(format!("prbot-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        path
    }
}
