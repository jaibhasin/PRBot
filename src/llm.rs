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
const OPENROUTER_MAX_ATTEMPTS: u32 = 3;
const OPENROUTER_RETRY_BASE: Duration = Duration::from_millis(250);
const OPENROUTER_RETRY_CAP: Duration = Duration::from_secs(10);

/// One bounded agent invocation: the model, its prompts, the tools it may call,
/// and how many model/tool rounds it gets.
pub struct AgentCall<'a> {
    /// OpenRouter model slug.
    pub model: &'a str,
    /// System prompt establishing the agent's role and output contract.
    pub system: &'a str,
    /// Initial user prompt, including the exact JSON schema the agent must return.
    pub user: &'a str,
    /// Tool definitions offered to the model.
    pub tools: Vec<Value>,
    /// Maximum number of model and tool-execution iterations.
    pub max_steps: usize,
    /// Sampling temperature for this agent call.
    pub temperature: f32,
    /// Short name used in step logs, for example `primary` or `verifier`.
    pub label: &'a str,
}

#[derive(Clone)]
pub struct LlmClient {
    client: reqwest::Client,
    api_key: String,
    endpoint: String,
    budget: Arc<Budget>,
    semaphore: Arc<Semaphore>,
}

impl LlmClient {
    /// Creates an OpenRouter client with the specified credentials, endpoint, budget, and concurrency limit.
    ///
    /// A missing endpoint uses the default OpenRouter URL. A concurrency value of zero is treated as one.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// let client = LlmClient::new(
    ///     "api-key",
    ///     None,
    ///     Arc::new(Budget::new(1, 10_000, 10.0)),
    ///     2,
    /// )
    /// .unwrap();
    /// ```
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

    pub async fn remaining_input_tokens(&self) -> u64 {
        self.budget.remaining_input_tokens().await
    }

