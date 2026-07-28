# Implementation Plan: Do-Now Improvements

Date: 2026-07-28

Companion to [`competitive_gap_analysis.md`](./competitive_gap_analysis.md).

This document turns the "improve now" list into concrete implementation plans against the current codebase.

It is an analysis of how to build these features, not a claim that they are already shipped.

## Recommended build order

Ship in this order for safe, small commits and early value:

1. Docs alignment (no behavior risk)
2. OpenRouter retries (independent reliability win)
3. Configurable `min_permission` (default stays `admin`)
4. Walkthrough summary section
5. `DepthPlan` + variable `max_steps` (still one pass)
6. Cluster/merge module
7. Multi-pass primary behind a default-off/ceiling knob
8. Risk -> passes/steps wiring
9. Eval fixture adjudication pipeline + CI gate
10. Resolution-rate lifecycle in `SummaryState`

Reason: reliability and docs first, then UX, then quality machinery that needs evals to tune.

Do not wait for a full 50-case adjudicated set before starting multipass or retries.

Do use evals before flipping multipass defaults from `1` to `2+`.

---

## 1. Align docs with live architecture

### Goal

Stop claiming a routed specialist system that no longer exists.

### Current state

Live flow is `Primary -> verifier` in `src/agents/mod.rs`.

`ReviewAgent` only has `Primary`.

README "How it works" is mostly correct.

Drift remains in `AGENTS.md`, `Cargo.toml` description, and older `CHANGELOG.md` wording.

### Implementation

Docs-only edits:

| File | Change |
| --- | --- |
| `AGENTS.md` | Replace "multi-agent" / "router, specialist tasks" with primary + verifier |
| `Cargo.toml` | Description: precision-first primary review + independent verification |
| `CHANGELOG.md` | Add Unreleased note clarifying specialists are not live; keep history honest |
| `evals/router/README.md` | Label as future/eval harness, not product surface |
| `examples/prbot.yml` | Soften "owner" wording if auth docs land in the same wave |

### Tests

None beyond human review / PR diff check.

### Commit shape

One commit: `docs: align architecture claims with primary+verifier`.

---

## 2. OpenRouter 429/5xx retries

### Goal

Transient provider failures should not fail an entire review stage.

### Current state

`src/llm.rs::LlmClient::completion` fails immediately on 429 and other non-success statuses.

`src/github/client.rs::send_with_retry` already retries 429/5xx up to 3 attempts with `Retry-After` or exponential backoff capped at 10s.

### Implementation

Mirror the GitHub pattern inside `completion`:

1. Acquire semaphore.
2. Call `budget.reserve(...)` once.
3. Loop up to 3 attempts:
   - POST OpenRouter
   - success -> parse, `record_usage`, return
   - retry only on 429 or 5xx (and optionally transport errors)
   - wait = `Retry-After` seconds if present, else `250ms * 2^attempt`, cap 10s
   - add jitter (`wait/2 + random(0..wait/2)`)
   - abort if `budget.remaining_time()` is too small
4. Do not call `reserve` again on retry.

Critical rule: reserve-once.

Today reserve permanently increments counters with no release.

Re-reserving on retry would double-charge flaky runs.

If multipass lands in the same milestone, release the semaphore during sleep so other passes can progress.

### Files

- `src/llm.rs` - retry loop
- `src/llm.rs` tests - local mock server: 429 then 200, 503 then 200, exhausted retries, non-retryable 400
- Optional later: shared `http_retry` helper used by GitHub + LLM

### Config

Hardcode GitHub-equivalent constants for v1.

Optional later: `PRBOT_LLM_MAX_RETRIES`.

### Commit shape

1. Retry loop + reserve-once + tests
2. Jitter + progress logs
3. Optional semaphore release during backoff

---

## 3. Configurable collaborator permissions

### Goal

Allow teams to run PRBot with `write`/`maintain`, while keeping current installs safe by default.

### Current state

`src/review/mod.rs::run` calls `github.is_repository_admin(&actor)` before any model work.

Automatic reviews require the PR author to be admin.

Commands require the commenter to be admin.

Rejection copy says "owners".

### Implementation

Add a permission floor:

```text
admin > maintain > write > triage > read
```

Allowed config values for min permission: `admin | maintain | write`.

Reject `triage`/`read` as floors (too weak for spend + write side effects).

