use super::types::{
    CheckOutputRequest, CheckRun, CheckRunsResponse, CommentRequest, CreateCheckRunRequest,
    CreateReviewRequest, CreatedCheckRun, CreatedReview, Issue, IssueComment, PermissionResponse,
    PullRequest, ReactionRequest, ReviewComment, ReviewInputComment,
};
use anyhow::{bail, Context, Result};
use reqwest::header::LINK;
use reqwest::{Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::time::Duration;

const GITHUB_API_URL: &str = "https://api.github.com";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const USER_AGENT: &str = "prbot";
const REVIEW_CHECK_NAME: &str = "PRBot review";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckConclusion {
    Success,
    Failure,
    Cancelled,
}

impl CheckConclusion {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitHubClient {
    client: reqwest::Client,
    token: String,
    owner: String,
    repository: String,
    base_url: String,
}

impl GitHubClient {
    /// Creates an authenticated GitHub client using the default GitHub API URL.
    ///
    /// The repository must be specified as `owner/repository`.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = GitHubClient::new("token", "owner/repository").unwrap();
    /// ```
    ///
    /// # Returns
    ///
    /// The configured client, or an error if the repository format or HTTP client configuration is invalid.
    pub fn new(token: impl Into<String>, repository: &str) -> Result<Self> {
        Self::with_base_url(token, repository, GITHUB_API_URL)
    }

    /// Creates a client configured for a repository and API base URL.
    ///
    /// The repository must use the `owner/repository` format.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = GitHubClient::with_base_url(
    ///     "token",
    ///     "octocat/Hello-World",
    ///     "https://api.github.com",
    /// )?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if `repository` is not in the `owner/repository` format or
    /// if the HTTP client cannot be built.
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

    /// Retrieves a pull request by its number.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = GitHubClient::new("token", "owner/repository")?;
    /// let pull_request = client.get_pull_request(42).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Returns
    ///
    /// The requested pull request.
    pub async fn get_pull_request(&self, pr_number: u64) -> Result<PullRequest> {
        self.get_json(&format!("pulls/{pr_number}"), "get pull request")
            .await
    }

    /// Determines whether a user has administrator permissions for the repository.
    ///
    /// A user who is not found is treated as not having administrator permissions.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = GitHubClient::new("token", "owner/repository")?;
    /// let allowed = client
    ///     .has_min_permission("octocat", crate::config::CollaboratorPermission::Admin)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    /// Returns whether `login` meets the configured collaborator permission floor.
    pub async fn has_min_permission(
        &self,
        login: &str,
        minimum: crate::config::CollaboratorPermission,
    ) -> Result<bool> {
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
        Ok(minimum.meets(&result.permission))
    }

    /// Lists the comments associated with a pull request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = GitHubClient::new("token", "owner/repository")?;
    /// let comments = client.list_issue_comments(42).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_issue_comments(&self, pr_number: u64) -> Result<Vec<IssueComment>> {
        self.get_paginated(
            &format!("issues/{pr_number}/comments?per_page=100"),
            "list pull request comments",
        )
        .await
    }

    /// Lists all review comments on a pull request.
    ///
    /// # Returns
    ///
    /// The pull request's review comments.
    /// # Examples
    ///
    /// ```
    /// # async fn example(client: &GitHubClient) -> anyhow::Result<()> {
    /// let comments = client.list_review_comments(42).await?;
    /// println!("Found {} review comments", comments.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_review_comments(&self, pr_number: u64) -> Result<Vec<ReviewComment>> {
        self.get_paginated(
            &format!("pulls/{pr_number}/comments?per_page=100"),
            "list review comments",
        )
        .await
    }

    /// Lists check runs associated with a commit.
    ///
    /// Retrieves up to 30 pages of check runs, stopping when a page contains fewer than 100 entries.
    ///
    /// # Arguments
    ///
    /// * `sha` - The commit SHA whose check runs should be listed.
    ///
    /// # Returns
    ///
    /// All check runs returned for the commit.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example(client: &GitHubClient) -> anyhow::Result<()> {
    /// let check_runs = client.list_check_runs("abc123").await?;
    /// println!("Found {} check runs", check_runs.len());
    /// # Ok(())
    /// # }
    /// ```
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

    /// Creates a completed PRBot check run against an exact commit.
    pub async fn create_review_check(
        &self,
        sha: &str,
        conclusion: CheckConclusion,
        title: &str,
        summary: &str,
    ) -> Result<u64> {
        let request = CreateCheckRunRequest {
            name: REVIEW_CHECK_NAME,
            head_sha: sha.to_owned(),
            status: "completed",
            conclusion: conclusion.as_str(),
            output: CheckOutputRequest {
                title: title.to_owned(),
                summary: summary.chars().take(65_535).collect(),
            },
        };
        let response = self
            .send_with_retry(
                Method::POST,
                "check-runs",
                Some(&request),
                "create PRBot review check",
            )
            .await?;
        let created: CreatedCheckRun = parse_json(response, "create PRBot review check").await?;
        Ok(created.id)
    }

    /// Fetches an issue by its repository-local number.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = GitHubClient::new("token", "owner/repository")?;
    /// let issue = client.get_issue(123).await?;
    /// assert_eq!(issue.number, 123);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Returns
    ///
    /// The requested [`Issue`].
    pub async fn get_issue(&self, number: u64) -> Result<Issue> {
        self.get_json(&format!("issues/{number}"), "get linked issue")
            .await
    }

    /// Creates a pull request review with a comment event.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = GitHubClient::new("token", "owner/repository")?;
    /// let review_id = client
    ///     .create_review(42, "commit-sha", "Looks good.", vec![])
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Arguments
    ///
    /// * `pr_number` - The pull request number.
    /// * `commit_id` - The commit the review targets.
    /// * `body` - The review body.
    /// * `comments` - Inline comments included in the review.
    ///
    /// # Returns
    ///
    /// The identifier of the created review.
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
            .send_with_retry(
                Method::POST,
                &format!("pulls/{pr_number}/reviews"),
                Some(&request),
                "create pull request review",
            )
            .await?;
        let created: CreatedReview = parse_json(response, "create pull request review").await?;
        Ok(created.id)
    }

    /// Creates a comment on a pull request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = GitHubClient::new("TOKEN", "owner/repo")?;
    /// let comment = client.create_issue_comment(123, "Thanks for the contribution!").await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Returns
    ///
    /// The newly created issue comment.
    pub async fn create_issue_comment(&self, pr_number: u64, body: &str) -> Result<IssueComment> {
        let request = CommentRequest {
            body: body.to_owned(),
        };
        let response = self
            .send_with_retry(
                Method::POST,
                &format!("issues/{pr_number}/comments"),
                Some(&request),
                "create pull request comment",
            )
            .await?;
        parse_json(response, "create pull request comment").await
    }

    /// Updates the body of an existing pull request comment.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = GitHubClient::new("token", "owner/repository")?;
    /// let comment = client.update_issue_comment(42, "Updated comment").await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Returns
    ///
    /// The updated pull request comment.
    pub async fn update_issue_comment(&self, comment_id: u64, body: &str) -> Result<IssueComment> {
        let request = CommentRequest {
            body: body.to_owned(),
        };
        let response = self
            .send_with_retry(
                Method::PATCH,
                &format!("issues/comments/{comment_id}"),
                Some(&request),
                "update pull request comment",
            )
            .await?;
        parse_json(response, "update pull request comment").await
    }

    /// Adds a reaction to an issue comment.
    ///
    /// # Arguments
    ///
    /// * `comment_id` - The ID of the issue comment.
    /// * `reaction` - The reaction content to add.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example(client: &GitHubClient) -> Result<(), Box<dyn std::error::Error>> {
    /// client.create_reaction(123, "+1").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_reaction(&self, comment_id: u64, reaction: &str) -> Result<()> {
        let request = ReactionRequest {
            content: reaction.to_owned(),
        };
        let response = self
            .send_with_retry(
                Method::POST,
                &format!("issues/comments/{comment_id}/reactions"),
                Some(&request),
                "create issue comment reaction",
            )
            .await?;
        parse_empty(response, "create issue comment reaction").await
    }

    /// Builds an authenticated request for a repository-scoped GitHub API path.
    ///
    /// # Examples
    ///
    /// ```
    /// let request = client.request(reqwest::Method::GET, "issues/1");
    /// ```
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

    /// Sends a GET request and deserializes the successful response body into `T`.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let issue: Issue = client.get_json("issues/1", "fetch issue").await?;
    /// ```
    ///
    /// # Arguments
    ///
    /// * `path` - The API path to request.
    /// * `operation` - A description used to provide context if the request or parsing fails.
    ///
    /// # Returns
    ///
    /// The deserialized response value.
    async fn get_json<T: DeserializeOwned>(&self, path: &str, operation: &str) -> Result<T> {
        let response = self
            .send_with_retry(Method::GET, path, None::<&()>, operation)
            .await?;
        parse_json(response, operation).await
    }

    /// Collects deserialized items from all pages of a GitHub API response.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example(client: &GitHubClient) -> anyhow::Result<()> {
    /// let comments = client
    ///     .get_paginated::<IssueComment>("issues/1/comments?per_page=100", "list comments")
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if a request, response deserialization, or pagination URL
    /// parsing fails.
    async fn get_paginated<T: DeserializeOwned>(
        &self,
        initial_path: &str,
        operation: &str,
    ) -> Result<Vec<T>> {
        let mut path = initial_path.to_owned();
        let mut all = Vec::new();
        loop {
            let response = self
                .send_with_retry(Method::GET, &path, None::<&()>, operation)
                .await?;
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

    /// Sends a GET request through the client's retry policy.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example(client: &GitHubClient) -> Result<()> {
    /// let response = client
    ///     .send_get_with_retry("issues/1", "fetch issue")
    ///     .await?;
    /// # let _ = response;
    /// # Ok(())
    /// # }
    /// ```
    async fn send_get_with_retry(&self, path: &str, operation: &str) -> Result<Response> {
        self.send_with_retry(Method::GET, path, None::<&()>, operation)
            .await
    }

    /// Sends an authenticated request and retries transient server or rate-limit responses.
    ///
    /// Retries up to three attempts using the `Retry-After` header when available, or an
    /// exponential backoff capped at ten seconds.
    ///
    /// # Arguments
    ///
    /// * `operation` - Description used to provide context if sending the request fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn example(client: &GitHubClient) -> anyhow::Result<()> {
    /// let response = client
    ///     .send_with_retry(reqwest::Method::GET, "issues/1", None::<&()>, "fetch issue")
    ///     .await?;
    /// assert!(response.status().is_success());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Returns
    ///
    /// The HTTP response, including responses that remain unsuccessful after the final attempt.
    async fn send_with_retry<T: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&T>,
        operation: &str,
    ) -> Result<Response> {
        let mut delay = Duration::from_millis(250);
        for attempt in 0..3 {
            let mut request = self.request(method.clone(), path);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request
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

/// Extracts the URL for the next page from an HTTP `Link` header.
///
/// # Examples
///
/// ```
/// let header = r#"<https://api.example.com/items?page=2>; rel="next""#;
/// assert_eq!(
///     next_link(header),
///     Some("https://api.example.com/items?page=2".to_owned())
/// );
/// ```
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

/// Parses a successful GitHub response body as JSON.
///
/// # Examples
///
/// ```no_run
/// # async fn example(response: reqwest::Response) {
/// let value: serde_json::Value = parse_json(response, "fetch data").await.unwrap();
/// # }
/// ```
///
/// Returns an error when the response body cannot be read, the response status
/// indicates failure, or the body is not valid JSON.
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

/// Validates that a GitHub response indicates success without deserializing its body.
///
/// # Errors
///
/// Returns an error containing the operation, response status, and response body when the response is unsuccessful.
///
/// # Examples
///
/// ```no_run
/// # async fn example() -> anyhow::Result<()> {
/// let response = reqwest::Client::new()
///     .get("https://api.github.com")
///     .send()
///     .await?;
/// parse_empty(response, "check the API").await?;
/// # Ok(())
/// # }
/// ```
async fn parse_empty(response: Response, operation: &str) -> Result<()> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub failed to {operation} ({status}): {body}");
    }
    Ok(())
}
