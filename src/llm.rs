use anyhow::{bail, Context, Result};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

mod budget;

pub use budget::Budget;
use budget::Usage;

const DEFAULT_OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const MAX_OUTPUT_TOKENS: u64 = 6_000;

#[derive(Clone)]
pub struct LlmClient {
    client: reqwest::Client,
    api_key: String,
    endpoint: String,
    budget: Arc<Budget>,
    semaphore: Arc<Semaphore>,
}

impl LlmClient {
    pub fn new(
        api_key: impl Into<String>,
        endpoint: Option<String>,
        budget: Arc<Budget>,
        concurrency: usize,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .build()
            .context("failed to build OpenRouter client")?;
        Ok(Self {
            client,
            api_key: api_key.into(),
            endpoint: endpoint.unwrap_or_else(|| DEFAULT_OPENROUTER_URL.to_owned()),
            budget,
            semaphore: Arc::new(Semaphore::new(concurrency.max(1))),
        })
    }

    pub async fn run_agent<F, Fut>(
        &self,
        model: &str,
        system: &str,
        user: &str,
        tools: Vec<Value>,
        max_steps: usize,
        execute_tool: F,
    ) -> Result<String>
    where
        F: Fn(String, String) -> Fut,
        Fut: Future<Output = Result<String>>,
    {
        let mut messages = vec![
            json!({"role":"system","content":system}),
            json!({"role":"user","content":user}),
        ];
        for _ in 0..max_steps {
            let response = self.completion(model, &messages, &tools).await?;
            let message = response
                .choices
                .into_iter()
                .next()
                .context("OpenRouter response contained no choices")?
                .message;
            if message.tool_calls.is_empty() {
                return message
                    .content
                    .filter(|value| !value.trim().is_empty())
                    .context("model returned neither tool calls nor content");
            }
            messages.push(serde_json::to_value(&message).context("serialize assistant message")?);
            for call in message.tool_calls {
                let result = execute_tool(call.function.name.clone(), call.function.arguments)
                    .await
                    .unwrap_or_else(|error| format!("tool error: {error:#}"));
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": result
                }));
            }
        }
        bail!("model exceeded the maximum repository tool steps")
    }

    async fn completion(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
    ) -> Result<ChatCompletionResponse> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .context("LLM concurrency semaphore closed")?;
        let serialized =
            serde_json::to_string(messages).context("failed to measure model request")?;
        let estimated_input = estimate_tokens(&serialized);
        self.budget
            .reserve(estimated_input, MAX_OUTPUT_TOKENS)
            .await?;
        let remaining = self.budget.remaining_time()?;
        let request = ChatCompletionRequest {
            model,
            messages,
            tools,
            tool_choice: if tools.is_empty() { None } else { Some("auto") },
            max_tokens: MAX_OUTPUT_TOKENS,
            temperature: 0.0,
        };

        let response = tokio::time::timeout(
            remaining,
            self.client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&request)
                .send(),
        )
        .await
        .context("OpenRouter request exceeded review deadline")?
        .context("failed to call OpenRouter")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read OpenRouter response")?;
        if !status.is_success() {
            if status == StatusCode::TOO_MANY_REQUESTS {
                bail!("OpenRouter rate limit exceeded: {body}");
            }
            bail!("OpenRouter returned {status}: {body}");
        }
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&body).context("failed to parse OpenRouter response")?;
        if let Some(usage) = &parsed.usage {
            self.budget.record_usage(usage).await;
        }
        Ok(parsed)
    }
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: &'a [Value],
    #[serde(skip_serializing_if = "slice_empty")]
    tools: &'a [Value],
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    max_tokens: u64,
    temperature: f32,
}

fn slice_empty<T>(value: &&[T]) -> bool {
    value.is_empty()
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: AssistantMessage,
}

#[derive(Deserialize, Serialize)]
struct AssistantMessage {
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ToolCall>,
}

#[derive(Deserialize, Serialize)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ToolFunction,
}

#[derive(Deserialize, Serialize)]
struct ToolFunction {
    name: String,
    arguments: String,
}

fn estimate_tokens(value: &str) -> u64 {
    (value.chars().count() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    #[tokio::test]
    async fn completes_bounded_repository_tool_loop() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let responses = [
            r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.001}}"#,
            r#"{"choices":[{"message":{"role":"assistant","content":"done","tool_calls":[]}}],"usage":{"prompt_tokens":12,"completion_tokens":1,"cost":0.001}}"#,
        ];
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) = listener.accept().expect("accept");
                requests.push(read_request(&mut stream));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
            requests
        });
        let budget = Arc::new(Budget::new(1, 10_000, 10.0));
        let client = LlmClient::new("key", Some(format!("http://{address}/chat")), budget, 1)
            .expect("client");
        let result = client
            .run_agent(
                "provider/reviewer",
                "system",
                "user",
                vec![json!({"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}})],
                2,
                |name, _arguments| async move {
                    assert_eq!(name, "read_file");
                    Ok("file contents".to_owned())
                },
            )
            .await
            .expect("agent");
        assert_eq!(result, "done");
        let requests = server.join().expect("server");
        assert!(requests[1].contains("tool_call_id"));
        assert!(requests[1].contains("file contents"));
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
}
