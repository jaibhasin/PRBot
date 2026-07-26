# PRBot

Multi-agent PR reviewer for GitHub.
Runs as a GitHub Action.
Uses OpenRouter (or compatible LLM APIs) for reviews.

Status: early scaffold (`v0.1.0`).
Review agents are not implemented yet.
The Action packaging and CLI entrypoints are ready.

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
