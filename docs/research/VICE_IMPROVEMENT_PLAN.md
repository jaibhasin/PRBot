# VICE / PRBot Improvement Plan

Research synthesis and execution roadmap for making PRBot (VICE) faster, more robust, and more professional.

Date: 2026-07-27
Scope: industry research + full-repo analysis of the current contextual review engine
Status: planning document (no product behavior changes in this PR)

## 1. Executive verdict

PRBot already has the right bones for a precision-first reviewer: ephemeral Git snapshots, relationship-aware bundles, specialist routing, independent verification, deterministic anchors, and incremental fingerprinting.

The full review is slow primarily because every run multiplies expensive work:

1. Cold Docker image build on the Action runner.
2. Fresh bare clone and deep Git history fetch.
3. Sequential per-file diffs and O(files x tree) context discovery.
4. Many parallel tool-using specialist agents that each re-embed the same large prompts.
5. A single sequential verifier after all reviewers finish.
6. Incremental mode skipping only LLM work, not clone/index/diff work.

Industry leaders solve latency and trust with the same hybrid pattern PRBot is already approaching: deterministic context engineering up front, bounded agentic investigation in the middle, strict verification and severity filtering at the end, and a measurable feedback loop.

This plan turns that pattern into concrete VICE milestones.

## 2. Industry research summary

### 2.1 Architectural consensus

Across CodeRabbit, Cursor Bugbot, Greptile, Qodo/PR-Agent, OpenAI Codex, Cloudflare's internal reviewer, Alibaba OpenCodeReview, OpenCode, and recent academic systems (BitsAI-CR, SWR-Bench, Lu et al. ICML 2025), successful systems converge on:

| Pattern | Why it matters | Who does it |
| --- | --- | --- |
| Hybrid pipeline + agent | Structure for speed/trust; tools for depth | CodeRabbit, Alibaba OCR, PRBot today |
| Curated context, not dump-everything | Excess context raises false positives and latency | CodeRabbit, Qodo, Greptile |
| Multi-agent specialization | Security/perf/correctness lenses beat one generalist | Cloudflare, Greptile, Bugbot v1, PRBot |
| Independent judge / filter | Precision is the adoption bottleneck | BitsAI-CR, CodeRabbit, Bugbot, PRBot verifier |
| Incremental / delta reviews | Re-review cost dominates at scale | Cloudflare, calimero, BitsAI-CR, PRBot partial |
| Prompt caching / shared context | Shared prefixes cut cost and latency | Cloudflare (85.7% cache hit), Anthropic tooling |
| Resolution / accept metrics | Hill-climb on what developers actually fix | Cursor Bugbot |
| Repo guidance files | Encode team invariants without hardcoding | Codex `AGENTS.md`, Bugbot rules, PRBot `.prbot.toml` |
| Severity gating | Ship P0/P1 only; hide nits | Codex, Greptile filters, BitsAI-CR |
| Sandbox isolation | Never trust PR code execution | CodeRabbit microVMs, PRBot no-exec policy |

### 2.2 Product-by-product lessons

#### CodeRabbit

- Hybrid, not pure agentic: static analyzers + AST/symbol context first, then targeted agent investigation.
- Ephemeral sandboxed clone with long timeouts (reviews often 10-20 minutes in their Cloud Run design).
- Cheap models compress tickets/logs/files; frontier models do hard reasoning.
- Separate judge model drops ungrounded findings before posting.
- Learnings loop from developer feedback into future reviews.
- Lesson for VICE: invest in context curation and verification before adding more agent loops.

#### Cursor Bugbot

- Started with 8 parallel passes + majority vote + validator.
- Moved to fully agentic tool use; largest quality gains came from dynamic context discovery.
- Key metric is resolution rate (did the author actually fix the finding before merge), not comment volume.
- Reads existing PR comments to avoid duplicates and build on prior feedback.
- Lesson for VICE: measure resolution rate; prefer fewer high-signal findings; use prior thread state.

#### Greptile

- Pre-indexes the full codebase into a persistent graph (files, functions, dependencies).
- Swarm of parallel agents review impact beyond the diff.
- Learns from human review comments over time.
- Configurable severity, collapse sections, path/label triggers.
- Lesson for VICE: persistent indexing beats per-run tree scans; make output noise configurable.

#### Qodo / PR-Agent

- Token-exact PR compression strategy; single-call tools for speed when possible.
- Asymmetric dynamic context: more lines before a change than after; expand to enclosing function/class.
- Model tiers: weak for describe/ask, strong for review, reasoning for self-reflection.
- Lesson for VICE: add token-aware bundle sizing and asymmetric hunk expansion; tier models by task.

