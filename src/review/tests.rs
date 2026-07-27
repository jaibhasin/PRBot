use super::{config_from_args, run, ReviewArgs};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

#[tokio::test]
async fn unauthorized_automatic_review_exits_before_api_key_validation() {
    let (base_url, server) = github_server("read");
    let result = run(args(&base_url)).await;
    assert!(result.is_ok());
    server.join().expect("server");
}

#[tokio::test]
async fn authorized_review_requires_api_key_before_repository_fetch() {
    let (base_url, server) = github_server("admin");
    let error = run(args(&base_url)).await.expect_err("missing key");
    assert!(error.to_string().contains("OPENROUTER_API_KEY"));
    server.join().expect("server");
}

#[test]
fn same_model_is_allowed_for_review_and_verification() {
    let mut args = args("http://127.0.0.1");
    args.review_model = Some("deepseek/deepseek-v4-flash".to_owned());
    args.verification_model = Some("deepseek/deepseek-v4-flash".to_owned());
    let config = config_from_args(&args).expect("same model allowed");
    assert_eq!(config.review_model, config.verification_model);
}

/// Builds review arguments for a repository pull request using the specified GitHub API endpoint.
///
/// # Examples
///
/// ```
/// let review_args = args("http://127.0.0.1:8080");
/// assert_eq!(
///     review_args.github_api_url.as_deref(),
///     Some("http://127.0.0.1:8080")
/// );
/// ```
fn args(base_url: &str) -> ReviewArgs {
    ReviewArgs {
        repository: Some("octocat/hello".to_owned()),
        pr_number: Some("1".to_owned()),
        openrouter_api_key: None,
        github_token: Some("token".to_owned()),
        github_api_url: Some(base_url.to_owned()),
        review_model: None,
        verification_model: None,
        max_review_minutes: 15,
        max_input_tokens: 500_000,
        max_cost_usd: 3.0,
        max_concurrency: 8,
        max_comments: 12,
        engine: "contextual".to_owned(),
        dry_run: false,
        eval_json: false,
    }
}

/// Starts a local HTTP server that serves a pull request response followed by a permission response.
///
/// # Arguments
///
/// * `permission` - The permission value included in the second response.
///
/// # Returns
///
/// A tuple containing the server's base URL and a handle for joining its serving thread.
///
/// # Examples
///
/// ```
/// let (base_url, server) = github_server("read");
/// assert!(base_url.starts_with("http://"));
/// server.join().expect("server");
/// ```
fn github_server(permission: &'static str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let pull_request = r#"{"number":1,"title":"Test","body":"","user":{"login":"contributor","type":"User"},"base":{"sha":"base","ref":"main"},"head":{"sha":"head","ref":"feature"}}"#;
        for body in [
            pull_request.to_owned(),
            format!(r#"{{"permission":"{permission}"}}"#),
        ] {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 8192];
            let _ = stream.read(&mut request).expect("read");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        }
    });
    (format!("http://{address}"), server)
}
