use super::{review_bundles, review_manifest};
use crate::config::ReviewConfig;
use crate::llm::{Budget, LlmClient};
use crate::repository::{GitRepository, RepositoryTools};
use crate::types::{
    ChangedFile, DiffHunk, DiffLine, DiffSide, FileStatus, ReviewAgent, ReviewBundle,
    ReviewManifest, RiskLevel,
};
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

#[tokio::test]
async fn primary_reviewer_verifies_findings_end_to_end() {
    let (address, server) = mock_server(vec![
        r#"{"findings":[{"path":"src/lib.rs","side":"RIGHT","anchor":"pub fn value() -> i32 { 2 }","priority":"P1","category":"correctness","title":"Changed result","body":"Existing callers require one.","evidence":[],"confidence":0.95}]}"#,
        r#"{"accepted_indices":[0]}"#,
    ]);

    let fixture = RepositoryFixture::new();
    let repository = Arc::new(
        GitRepository::from_worktree(&fixture.root, &fixture.base, &fixture.head)
            .expect("repository"),
    );
    let tools = Arc::new(RepositoryTools::new(repository, "PR context".to_owned()));
    let budget = Arc::new(Budget::new(1, 100_000, 10.0));
    let client =
        LlmClient::new("key", Some(format!("http://{address}/chat")), budget, 1).expect("client");
    let manifest = manifest();
    let config = ReviewConfig {
        max_concurrency: 1,
        ..ReviewConfig::default()
    };
    let result = review_manifest(&client, tools, &manifest, &config).await;

    assert!(result.failed_bundles.is_empty());
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].agent, ReviewAgent::Primary);
    assert_eq!(result.agent_runs.len(), 1);
    assert_eq!(result.agent_runs[0].agent, ReviewAgent::Primary);
    assert_eq!(result.agent_runs[0].bundle_ids, ["bundle"]);
    let requests = server.join().expect("server");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("Review these selected pull-request bundles"));
    assert!(!requests[0].contains("Route these review bundles"));
    assert!(requests[1].contains("accepted_indices"));
}

#[tokio::test]
async fn verifier_failure_marks_review_coverage_incomplete() {
    let (address, server) = mock_server(vec![
        r#"{"findings":[{"path":"src/lib.rs","side":"RIGHT","anchor":"pub fn value() -> i32 { 2 }","priority":"P1","category":"correctness","title":"Changed result","body":"Existing callers require one.","evidence":[],"confidence":0.95}]}"#,
        "not-json",
    ]);
    let fixture = RepositoryFixture::new();
    let repository = Arc::new(
        GitRepository::from_worktree(&fixture.root, &fixture.base, &fixture.head)
            .expect("repository"),
    );
    let tools = Arc::new(RepositoryTools::new(repository, "PR context".to_owned()));
    let budget = Arc::new(Budget::new(1, 100_000, 10.0));
    let client =
        LlmClient::new("key", Some(format!("http://{address}/chat")), budget, 1).expect("client");
    let result = review_manifest(
        &client,
        tools,
        &manifest(),
        &ReviewConfig {
            max_concurrency: 1,
            ..ReviewConfig::default()
        },
    )
    .await;

    assert!(result.findings.is_empty());
    assert_eq!(result.failed_bundles, ["independent-verifier"]);
    let correctness = result
        .agent_runs
        .iter()
        .find(|run| run.agent == ReviewAgent::Primary)
        .expect("primary");
    assert_eq!(correctness.candidate_findings, 1);
    assert_eq!(server.join().expect("server").len(), 2);
}

