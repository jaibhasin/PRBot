# Qodo harness

Runs PRBot against [Qodo/PR-Review-Bench](https://huggingface.co/datasets/Qodo/PR-Review-Bench).

## Flow

1. Download dataset
2. Select 10 unused PRs
3. LLM categorizes GT: `functional` / `style` / `other`
4. PRBot `--eval-json` (no GitHub writes)
5. LLM judges vs functional GT
6. Write `SUMMARY.md` + append `progress/SCOREBOARD.md`

## Run

```bash
export OPENROUTER_API_KEY=...
export GITHUB_TOKEN=...
cargo build --release

PRBOT_EVAL_LIMIT=2 ./evals/qodo/scripts/run_batch.sh   # smoke
./evals/qodo/scripts/run_batch.sh                      # full batch of 10
```

Optional:

```bash
export PRBOT_EVAL_CATEGORIZE_MODEL=openai/gpt-5.6-luna
export PRBOT_EVAL_JUDGE_MODEL=anthropic/claude-sonnet-4.6
```

## Outputs

`batches/batch-NNN/`: `selection.json`, `ground_truth.jsonl`, `categorized.jsonl`, `prbot_output.jsonl`, `judged.jsonl`, `metrics.json`, `SUMMARY.md`

`progress/SCOREBOARD.md`: version history
