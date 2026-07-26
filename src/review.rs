use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::github::{GitHubClient, GitHubUser, IssueComment, PullRequestFile};

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const OPENROUTER_MODEL: &str = "deepseek/deepseek-v4-flash";
const MAX_REVIEW_FILES: usize = 25;
const MAX_REVIEW_PATCH_CHARS: usize = 80_000;
const MAX_FINDINGS: usize = 12;

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
    action: Option<String>,
    number: Option<u64>,
    pull_request: Option<PullRequestRef>,
    issue: Option<IssueRef>,
    comment: Option<EventComment>,
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

#[derive(Debug, Deserialize)]
struct EventComment {
    id: u64,
    body: String,
    user: GitHubUser,
}

/// Entry point for PR review and pull request comment interactions.
pub async fn run(args: ReviewArgs) -> Result<()> {
    let dry_run = args.dry_run || env_flag("PRBOT_DRY_RUN");

    let repository = args
        .repository
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing --repository or GITHUB_REPOSITORY"))?;

    let event = read_event_payload()?;
    let pr_number = resolve_pr_number_from_event(args.pr_number.as_deref(), event.as_ref())?;

    let github_token = args
        .github_token
        .as_deref()
        .filter(|value| !value.is_empty());
    if github_token.is_none() {
        bail!("missing --github-token or GITHUB_TOKEN");
    }

    if args.openrouter_api_key.as_deref().unwrap_or("").is_empty() && !dry_run {
        bail!("missing --openrouter-api-key or OPENROUTER_API_KEY (or pass --dry-run)");
    }

    println!("prbot review: repository={repository} pr=#{pr_number} dry_run={dry_run}");

    if dry_run {
        return Ok(());
    }

    let api_key = args
        .openrouter_api_key
        .as_deref()
        .expect("OpenRouter API key was validated above");
    let github = GitHubClient::new(
        github_token.expect("GitHub token was validated above"),
        &repository,
    )?;

    if let Some(comment) = issue_comment_event(event.as_ref()) {
        if comment.user.user_type.eq_ignore_ascii_case("Bot") {
            println!(
                "Skipping bot comment #{} to prevent a feedback loop",
                comment.id
            );
            return Ok(());
        }

        let comments = github.list_issue_comments(pr_number).await?;
        let prompt = build_comment_prompt(comment, &comments);
        let raw_response = call_openrouter(api_key, &prompt).await?;
        let reply = parse_agent_reply(&raw_response)?;
        let posted = github
            .create_issue_comment(pr_number, &reply.comment)
            .await?;
        github
            .create_issue_comment_reaction(comment.id, &reply.reaction)
            .await?;

        println!(
            "Replied to human comment #{} with comment #{} and reaction {}",
            comment.id, posted.id, reply.reaction
        );
    } else {
        run_code_quality_review(&github, api_key, &repository, pr_number).await?;
    }

    Ok(())
}