    pub fn remaining_time_secs(&self) -> u64 {
        self.budget
            .remaining_time()
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    /// Runs a bounded tool-using conversation and returns the model's final content.
    ///
    /// Tool execution errors are added to the conversation as tool results so the model
    /// can continue. An error is returned if the model provides no choices or usable
    /// final content, or if `max_steps` is exceeded.
    ///
    /// # Arguments
    ///
    /// * `run` — Model, prompts, tools, step ceiling, and step-log label for this agent.
    /// * `execute_tool` — Callback that executes a tool by name with its arguments.
    ///
    /// # Examples
    ///
    /// ```
    /// # async fn example(client: &LlmClient) -> anyhow::Result<()> {
    /// let result = client
    ///     .run_agent(
    ///         AgentCall {
    ///             model: "model",
    ///             system: "You are helpful.",
    ///             user: "Say hello.",
    ///             tools: Vec::new(),
    ///             max_steps: 3,
    ///             temperature: 0.0,
    ///             label: "example",
    ///         },
    ///         |_name, _arguments| async { Ok::<_, anyhow::Error>("done".to_owned()) },
    ///     )
    ///     .await?;
    ///
    /// assert_eq!(result, "Hello!");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Returns
    ///
    /// The model's non-empty final response content.
    pub async fn run_agent<F, Fut>(&self, call: AgentCall<'_>, execute_tool: F) -> Result<String>
    where
        F: Fn(String, String) -> Fut,
        Fut: Future<Output = Result<String>>,
    {
        let AgentCall {
            model,
            system,
            user,
            tools,
            max_steps,
            temperature,
            label,
        } = call;
        let mut messages = vec![
            json!({"role":"system","content":system}),
            json!({"role":"user","content":user}),
        ];
        crate::progress::step(format!(
            "{label}: start model={model} max_steps={max_steps}"
        ));
        let empty_tools: Vec<Value> = Vec::new();
        for step in 0..max_steps {
            let serialized =
                serde_json::to_string(&messages).context("failed to measure agent messages")?;
            let estimated_input = estimate_tokens(&serialized);
            let remaining_input = self.budget.remaining_input_tokens().await;
            let remaining_after = remaining_input.saturating_sub(estimated_input);
            // Finalize on the last step, or earlier once another tool round would likely
            // blow the cumulative input-token budget. Never skip tools on step 0 unless
            // it is also the final step.
            let force_finalize = step + 1 >= max_steps
                || (step > 0 && remaining_after < estimated_input.saturating_add(2_000));
            let step_tools = if force_finalize {
                empty_tools.as_slice()
            } else {
                tools.as_slice()
            };
            let snapshot = self.budget.snapshot().await;
            if force_finalize {
                crate::progress::step(format!(
                    "{label}: step {}/{} finalize budget_in={} remaining_in={}",
                    step + 1,
                    max_steps,
                    snapshot.input_tokens,
                    remaining_input
                ));
                messages.push(json!({
                    "role": "user",
                    "content": "Stop using tools. Using only the evidence already gathered, reply now with the final answer in exactly the JSON schema requested in the first user message. Output JSON only."
                }));
            } else {
                crate::progress::step(format!(
                    "{label}: step {}/{} calling model budget_in={} remaining_in={}",
                    step + 1,
                    max_steps,
                    snapshot.input_tokens,
                    remaining_input
                ));
            }
            let response = self
                .completion(model, &messages, step_tools, MAX_OUTPUT_TOKENS, temperature)
                .await?;
            let message = response
                .choices
                .into_iter()
                .next()
                .context("OpenRouter response contained no choices")?
                .message;
            if message.tool_calls.is_empty() || force_finalize {
                let content_len = message.content.as_ref().map(|c| c.len()).unwrap_or(0);
                crate::progress::step(format!(
                    "{label}: step {}/{} finished content_chars={content_len}",
                    step + 1,
                    max_steps
                ));
                return message
                    .content
                    .filter(|value| !value.trim().is_empty())
                    .context("model returned neither tool calls nor content");
            }
            let tool_names = message
                .tool_calls
                .iter()
                .map(|call| call.function.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            crate::progress::step(format!(
                "{label}: step {}/{} tool_calls={tool_names}",
                step + 1,
                max_steps
            ));
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

    /// Requests a short text response without exposing repository tools.
    ///
    /// This is used for bounded classification tasks such as choosing a GitHub
    /// reaction before PRBot begins a longer command.
    pub async fn respond(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_output_tokens: u64,
    ) -> Result<String> {
        let messages = [
            json!({"role":"system","content":system}),
            json!({"role":"user","content":user}),
        ];
        let response = self
            .completion(model, &messages, &[], max_output_tokens, 0.0)
            .await?;
        response
            .choices
            .into_iter()
            .next()
            .context("OpenRouter response contained no choices")?
            .message
            .content
            .filter(|value| !value.trim().is_empty())
            .context("model returned no content")
    }

    /// Sends a chat completion request and records any reported usage against the shared budget.
    ///
    /// Retries transient HTTP 429 and 5xx responses up to three attempts.
    /// Budget reservation happens once per logical completion, not per retry.
    async fn completion(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        max_output_tokens: u64,
        temperature: f32,
    ) -> Result<ChatCompletionResponse> {
        let serialized =
            serde_json::to_string(messages).context("failed to measure model request")?;
        let estimated_input = estimate_tokens(&serialized);
        self.budget
            .reserve(estimated_input, max_output_tokens)
            .await?;
        let request = ChatCompletionRequest {
            model,
            messages,
            tools,
            tool_choice: if tools.is_empty() { None } else { Some("auto") },
            max_tokens: max_output_tokens,
            temperature,
        };

        let mut delay = OPENROUTER_RETRY_BASE;
        for attempt in 0..OPENROUTER_MAX_ATTEMPTS {
            let permit = self
                .semaphore
                .acquire()
                .await
                .context("LLM concurrency semaphore closed")?;
            let remaining = self.budget.remaining_time()?;
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
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = response
                .text()
                .await
                .context("failed to read OpenRouter response")?;
            drop(permit);

            if status.is_success() {
                let parsed: ChatCompletionResponse =
                    serde_json::from_str(&body).context("failed to parse OpenRouter response")?;
                if let Some(usage) = &parsed.usage {
                    self.budget.record_usage(usage).await;
                }
                return Ok(parsed);
            }

            let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            if !retryable || attempt + 1 == OPENROUTER_MAX_ATTEMPTS {
                if status == StatusCode::TOO_MANY_REQUESTS {
                    bail!("OpenRouter rate limit exceeded: {body}");
                }
                bail!("OpenRouter returned {status}: {body}");
            }

            let wait = retry_after.unwrap_or(delay).min(OPENROUTER_RETRY_CAP);
            let remaining = self.budget.remaining_time().unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                bail!("OpenRouter returned {status} with no remaining review time: {body}");
            }
            crate::progress::step(format!(
                "openrouter: retry attempt={} status={} wait_ms={}",
                attempt + 1,
                status.as_u16(),
                wait.as_millis()
            ));
            if !wait.is_zero() {
                tokio::time::sleep(wait.min(remaining)).await;
            }
            delay = (delay * 2).min(OPENROUTER_RETRY_CAP);
        }
        unreachable!("retry loop always returns on final attempt")
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

/// Determines whether a slice contains no elements.
///
/// # Examples
///
/// ```
/// assert!(slice_empty(&&[][..]));
/// assert!(!slice_empty(&&[1, 2][..]));
/// ```
///
/// # Returns
///
/// `true` if the slice is empty, `false` otherwise.
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

/// Estimates the number of tokens represented by a string.
///
/// # Examples
///
/// ```
/// assert_eq!(estimate_tokens("hello"), 2);
/// assert_eq!(estimate_tokens(""), 0);
/// ```
///
/// # Returns
///
/// An approximate token count based on the string's Unicode scalar value count.
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
                AgentCall {
                    model: "provider/reviewer",
                    system: "system",
                    user: "user",
                    tools: vec![json!({"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}})],
                    max_steps: 2,
                    temperature: 0.0,
                    label: "test",
                },
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

    #[tokio::test]
    async fn retries_transient_openrouter_failures() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let success = r#"{"choices":[{"message":{"role":"assistant","content":"ok","tool_calls":[]}}],"usage":{"prompt_tokens":4,"completion_tokens":1,"cost":0.001}}"#;
        let server = thread::spawn(move || {
            let mut statuses = Vec::new();
            for (status, body) in [
                ("429 Too Many Requests", r#"{"error":"rate"}"#),
                ("503 Service Unavailable", r#"{"error":"busy"}"#),
                ("200 OK", success),
            ] {
                let (mut stream, _) = listener.accept().expect("accept");
                let _ = read_request(&mut stream);
                statuses.push(status.to_owned());
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nRetry-After: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
            statuses
        });
        let budget = Arc::new(Budget::new(1, 10_000, 10.0));
        let client = LlmClient::new(
            "key",
            Some(format!("http://{address}/chat")),
            budget.clone(),
            1,
        )
        .expect("client");
        let result = client
            .respond("provider/reviewer", "system", "user", 16)
            .await
            .expect("response");
        assert_eq!(result, "ok");
        let statuses = server.join().expect("server");
        assert_eq!(statuses.len(), 3);
        // Reserve happens once even when HTTP retries.
        let snapshot = budget.snapshot().await;
        assert!(snapshot.input_tokens > 0);
        assert_eq!(snapshot.output_tokens, 1);
    }

    #[tokio::test]
    async fn does_not_retry_non_transient_openrouter_failures() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = read_request(&mut stream);
            let body = r#"{"error":"bad request"}"#;
            let response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
            // A second accept would hang if the client retried.
            listener.set_nonblocking(true).expect("nonblocking");
            listener.accept().is_err()
        });
        let budget = Arc::new(Budget::new(1, 10_000, 10.0));
        let client = LlmClient::new("key", Some(format!("http://{address}/chat")), budget, 1)
            .expect("client");
        let error = client
            .respond("provider/reviewer", "system", "user", 16)
            .await
            .expect_err("should fail");
        assert!(error.to_string().contains("400"));
        assert!(server.join().expect("server"));
    }

    #[tokio::test]
    async fn finalizes_before_input_token_budget_is_exhausted() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let tool_body = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}}]}}],"usage":{"prompt_tokens":80,"completion_tokens":8,"cost":0.001}}"#;
        let final_body = r#"{"choices":[{"message":{"role":"assistant","content":"{\"findings\":[]}","tool_calls":[]}}],"usage":{"prompt_tokens":90,"completion_tokens":6,"cost":0.001}}"#;
        let server = thread::spawn(move || {
            let mut saw_finalize = false;
            let mut request_count = 0_usize;
            // Accept a few requests; stop after finalize response.
            for _ in 0..8 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let request = read_request(&mut stream);
                request_count += 1;
                let finalize = request.contains("Stop using tools");
                saw_finalize |= finalize;
                let body = if finalize { final_body } else { tool_body };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
                if finalize {
                    break;
                }
            }
            (saw_finalize, request_count)
        });

        // Tight cumulative input budget: endless tool looping would exhaust it.
        let budget = Arc::new(Budget::new(1, 350, 10.0));
        let client = LlmClient::new(
            "key",
            Some(format!("http://{address}/chat")),
            budget.clone(),
            1,
        )
        .expect("client");
        let result = client
            .run_agent(
                AgentCall {
                    model: "provider/reviewer",
                    system: "system",
                    user: "user",
                    tools: vec![json!({"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}})],
                    max_steps: 12,
                    temperature: 0.0,
                    label: "test",
                },
                |_name, _arguments| async move {
                    Ok("x".repeat(400))
                },
            )
            .await
            .expect("agent should finalize instead of exhausting input budget");
        assert_eq!(result, "{\"findings\":[]}");
        let snapshot = budget.snapshot().await;
        assert!(
            snapshot.input_tokens <= 350,
            "used {} input tokens",
            snapshot.input_tokens
        );
        let (saw_finalize, request_count) = server.join().expect("server");
        assert!(saw_finalize, "expected a no-tools finalize turn");
        assert!(request_count >= 2, "expected tool use then finalize");
        assert!(
            request_count < 12,
            "should finalize before burning all steps"
        );
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
