# Agent instructions

These are common instructions for agents working in this repo.

## General guidelines

- Never use the em dash "—". Use plain dash "-" instead.
- When writing commit messages, NEVER auto-add your agent name as co-author.
- When writing or substantially editing long Markdown files, put each full sentence on its own line.
  Preserve normal Markdown structure, but avoid wrapping multiple sentences onto one physical line.
- When making technical decisions, prefer quality, simplicity, robustness, scalability, and long-term maintainability over short-term development speed.
- When doing bug fixes, reproduce the bug in an end-to-end way first (CLI and/or GitHub Action path), as close as possible to how a user would hit it.
- Apply a high standard to engineering excellence: lint, test failures, and flaky tests.
  If you see one, even if it is not caused by your current change, fix it along the way.
- Prefer frequent, small commits while writing code.
  Commit after each meaningful unit of work (one fix, one feature slice, one refactor step).
  More commits are better than one large commit at the end.

## Repo architecture

PRBot is a Rust CLI wrapped as a GitHub Action.

Flow:

1. A consumer workflow calls this Action.
2. Docker starts `entrypoint.sh`.
3. `entrypoint.sh` runs `prbot review`.
4. The CLI talks to GitHub and OpenRouter, runs a primary review plus independent verification, then posts PR feedback.

Important paths:

- `src/main.rs` - CLI entrypoint and subcommands
- `src/config.rs` - review config, defaults, and `.prbot.toml` loading
- `src/types.rs` - shared review, finding, and outcome types
- `src/review/` - review orchestration (args, events, commands, contextual and legacy engines)
- `src/agents/` - primary reviewer, verifier, and prompts
- `src/llm.rs` / `src/llm/` - OpenRouter client and token/cost budgets
- `src/repository/` - ephemeral Git store, diffs, syntax context, and read-only tools
- `src/reporting/` - anchors, dedupe, and summary comment rendering
- `src/github/` - GitHub API helpers (diff, comments, reviews, checks)
- `action.yml` - Action inputs and metadata
- `Dockerfile` - builds/runs the binary in Actions
- `entrypoint.sh` - maps Action inputs to CLI flags/env
- `examples/prbot.yml` - copy-paste workflow for other repos
- `evals/` - release-gate scorers, fixture catalog, and Qodo harness
- `.github/workflows/` - CI and self-test for this repo

Coding guidance:

- Put product logic in `src/`.
- Touch `action.yml` / `Dockerfile` / `entrypoint.sh` only when Action inputs or runtime wiring change.
- Keep `main.rs` thin: parse CLI, dispatch, exit.
- Prefer new modules over growing large files forever.
  Good split targets: GitHub client, LLM client, prompts/agents, comment formatting, CLI args, eval scripts.

## File size limits

- Prefer keeping Rust source files under about 300 lines.
- If a file approaches 500 lines, split it before adding more features.
- Good split boundaries: GitHub client, LLM client, prompts/agents, comment formatting, CLI args.
- Exceptions are allowed for generated code or dense static tables.
  If you use an exception, note why in the commit message or a short code comment.