#### OpenAI Codex Code Review

- Focuses on P0/P1 only.
- Uses hierarchical `AGENTS.md` as review rules and cites them.
- Manual `@codex review` plus optional auto-review.
- Structured JSON schema when built with the SDK for reliable inline comments.
- Lesson for VICE: severity-first posting; strengthen rule citation; enforce structured outputs.

#### Cloudflare internal AI review

- Median review ~3m39s at massive scale.
- Parallel specialist reviewers with per-task and overall timeouts.
- Shared context file + prompt caching drove 85.7% cache hits.
- Coordinator aggregates, dedups, ranks.
- Lesson for VICE: shared prompt prefixes, hard stage budgets, and cache-friendly prompt layout are the biggest latency levers after clone/index.

#### Alibaba OpenCodeReview

- Deterministic engineering x agent hybrid at production scale.
- Fine-grained rule matching per file keeps model attention sharp.
- Toolset distilled from production tool-call traces (frequency, repetition, impact).
- Lesson for VICE: tune the tool surface from telemetry, not intuition.

#### OpenCode

- Runs inside the consumer's GitHub Actions runner.
- Comment triggers (`/oc`) plus auto PR review.
- Secure by keeping execution in the customer's CI, not a third-party sandbox of untrusted plugins.
- Lesson for VICE: Actions-native trust model is already a strength; keep no-exec defaults unless sandboxed execution is explicit and optional.

#### Coursera / education ecosystem

- Coursera course "AI Code Review Automation with GitHub Actions" (Pragmatic AI Labs / pmat) emphasizes:
  - Define review criteria before prompting.
  - Combine static complexity analysis with LLM semantic review.
  - Iterate prompts against real PRs.
  - Handle hallucination and inconsistency explicitly.
- Coursera's own AI peer-review grading uses rubrics for consistency and speed.
- Lesson for VICE: rubric/criteria-first review beats open-ended "find issues" prompts.

#### Academic signals (2024-2026)

- Lu et al. (ICML 2025): code slicing + multi-role LLMs + FAR filters beat plain LLM review by large margins on industrial C++ MRs.
- BitsAI-CR: RuleChecker + ReviewFilter raised precision from ~60% to ~75%.
- SWR-Bench: multi-review aggregation can boost F1 substantially; current ACR tools underperform humans on real PRs.
- Tencent false-positive study: hybrid static + LLM filtering removes 94-98% of false alarms cost-effectively.
- Lesson for VICE: verification/filtering is not optional; it is the product.

### 2.3 What "good" looks like in 2026

A professional AI PR reviewer should hit these targets:

1. First useful signal in under ~2-4 minutes for typical PRs.
2. Full review complete before the author context-switches away (~3-6 minutes median).
3. High precision on published comments; prefer silence over nits.
4. Incremental pushes cost far less than the first review.
5. Output is sectioned, severitized, and actionable with exact anchors.
6. Failures are visible (partial/failed coverage), never falsely clean.
7. Quality is measured by resolution rate / precision / P0-P1 recall, not comment count.

## 3. Current VICE architecture (as implemented)

### 3.1 Pipeline today

```text
Action start (Docker build/run)
  -> entrypoint.sh maps inputs
  -> authorize actor (admin-only spend)
  -> fetch PR + comments
  -> clone bare repo (base + head)
  -> load trusted .prbot.toml + AGENTS.md from base
  -> per-file git diffs -> hunks
  -> sequential context graph + bundles
  -> restore summary state / incremental selection
  -> router LLM
  -> parallel specialist reviewers (tools, up to 12 steps each)
  -> sequential verifier (tools, up to 8 steps)
  -> anchor resolve + fingerprint dedupe
  -> formal review + summary comment + check run
```

### 3.2 What is already strong

- Exact head SHA verification and merge-base diffs.
- Ephemeral bare Git store; no execution of PR code.
- Bounded read-only repository tools.
- Base-only trusted config and hierarchical `AGENTS.md`.
- Router + correctness + specialists + independent verifier.
- Deterministic anchor resolution (right/left/context/multiline/file fallback).
- Persistent summary with fingerprints and incremental bundle selection.
- Budget ceilings for minutes, tokens, cost, concurrency, and comments.
- Eval harness scaffolding (fixtures, Qodo bench, routing scorer).

### 3.3 Why it feels slow

Measured and structural bottlenecks:

1. Action cold start builds from `Dockerfile` instead of a prebuilt image tag.
2. Every run reclones; incremental mode does not reuse objects or indexes.
3. Diff construction launches one `git diff` subprocess per eligible file.
4. `build_context` walks the tree repeatedly (up to 100k paths) with no inverted index or cache.
5. Same-head no-op detection happens after clone/diff/context.
6. Each reviewer prompt duplicates full patches + 16k repo map.
7. Correctness/security/performance are per-bundle tool loops; architecture/docs are grouped.
8. Router fallback treats "no specialists needed" as error and can expand work.
9. Verification waits for all reviewers, then runs one monolithic judge.
10. OpenRouter calls lack 429/5xx retry; reservations are not refunded on cancel.
11. Git/tool/stage telemetry is thin, so latency cannot be hill-climbed surgically.
12. Historical Qodo evidence: one run exhausted ~$3 / ~418k input tokens in ~274s without completing cleanly.

### 3.4 Robustness gaps

1. Persisted review state is restored from any issue comment containing the marker (not author-authenticated).
2. Incremental cache key ignores base SHA, models, config hash, and PRBot version.
3. Concurrent Action runs have no lease/lock.
4. Formal review publish is not idempotent if summary/check fails afterward.
5. Markdown/JSON are outside eligible extensions, so docs-only PRs can be empty.
6. Reviewer and verifier default to the same model, weakening independence.
7. No structured-output enforcement / JSON repair loop.
8. Evidence spans are not deterministically validated before publish.
9. Existing GitHub threads are not reconciled (resolved/outdated).
10. Eval catalog is still pending adjudication; CI does not gate live contextual quality.

## 4. Target architecture for VICE

Keep the hybrid model. Do not abandon verification or safety for raw agent autonomy.

```text
Fast path
  authorize -> load prior state by (base, head, config, version)
  -> if unchanged and complete: refresh check / exit

Prepare (parallel where possible)
  reuse or shallow-fetch Git objects
  one authoritative diff partition
  load/update persistent symbol+import index for changed neighborhood
  static signals (optional lints later)

Triage
  cheap router/classifier: risk, languages, specialist need, size class
  allow zero specialists
  choose review depth: skim / standard / deep

Review
  shared cached context prefix
  token-bounded bundles
  parallel specialists with per-task timeouts
  tool use only for unresolved questions

Judge
  batch verify candidates (not one giant prompt forever)
  drop P3 by default; require confidence + grounded evidence
  majority / agreement boost when overlapping

Publish
  idempotent review upsert
  summary + check
  stage latency + resolution telemetry
```

North-star product promise:

- Fast enough to feel like a teammate already looking.
- Quiet enough that engineers trust every comment.
- Honest enough to say when coverage was partial.

## 5. Prioritized roadmap

### Phase 0 - Instrumentation and truth (foundation)

Goal: know where time and money go before changing behavior.

Deliverables:

1. Stage timers in summary output: auth, clone, diff, context, router, reviewers, verifier, publish.
2. Per-agent counters: HTTP calls, tool calls, input/output tokens, wall time, queue wait.
3. Cache-key fields logged even before caching exists.
4. Fail the summary cleanly when stages truncate.

Acceptance:

- Local dry-run and one live self-test PR show a complete stage breakdown.
- Qodo batch rows record stage timings.

### Phase 1 - Latency wins with low quality risk

Goal: cut wall-clock without changing review semantics much.

1. Ship a prebuilt GHCR/Docker Hub image; stop rebuilding the binary on every consumer run.
2. Move same-head complete-review short-circuit before clone when state + check already prove completeness.
3. Parallelize independent GitHub fetches (check runs, linked issue, comments where safe).
4. Compute one multi-file `git diff` and partition in memory instead of N subprocesses.
5. Add read/parse caches inside a run for `list_tree`, file bytes, and syntax signals.
6. Cap and share the repo map; put stable system/instructions/schema first for provider prompt caching.
7. Allow router to assign zero specialists without fallback-to-all.
8. Add OpenRouter retries with jittered backoff for 429/5xx.
9. Bound clone deepen policy more aggressively; fail soft with partial history rather than unshallowing huge repos by default.
10. Enforce end-to-end deadline across publish, not only LLM `.send()`.

Expected impact:

- Large cold-start reduction from image reuse.
- Material CPU reduction on medium/large repos from context caching and single diff.
- Fewer wasted specialist calls on simple PRs.

### Phase 2 - Incremental review that actually saves work

Goal: second push reviews only what changed, including prepare cost.

