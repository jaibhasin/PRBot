# Evals

## Gate scorer

```bash
python3 scripts/evaluate.py evals/sample.jsonl --allow-small-sample
python3 scripts/evaluate.py path/to/results.jsonl
```

Targets: precision ≥ 90%, P0/P1 recall ≥ 75%, anchors 100%, coverage ≥ 99%, duplicates < 1%, unauthorized calls 0, no false-clean partials. Needs ≥ 50 cases unless `--allow-small-sample`.

`evals/sample.jsonl` is CI smoke only.  
`evals/fixtures/catalog.jsonl` is a 50-case skeleton, still pending adjudication.

## Qodo harness

See `evals/qodo/README.md`.
