# Future Review Checklist

Current flow: `PR -> one reviewer -> verifier -> comments`.

## Later improvements

- Cache the Action image, Git objects, diff, file reads, and context by base SHA.
- Reuse a completed review only when head SHA, base SHA, models, config, and PRBot version match.
- Keep one reviewer and verifier by default.
- Add one specialist only for high-risk changes: auth, payments, migrations, APIs, or concurrency.
- Set per-task limits for cost, time, tokens, and tool calls.
- Settle reserved budget against actual provider usage, and stop optional tasks before the verifier budget is at risk.
- Retry only HTTP 429 and 5xx responses with `Retry-After` or jittered backoff.
- Record stage latency, tokens, cost, retries, completion rate, precision, P0/P1 recall, and resolution rate.
- Add multi-pass or multi-agent review only if evals prove a quality gain worth the additional cost.

## Industry ideas

- Cursor Bugbot: dynamic context, validation, deduplication, and resolution-rate optimization.
- Codex: adapt depth to PR complexity, follow repository instructions, and optionally validate risky changes in a sandbox.
- GitHub Copilot: repository-wide and path-specific review instructions.

Sources: [Cursor Bugbot](https://cursor.com/blog/building-bugbot), [Codex](https://openai.com/index/introducing-upgrades-to-codex/), and [Copilot](https://docs.github.com/en/copilot/how-tos/use-copilot-agents/request-a-code-review/use-code-review?tool=vscode).
