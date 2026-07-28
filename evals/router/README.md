# Specialist routing eval

This harness is for a future specialist-routing design.
The shipped product uses one primary reviewer plus an independent verifier, not live specialist routing.

`fixtures.jsonl` contains labelled positive and negative routing cases for architecture, security, performance, and documentation.
Documentation cases include stale public docs, correctly updated docs, internal refactors, and the explicit exclusion of `AGENTS.md`.

Evaluation output must preserve `case_id`, `priority`, and `expected_agents`, then add the router's `actual_agents`.

```bash
python3 scripts/evaluate_routing.py evals/router/sample-results.jsonl
python3 scripts/evaluate_routing.py path/to/model-results.jsonl
```

The routing gate requires at least 95% specialist recall overall, 100% recall for P0/P1 assignments, and at least 90% routing precision.
It is not part of the current product release gate.
