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
    /// Creates a budget with time, input-token, and estimated-cost limits.
    ///
    /// The time limit is set to at least one minute.
    ///
    /// # Examples
    ///
    /// ```
    /// let budget = Budget::new(1, 10_000, 1.0);
    /// assert!(budget.remaining_time().is_ok());
    /// ```
    pub fn new(max_minutes: u64, max_input_tokens: u64, max_cost_usd: f64) -> Self {
        Self {
            started: Instant::now(),
            deadline: Duration::from_secs(max_minutes.max(1) * 60),
            max_input_tokens,
            max_cost_usd,
            state: Mutex::new(BudgetState::default()),
        }
    }

    /// Reserves resources for a planned operation against the available budgets.
    ///
    /// # Arguments
    ///
    /// * `input_tokens` - Number of input tokens to reserve.
    /// * `max_output_tokens` - Maximum number of output tokens expected.
    ///
    /// # Errors
    ///
    /// Returns an error if the time budget is exhausted, or if the reservation exceeds the input-token or estimated-cost budget.
    ///
    /// # Examples
    ///
    /// ```
    /// # async fn example(budget: &Budget) {
    /// budget.reserve(1_000, 2_000).await.unwrap();
    /// # }
    /// ```
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

    /// Records completion-token usage and any reported cost in the budget.
    ///
    /// # Examples
    ///
    /// ```
    /// # async fn example() {
    /// let budget = Budget::new(1, 10_000, 1.0);
    /// let usage = Usage {
    ///     _prompt_tokens: 0,
    ///     completion_tokens: 100,
    ///     cost: Some(0.02),
    /// };
    ///
    /// budget.record_usage(&usage).await;
    /// assert_eq!(budget.snapshot().await.output_tokens, 100);
    /// # }
    /// ```
    pub(super) async fn record_usage(&self, usage: &Usage) {
        let mut state = self.state.lock().await;
        state.output_tokens += usage.completion_tokens;
        if let Some(cost) = usage.cost {
            state.reported_cost_usd += cost;
            state.estimated_cost_usd = state.estimated_cost_usd.max(state.reported_cost_usd);
        }
    }

    /// Determines the duration remaining in the budget.
    ///
    /// # Examples
    ///
    /// ```
    /// let budget = Budget::new(1, 1_000, 1.0);
    /// assert!(budget.remaining_time().is_ok());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when the time budget has been exhausted.
    pub fn remaining_time(&self) -> Result<Duration> {
        self.deadline
            .checked_sub(self.started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .context("review time budget exhausted")
    }

    /// Captures the current token usage, estimated cost, and elapsed time.
    ///
    /// # Examples
    ///
    /// ```
    /// let runtime = tokio::runtime::Runtime::new().unwrap();
    /// runtime.block_on(async {
    ///     let budget = Budget::new(1, 10_000, 1.0);
    ///     let snapshot = budget.snapshot().await;
    ///
    ///     assert_eq!(snapshot.input_tokens, 0);
    ///     assert_eq!(snapshot.output_tokens, 0);
    /// });
    /// ```
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
