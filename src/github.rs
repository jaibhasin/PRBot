//! GitHub API helpers for pull request timeline comments and reactions.

use anyhow::{bail, Context, Result};
use reqwest::{Client, Method, RequestBuilder, Response};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const GITHUB_API_URL: &str = "https://api.github.com";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const USER_AGENT: &str = "prbot";

#[derive(Debug)]
pub struct GitHubClient {
    client: Client,
    token: String,
    owner: String,
    repository: String,
}

impl GitHubClient {
    pub fn new(token: impl Into<String>, repository: &str) -> Result<Self> {
        let mut parts = repository.split('/');
        let owner = parts.next().unwrap_or_default();
        let repository_name = parts.next().unwrap_or_default();

        if owner.is_empty() || repository_name.is_empty() || parts.next().is_some() {
            bail!("invalid repository '{repository}', expected owner/repo");
        }

        Ok(Self {
            client: Client::new(),
            token: token.into(),
            owner: owner.to_owned(),
            repository: repository_name.to_owned(),
        })
    }

    /// List the most recent timeline comments on a pull request.
    pub async fn list_issue_comments(&self, pr_number: u64) -> Result<Vec<IssueComment>> {
        let path = format!("issues/{pr_number}/comments?per_page=100");
        self.get_json(&path, "list pull request comments").await
    }

    /// List files changed by a pull request, including their unified diffs when GitHub provides them.
    pub async fn list_pull_request_files(&self, pr_number: u64) -> Result<Vec<PullRequestFile>> {
        let path = format!("pulls/{pr_number}/files?per_page=100");
        self.get_json(&path, "list pull request files").await
    }

    /// Return the SHA at the head of a pull request, required for inline review comments.
    pub async fn get_pull_request(&self, pr_number: u64) -> Result<PullRequest> {
        let path = format!("pulls/{pr_number}");
        self.get_json(&path, "get pull request").await
    }

    /// Add an inline comment to a changed line in a pull request.
    pub async fn create_pull_request_review_comment(
        &self,
        pr_number: u64,
        body: &str,
        commit_id: &str,
        path: &str,
        line: u64,
    ) -> Result<PullRequestReviewComment> {
        let endpoint = format!("pulls/{pr_number}/comments");
        let response = self
            .request(Method::POST, &endpoint)
            .json(&CreateReviewCommentRequest {
                body,
                commit_id,
                path,
                line,
                side: "RIGHT",
            })
            .send()
            .await
            .context("failed to create inline pull request review comment")?;

        parse_json_response(response, "create inline pull request review comment").await
    }

    /// Create a regular timeline comment on a pull request.
    pub async fn create_issue_comment(&self, pr_number: u64, body: &str) -> Result<IssueComment> {
        let path = format!("issues/{pr_number}/comments");
        let response = self
            .request(Method::POST, &path)
            .json(&CreateCommentRequest { body })
            .send()
            .await
            .context("failed to create GitHub pull request comment")?;

        parse_json_response(response, "create pull request comment").await
    }

    /// Add a reaction to a regular timeline comment.
    pub async fn create_issue_comment_reaction(
        &self,
        comment_id: u64,
        reaction: &str,
    ) -> Result<()> {
        let path = format!("issues/comments/{comment_id}/reactions");
        let response = self
            .request(Method::POST, &path)
            .json(&CreateReactionRequest { content: reaction })
            .send()
            .await
            .context("failed to create GitHub comment reaction")?;

        parse_empty_response(response, "create comment reaction").await
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(
                method,
                format!(
                    "{GITHUB_API_URL}/repos/{}/{}/{}",
                    self.owner, self.repository, path
                ),
            )
            .header("Accept", GITHUB_ACCEPT)
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .header("User-Agent", USER_AGENT)
            .bearer_auth(&self.token)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str, operation: &str) -> Result<T> {
        let response = self
            .request(Method::GET, path)
            .send()
            .await
            .with_context(|| format!("failed to {operation}"))?;

        parse_json_response(response, operation).await
    }
}

#[derive(Debug, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub body: String,
    pub user: GitHubUser,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestFile {
    pub filename: String,
    pub status: String,
    pub patch: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestReviewComment {
    pub id: u64,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub head: PullRequestHead,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestHead {
    pub sha: String,
}

#[derive(Debug, Deserialize)]
pub struct GitHubUser {
    pub login: String,
    #[serde(rename = "type")]
    pub user_type: String,
}

#[derive(Debug, Serialize)]
struct CreateCommentRequest<'a> {
    body: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateReactionRequest<'a> {
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateReviewCommentRequest<'a> {
    body: &'a str,
    commit_id: &'a str,
    path: &'a str,
    line: u64,
    side: &'static str,
}

async fn parse_json_response<T: DeserializeOwned>(
    response: Response,
    operation: &str,
) -> Result<T> {
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

async fn parse_empty_response(response: Response, operation: &str) -> Result<()> {
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("failed to read GitHub response while trying to {operation}"))?;

    if !status.is_success() {
        bail!("GitHub failed to {operation} ({status}): {body}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_owner_and_repository() {
        let client = GitHubClient::new("token", "octocat/hello-world").expect("client");
        assert_eq!(client.owner, "octocat");
        assert_eq!(client.repository, "hello-world");
    }

    #[test]
    fn rejects_malformed_repository() {
        assert!(GitHubClient::new("token", "octocat").is_err());
        assert!(GitHubClient::new("token", "octocat/hello-world/extra").is_err());
    }
}
