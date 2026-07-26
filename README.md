# PRBot

Multi-agent PR reviewer for GitHub.
Runs as a GitHub Action.
Uses OpenRouter (or compatible LLM APIs) for reviews.

Status: early experimental (`v0.1.1`).
The first agent is a code-quality reviewer.
It reviews changed patches, posts validated inline findings on added lines, and always leaves a PR timeline summary comment.
There is no `@prbot` mention yet.
Comment replies work when the consumer workflow listens for `issue_comment`.

## How it behaves

- **Auto review:** runs on PR open / sync / reopen / ready for review.
- **Summary comment:** always posts a timeline comment (findings, "looks fine", or "no reviewable files").
- **Inline comments:** only for high-confidence findings anchored to added (`+`) lines.
- **Human comments:** if `issue_comment` is enabled, a human comment can trigger a reply.
- **No `@` bot:** this is a GitHub Action, not a GitHub App, so you do not invoke it with `@prbot`.

## Code-quality reviewer

On pull request events, PRBot fetches changed files from GitHub and selects up to 25 reviewable source files with a combined patch budget of 80,000 characters.
It supports common source/config types, including `.ts`, `.js`, `.rs`, `.py`, `.css`, and `.scss`.
It excludes deleted files, dependency locks, generated code, vendored code, and minified JavaScript.
The model must return structured findings with a file, an added-line number, severity, and explanation.
PRBot rejects findings that do not point at an added line before creating an inline review comment.

This first reviewer is intentionally narrower than products such as CodeRabbit.
Those tools usually add repo-wide context, stronger filtering, and richer workflows.

## Install in another repository

1. Copy [`examples/prbot.yml`](examples/prbot.yml) to `.github/workflows/prbot.yml` (or paste the workflow below).
2. Add repository **secret** `OPENROUTER_API_KEY` under **Settings → Secrets and variables → Actions → Secrets**.
   Do not put the key in Variables.
   Variables are not the same as `secrets.*`.
3. Open a pull request.
4. Pin a release tag such as `v0.1.1`.
   After upgrading PRBot, bump the tag in your workflow and push that change on the PR branch.
   Re-running an old Actions job still uses the old workflow file from that run.

```yaml
name: PRBot

on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review]
  issue_comment:
    types: [created]

permissions:
  contents: read
  pull-requests: write
  issues: write

jobs:
  review:
    if: ${{ github.event.sender.type != 'Bot' }}
    runs-on: ubuntu-latest
    steps:
      - name: Run PRBot
        uses: jaibhasin/PRBot@v0.1.1
        with:
          openrouter_api_key: ${{ secrets.OPENROUTER_API_KEY }}
          github_token: ${{ secrets.GITHUB_TOKEN }}
```

`GITHUB_TOKEN` is created automatically by GitHub Actions.
You pass it so PRBot can read the PR and post comments.

## Versioning

| Thing | Source of truth |
| --- | --- |
| Crate / CLI version | `Cargo.toml` → `version` |
| Action release users pin | Git tags like `v0.1.1` |
| Floating major pin (optional later) | moving tag `v0` |

Release checklist:

1. Bump `version` in `Cargo.toml` when cutting a release.
2. Commit and push to `main`.
3. Create a GitHub release/tag such as `v0.1.1`.
4. Consumers install with `uses: jaibhasin/PRBot@v0.1.1`.

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
