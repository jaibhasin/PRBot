use super::types::{
    CheckRun, CheckRunsResponse, CommentRequest, CreateReviewRequest, CreatedReview, Issue,
    IssueComment, PermissionResponse, PullRequest, ReactionRequest, ReviewComment,
    ReviewInputComment,
};
use anyhow::{bail, Context, Result};
use reqwest::header::LINK;
use reqwest::{Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use std::time::Duration;

const GITHUB_API_URL: &str = "https://api.github.com";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const USER_AGENT: &str = "prbot";

#[derive(Clone, Debug)]
pub struct GitHubClient {
    client: reqwest::Client,
    token: String,
    owner: String,
    repository: String,
    base_url: String,
}

impl GitHubClient {
    pub fn new(token: impl Into<String>, repository: &str) -> Result<Self> {
        Self::with_base_url(token, repository, GITHUB_API_URL)
    }

    pub fn with_base_url(
        token: impl Into<String>,
        repository: &str,
        base_url: impl Into<String>,
    ) -> Result<Self> {
        let mut parts = repository.split('/');
        let owner = parts.next().unwrap_or_default();
        let repository_name = parts.next().unwrap_or_default();
        if owner.is_empty() || repository_name.is_empty() || parts.next().is_some() {
            bail!("invalid repository '{repository}', expected owner/repo");
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(45))
            .build()
            .context("failed to build GitHub client")?;
        Ok(Self {
            client,
            token: token.into(),
            owner: owner.to_owned(),
            repository: repository_name.to_owned(),
            base_url: base_url.into(),
        })
    }

    pub async fn get_pull_request(&self, pr_number: u64) -> Result<PullRequest> {
        self.get_json(&format!("pulls/{pr_number}"), "get pull request")
            .await
    }

    pub async fn is_repository_admin(&self, login: &str) -> Result<bool> {
        let encoded = login.replace('/', "%2F");
        let response = self
            .send_get_with_retry(
                &format!("collaborators/{encoded}/permission"),
                "check repository permission",
            )
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        let result: PermissionResponse =
            parse_json(response, "check repository permission").await?;
        Ok(result.permission == "admin")
    }

    pub async fn list_issue_comments(&self, pr_number: u64) -> Result<Vec<IssueComment>> {
        self.get_paginated(
            &format!("issues/{pr_number}/comments?per_page=100"),
            "list pull request comments",
        )
        .await
    }

    pub async fn list_review_comments(&self, pr_number: u64) -> Result<Vec<ReviewComment>> {
        self.get_paginated(
            &format!("pulls/{pr_number}/comments?per_page=100"),
            "list review comments",
        )
        .await
    }

    pub async fn list_check_runs(&self, sha: &str) -> Result<Vec<CheckRun>> {
        let mut all = Vec::new();
        for page in 1..=30 {
            let response: CheckRunsResponse = self
                .get_json(
                    &format!("commits/{sha}/check-runs?per_page=100&page={page}"),
                    "list check runs",
                )
                .await?;
            let count = response.check_runs.len();
            all.extend(response.check_runs);
            if count < 100 {
                break;
            }
        }
        Ok(all)
    }

    pub async fn get_issue(&self, number: u64) -> Result<Issue> {
        self.get_json(&format!("issues/{number}"), "get linked issue")
            .await
    }

    pub async fn create_review(
        &self,
        pr_number: u64,
        commit_id: &str,
        body: &str,
        comments: Vec<ReviewInputComment>,
    ) -> Result<u64> {
        let request = CreateReviewRequest {
            commit_id: commit_id.to_owned(),
            body: body.to_owned(),
            event: "COMMENT",
            comments,
        };
        let response = self
            .request(Method::POST, &format!("pulls/{pr_number}/reviews"))
            .json(&request)
            .send()
            .await
            .context("failed to create pull request review")?;
        let created: CreatedReview = parse_json(response, "create pull request review").await?;
        Ok(created.id)
    }

    pub async fn create_issue_comment(&self, pr_number: u64, body: &str) -> Result<IssueComment> {
        let response = self
            .request(Method::POST, &format!("issues/{pr_number}/comments"))
            .json(&CommentRequest {
                body: body.to_owned(),
            })
            .send()
            .await
            .context("failed to create pull request comment")?;
        parse_json(response, "create pull request comment").await
    }

    pub async fn update_issue_comment(&self, comment_id: u64, body: &str) -> Result<IssueComment> {
        let response = self
            .request(Method::PATCH, &format!("issues/comments/{comment_id}"))
            .json(&CommentRequest {
                body: body.to_owned(),
            })
            .send()
            .await
            .context("failed to update pull request comment")?;
        parse_json(response, "update pull request comment").await
    }

    pub async fn create_reaction(&self, comment_id: u64, reaction: &str) -> Result<()> {
        let response = self
            .request(
                Method::POST,
                &format!("issues/comments/{comment_id}/reactions"),
            )
            .json(&ReactionRequest {
                content: reaction.to_owned(),
            })
            .send()
            .await
            .context("failed to create issue comment reaction")?;
        parse_empty(response, "create issue comment reaction").await
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(
                method,
                format!(
                    "{}/repos/{}/{}/{}",
                    self.base_url.trim_end_matches('/'),
                    self.owner,
                    self.repository,
                    path
                ),
            )
            .header("Accept", GITHUB_ACCEPT)
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .header("User-Agent", USER_AGENT)
            .bearer_auth(&self.token)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str, operation: &str) -> Result<T> {
        let response = self.send_get_with_retry(path, operation).await?;
        parse_json(response, operation).await
    }

    async fn get_paginated<T: DeserializeOwned>(
        &self,
        initial_path: &str,
        operation: &str,
    ) -> Result<Vec<T>> {
        let mut path = initial_path.to_owned();
        let mut all = Vec::new();
        loop {
            let response = self.send_get_with_retry(&path, operation).await?;
            let next = response
                .headers()
                .get(LINK)
                .and_then(|value| value.to_str().ok())
                .and_then(next_link);
            let mut page: Vec<T> = parse_json(response, operation).await?;
            all.append(&mut page);
            let Some(url) = next else {
                break;
            };
            path = url
                .split("/repos/")
                .nth(1)
                .and_then(|value| value.splitn(3, '/').nth(2))
                .context("GitHub pagination returned an invalid next URL")?
                .to_owned();
        }
        Ok(all)
    }

    async fn send_get_with_retry(&self, path: &str, operation: &str) -> Result<Response> {
        let mut delay = Duration::from_millis(250);
        for attempt in 0..3 {
            let response = self
                .request(Method::GET, path)
                .send()
                .await
                .with_context(|| format!("failed to {operation}"))?;
            if response.status().is_success()
                || (!response.status().is_server_error()
                    && response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS)
                || attempt == 2
            {
                return Ok(response);
            }
            let wait = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or(delay)
                .min(Duration::from_secs(10));
            tokio::time::sleep(wait).await;
            delay *= 2;
        }
        unreachable!("retry loop always returns on final attempt")
    }
}

pub(super) fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let mut sections = part.trim().split(';');
        let url = sections
            .next()?
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>');
        let relation = sections.next()?.trim();
        (relation == "rel=\"next\"").then(|| url.to_owned())
    })
}

async fn parse_json<T: DeserializeOwned>(response: Response, operation: &str) -> Result<T> {
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("failed to read GitHub response while trying to {operation}"))?;
    if !status.is_success() {
        bail!("GitHub failed to {operation} ({status}): {body}");
    }
    serde_json::from_str(&body)
        .with_context(|| format!("failed to parse GitHub response while trying to {operation}"))
}

async fn parse_empty(response: Response, operation: &str) -> Result<()> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub failed to {operation} ({status}): {body}");
    }
    Ok(())
}