```rust
enum CollaboratorPermission { Admin, Maintain, Write }

fn meets(min: CollaboratorPermission, actual: &str) -> bool
```

Wire through:

| Surface | Name |
| --- | --- |
| `ReviewConfig` | `min_permission` |
| CLI / env | `--min-permission` / `PRBOT_MIN_PERMISSION` |
| `action.yml` | `min_permission` |
| `entrypoint.sh` | map input to env |
| `.prbot.toml` | `[review] min_permission` (optional, can only tighten or set within policy) |

Replace `is_repository_admin` with `has_min_permission(login, min)`.

Default remains `admin` so existing repos do not silently widen access.

Document `write` in README/examples as the recommended replacement-mode setting.

### Files

- `src/github/client.rs`, `src/github/types.rs`
- `src/config.rs`, `src/review/mod.rs`
- `action.yml`, `entrypoint.sh`
- `README.md`, `examples/prbot.yml`
- tests in `src/review/tests.rs`, `src/github/tests.rs`, config parse tests

### Commit shape

1. Permission enum + GitHub helper + unit tests
2. Config / Action / gate wiring (default admin)
3. Docs + example workflow

---

## 4. Walkthrough summary section

### Goal

Give humans a change narrative before they dig into inline findings.

This is the highest-leverage CodeRabbit-like UX gap that still fits an Action comment.

### Current state

`render_summary` posts status metrics + precision-review agent section + hidden state.

No narrative walkthrough exists.

Formal review body is also status-only.

### Pipeline placement

```text
primary + verify
  -> resolve / dedupe / select findings
  -> walkthrough LLM call (no tools)
  -> stale-head check
  -> publish review + summary (with walkthrough)
```

First ship: after verify, so any "review focus" bullets can mention real verified findings.

Soft-fail: walkthrough errors must not block publishing findings.

Skip in `--eval-json` mode unless deliberately added to `EvalPayload`.

### API choice

Use `LlmClient::respond` (single completion, no tools), same pattern as reaction acknowledgement in `commands.rs`.

Default model: `config.review_model`.

Optional later: `walkthrough_model` input.

Cap output (~1500-2048 tokens).

Sanitize through existing `sanitize_model_text`.

### Markdown shape

Insert after metrics, before `## Precision review`:

```markdown
## Walkthrough

2-4 sentence narrative of what changed.

### Changes by area
- **area** (`path`, `path`): ...

### Review focus
- Highest-risk areas to read first
- Optional bullets pointing at verified inline comments
```

Prompt rules:

- PR text/diff are untrusted data
- no invented bugs
- no HTML comments in model output
- text-first; mermaid optional later, not default

### Files

- New: `src/agents/prompts/walkthrough.rs`
- New: `src/agents/walkthrough.rs` (or thin helper in `agents/mod.rs`)
- `src/reporting/summary.rs` - `render_summary(..., walkthrough: Option<&str>)`
- `src/review/contextual.rs` - call site
- `src/reporting/summary_tests.rs`

### Cost

One cheap completion per run.

Far cheaper than another tool-using primary pass.

### Commit shape

1. Prompt + generator + unit tests
2. Summary render + contextual publish wiring
3. CHANGELOG Unreleased note

---

## 5. Risk-scaled depth

### Goal

Use `RiskLevel` to change review effort, not only prompt text.

### Current state

`repository/context.rs::risk_for` sets bundle risk from path/patch heuristics.

Primary and verifier hardcode `max_steps: 6`.

Risk appears in the prompt bundle summary only.

### Implementation

Add a pure planner:

```rust
// src/agents/depth.rs
struct DepthPlan {
    primary_passes: usize,
    primary_max_steps: usize,
    verifier_max_steps: usize,
}

fn max_bundle_risk(bundles: &[ReviewBundle]) -> RiskLevel;
fn depth_for(risk: RiskLevel, config: &ReviewConfig) -> DepthPlan;
```

Suggested conservative defaults:

| Max bundle risk | Passes | Primary steps | Verifier steps |
| --- | ---: | ---: | ---: |
| Low | 1 | 4 | 4 |
| Medium | 1 | 6 | 6 |
| High | 2 | 8 | 6 |
| Critical | 3 | 10 | 8 |

Clamp by config ceilings.

Before optional pass 2/3, check remaining budget/time and skip if thin.

Do not auto-raise global `max_cost_usd`.