async fn run_code_quality_review(
    github: &GitHubClient,
    api_key: &str,
    repository: &str,
    pr_number: u64,
) -> Result<()> {
    let files = github.list_pull_request_files(pr_number).await?;
    let selected_files = select_review_files(&files);
    if selected_files.is_empty() {
        let summary = format!(
            "<!-- prbot-code-quality -->\n\
**PRBot code-quality review**\n\n\
No reviewable source changes found in `{repository}#{pr_number}`.\n\
PRBot currently reviews common source/config files (for example `.ts`, `.js`, `.rs`, `.py`) and skips things like pure CSS-only diffs, lockfiles, and generated/vendor paths."
        );
        let posted = github.create_issue_comment(pr_number, &summary).await?;
        println!(
            "No reviewable source changes; posted summary comment #{}",
            posted.id
        );
        return Ok(());
    }

    let prompt = build_code_quality_prompt(repository, pr_number, &selected_files);
    let raw_response = call_openrouter(api_key, &prompt).await?;
    let findings = parse_code_quality_findings(&raw_response)?;
    let pull_request = github.get_pull_request(pr_number).await?;
    let commit_id = pull_request.head.sha;

    let mut posted = 0;
    let mut skipped = 0;
    for finding in findings.into_iter().take(MAX_FINDINGS) {
        if !is_valid_finding(&finding, &selected_files) {
            skipped += 1;
            println!(
                "Skipping invalid model finding for {}:{} (must reference an added line)",
                finding.path, finding.line
            );
            continue;
        }

        let body = format!(
            "**Code quality - {}**\n\n{}",
            finding.severity,
            finding.body.trim()
        );
        let comment = github
            .create_pull_request_review_comment(
                pr_number,
                &body,
                &commit_id,
                &finding.path,
                finding.line,
            )
            .await?;
        println!(
            "Posted code-quality comment #{} on {}:{}",
            comment.id, finding.path, finding.line
        );
        posted += 1;
    }

    let file_list = selected_files
        .iter()
        .map(|file| format!("- `{}`", file.filename))
        .collect::<Vec<_>>()
        .join("\n");
    let summary = if posted == 0 {
        format!(
            "<!-- prbot-code-quality -->\n\
**PRBot code-quality review**\n\n\
Looks fine from a high-confidence pass.\n\
No inline findings to post for `{repository}#{pr_number}`.\n\n\
Reviewed files:\n{file_list}\n\n\
_Skipped {skipped} invalid/unanchored model suggestion(s)._"
        )
    } else {
        format!(
            "<!-- prbot-code-quality -->\n\
**PRBot code-quality review**\n\n\
Posted **{posted}** inline finding(s) on `{repository}#{pr_number}`.\n\n\
Reviewed files:\n{file_list}\n\n\
_Skipped {skipped} invalid/unanchored model suggestion(s)._"
        )
    };
    let summary_comment = github.create_issue_comment(pr_number, &summary).await?;
    println!(
        "Code-quality agent posted {posted} inline comment(s) and summary #{}",
        summary_comment.id
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
struct CodeQualityReply {
    findings: Vec<CodeQualityFinding>,
}

#[derive(Clone, Debug, Deserialize)]
struct CodeQualityFinding {
    path: String,
    line: u64,
    severity: String,
    body: String,
}

fn select_review_files(files: &[PullRequestFile]) -> Vec<PullRequestFile> {
    let mut total_chars = 0;
    files
        .iter()
        .filter(|file| is_reviewable_source_file(file))
        .filter_map(|file| {
            let patch = file.patch.as_ref()?;
            if total_chars + patch.len() > MAX_REVIEW_PATCH_CHARS {
                return None;
            }
            total_chars += patch.len();
            Some(PullRequestFile {
                filename: file.filename.clone(),
                status: file.status.clone(),
                patch: Some(patch.clone()),
            })
        })
        .take(MAX_REVIEW_FILES)
        .collect()
}

fn is_reviewable_source_file(file: &PullRequestFile) -> bool {
    let name = file.filename.as_str();
    let extension = name.rsplit('.').next().unwrap_or_default();
    let supported = matches!(
        extension,
        "rs" | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "rb"
            | "php"
            | "cs"
            | "cpp"
            | "cc"
            | "c"
            | "h"
            | "hpp"
            | "swift"
            | "scala"
            | "sh"
            | "sql"
            | "yaml"
            | "yml"
            | "toml"
            | "css"
            | "scss"
            | "sass"
            | "less"
    );
    supported
        && file.status != "removed"
        && !name.ends_with(".min.js")
        && !name.contains("/generated/")
        && !name.contains("/vendor/")
        && !name.contains("/node_modules/")
}

fn build_code_quality_prompt(
    repository: &str,
    pr_number: u64,
    files: &[PullRequestFile],
) -> String {
    let patches = files
        .iter()
        .filter_map(|file| {
            file.patch
                .as_ref()
                .map(|patch| format!("### {}\n```diff\n{}\n```", file.filename, patch))
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        "You are the code-quality reviewer for pull request {repository}#{pr_number}.\n\
Review only the supplied changes. Find concrete, high-confidence correctness, security, reliability, or maintainability problems that this PR introduces.\n\
Do not comment on style, pre-existing issues, missing tests, or speculative concerns.\n\
Every finding must target a line beginning with '+' in the diff (not the +++ file header).\n\
Return only valid JSON in this exact shape:\n\
{{\"findings\":[{{\"path\":\"src/example.rs\",\"line\":42,\"severity\":\"high|medium|low\",\"body\":\"Concise Markdown explanation, impact, and a specific fix.\"}}]}}\n\
Return {{\"findings\":[]}} when there are no high-confidence findings.\n\
Changed files:\n\n{patches}"
    )
}

fn parse_code_quality_findings(raw: &str) -> Result<Vec<CodeQualityFinding>> {
    let trimmed = raw.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let reply: CodeQualityReply =
        serde_json::from_str(candidate).context("code-quality agent returned invalid JSON")?;
    Ok(reply.findings)
}

fn is_valid_finding(finding: &CodeQualityFinding, files: &[PullRequestFile]) -> bool {
    if finding.body.trim().is_empty()
        || !matches!(finding.severity.as_str(), "high" | "medium" | "low")
    {
        return false;
    }
    files
        .iter()
        .find(|file| file.filename == finding.path)
        .and_then(|file| file.patch.as_deref())
        .is_some_and(|patch| added_line_numbers(patch).contains(&finding.line))
}

fn added_line_numbers(patch: &str) -> Vec<u64> {
    let mut line_number = 0_u64;
    let mut added = Vec::new();
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            if let Some(new_range) = header
                .split_whitespace()
                .nth(1)
                .and_then(|value| value.strip_prefix('+'))
            {
                line_number = new_range
                    .split(',')
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
            }
        } else if line.starts_with('\\') {
            // "\ No newline at end of file" markers describe the previous
            // line and do not correspond to a line in either file.
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added.push(line_number);
            line_number += 1;
        } else if !line.starts_with('-') {
            line_number += 1;
        }
    }
    added
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'static str,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: String,
}

async fn call_openrouter(api_key: &str, prompt: &str) -> Result<String> {
    let request = ChatCompletionRequest {
        model: OPENROUTER_MODEL,
        messages: vec![ChatMessage {
            role: "user",
            content: prompt,
        }],
    };

    let response = reqwest::Client::new()
        .post(OPENROUTER_URL)
        .bearer_auth(api_key)
        .json(&request)
        .send()
        .await
        .context("failed to call OpenRouter")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read OpenRouter response")?;

    if !status.is_success() {
        bail!("OpenRouter returned {status}: {body}");
    }

    let completion: ChatCompletionResponse =
        serde_json::from_str(&body).context("failed to parse OpenRouter response")?;
    completion
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .ok_or_else(|| anyhow::anyhow!("OpenRouter response contained no choices"))
}

#[derive(Debug, Deserialize)]
struct ModelReply {
    comment: String,
    reaction: Option<String>,
}

#[derive(Debug)]
struct AgentReply {
    comment: String,
    reaction: String,
}

fn issue_comment_event(event: Option<&EventPayload>) -> Option<&EventComment> {
    let event = event?;
    if event.comment.is_some() && event.action.as_deref().unwrap_or("created") == "created" {
        event.comment.as_ref()
    } else {
        None
    }
}

fn build_comment_prompt(comment: &EventComment, comments: &[IssueComment]) -> String {
    let recent_comments = comments
        .iter()
        .rev()
        .take(20)
        .rev()
        .map(|item| format!("@{}: {}", item.user.login, truncate(&item.body, 2_000)))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are PRBot, an assistant participating in a GitHub pull request timeline.\n\
You currently have no pull request diff, source files, or code tools.\n\
Reply to the human's comment using only the conversation context below.\n\
Support review discussion, suggestions, and general Q&A.\n\
If the human asks for a code-specific review that cannot be answered without the diff, say so clearly and ask for the needed context.\n\
Return only valid JSON with this exact shape:\n\
{{\"comment\":\"Markdown reply\",\"reaction\":\"eyes\"}}\n\
The reaction must be exactly one of: +1, -1, laugh, confused, heart, hooray, rocket, eyes.\n\
Choose the reaction that best acknowledges the human's message.\n\
Triggering human comment from @{}:\n{}\n\
Recent pull request timeline comments:\n{}",
        comment.user.login,
        truncate(&comment.body, 4_000),
        if recent_comments.is_empty() {
            "(none)"
        } else {
            &recent_comments
        }
    )
}

