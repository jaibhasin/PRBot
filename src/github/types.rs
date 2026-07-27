use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct GitHubUser {
    pub login: String,
    #[serde(rename = "type")]
    pub user_type: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub user: GitHubUser,
    pub base: PullRequestRef,
    pub head: PullRequestRef,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PullRequestRef {
    pub sha: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub body: String,
    pub user: GitHubUser,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReviewComment {
    pub body: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CheckRunsResponse {
    pub check_runs: Vec<CheckRun>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub output: Option<CheckOutput>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CheckOutput {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PermissionResponse {
    pub permission: String,
}

#[derive(Debug, Deserialize)]
pub struct CreatedReview {
    pub id: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewInputComment {
    pub path: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateReviewRequest {
    pub commit_id: String,
    pub body: String,
    pub event: &'static str,
    pub comments: Vec<ReviewInputComment>,
}

#[derive(Debug, Serialize)]
pub struct CommentRequest {
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct ReactionRequest {
    pub content: String,
}
