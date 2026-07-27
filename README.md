# PRBot

PRBot is a precision-first, multi-agent pull request reviewer that runs entirely as a GitHub Action.
It uses OpenRouter models, an ephemeral local Git object store, syntax-aware related-file discovery, bounded read-only repository tools, and independent finding verification.

Status: experimental.

## How reviews work

PRBot does more than send GitHub patch fragments to one model.

1. It authorizes the triggering GitHub user before making any LLM call.
2. It fetches the exact pull request base and head into an ephemeral bare Git repository.
3. It computes the authoritative local diff, including deletions, renames, and multiline changes.
4. It builds a relationship map from imports, symbols, references, matching tests, manifests, and directory structure.
5. It assigns every eligible changed hunk to a semantic review bundle.
6. It reviews bundles concurrently with bounded read-only tools.
7. It runs a cross-bundle audit and independently verifies every candidate finding with a different model.
8. It resolves exact diff anchors, removes duplicates, creates one formal GitHub review, and updates one persistent summary.

Syntax-aware symbol extraction supports Rust, TypeScript, JavaScript, Python, and Go.
Other supported source and configuration files use import heuristics and bounded code search.

PRBot never runs project code, tests, package managers, shell commands selected by a model, or network requests selected by a model.
Repository files, pull request text, and comments are always treated as untrusted data.

## Owner-only cost control

Only users with GitHub repository `admin` permission can spend model tokens.

- Pull requests authored by a repository owner are reviewed automatically.
- Pull requests from everyone else wait for an owner to comment `/prbot review`.
- Only owners can use interactive `/prbot` commands.
- Unauthorized events are rejected before PRBot checks for an OpenRouter key or calls a model.

This is a GitHub Action, not a GitHub App.
The `/prbot` syntax is a text command and replies are authored by `github-actions[bot]`.

Supported commands:

```text
/prbot review
/prbot ask Why does this change need the compatibility fallback?
/prbot explain <finding URL or description>
```

Ordinary pull request comments do not trigger PRBot.

## Install

Copy [`examples/prbot.yml`](examples/prbot.yml) to `.github/workflows/prbot.yml.
Add `OPENROUTER_API_KEY` as a repository Actions secret.

```yaml
name: PRBot

on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review]
  issue_comment:
    types: [created]

permissions:
  checks: read
  contents: read
  pull-requests: write
  issues: write

jobs:
  review:
    if: ${{ github.event_name == 'pull_request' || (github.event.issue.pull_request && github.event.sender.type != 'Bot') }}
    runs-on: ubuntu-latest
    steps:
      - name: Run PRBot
        uses: jaibhasin/PRBot@v0.1.1
        with:
          openrouter_api_key: ${{ secrets.OPENROUTER_API_KEY }}
          github_token: ${{ secrets.GITHUB_TOKEN }}
```

No `actions/checkout` step is required.
PRBot fetches exact Git revisions internally and never executes their contents.

Normal fork pull request events cannot access repository secrets.
They exit before an LLM call, and an owner can review the fork safely by posting `/prbot review`.
The `issue_comment` workflow runs from the trusted default branch and fetches the fork PR head only as read-only Git data.
PRBot does not require `pull_request_target`.

## Configuration

Action inputs are hard ceilings:

| Input | Default | Purpose |
| --- | ---: | --- |
| `review_model` | `deepseek/deepseek-v4-pro` | Review and audit model |
| `verification_model` | `openai/gpt-5.6-luna` | Independent verification model |
| `max_review_minutes` | `15` | Wall-clock deadline |
| `max_input_tokens` | `500000` | Total estimated input-token ceiling |
| `max_cost_usd` | `3.00` | Estimated model-cost ceiling |
| `max_concurrency` | `8` | Concurrent semantic bundles |
| `max_comments` | `12` | Maximum published inline findings |
| `engine` | `legacy` | `contextual` dogfood engine or legacy fallback |
| `dry_run` | `false` | Build and print the manifest without LLM or GitHub writes |

The review and verification model IDs must be different.
The release defaults should be updated only after the model pair passes the repository evaluation suite.
Set `engine: contextual` explicitly while dogfooding the new engine.
The default remains `legacy` until at least 50 held-out, human-adjudicated cases pass the quality gate described in [`evals/README.md`](evals/README.md).
A 50-case fixture catalog skeleton lives in [`evals/fixtures/`](evals/fixtures/); cases remain pending adjudication until labeled.

Repositories can add a trusted `.prbot.toml` file:

```toml
[review]
auto_review = "owner-authored"
include = ["**/*"]
exclude = ["**/vendor/**", "**/generated/**", "**/*.lock"]
instructions = ["Prioritize user-visible correctness regressions."]
max_comments = 8

[[path_rules]]
glob = "src/auth/**"
instructions = ["Prioritize authorization boundary regressions."]
```

Repository configuration is loaded from the base revision, never from the pull request head.
It can reduce action-level ceilings but cannot increase them.
Hierarchical `AGENTS.md` files from the base revision are also applied to matching paths.

## Review output

PRBot publishes at most one formal review per run.
It supports right-side additions, left-side deletions, context lines, multiline anchors, and file-level fallback when an anchor is ambiguous.
The model supplies exact anchor text, while deterministic code resolves and validates the GitHub line range.

On later pushes, PRBot reviews only bundles affected since the previous reviewed head while retaining full-PR context.
Stable fingerprints prevent unchanged findings from being reposted.
Fingerprints for changed paths are cleared so those areas can be revalidated.

A single hidden-state summary comment is updated on every run.
It reports:

- Reviewed head SHA.
- Eligible and assigned hunk coverage.
- Whether the run was incremental and how many bundles were reviewed.
- Published and rejected findings.
- Failed or truncated stages.
- Reviewer and verifier model IDs.
- Input tokens, output tokens, estimated cost, and elapsed time.

PRBot says “No verified findings” only after complete eligible coverage.
Partial and failed runs are always reported as such.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
docker build -t prbot .
```

Important source boundaries:

```text
src/review/       Event authorization and orchestration
src/repository/   Git snapshots, diffs, context graph, and read-only tools
src/agents/       Parallel reviewers, cross-bundle audit, and verification
src/reporting/    Anchor resolution, fingerprints, and summary state
src/github/       Paginated GitHub API client and batched publishing
src/llm.rs        OpenRouter tool loop, concurrency, and budget ledger
```

## Design references

The architecture uses independently implemented patterns inspired by [PR-Agent context management](https://docs.pr-agent.ai/core-abilities/dynamic_context/), [Aider repository maps](https://aider.chat/docs/repomap.html), [OpenCode tools](https://opencode.ai/docs/tools), [Serge](https://huggingface.github.io/serge/), [Alibaba OpenCodeReview](https://github.com/alibaba/open-code-review), [Mira](https://docs.miracode.ai/), and the [Codex GitHub Action](https://github.com/openai/codex-action).
No source code was copied from those projects.
