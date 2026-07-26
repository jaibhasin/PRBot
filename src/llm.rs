use crate::types::BudgetSnapshot;
use anyhow::{bail, Context, Result};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

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

pub struct Budget {
    started: Instant,
    deadline: Duration,
    max_input_tokens: u64,
    max_cost_usd: f64,
    state: Mutex<BudgetState>,
}

#[derive(Default)]
struct BudgetState {
    input_tokens: u64,
    output_tokens: u64,
    estimated_cost_usd: f64,
}

impl Budget {
    pub fn new(max_minutes: u64, max_input_tokens: u64, max_cost_usd: f64) -> Self {
        Self {
            started: Instant::now(),
            deadline: Duration::from_secs(max_minutes.max(1) * 60),
            max_input_tokens,
            max_cost_usd,
            state: Mutex::new(BudgetState::default()),
        }
    }

    async fn reserve(&self, input_tokens: u64, max_output_tokens: u64) -> Result<()> {
        self.remaining_time()?;
        let estimated_cost =
            (input_tokens as f64 * 2.0 + max_output_tokens as f64 * 10.0) / 1_000_000.0;
        let mut state = self.state.lock().await;
        if state.input_tokens.saturating_add(input_tokens) > self.max_input_tokens {
            bail!("review input-token budget exhausted");
        }
        if state.estimated_cost_usd + estimated_cost > self.max_cost_usd {
            bail!("review estimated-cost budget exhausted");
        }
        state.input_tokens += input_tokens;
        state.estimated_cost_usd += estimated_cost;
        Ok(())
    }

    async fn record_usage(&self, usage: &Usage) {
        let mut state = self.state.lock().await;
        state.input_tokens = state.input_tokens.max(usage.prompt_tokens);
        state.output_tokens += usage.completion_tokens;
        if let Some(cost) = usage.cost {
            state.estimated_cost_usd = state.estimated_cost_usd.max(cost);
        }
    }

    pub fn remaining_time(&self) -> Result<Duration> {
        self.deadline
            .checked_sub(self.started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .context("review time budget exhausted")
    }

    pub async fn snapshot(&self) -> BudgetSnapshot {
        let state = self.state.lock().await;
        BudgetSnapshot {
            input_tokens: state.input_tokens,
            output_tokens: state.output_tokens,
            estimated_cost_usd: state.estimated_cost_usd,
            elapsed_seconds: self.started.elapsed().as_secs(),
        }
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

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    cost: Option<f64>,
}

fn estimate_tokens(value: &str) -> u64 {
    (value.chars().count() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn budget_rejects_input_over_limit() {
        let budget = Budget::new(1, 10, 10.0);
        assert!(budget.reserve(11, 0).await.is_err());
    }
}
