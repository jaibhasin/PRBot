# PRBot contextual engine evaluation

The contextual engine is now the Action default.
Use this gate to validate quality and decide whether to keep it, pin better models, or temporarily roll back with `engine: legacy`.

Use at least 50 held-out, human-adjudicated pull request cases across Rust, TypeScript, JavaScript, Python, and Go.
Include real historical defects, controlled mutations, clean changes, cross-file defects, and security-sensitive changes.
Use two human reviewers and adjudicate disagreements before scoring.

Each JSONL row has this shape:

```json
{
  "case_id": "rust-001",
  "language": "rust",
  "eligible_hunks": 3,
  "assigned_hunks": 3,
  "reported_clean": false,
  "unauthorized_model_calls": 0,
  "expected_findings": [
    {"id": "overflow", "priority": "P1"}
  ],
  "published_findings": [
    {
      "expected_id": "overflow",
      "actionable": true,
      "anchor_valid": true,
      "fingerprint": "stable-fingerprint"
    }
  ]
}
```

Run:

```bash
python3 scripts/evaluate.py path/to/adjudicated-results.jsonl
```

The scorer requires:

- At least 90 percent actionable precision.
- At least 75 percent recall for P0 and P1 findings.
- 100 percent valid anchors.
- At least 99 percent eligible-hunk coverage.
- Less than 1 percent duplicates.
- Zero unauthorized model calls.
- Zero clean claims after partial coverage.

`sample.jsonl` only verifies the scorer in CI.
It is not a quality benchmark and is accepted only with `--allow-small-sample`.

A 50-case catalog skeleton is checked into `evals/fixtures/catalog.jsonl`.
Those rows are pending human adjudication and do not satisfy the release gate until labeled results are scored without `--allow-small-sample`.
