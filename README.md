# PRBot

Multi-agent PR reviewer for GitHub.
Runs as a GitHub Action.
Uses OpenRouter (or compatible LLM APIs) for reviews.

Status: early implementation (`v0.1.0`).
The first agent is a code-quality reviewer.
It sends reviewable changed source patches to OpenRouter and posts only validated inline findings that target added lines.
Pull request conversation replies are also supported.

## Code-quality reviewer

On pull request events, PRBot fetches changed files from GitHub and selects up to 25 reviewable source files with a combined patch budget of 80,000 characters.
It excludes deleted files, dependency locks, generated code, vendored code, and minified JavaScript.
The model receives the selected unified diffs and must return structured findings with a file, an added-line number, severity, and explanation.
PRBot rejects findings that do not point at an added line in the submitted diff before creating an inline GitHub review comment.

This makes the first reviewer useful and safe, but it is intentionally narrower than mature products such as CodeRabbit or Codex review.
Those products improve precision through repository-wide context, language-aware static analysis, test and CI signals, historical feedback, rule packs, and stronger multi-pass or tool-using review models.

## Install in another repository

1. Add a workflow file (see [`examples/prbot.yml`](examples/prbot.yml)):

```yaml
name: PRBot
on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review]
jobs:
  review:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: YOUR_GITHUB_USER/prbot@v0.1.0
        with:
          openrouter_api_key: ${{ secrets.OPENROUTER_API_KEY }}
          github_token: ${{ secrets.GITHUB_TOKEN }}
```

2. Add `OPENROUTER_API_KEY` under **Settings → Secrets and variables → Actions**.
3. Open a pull request.

`GITHUB_TOKEN` is created automatically by GitHub Actions.
You pass it so PRBot can read the PR and post comments.
Users do not invent that secret themselves.

## Versioning

| Thing | Source of truth |
| --- | --- |
| Crate / CLI version | `Cargo.toml` → `version` |
| Action release users pin | Git tags like `v0.1.0` |
| Floating major pin (optional later) | moving tag `v0` |

Release checklist:

1. Bump `version` in `Cargo.toml`.
2. Commit the change.
3. Tag: `git tag v0.1.0 && git push origin v0.1.0`
4. Users install with `uses: YOUR_GITHUB_USER/prbot@v0.1.0`

## Local CLI

```bash
cargo build --release
./target/release/prbot version
./target/release/prbot review --help
```

Dry-run example (no LLM call):

```bash
GITHUB_REPOSITORY=owner/repo \
GITHUB_TOKEN=ghp_xxx \
PRBOT_PR_NUMBER=1 \
./target/release/prbot review --dry-run
```

## Repository layout

```text
action.yml          # GitHub Action metadata
Dockerfile          # Builds and runs the Rust binary in Actions
entrypoint.sh       # Maps Action inputs → CLI
src/                # Rust CLI + future agents
examples/prbot.yml  # Copy-paste workflow for consumers
.github/workflows/  # CI for this repo
```

## Development

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```