1. Authenticate summary state (bot author + signed/versioned payload).
2. Key state by `base_sha + head_sha + engine + models + config_hash + prbot_version`.
3. Persist a lightweight neighborhood index artifact in Actions cache keyed by repo + base SHA.
4. On synchronize, recompute only affected bundles and related edges.
5. Reconcile prior findings with current diff and GitHub thread resolution.
6. Add a run lease comment/check to prevent duplicate concurrent publishes.
7. Make formal review creation idempotent via fingerprint of `(head, findings set)`.

Expected impact:

- Incremental runs approach "router + few specialists + verify" instead of full prepare+review.

### Phase 3 - Quality and professionalism

Goal: fewer comments, higher resolution rate, clearer UX.

1. Default publish policy: P0/P1 always; P2 only under budget; P3 suppressed unless configured.
2. Cite matching `.prbot.toml` / `AGENTS.md` rules in finding bodies.
3. Split verification into batches; optionally use a stronger/different verifier model by default.
4. Deterministically validate evidence paths/line ranges before accept.
5. Reject ambiguous anchors instead of silent file-level downgrade when severity is P0/P1 (or mark confidence drop).
6. Expand eligible types for docs steward: Markdown and selected JSON/YAML config.
7. Token-aware bundle packing and asymmetric hunk expansion (more before than after; enclose function/class).
8. Read prior human/bot comments to suppress duplicates (Bugbot/Codex pattern).
9. Richer summary UX: risk score, coverage map, skipped paths, stage timings, model tiers.
10. Optional "review effort" input: `low | medium | high` mapping to specialist count, tool steps, and verifier strictness (Copilot pattern).

### Phase 4 - Context intelligence

Goal: stop rebuilding naive graphs every run.

1. Replace repeated tree scans with a persistent symbol/import index (tree-sitter based, language pack already present).
2. Build true edges for imports, definitions, references, tests, and manifests.
3. Precompute repo map summaries with cheap models only when the index is cold or base moved far.
4. Optional static signals: `clippy`/`eslint`/`ruff` as data for the model, still without executing untrusted project scripts by default.
5. Path-rule aware specialist forcing (auth paths always get security, etc.).

### Phase 5 - Learning loop and release quality

Goal: hill-climb like Bugbot/CodeRabbit.

1. Adjudicate the 50-case fixture catalog; enforce precision/recall gates in CI.
2. Keep Qodo scoreboard green as a release signal.
3. Add resolution-rate estimator: at merge time, classify whether active findings were fixed.
4. Track dismiss/ignore patterns from issue comments and thread resolutions.
5. Repin default review/verification models only after evals pass.
6. Split oversized modules (`git.rs`, `client.rs`, `tools.rs`, orchestration) under the 300/500 line guidance as features land.

## 6. Concrete design decisions

### 6.1 Keep no-exec as default

CodeRabbit's power comes partly from running analyzers in microVMs.
VICE's Action-native trust model is different and already a selling point.

Decision:

- Default remains: no project code execution, no model-selected shell, no network tools.
- Optional later: `enable_static_analyzers=true` running only allowlisted tools on a fetched snapshot with explicit sandboxing.

### 6.2 Prefer hybrid depth over unbounded agents

Bugbot gained quality by going agentic.
Cloudflare and CodeRabbit show agentic loops need hard budgets.

Decision for VICE:

- Keep tool-using specialists.
- Reduce default tool steps for `low` effort.
- Add per-task timeouts and early-stop when no suspicious signals remain.
- Never let tool loops exceed the global minute/token budget.

### 6.3 Severity-first product surface

Codex posts P0/P1 only.
Noise kills adoption faster than missed nits.

Decision:

- Make `max_comments` a publish cap, not a quality target.
- Add `min_priority` (default P2) and `include_p3=false`.
- Summary can still mention suppressed nit counts.

### 6.4 Model tiering

| Role | Tier | Default intent |
| --- | --- | --- |
| Reaction / classify / compress | weak/fast | cheap, cached |
| Router | weak/fast or review | structured JSON |
| Correctness / security | review | current review model |
| Architecture (large/cross-file) | review or reasoning | deeper only when routed |
| Verifier | verification (ideally different family) | adversarial filter |

### 6.5 Cache hierarchy

1. Action image cache (binary).
2. Git object cache (repo pack by remote URL).
3. Symbol index cache (base SHA).
4. Prompt-prefix cache (provider).
5. Review state cache (PR summary, authenticated).

## 7. Suggested implementation slices (small PRs)

These are ordered for frequent commits and measurable gains:

