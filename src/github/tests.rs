use super::client::next_link;
use super::{GitHubClient, ReviewInputComment};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

#[test]
fn accepts_only_owner_and_repository_shape() {
    assert!(GitHubClient::new("token", "octocat/hello").is_ok());
    assert!(GitHubClient::new("token", "octocat").is_err());
    assert!(GitHubClient::new("token", "a/b/c").is_err());
}

#[test]
fn parses_next_pagination_link() {
    let link = r#"<https://api.github.com/repos/a/b/issues/1/comments?page=2>; rel="next", <https://api.github.com/repos/a/b/issues/1/comments?page=3>; rel="last""#;
    assert!(next_link(link).expect("next").ends_with("page=2"));
}

#[tokio::test]
async fn follows_pagination_and_checks_admin_permission() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let first_body = r#"[{"id":1,"body":"first","user":{"login":"owner","type":"User"}}]"#;
    let second_body = r#"[{"id":2,"body":"second","user":{"login":"owner","type":"User"}}]"#;
    let permission_body = r#"{"permission":"admin"}"#;
    let responses = vec![
        response(
            first_body,
            Some(&format!(
                "<http://{address}/repos/octocat/hello/issues/1/comments?per_page=100&page=2>; rel=\"next\""
            )),
        ),
        response(second_body, None),
        response(permission_body, None),
    ];
    let server = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 8192];
            let _ = stream.read(&mut request).expect("read");
            stream.write_all(response.as_bytes()).expect("write");
        }
    });
    let client = GitHubClient::with_base_url("token", "octocat/hello", format!("http://{address}"))
        .expect("client");
    let comments = client.list_issue_comments(1).await.expect("comments");
    assert_eq!(comments.len(), 2);
    assert!(client
        .is_repository_admin("owner")
        .await
        .expect("permission"));
    server.join().expect("server");
}

#[tokio::test]
async fn publishes_one_formal_review_with_multiline_comments() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let (sender, receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        sender.send(read_request(&mut stream)).expect("send");
        let body = r#"{"id":99}"#;
        stream
            .write_all(response(body, None).as_bytes())
            .expect("write");
    });
    let client = GitHubClient::with_base_url("token", "octocat/hello", format!("http://{address}"))
        .expect("client");
    let id = client
        .create_review(
            1,
            "head",
            "Verified findings",
            vec![ReviewInputComment {
                path: "src/main.rs".to_owned(),
                body: "Finding".to_owned(),
                line: Some(12),
                side: Some("RIGHT".to_owned()),
                start_line: Some(10),
                start_side: Some("RIGHT".to_owned()),
                subject_type: None,
            }],
        )
        .await
        .expect("review");
    assert_eq!(id, 99);
    let request = receiver.recv().expect("request");
    assert!(request.contains(r#""event":"COMMENT""#));
    assert!(request.contains(r#""start_line":10"#));
    assert!(request.contains(r#""line":12"#));
    server.join().expect("server");
}

#[tokio::test]
async fn retries_transient_write_failures() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let (sender, receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("write");
        let (mut stream, _) = listener.accept().expect("accept");
        sender.send(read_request(&mut stream)).expect("send");
        let body = r#"{"id":1,"body":"ok","user":{"login":"owner","type":"User"}}"#;
        stream
            .write_all(response(body, None).as_bytes())
            .expect("write");
    });
    let client = GitHubClient::with_base_url("token", "octocat/hello", format!("http://{address}"))
        .expect("client");
    let comment = client
        .create_issue_comment(1, "hello")
        .await
        .expect("comment");
    assert_eq!(comment.id, 1);
    let request = receiver.recv().expect("request");
    assert!(request.contains("hello"));
    server.join().expect("server");
}

#[tokio::test]
async fn treats_non_collaborator_not_found_as_unauthorized() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0_u8; 8192];
        let _ = stream.read(&mut request).expect("read");
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("write");
    });
    let client = GitHubClient::with_base_url("token", "octocat/hello", format!("http://{address}"))
        .expect("client");
    assert!(!client
        .is_repository_admin("outsider")
        .await
        .expect("permission"));
    server.join().expect("server");
}

fn response(body: &str, link: Option<&str>) -> String {
    let link = link
        .map(|value| format!("Link: {value}\r\n"))
        .unwrap_or_default();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{link}Connection: close\r\n\r\n{body}",
        body.len()
    )
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
    String::from_utf8(bytes).expect("utf8")
}