fn parse_agent_reply(raw: &str) -> Result<AgentReply> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("OpenRouter returned an empty comment response");
    }

    let candidate = trimmed
        .strip_prefix("```json")
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);

    if let Ok(reply) = serde_json::from_str::<ModelReply>(candidate) {
        if reply.comment.trim().is_empty() {
            bail!("OpenRouter returned an empty comment");
        }

        return Ok(AgentReply {
            comment: reply.comment,
            reaction: normalize_reaction(reply.reaction.as_deref()),
        });
    }

    Ok(AgentReply {
        comment: trimmed.to_owned(),
        reaction: "eyes".to_owned(),
    })
}

fn normalize_reaction(reaction: Option<&str>) -> String {
    let normalized = reaction.unwrap_or_default().trim().to_ascii_lowercase();

    match normalized.as_str() {
        "+1" | "-1" | "laugh" | "confused" | "heart" | "hooray" | "rocket" | "eyes" => normalized,
        _ => "eyes".to_owned(),
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let truncated = value.chars().take(max_chars).collect::<String>();
    if truncated.chars().count() == value.chars().count() {
        truncated
    } else {
        format!("{truncated}...")
    }
}

fn env_flag(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn resolve_pr_number_from_event(
    explicit: Option<&str>,
    event: Option<&EventPayload>,
) -> Result<u64> {
    if let Some(raw) = explicit {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed
                .parse::<u64>()
                .with_context(|| format!("invalid PR number '{trimmed}'"));
        }
    }

    let Some(event) = event else {
        bail!("missing --pr-number / PRBOT_PR_NUMBER and GITHUB_EVENT_PATH is unset");
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

    bail!("could not determine pull request number from event payload");
}

fn read_event_payload() -> Result<Option<EventPayload>> {
    let event_path = env::var_os("GITHUB_EVENT_PATH").map(PathBuf::from);
    let Some(path) = event_path else {
        return Ok(None);
    };

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read GitHub event file {}", path.display()))?;
    let event: EventPayload =
        serde_json::from_str(&raw).context("failed to parse GitHub event JSON")?;
    Ok(Some(event))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_explicit_pr_number() {
        let number = resolve_pr_number_from_event(Some("17"), None).expect("number");
        assert_eq!(number, 17);
    }

    #[test]
    fn parses_pull_request_event_file() {
        let dir = tempfile_dir();
        let path = dir.join("event.json");
        let mut file = fs::File::create(&path).expect("create event");
        write!(file, r#"{{"pull_request":{{"number":99}}}}"#).expect("write event");

        env::set_var("GITHUB_EVENT_PATH", &path);
        let event = read_event_payload().expect("event").expect("event payload");
        let number = resolve_pr_number_from_event(None, Some(&event)).expect("number from event");
        env::remove_var("GITHUB_EVENT_PATH");
        assert_eq!(number, 99);
    }

    #[test]
    fn parses_structured_agent_reply_and_reaction() {
        let reply = parse_agent_reply(
            r#"```json
{"comment":"Thanks for the suggestion!","reaction":"heart"}
```"#,
        )
        .expect("reply");

        assert_eq!(reply.comment, "Thanks for the suggestion!");
        assert_eq!(reply.reaction, "heart");
    }

    #[test]
    fn defaults_invalid_reaction_to_eyes() {
        let reply = parse_agent_reply(r#"{"comment":"I need more context.","reaction":"party"}"#)
            .expect("reply");

        assert_eq!(reply.reaction, "eyes");
    }

    #[test]
    fn falls_back_to_plain_text_agent_reply() {
        let reply = parse_agent_reply("I need more context.").expect("reply");

        assert_eq!(reply.comment, "I need more context.");
        assert_eq!(reply.reaction, "eyes");
    }

    #[test]
    fn selects_only_source_files_within_the_prompt_budget() {
        let files = vec![
            PullRequestFile {
                filename: "src/main.rs".to_owned(),
                status: "modified".to_owned(),
                patch: Some("@@ -1 +1 @@\n-old\n+new".to_owned()),
            },
            PullRequestFile {
                filename: "Cargo.lock".to_owned(),
                status: "modified".to_owned(),
                patch: Some("@@ -1 +1 @@\n-old\n+new".to_owned()),
            },
        ];

        let selected = select_review_files(&files);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].filename, "src/main.rs");
    }

    #[test]
    fn validates_findings_against_added_diff_lines() {
        let files = vec![PullRequestFile {
            filename: "src/main.rs".to_owned(),
            status: "modified".to_owned(),
            patch: Some("@@ -10,2 +10,3 @@\n unchanged\n-old\n+new\n+another".to_owned()),
        }];
        let valid = CodeQualityFinding {
            path: "src/main.rs".to_owned(),
            line: 11,
            severity: "high".to_owned(),
            body: "This is a concrete problem.".to_owned(),
        };
        let invalid_line = CodeQualityFinding {
            line: 10,
            ..valid.clone()
        };

        assert!(is_valid_finding(&valid, &files));
        assert!(!is_valid_finding(&invalid_line, &files));
    }

    #[test]
    fn ignores_no_newline_at_end_of_file_markers_when_counting_added_lines() {
        let patch = "@@ -1,2 +1,2 @@\n context\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file";

        assert_eq!(added_line_numbers(patch), vec![2]);
    }

    #[test]
    fn parses_code_quality_json() {
        let findings = parse_code_quality_findings(
            r#"{"findings":[{"path":"src/main.rs","line":12,"severity":"medium","body":"Avoid this."}]}"#,
        )
        .expect("findings");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 12);
    }

    fn tempfile_dir() -> PathBuf {
        let path = env::temp_dir().join(format!("prbot-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        path
    }
}