1. `telemetry`: stage timings + agent counters in summary.
2. `action-image`: publish/consume prebuilt image.
3. `fast-noop`: authenticated state + pre-clone same-head exit.
4. `diff-once`: single diff partition + in-run file caches.
5. `router-zero`: allow empty specialist assignment; remove forced fallback.
6. `llm-retry`: OpenRouter retry/backoff + reservation refunds.
7. `prompt-cache`: reorder prompts for stable prefixes; shrink duplicated maps.
8. `verify-batch`: batched verifier + evidence validation.
9. `severity-gate`: min priority + rule citations.
10. `incremental-index`: Actions cache for neighborhood index.
11. `docs-json`: eligible Markdown/JSON for documentation steward.
12. `evals-gate`: adjudicated fixtures + CI quality gate.
13. `effort-levels`: `low|medium|high` review depth input.
14. `module-splits`: file-size cleanup as each area is touched.

## 8. Success metrics

Track per release:

| Metric | Current signal | Target direction |
| --- | --- | --- |
| Median end-to-end review latency | often dominated by prepare + many LLM loops | down sharply on typical PRs |
| p95 latency | unknown / uninstrumented | bounded under deadline with honest partials |
| Estimated USD / PR | can approach $3 ceiling | down via cache + triage + incremental |
| Input tokens / PR | can exceed hundreds of thousands | down via shared context + compression |
| Published comments / PR | capped at 12 | fewer, higher severity |
| Precision on adjudicated fixtures | pending | >= 90% |
| P0/P1 recall | pending | >= 75% |
| Anchor validity | partially enforced | ~100% for published inline comments |
| Resolution rate | not measured | rising over time |
| Incremental speedup vs full | LLM-only today | prepare+LLM both reduced |

## 9. Risks and non-goals

Risks:

- Over-caching can hide base-branch updates if keys omit base SHA.
- Aggressive severity filtering can miss real P2s unless effort mode exists.
- Persistent indexes add operational complexity in Actions.
- Same-model verifier can rubber-stamp reviewer findings.

Non-goals for the near term:

- Becoming a full coding agent that pushes fixes by default.
- Running untrusted PR build/test scripts in-process.
- Matching CodeRabbit's 20-minute deep sandbox reviews at the cost of CI snappiness.
- Replacing human review on high-risk changes.

## 10. Appendix A - Repository map for implementers

Important paths:

- `src/review/` - authorization, orchestration, incremental selection
- `src/repository/` - Git, diff, context, tools, syntax, safety
- `src/agents/` - router, tasks, specialists, verifier, prompts
- `src/reporting/` - anchors, fingerprints, summary state
- `src/llm.rs` + `src/llm/budget.rs` - OpenRouter loop and ceilings
- `src/github/` - API client and publish helpers
- `action.yml` / `Dockerfile` / `entrypoint.sh` - Action wiring
- `evals/` - fixtures, Qodo bench, scoreboard

Highest-churn files for Phase 1-2:

- `src/repository/context.rs`
- `src/repository/diff.rs`
- `src/repository/git.rs`
- `src/review/contextual.rs`
- `src/review/incremental.rs`
- `src/review/review_context.rs`
- `src/agents/mod.rs`
- `src/agents/router.rs`
- `src/agents/verifier.rs`
- `src/llm.rs`
- `action.yml`

## 11. Appendix B - Primary sources consulted

Industry / product:

- CodeRabbit architecture and hybrid AI blogs; Google Cloud Run writeup
- Cursor Bugbot engineering blog and docs
- Greptile graph-context and system architecture docs
- Qodo/PR-Agent dynamic context and compression docs
- OpenAI Codex GitHub review docs, custom `AGENTS.md` rules, Codex SDK cookbook
- Cloudflare "Orchestrating AI Code Review at scale"
- Alibaba OpenCodeReview README and hybrid design notes
- OpenCode GitHub Actions docs
- GitHub Copilot code review docs (effort levels, auto-review)
- Coursera "AI Code Review Automation with GitHub Actions" course outline
- calimero-network/ai-code-reviewer multi-agent/incremental patterns

Academic / empirical:

- Lu et al., Towards Practical Defect-Focused Automated Code Review (ICML 2025)
- BitsAI-CR: Automated Code Review via LLM in Practice (arXiv 2501.15134)
- SWR-Bench (arXiv 2509.01494)
- Reducing False Positives in Static Bug Detection with LLMs (Tencent industrial study)

## 12. Recommended immediate next PR

Implement Phase 0 + the first three Phase 1 items in one focused engineering track:

1. Stage telemetry in the summary comment.
2. Prebuilt Action image.
3. Pre-clone same-head no-op (with authenticated state).
4. Single-diff partition + in-run caches.

That sequence attacks the user's main complaint (time) while making later quality work measurable.
