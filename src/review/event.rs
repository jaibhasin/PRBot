use crate::github::GitHubUser;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct EventPayload {
    pub action: Option<String>,
    pub number: Option<u64>,
    pub pull_request: Option<PullRequestRef>,
    pub issue: Option<IssueRef>,
    pub comment: Option<EventComment>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestRef {
    number: u64,
}

#[derive(Debug, Deserialize)]
pub struct IssueRef {
    number: u64,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct EventComment {
    id: u64,
    body: String,
    user: GitHubUser,
}

#[derive(Clone, Debug)]
pub enum Command {
    Review,
    Ask(String),
    Explain(String),
}

impl Command {
    pub fn name(&self) -> &str {
        match self {
            Self::Review => "review",
            Self::Ask(_) => "ask",
            Self::Explain(_) => "explain",
        }
    }
}

pub enum Invocation {
    Automatic,
    Command {
        actor: String,
        command: Command,
        comment_id: u64,
    },
    Ignored(&'static str),
}

pub fn resolve_invocation(event: Option<&EventPayload>) -> Invocation {
    let Some(event) = event else {
        return Invocation::Automatic;
    };
    let Some(comment) = event.comment.as_ref() else {
        return Invocation::Automatic;
    };
    if event.action.as_deref().unwrap_or("created") != "created" {
        return Invocation::Ignored("comment action was not created");
    }
    if comment.user.user_type.eq_ignore_ascii_case("Bot") {
        return Invocation::Ignored("bot comment");
    }
    let Some(command) = parse_command(&comment.body) else {
        return Invocation::Ignored("comment did not start with /prbot");
    };
    Invocation::Command {
        actor: comment.user.login.clone(),
        command,
        comment_id: comment.id,
    }
}

pub fn resolve_pr_number(explicit: Option<&str>, event: Option<&EventPayload>) -> Result<u64> {
    if let Some(raw) = explicit.filter(|value| !value.trim().is_empty()) {
        return raw
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid PR number '{}'", raw.trim()));
    }
    let Some(event) = event else {
        bail!("missing PRBOT_PR_NUMBER and GITHUB_EVENT_PATH");
    };
    if let Some(pr) = &event.pull_request {
        return Ok(pr.number);
    }
    if let Some(number) = event.number {
        return Ok(number);
    }
    if let Some(issue) = &event.issue {
        if issue.pull_request.is_some() {
            return Ok(issue.number);
        }
    }
    bail!("could not determine pull request number from event payload")
}

pub fn read_event_payload() -> Result<Option<EventPayload>> {
    let Some(path) = env::var_os("GITHUB_EVENT_PATH").map(PathBuf::from) else {
        return Ok(None);
    };
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read GitHub event file {}", path.display()))?;
    Ok(Some(
        serde_json::from_str(&raw).context("failed to parse GitHub event JSON")?,
    ))
}

fn parse_command(body: &str) -> Option<Command> {
    let trimmed = body.trim();
    let mut parts = trimmed.splitn(3, char::is_whitespace);
    if !parts.next()?.eq_ignore_ascii_case("/prbot") {
        return None;
    }
    match parts
        .next()
        .unwrap_or("review")
        .to_ascii_lowercase()
        .as_str()
    {
        "review" => Some(Command::Review),
        "ask" => {
            let question = parts.next().unwrap_or_default().trim();
            (!question.is_empty()).then(|| Command::Ask(question.to_owned()))
        }
        "explain" => {
            let target = parts.next().unwrap_or_default().trim();
            (!target.is_empty()).then(|| Command::Explain(target.to_owned()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_explicit_prbot_commands() {
        assert!(matches!(
            parse_command("/prbot review"),
            Some(Command::Review)
        ));
        assert!(matches!(
            parse_command("/prbot ask why?"),
            Some(Command::Ask(_))
        ));
        assert!(parse_command("please review this").is_none());
        assert!(parse_command("@prbot review").is_none());
    }
}
