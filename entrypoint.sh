#!/usr/bin/env bash
set -euo pipefail

# Docker Actions expose `inputs.*` as INPUT_* env vars automatically.
if [[ -n "${INPUT_OPENROUTER_API_KEY:-}" ]]; then
  export OPENROUTER_API_KEY="${INPUT_OPENROUTER_API_KEY}"
fi

# Only override the job token when the caller explicitly passes one.
if [[ -n "${INPUT_GITHUB_TOKEN:-}" ]]; then
  export GITHUB_TOKEN="${INPUT_GITHUB_TOKEN}"
fi

if [[ -n "${INPUT_PR_NUMBER:-}" ]]; then
  export PRBOT_PR_NUMBER="${INPUT_PR_NUMBER}"
fi

if [[ -n "${INPUT_DRY_RUN:-}" ]]; then
  export PRBOT_DRY_RUN="${INPUT_DRY_RUN}"
fi

if [[ -n "${INPUT_REVIEW_MODEL:-}" ]]; then
  export PRBOT_REVIEW_MODEL="${INPUT_REVIEW_MODEL}"
fi

if [[ -n "${INPUT_VERIFICATION_MODEL:-}" ]]; then
  export PRBOT_VERIFICATION_MODEL="${INPUT_VERIFICATION_MODEL}"
fi

if [[ -n "${INPUT_MAX_REVIEW_MINUTES:-}" ]]; then
  export PRBOT_MAX_REVIEW_MINUTES="${INPUT_MAX_REVIEW_MINUTES}"
fi

if [[ -n "${INPUT_MAX_INPUT_TOKENS:-}" ]]; then
  export PRBOT_MAX_INPUT_TOKENS="${INPUT_MAX_INPUT_TOKENS}"
fi

if [[ -n "${INPUT_MAX_COST_USD:-}" ]]; then
  export PRBOT_MAX_COST_USD="${INPUT_MAX_COST_USD}"
fi

if [[ -n "${INPUT_MAX_CONCURRENCY:-}" ]]; then
  export PRBOT_MAX_CONCURRENCY="${INPUT_MAX_CONCURRENCY}"
fi

if [[ -n "${INPUT_MAX_COMMENTS:-}" ]]; then
  export PRBOT_MAX_COMMENTS="${INPUT_MAX_COMMENTS}"
fi

if [[ -n "${INPUT_ENGINE:-}" ]]; then
  export PRBOT_ENGINE="${INPUT_ENGINE}"
fi

# Docker Actions pass `args` from action.yml after the entrypoint.
if [[ $# -eq 0 ]]; then
  set -- review
fi

cmd="$1"
shift || true

extra_args=()

if [[ "${PRBOT_DRY_RUN:-false}" == "true" ]]; then
  extra_args+=(--dry-run)
fi

if [[ -n "${PRBOT_PR_NUMBER:-}" ]]; then
  extra_args+=(--pr-number "${PRBOT_PR_NUMBER}")
fi

exec prbot "${cmd}" "${extra_args[@]}" "$@"