Spend more of the existing budget on High/Critical; save on Low.

### Files

- New: `src/agents/depth.rs`
- `src/agents/mod.rs`, `src/agents/verifier.rs` - plumb `max_steps`
- `src/config.rs` - ceilings
- Later: combine with multipass so High/Critical raise pass count

### Commit shape

1. `DepthPlan` + unit tests (no behavior change)
2. Wire variable `max_steps` with passes still 1
3. Connect to multipass once that lands

---

## 6. Multi-pass primary + cluster/majority merge

### Goal

Raise recall without abandoning the precision-first verifier.

### Current state

One primary agent call, then verifier.

`max_concurrency` exists but primary/verifier are sequential, so parallel capacity is unused.

Publish fingerprint includes path, category, priority, normalized anchor, and title.

That is too strict for cross-pass clustering.

### Algorithm

Diversified passes (deterministic):

1. Pass 0: current order, temperature `0.0`, full review
2. Pass 1: reversed file order, correctness/reliability lens, temperature `0.1`
3. Pass 2: high-risk bundles first, security/concurrency/API lens, temperature `0.2`

Cluster key (looser than publish fingerprint):

```text
sha256(path + side + category + normalized(anchor) + normalized(end_anchor))
```

Exclude priority and title so wording/priority drift still merges.

Optional soft match: same path/side and high token Jaccard on anchors.

Merge rules:

- Keep if support >= `majority_k` (default 2 when passes >= 2)
- Or singleton with confidence >= ~0.92 and priority in `{P0,P1}`
- Representative = highest confidence, then highest priority, then richest body/evidence
- Cap merged candidates before verifier (for example `max_comments * 3`)

Then call existing `verifier::verify_findings` unchanged.

Failure policy: a failed pass contributes empty findings; only mark primary failed if all passes fail.

### Config

```rust
primary_passes: usize,                 // default 1
majority_k: usize,                     // default 2
keep_high_confidence_singleton: f32,   // default 0.92
```

CLI/Action: `--primary-passes` / `PRBOT_PRIMARY_PASSES`.

Keep default `1` until evals justify enabling 2+.

Risk planner can choose `1..=ceiling`.

### Files

- New: `src/agents/cluster.rs`
- New optional: `src/agents/multipass.rs`
- `src/agents/mod.rs` - orchestration
- `src/agents/prompts/primary.rs` - pass variants
- `src/llm.rs` - `AgentCall.temperature`
- `src/config.rs`, `action.yml`, `entrypoint.sh`
- `src/agents/integration_tests.rs`

### Budget / concurrency

Share one `LlmClient`, one `Budget`, one semaphore.

Run passes with `join_all`; semaphore bounds parallel HTTP/tool rounds.

Skip later passes if remaining tokens/time cannot support another pass plus verifier.

### Cost impact

Up to Nx primary cost when enabled.

Wall clock ~1x primary + verifier if parallelized.

Mitigation: default off (passes=1), risk-gated enablement, budget early-exit.

### Commit shape

1. `cluster.rs` + unit tests
2. Temperature plumbing
3. Prompt pass variants
4. Multipass behind default `1`
5. Integration tests + Action knobs
6. Risk-driven pass count

---

## 7. Eval fixtures + real quality gate

### Goal

Make quality changes measurable offline before flipping defaults.

### Current state

`scripts/evaluate.py` already encodes the release gate.

CI only runs smoke (`evals/sample.jsonl --allow-small-sample`) and checks catalog line count.

`evals/fixtures/catalog.jsonl` is 50 `pending_adjudication` stubs.

`--eval-json` prints `EvalPayload` but nothing converts it into evaluate.py rows.

### Target pipeline

```text
adjudicated fixture defs
  -> prbot review --eval-json
  -> draft published findings
  -> human labels expected_id / actionable / anchor_valid
  -> evaluate.py JSONL
  -> CI gate without --allow-small-sample once >= 50
```

### Data model

Keep `evaluate.py` result schema stable.

Extend fixture defs (not results) with:

- `repository`, `pr_number`, `head_sha`
- `expected_findings[{id, priority, path, notes}]`
- `status`: `pending_adjudication | ready | adjudicated | retired`

Store committed golden results in `evals/fixtures/results.jsonl` for CI.

Do not run live LLM calls in the default PR CI gate.

### Files

