#!/usr/bin/env bash
# Run one Qodo eval batch end-to-end.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SCRIPTS="$ROOT/evals/qodo/scripts"
cd "$SCRIPTS"

BATCH_ID="${1:-}"
SIZE="${BATCH_SIZE:-10}"
SEED="${BATCH_SEED:-42}"
ENGINE="${PRBOT_ENGINE:-contextual}"
LIMIT="${PRBOT_EVAL_LIMIT:-0}"
CATEGORIZE_MODEL="${PRBOT_EVAL_CATEGORIZE_MODEL:-deepseek/deepseek-v4-flash}"
JUDGE_MODEL="${PRBOT_EVAL_JUDGE_MODEL:-deepseek/deepseek-v4-flash}"
export PRBOT_REVIEW_MODEL="${PRBOT_REVIEW_MODEL:-deepseek/deepseek-v4-flash}"
export PRBOT_VERIFICATION_MODEL="${PRBOT_VERIFICATION_MODEL:-deepseek/deepseek-v4-flash}"
NOTES="${PRBOT_EVAL_NOTES:-}"

if [[ -z "${OPENROUTER_API_KEY:-}" ]]; then
  echo "OPENROUTER_API_KEY is required" >&2
  exit 1
fi
if [[ -z "${GITHUB_TOKEN:-}" ]]; then
  echo "GITHUB_TOKEN is required" >&2
  exit 1
fi

python3 download_dataset.py
if [[ -z "$BATCH_ID" ]]; then
  python3 select_batch.py --size "$SIZE" --seed "$SEED"
  BATCH_ID="$(python3 - <<'PY'
from common import BATCHES_DIR
dirs = sorted(p.name for p in BATCHES_DIR.glob("batch-*") if p.is_dir())
print(dirs[-1])
PY
)"
fi

echo "Using batch: $BATCH_ID"
python3 categorize_gt.py --batch-id "$BATCH_ID" --model "$CATEGORIZE_MODEL"

if [[ ! -x "$ROOT/target/release/prbot" ]]; then
  (cd "$ROOT" && cargo build --release)
fi

LIMIT_ARGS=()
if [[ "$LIMIT" != "0" ]]; then
  LIMIT_ARGS+=(--limit "$LIMIT")
fi
python3 run_prbot_batch.py --batch-id "$BATCH_ID" --engine "$ENGINE" "${LIMIT_ARGS[@]}"
python3 judge_results.py --batch-id "$BATCH_ID" --model "$JUDGE_MODEL"
python3 update_scoreboard.py --batch-id "$BATCH_ID" --engine "$ENGINE" --notes "$NOTES"

echo "Done. See:"
echo "  $ROOT/evals/qodo/batches/$BATCH_ID/SUMMARY.md"
echo "  $ROOT/evals/qodo/progress/SCOREBOARD.md"
