use super::incremental::{related_paths_for_bundles, select_bundles_for_paths};
use super::legacy;
use crate::agents;
use crate::config::{ReviewConfig, ReviewEngine};
use crate::github::{CheckConclusion, GitHubClient, IssueComment, PullRequest, ReviewInputComment};
use crate::llm::{Budget, LlmClient};
use crate::reporting::{
    deduplicate, finding_body, parse_summary_state, render_review_body, render_summary,
    resolve_findings, SummaryState, SUMMARY_MARKER,
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
    if !eval_mode
        && state.reviewed_sha == pull_request.head.sha
        && !requires_full_recovery(&state, eval_mode)
    {
        let check_exists =
            has_completed_review_check(&github.list_check_runs(&pull_request.head.sha).await?);
        if !check_exists {
            let (conclusion, title) = review_check_result(
                state.coverage_complete.unwrap_or(false),
                state.blocking_findings(),
            );
            github
                .create_review_check(
                    &pull_request.head.sha,
                    conclusion,
                    &title,
                    previous_comment
                        .map(|comment| comment.body.as_str())
                        .unwrap_or("Recovered persisted PRBot review result."),
                )
                .await?;
        }
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

    let previous_sha = state.reviewed_sha.clone();
    let incremental = !eval_mode && !previous_sha.is_empty();
    let recovery_full_review = requires_full_recovery(&state, eval_mode);
    let changed_paths = if incremental {
        repository
            .changed_paths_between(&previous_sha, &pull_request.head.sha)
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let mut forgotten = BTreeSet::new();
    let affected_bundles = if incremental {
        let selected = select_bundles_for_paths(manifest, &changed_paths);
        let invalidate = related_paths_for_bundles(&selected);
        forgotten = state.forget_paths(&invalidate);
        selected
    } else {
        Vec::new()
    };
    let selected_bundles = if recovery_full_review || !incremental {
        manifest.bundles.clone()
    } else {
        affected_bundles
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
        agents::empty_result()
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

    let agents::AgentReviewResult {
        findings,
        mut failed_bundles,
        agent_runs,
    } = result;
    let (resolved, unanchored) = resolve_findings(findings, &manifest.files);
    let file_level_count = resolved.iter().filter(|finding| finding.file_level).count();
    let resolved_count = resolved.len();
    let new_findings = deduplicate(resolved, &state.fingerprints);
    let duplicate_count = resolved_count.saturating_sub(new_findings.len());
    let (publish, overflow) =
        select_publishable_findings(&mut state, new_findings, config.max_comments);
    if overflow > 0 {
        failed_bundles.push(format!("comment-limit-overflow ({overflow} unpublished)"));
    }

    let current = github.get_pull_request(pr_number).await?;
    if current.head.sha != pull_request.head.sha {
        if !eval_mode {
            github
                .create_review_check(
                    &pull_request.head.sha,
                    CheckConclusion::Cancelled,
                    "PRBot review cancelled",
                    "The pull request head changed while PRBot was reviewing it.",
                )
                .await?;
        }
        return Ok(ReviewResult::Stale(current));
    }

    if let Some(command_id) = command_id {
        state.handled_comment_ids.insert(command_id);
    }
    state.version = 4;
    state.reviewed_sha = pull_request.head.sha.clone();
    let coverage_complete = manifest.complete() && failed_bundles.is_empty() && unanchored == 0;
    state.coverage_complete = Some(coverage_complete);
    state.resolve_forgotten(
        &forgotten,
        coverage_complete && !selected_bundles.is_empty(),
    );
    let status = if selected_bundles.is_empty() && incremental {
        RunStatus::Skipped
    } else if !failed_bundles.is_empty() && publish.is_empty() && !coverage_complete {
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
        active_findings: state.fingerprints.len(),
        ever_published_findings: state.published_fingerprints.len(),
        resolved_findings: state.resolved_fingerprints.len(),
        resolution_rate: state.resolution_rate(),
        skipped_findings: unanchored + duplicate_count + overflow,
        failed_bundles,
        budget: budget.snapshot().await,
        incremental: Some(incremental),
        reviewed_bundles: Some(selected_bundles.len()),
        agent_runs,
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
    let walkthrough = agents::generate_walkthrough(
        &client,
        &budget,
        pr_context,
        manifest,
        &selected_bundles,
        &manifest.files,
        &publish,
        config,
    )
    .await;
    let review_body = render_review_body(&outcome.agent_runs);
    if !publish.is_empty() {
        let input = publish.iter().map(review_comment).collect::<Vec<_>>();
        let id = github
            .create_review(pr_number, &pull_request.head.sha, &review_body, input)
            .await?;
        println!("PRBot created formal review #{id}");
    }
    let summary = render_summary(
        repository_name,
        pr_number,
        &outcome,
        &publish,
        &state,
        &config.review_model,
        &config.verification_model,
        walkthrough.as_deref(),
    );
    if let Some(comment) = previous_comment {
        github.update_issue_comment(comment.id, &summary).await?;
    } else {
        github.create_issue_comment(pr_number, &summary).await?;
    }
    let (conclusion, title) = review_check_result(coverage_complete, state.blocking_findings());
    github
        .create_review_check(&pull_request.head.sha, conclusion, &title, &summary)
        .await?;
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

/// Retains only findings that can be published and records those as active.
fn select_publishable_findings(
    state: &mut SummaryState,
    mut findings: Vec<crate::types::ResolvedFinding>,
    max_comments: usize,
) -> (Vec<crate::types::ResolvedFinding>, usize) {
    let overflow = findings.len().saturating_sub(max_comments);
    findings.truncate(max_comments);
    for finding in &findings {
        state.remember_finding(finding);
    }
    (findings, overflow)
}

fn has_completed_review_check(checks: &[crate::github::CheckRun]) -> bool {
    checks
        .iter()
        .any(|check| check.name == "PRBot review" && check.status == "completed")
}

fn requires_full_recovery(state: &SummaryState, eval_mode: bool) -> bool {
    !eval_mode && !state.reviewed_sha.is_empty() && state.coverage_complete != Some(true)
}

fn review_check_result(
    coverage_complete: bool,
    blocking_findings: usize,
) -> (CheckConclusion, String) {
    if !coverage_complete {
        return (
            CheckConclusion::Failure,
            "PRBot review incomplete".to_owned(),
        );
    }
    if blocking_findings > 0 {
        return (
            CheckConclusion::Failure,
            format!("PRBot found {blocking_findings} required change(s)"),
        );
    }
    (CheckConclusion::Success, "PRBot review passed".to_owned())
}

#[cfg(test)]
#[path = "contextual_tests.rs"]
mod tests;