#[tokio::test]
async fn primary_reviewer_receives_every_selected_bundle_in_one_request() {
    let (address, server) = mock_server(vec![r#"{"findings":[]}"#]);
    let fixture = RepositoryFixture::new();
    let repository = Arc::new(
        GitRepository::from_worktree(&fixture.root, &fixture.base, &fixture.head)
            .expect("repository"),
    );
    let tools = Arc::new(RepositoryTools::new(repository, "PR context".to_owned()));
    let budget = Arc::new(Budget::new(1, 100_000, 10.0));
    let client =
        LlmClient::new("key", Some(format!("http://{address}/chat")), budget, 1).expect("client");
    let result = review_manifest(
        &client,
        tools,
        &manifest_with_two_bundles(),
        &ReviewConfig::default(),
    )
    .await;

    assert!(result.failed_bundles.is_empty());
    assert!(result.findings.is_empty());
    assert_eq!(result.agent_runs[0].bundle_ids, ["bundle", "second"]);
    let requests = server.join().expect("server");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("src/lib.rs"));
    assert!(requests[0].contains("src/second.rs"));
}

#[tokio::test]
async fn primary_reviewer_failure_skips_verification() {
    let (address, server) = mock_server(vec!["not-json"]);
    let fixture = RepositoryFixture::new();
    let repository = Arc::new(
        GitRepository::from_worktree(&fixture.root, &fixture.base, &fixture.head)
            .expect("repository"),
    );
    let tools = Arc::new(RepositoryTools::new(repository, "PR context".to_owned()));
    let budget = Arc::new(Budget::new(1, 100_000, 10.0));
    let client =
        LlmClient::new("key", Some(format!("http://{address}/chat")), budget, 1).expect("client");
    let result = review_manifest(&client, tools, &manifest(), &ReviewConfig::default()).await;

    assert_eq!(result.failed_bundles, ["primary-reviewer"]);
    assert_eq!(
        result.agent_runs[0].status,
        crate::types::AgentStatus::Failed
    );
    assert_eq!(server.join().expect("server").len(), 1);
}

#[tokio::test]
async fn empty_bundle_selection_skips_model_calls() {
    let fixture = RepositoryFixture::new();
    let repository = Arc::new(
        GitRepository::from_worktree(&fixture.root, &fixture.base, &fixture.head)
            .expect("repository"),
    );
    let tools = Arc::new(RepositoryTools::new(repository, "PR context".to_owned()));
    let budget = Arc::new(Budget::new(1, 100_000, 10.0));
    let client = LlmClient::new("key", Some("http://127.0.0.1:1/chat".to_owned()), budget, 1)
        .expect("client");
    let manifest = manifest();
    let result = review_bundles(&client, tools, &manifest, &[], &ReviewConfig::default()).await;

    assert!(result.findings.is_empty());
    assert!(result.failed_bundles.is_empty());
    assert_eq!(result.agent_runs.len(), 1);
    assert_eq!(
        result.agent_runs[0].status,
        crate::types::AgentStatus::Skipped
    );
}

fn manifest() -> ReviewManifest {
    let file = ChangedFile {
        path: "src/lib.rs".to_owned(),
        old_path: None,
        status: FileStatus::Modified,
        patch: "@@ -1 +1 @@\n-pub fn value() -> i32 { 1 }\n+pub fn value() -> i32 { 2 }\n"
            .to_owned(),
        hunks: vec![DiffHunk {
            header: "@@ -1 +1 @@".to_owned(),
            old_start: 1,
            new_start: 1,
            lines: vec![
                DiffLine {
                    side: DiffSide::Left,
                    old_line: Some(1),
                    new_line: None,
                    content: "pub fn value() -> i32 { 1 }".to_owned(),
                },
                DiffLine {
                    side: DiffSide::Right,
                    old_line: None,
                    new_line: Some(1),
                    content: "pub fn value() -> i32 { 2 }".to_owned(),
                },
            ],
        }],
    };
    ReviewManifest {
        files: vec![file],
        bundles: vec![ReviewBundle {
            id: "bundle".to_owned(),
            paths: vec!["src/lib.rs".to_owned()],
            hunk_count: 1,
            risk: RiskLevel::High,
            related_files: Vec::new(),
        }],
        ..ReviewManifest::default()
    }
}

fn manifest_with_two_bundles() -> ReviewManifest {
    let mut manifest = manifest();
    manifest.files.push(ChangedFile {
        path: "src/second.rs".to_owned(),
        old_path: None,
        status: FileStatus::Added,
        patch: "@@ -0,0 +1 @@\n+pub fn second() {}\n".to_owned(),
        hunks: vec![DiffHunk {
            header: "@@ -0,0 +1 @@".to_owned(),
            old_start: 0,
            new_start: 1,
            lines: vec![DiffLine {
                side: DiffSide::Right,
                old_line: None,
                new_line: Some(1),
                content: "pub fn second() {}".to_owned(),
            }],
        }],
    });
    manifest.bundles.push(ReviewBundle {
        id: "second".to_owned(),
        paths: vec!["src/second.rs".to_owned()],
        hunk_count: 1,
        risk: RiskLevel::Low,
        related_files: Vec::new(),
    });
    manifest
}

struct RepositoryFixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    base: String,
    head: String,
}

impl RepositoryFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        git(&root, &["init"]);
        git(&root, &["config", "user.email", "prbot@example.com"]);
        git(&root, &["config", "user.name", "PRBot"]);
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join("src/lib.rs"), "pub fn value() -> i32 { 1 }\n").expect("base");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "base"]);
        let base = git_output(&root, &["rev-parse", "HEAD"]);
        fs::write(root.join("src/lib.rs"), "pub fn value() -> i32 { 2 }\n").expect("head");
        git(&root, &["commit", "-am", "change"]);
        let head = git_output(&root, &["rev-parse", "HEAD"]);
        Self {
            _temp: temp,
            root,
            base,
            head,
        }
    }
}

fn git(path: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(path: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).expect("read");
        assert!(read > 0);
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("read");
        assert!(read > 0);
        bytes.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(bytes).expect("request utf8")
}

fn mock_server(contents: Vec<&'static str>) -> (SocketAddr, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for content in contents {
            let (mut stream, _) = listener.accept().expect("accept");
            requests.push(read_request(&mut stream));
            let body = json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": content,
                        "tool_calls": []
                    }
                }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        }
        requests
    });
    (address, server)
}