- `evals/fixtures/README.md` - status machine + schema
- `scripts/generate_fixture_catalog.py` - stop clobbering adjudicated fields
- New: `scripts/run_fixture_batch.py`
- New: mapper from `EvalPayload` -> evaluate.py draft rows
- `.github/workflows/ci.yml` - score committed results
- Pilot first: 5-10 cases with `--allow-small-sample`, then full 50

### Important constraints

- Qodo LLM judge is not a substitute for human adjudication
- Router evals are orthogonal and target non-live specialists
- `eval_mode` resets `SummaryState`, so fixtures do not exercise incremental/resolution behavior unless specially designed

### Commit shape

1. Docs + protect adjudicated catalog fields
2. Fixture def fields for a pilot set
3. Batch runner + mapper
4. Commit pilot results + soft CI gate
5. Fill to 50 + hard gate

---

## 8. Resolution rate from fingerprint lifecycle

### Goal

Track whether published findings get fixed across later commits, Bugbot-style.

### Current state

`SummaryState` remembers fingerprints for dedupe and forgets them on path invalidation.

`forget_paths` does not distinguish fixed vs outdated vs dismissed.

`RunOutcome.active_findings` is open-set size only.

### Definition to lock early

Resolution rate = resolved ever-published fingerprints / ever-published fingerprints.

A fingerprint becomes resolved only when:

1. It was removed by path invalidation for a scope that is actually re-reviewed
2. The subsequent review of that scope does not republish the same fingerprint

Do not count at `forget_paths` time alone.

That would treat every path touch as a fix.

### Data model (`SummaryState` version 4+)

Additive fields with `serde(default)`:

- `published_fingerprints: BTreeSet<String>`
- `resolved_fingerprints: BTreeSet<String>`
- `fingerprint_status: BTreeMap<String, FindingLifecycle>`
- optional capped events or just counters

```rust
enum FindingLifecycle { Open, Resolved, Outdated, Dismissed }
```

Extend `RunOutcome` with `ever_published_findings`, `resolved_findings`, `open_findings`, `resolution_rate`.

Show rate in `render_summary`.

### Algorithm in `run_review`

```text
load state
incremental:
  forgotten = forget_paths_returning(invalidate_paths)
review selected bundles
publish/remember new findings into published_fingerprints
for fp in forgotten:
  if republished this run -> Open
  else if coverage complete for that scope -> Resolved
  else leave pending (do not resolve on failed/partial runs)
compute rate
persist state
```

Optional later: finalize on `pull_request` closed/merged.

### Files

- `src/reporting/summary.rs`
- `src/review/contextual.rs`
- `src/review/incremental.rs` (return forgotten fps, or change `forget_paths`)
- `src/types.rs` (`RunOutcome`)
- summary/contextual tests
- optional offline aggregator script

### Edge cases

- Anchor/title drift creates a new fingerprint (old looks resolved) - acceptable proxy
- Overflow unpublished findings must stay out of the denominator
- Incomplete coverage must not mark forgotten fps resolved
- Cap event logs so the HTML state comment does not bloat

### Commit shape

1. Additive state fields + parse compatibility tests
2. `forget_paths` returns removed fingerprints
3. Post-publish lifecycle transitions + summary/outcome fields
4. Unit tests for resolve / republish / new publish
5. Optional merge finalize

---

## Cross-feature dependency map

```text
docs alignment -------------------- independent
OpenRouter retries --------------- independent; helps multipass under rate limits
min_permission ------------------- independent product policy
walkthrough ---------------------- needs only summary/publish path
DepthPlan(max_steps) ------------- feeds multipass
cluster + multipass -------------- needs retries + depth; default passes=1
risk -> passes ------------------- needs DepthPlan + multipass
eval gate ------------------------ measures multipass/prompt changes
resolution rate ------------------ online KPI; complements offline evals
```

## What "done" looks like for replacement confidence

You can claim replacement readiness for a repo when:

1. Offline gate passes on adjudicated fixtures without smoke exceptions
2. Multipass is enabled where evals show better P0/P1 recall without precision collapse
3. Summary walkthrough makes humans oriented without extra noise
4. Auth policy matches how the team actually reviews PRs
5. Resolution rate is visible and trending up on real PRs

Until then, keep defaults conservative: `primary_passes=1`, `min_permission=admin`, walkthrough soft-fail, retries on.
