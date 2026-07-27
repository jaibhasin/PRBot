# Held-out evaluation fixtures

This directory stores labeled case definitions for the contextual engine quality gate.

## Current state

`catalog.jsonl` is a 50-case skeleton across Rust, TypeScript, JavaScript, Python, and Go.
Cases are marked `pending_adjudication` until humans label real pull request outcomes.

`../sample.jsonl` is only a scorer smoke test.
It must never be treated as evidence that the quality gate passed.

## How to progress a case

1. Capture or synthesize a PR with known base/head SHAs.
2. Run PRBot (default engine is contextual) and record published findings.
3. Have two reviewers label actionable precision, recall for P0/P1, and anchor validity.
4. Adjudicate disagreements.
5. Append the adjudicated row to a results JSONL file and score with:

```bash
python3 scripts/evaluate.py path/to/adjudicated-results.jsonl
```

Regenerate the skeleton with:

```bash
python3 scripts/generate_fixture_catalog.py
```
