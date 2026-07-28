# Future Review Checklist

Current flow: `PR -> one reviewer -> verifier -> comments`.

Deeper analysis: [`competitive_gap_analysis.md`](./competitive_gap_analysis.md).

How to build the do-now items: [`implementation_plan_do_now.md`](./implementation_plan_do_now.md).

## Do now

- Adjudicate a real eval corpus and gate on precision/recall, not only smoke samples.
- Track resolution rate from remembered fingerprints across later commits and merges.
- Add 2-3 diversified primary passes with clustering before the verifier.
- Retry OpenRouter HTTP 429 and 5xx with `Retry-After` or jittered backoff.
- Align docs with the live primary+verifier architecture (no implied specialist router).
- Enrich the summary comment with a human walkthrough of the change.
- Let risk level scale depth: tool steps, passes, and budget, not only prompt text.
- Make collaborator permissions configurable (`admin` today is too strict for replacement use).

## Later improvements

- Cache the Action image, Git objects, diff, file reads, and context by base SHA.
- Reuse a completed review only when head SHA, base SHA, models, config, and PRBot version match.
- Keep one reviewer and verifier by default.
- Add one specialist only for high-risk changes: auth, payments, migrations, APIs, or concurrency.
- Set per-task limits for cost, time, tokens, and tool calls.
- Settle reserved budget against actual provider usage, and stop optional tasks before the verifier budget is at risk.
- Record stage latency, tokens, cost, retries, completion rate, precision, P0/P1 recall, and resolution rate.
- Add multi-pass or multi-agent review only if evals prove a quality gain worth the additional cost.
- Support ambient thread replies, finding lifecycle states, and high-confidence suggested patches.
- Optionally ingest deterministic lint/SAST output as evidence rather than raw noisy comments.

## Industry ideas

- Cursor Bugbot: multi-pass majority voting, agentic tools, dynamic context, resolution-rate optimization.
- Codex: adapt depth to PR complexity, follow repository instructions, and optionally validate risky changes in a sandbox.
- GitHub Copilot: repository-wide and path-specific review instructions.
- CodeRabbit: walkthrough UX, path instructions, learnings from feedback, chat on review threads, lint hybrid.

Sources: [Cursor Bugbot](https://cursor.com/blog/building-bugbot), [Codex](https://openai.com/index/introducing-upgrades-to-codex/), [Copilot](https://docs.github.com/en/copilot/how-tos/use-copilot-agents/request-a-code-review/use-code-review?tool=vscode), and [CodeRabbit docs](https://docs.coderabbit.ai/pr-reviews/walkthroughs).
