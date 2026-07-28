# Fixture catalog

`catalog.jsonl` holds fixture definitions for the offline quality gate.

Statuses:

- `pending_adjudication` - skeleton only
- `ready` - has repository/PR refs and expected findings, not yet scored
- `adjudicated` - human-labeled evaluate.py rows exist
- `retired` - kept for history, excluded from the gate

```bash
python3 scripts/generate_fixture_catalog.py
python3 scripts/run_fixture_batch.py --catalog evals/fixtures/catalog.jsonl --out /tmp/eval-draft.jsonl
python3 scripts/evaluate.py evals/sample.jsonl --allow-small-sample
```

`generate_fixture_catalog.py` only fills missing pending stubs.
It does not overwrite adjudicated or ready rows.

`run_fixture_batch.py` runs `prbot review --eval-json` for ready cases and writes draft evaluate.py rows.
Humans must set `expected_id` / `actionable` / `anchor_valid` before committing golden results.

CI currently scores `evals/sample.jsonl` as a smoke gate.
Commit adjudicated results to `evals/fixtures/results.jsonl` before enabling the hard 50-case gate.
