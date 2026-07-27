# Qodo harness

Runs PRBot against [Qodo/PR-Review-Bench](https://huggingface.co/datasets/Qodo/PR-Review-Bench).

## Flow

1. Select PRs
2. Categorize ground truth once
3. Review PRs concurrently
4. Judge cases concurrently
5. Write metrics and scoreboard

## Run

```bash
export OPENROUTER_API_KEY=...
export GITHUB_TOKEN=...
cargo build --release

PRBOT_EVAL_LIMIT=2 ./evals/qodo/scripts/run_batch.sh   # smoke
./evals/qodo/scripts/run_batch.sh                      # full batch of 10
```

All LLM steps use `deepseek/deepseek-v4-flash`.

Categorization is frozen in `categorized.jsonl` and reused on later runs.

Concurrency defaults: 3 PRs, 4 internal calls per PR, and 4 categorization or judging workers.
Override with `PRBOT_EVAL_REVIEW_WORKERS`, `PRBOT_MAX_CONCURRENCY`, and `PRBOT_EVAL_META_WORKERS`.

## Outputs

`batches/batch-NNN/`: `selection.json`, `ground_truth.jsonl`, `categorized.jsonl`, `prbot_output.jsonl`, `judged.jsonl`, `metrics.json`, `run_metadata.json`, `SUMMARY.md`

`progress/SCOREBOARD.md`: version history

- Smoke runs do not update the scoreboard.
- Matching requires overlapping file and line locations.
- Category and compliance breakdowns report recall only.
- Compare revisions using the same batch ID.
