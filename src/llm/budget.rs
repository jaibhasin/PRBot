use crate::types::BudgetSnapshot;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

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
    reported_cost_usd: f64,
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

    pub(super) async fn reserve(&self, input_tokens: u64, max_output_tokens: u64) -> Result<()> {
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

    pub(super) async fn record_usage(&self, usage: &Usage) {
        let mut state = self.state.lock().await;
        state.output_tokens += usage.completion_tokens;
        if let Some(cost) = usage.cost {
            state.reported_cost_usd += cost;
            state.estimated_cost_usd = state.estimated_cost_usd.max(state.reported_cost_usd);
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

#[derive(Deserialize)]
pub(super) struct Usage {
    #[serde(default, rename = "prompt_tokens")]
    pub _prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_input_over_limit() {
        let budget = Budget::new(1, 10, 10.0);
        assert!(budget.reserve(11, 0).await.is_err());
    }
}
