# PRBot

PRBot is a precision-first GitHub Action that reviews pull requests with OpenRouter models.
It is built to find real, actionable problems instead of producing noisy AI feedback.

PRBot combines a primary code reviewer with independent finding verification, then publishes only verified feedback.

Status: experimental.

## Install

1. Add `OPENROUTER_API_KEY` as a repository Actions secret.
2. Create `.github/workflows/prbot.yml` with the following workflow.
3. Open or update a pull request.

```yaml
name: PRBot

on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review]
  issue_comment:
    types: [created]

permissions:
  checks: write
  contents: read
  pull-requests: write
  issues: write

jobs:
  review:
    if: ${{ github.event_name == 'pull_request' || (github.event.issue.pull_request && github.event.sender.type != 'Bot') }}
    runs-on: ubuntu-latest
    steps:
      - name: Run PRBot
        uses: jaibhasin/PRBot@v1.0.2
        with:
          openrouter_api_key: ${{ secrets.OPENROUTER_API_KEY }}
          github_token: ${{ secrets.GITHUB_TOKEN }}
```

You can also copy [`examples/prbot.yml`](examples/prbot.yml).
No checkout step is needed.

On each version tag, the Action image is published to `ghcr.io/jaibhasin/prbot`.
After the first publish, set that package visibility to **Public** in the repo Packages settings if GitHub created it as private.

## How it works

- PRBot automatically reviews pull requests authored by collaborators who meet `min_permission` (default: GitHub `admin`).
- Collaborators who meet `min_permission` can request a review on any pull request by commenting `/prbot review`.
- It fetches the exact pull request revisions and analyzes them as read-only Git data.
- It maps related code and tests, reviews the relevant changes, and independently verifies each potential finding.
- It posts one GitHub review with verified comments and a `PRBot review` check.

PRBot never runs pull request code, tests, package managers, or model-selected shell commands.

## Optional configuration

Use Action inputs to choose models and set hard limits for time, cost, tokens, concurrency, and comment count.
Add a trusted `.prbot.toml` file to narrow review paths or provide repository-specific instructions.

```toml
[review]
exclude = ["**/generated/**", "**/*.lock"]
instructions = ["Prioritize user-visible correctness regressions."]
max_comments = 8
min_permission = "write"
primary_passes = 1
```

Set `min_permission` to `write` or `maintain` when ordinary collaborators should be able to trigger reviews.
The Action default remains `admin` for backward-compatible installs.

Supported commands:

```text
/prbot review
/prbot ask <question>
/prbot explain <finding URL or description>
```
