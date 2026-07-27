use super::incremental::{related_paths_for_bundles, select_bundles_for_paths};
use super::legacy;
use crate::agents;
use crate::config::{ReviewConfig, ReviewEngine};
use crate::github::{GitHubClient, IssueComment, PullRequest, ReviewInputComment};
use crate::llm::{Budget, LlmClient};
use crate::reporting::{
    deduplicate, finding_body, parse_summary_state, render_summary, resolve_findings,
    SUMMARY_MARKER,
};
use crate::repository::{GitRepository, RepositoryTools};
use crate::types::{DiffSide, RunOutcome, RunStatus};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeSet;
use std::env;
use std::sync::Arc;

pub enum ReviewResult {
    Complete,
    Stale(PullRequest),
    Evaluated(EvalPayload),
}

#[derive(Clone, Debug, Serialize)]
pub struct EvalPayload {
    pub repository: String,
    pub pr_number: u64,
    pub outcome: RunOutcome,
    pub findings: Vec<crate::types::ResolvedFinding>,
}

/// Runs a pull request review, publishes new findings, and updates the review summary.
///
/// Incremental reviews use the previously reviewed commit to select affected bundles and
/// preserve finding fingerprints across runs. Returns `ReviewResult::Stale` when the pull
/// request changes while the review is running.
///
/// # Examples
///
/// ```ignore
/// let result = run_review(
///     &github,
///     api_key,
///     repository_name,
///     pr_number,
///     &pull_request,
///     repository,
///     &manifest,
///     pr_context,
///     &comments,
///     command_id,
///     &config,
///     budget,
/// ).await?;
///
/// assert!(matches!(result, ReviewResult::Complete | ReviewResult::Stale(_)));
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// # Parameters
///
/// - `comments` contains existing issue comments used to restore review state.
/// - `command_id` identifies the command that requested the review, when applicable.
/// - `config` controls the review engine, concurrency, models, and comment limit.
/// - `budget` tracks the review's shared resource usage.
///
/// # Returns
///
/// The completed review result, or the current pull request when its head commit changed
/// before findings were published.
#[allow(clippy::too_many_arguments)]
pub async fn run_review(
    github: &GitHubClient,
    api_key: &str,
    repository_name: &str,
    pr_number: u64,
    pull_request: &PullRequest,
    repository: Arc<GitRepository>,
    manifest: &crate::types::ReviewManifest,
    pr_context: &str,
    comments: &[IssueComment],
    command_id: Option<u64>,
    config: &ReviewConfig,
    budget: Arc<Budget>,
    eval_mode: bool,
) -> Result<ReviewResult> {
    let previous_comment = comments
        .iter()
        .rev()
        .find(|comment| comment.body.contains(SUMMARY_MARKER));
    let mut state = if eval_mode {
        Default::default()
    } else {
        previous_comment
            .and_then(|comment| parse_summary_state(&comment.body))
            .unwrap_or_default()
    };
    if !eval_mode {
        for comment in github.list_review_comments(pr_number).await? {
            if let Some(fingerprint) = finding_marker(&comment.body) {
                state.fingerprints.insert(fingerprint);
            }
        }
        if state.reviewed_sha == pull_request.head.sha {
            if let Some(command_id) = command_id {
                github
                    .create_issue_comment(
                        pr_number,
                        &format!(
                            "<!-- prbot-command:{command_id} -->\nPRBot already reviewed `{}`. Push a new commit before requesting another review.",
                            pull_request.head.sha
                        ),
                    )
                    .await?;
            } else {
                println!("PRBot already reviewed head {}", pull_request.head.sha);
            }
            return Ok(ReviewResult::Complete);
        }
    }

    let previous_sha = state.reviewed_sha.clone();
    let incremental = !eval_mode && !previous_sha.is_empty();
    let changed_paths = if incremental {
        repository
            .changed_paths_between(&previous_sha, &pull_request.head.sha)
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let selected_bundles = if incremental {
        let selected = select_bundles_for_paths(manifest, &changed_paths);
        let invalidate = related_paths_for_bundles(&selected);
        state.forget_paths(&invalidate);
        selected
    } else {
        manifest.bundles.clone()
    };
    if incremental {
        println!(
            "PRBot incremental review: {} changed path(s), {} bundle(s)",
            changed_paths.len(),
            selected_bundles.len()
        );
    }

    let client = LlmClient::new(
        api_key,
        env::var("OPENROUTER_URL").ok(),
        Arc::clone(&budget),
        config.max_concurrency,
    )?;
    let tools = Arc::new(RepositoryTools::new(
        Arc::clone(&repository),
        pr_context.to_owned(),
    ));
    let result = if selected_bundles.is_empty() && incremental {
        agents::AgentReviewResult {
            findings: Vec::new(),
            failed_bundles: Vec::new(),
        }
    } else {
        match config.engine {
            ReviewEngine::Contextual => {
                if incremental {
                    agents::review_bundles(
                        &client,
                        Arc::clone(&tools),
                        manifest,
                        &selected_bundles,
                        config,
                    )
                    .await
                } else {
                    agents::review_manifest(&client, Arc::clone(&tools), manifest, config).await
                }
            }
            ReviewEngine::Legacy => legacy::review(&client, manifest, config).await,
        }
    };

    let (resolved, unanchored) = resolve_findings(result.findings, &manifest.files);
    let file_level_count = resolved.iter().filter(|finding| finding.file_level).count();
    let resolved_count = resolved.len();
    let new_findings = deduplicate(resolved, &state.fingerprints);
    let duplicate_count = resolved_count.saturating_sub(new_findings.len());
    for finding in &new_findings {
        state.remember_finding(finding);
    }
    let mut publish = new_findings;
    let overflow = publish.len().saturating_sub(config.max_comments);
    publish.truncate(config.max_comments);

    let current = github.get_pull_request(pr_number).await?;
    if current.head.sha != pull_request.head.sha {
        return Ok(ReviewResult::Stale(current));
    }

    if !eval_mode && !publish.is_empty() {
        let input = publish.iter().map(review_comment).collect::<Vec<_>>();
        let id = github
            .create_review(
                pr_number,
                &pull_request.head.sha,
                "PRBot independently verified the following findings.",
                input,
            )
            .await?;
        println!("PRBot created formal review #{id}");
    }

    if let Some(command_id) = command_id {
        state.handled_comment_ids.insert(command_id);
    }
    state.version = 1;
    state.reviewed_sha = pull_request.head.sha.clone();
    let coverage_complete = manifest.complete() && result.failed_bundles.is_empty();
    let status = if selected_bundles.is_empty() && incremental {
        RunStatus::Skipped
    } else if !result.failed_bundles.is_empty() && publish.is_empty() && !coverage_complete {
        RunStatus::Failed
    } else if coverage_complete {
        RunStatus::Complete
    } else {
        RunStatus::Partial
    };
    let outcome = RunOutcome {
        status,
        reviewed_sha: pull_request.head.sha.clone(),
        coverage_complete,
        eligible_hunks: manifest.eligible_hunks(),
        assigned_hunks: manifest.assigned_hunks(),
        findings: publish.len(),
        skipped_findings: unanchored + duplicate_count + overflow,
        failed_bundles: result.failed_bundles,
        budget: budget.snapshot().await,
        incremental: Some(incremental),
        reviewed_bundles: Some(selected_bundles.len()),
    };
    if eval_mode {
        println!(
            "PRBot eval review ready: findings={} file_level={} coverage={}/{}",
            publish.len(),
            file_level_count,
            outcome.assigned_hunks,
            outcome.eligible_hunks
        );
        return Ok(ReviewResult::Evaluated(EvalPayload {
            repository: repository_name.to_owned(),
            pr_number,
            outcome,
            findings: publish,
        }));
    }
    let summary = render_summary(
        repository_name,
        pr_number,
        &outcome,
        &publish,
        &state,
        &config.review_model,
        &config.verification_model,
    );
    if let Some(comment) = previous_comment {
        github.update_issue_comment(comment.id, &summary).await?;
    } else {
        github.create_issue_comment(pr_number, &summary).await?;
    }
    if let Some(command_id) = command_id {
        let _ = github.create_reaction(command_id, "eyes").await;
    }
    println!(
        "PRBot completed review: findings={} file_level={} coverage={}/{}",
        publish.len(),
        file_level_count,
        outcome.assigned_hunks,
        outcome.eligible_hunks
    );
    Ok(ReviewResult::Complete)
}

/// Converts a resolved finding into a GitHub review comment, using a file-level
/// comment when the finding cannot be anchored to a line.
///
/// # Examples
///
/// ```rust,ignore
/// let comment = review_comment(&finding);
/// assert_eq!(comment.path, finding.candidate.path);
/// assert_eq!(comment.body, finding_body(&finding));
/// ```
fn review_comment(finding: &crate::types::ResolvedFinding) -> ReviewInputComment {
    if finding.file_level || finding.line.is_none() {
        return ReviewInputComment {
            path: finding.candidate.path.clone(),
            body: finding_body(finding),
            line: None,
            side: None,
            start_line: None,
            start_side: None,
            subject_type: Some("file".to_owned()),
        };
    }
    let side = match finding.side {
        DiffSide::Left => "LEFT",
        DiffSide::Right | DiffSide::Context => "RIGHT",
    }
    .to_owned();
    ReviewInputComment {
        path: finding.candidate.path.clone(),
        body: finding_body(finding),
        line: finding.line,
        side: Some(side.clone()),
        start_line: finding.start_line,
        start_side: finding.start_line.map(|_| side),
        subject_type: None,
    }
}

/// Extracts the finding fingerprint from a PRBot finding marker.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     finding_marker("Issue details <!-- prbot:finding:abc123 -->"),
///     Some("abc123".to_owned())
/// );
/// assert_eq!(finding_marker("Issue details"), None);
/// ```
fn finding_marker(body: &str) -> Option<String> {
    let prefix = "<!-- prbot:finding:";
    let start = body.find(prefix)? + prefix.len();
    let rest = &body[start..];
    let end = rest.find(" -->")?;
    Some(rest[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CandidateFinding, FindingCategory, Priority, ResolvedFinding};

    #[test]
    fn file_level_comments_use_subject_type() {
        let finding = ResolvedFinding {
            candidate: CandidateFinding {
                path: "src/main.rs".to_owned(),
                side: DiffSide::Right,
                anchor: "ambiguous".to_owned(),
                end_anchor: None,
                priority: Priority::P1,
                category: FindingCategory::Correctness,
                title: "Bug".to_owned(),
                body: "Impact".to_owned(),
                evidence: Vec::new(),
                confidence: 0.9,
            },
            line: None,
            start_line: None,
            side: DiffSide::Right,
            fingerprint: "fp".to_owned(),
            file_level: true,
        };
        let comment = review_comment(&finding);
        assert_eq!(comment.subject_type.as_deref(), Some("file"));
        assert!(comment.line.is_none());
    }
}
