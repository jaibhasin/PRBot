#!/usr/bin/env python3
"""Generate the held-out fixture catalog skeleton used by the quality gate.

This does not invent adjudicated model results.
It creates labeled case definitions that humans (or a future runner) fill in.
"""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "evals" / "fixtures" / "catalog.jsonl"

LANGUAGES = ["rust", "typescript", "javascript", "python", "go"]
KINDS = [
    ("overflow", "P1", "integer overflow or wrap"),
    ("authz", "P0", "authorization bypass"),
    ("null", "P1", "null/None dereference"),
    ("race", "P1", "concurrency race"),
    ("compat", "P2", "backward-incompatible API"),
    ("inject", "P0", "injection or secret leak"),
    ("clean", "P1", "clean change with no defect"),
    ("crossfile", "P1", "missed caller after signature change"),
    ("perf", "P2", "accidental quadratic path"),
    ("partial", "P1", "partial coverage must not claim clean"),
]


def main() -> None:
    OUT.parent.mkdir(parents=True, exist_ok=True)
    rows = []
    case_id = 1
    while len(rows) < 50:
        for language in LANGUAGES:
            for kind, priority, summary in KINDS:
                if len(rows) >= 50:
                    break
                rows.append(
                    {
                        "case_id": f"{language}-{case_id:03d}-{kind}",
                        "language": language,
                        "kind": kind,
                        "priority": priority,
                        "summary": summary,
                        "status": "pending_adjudication",
                        "notes": "Replace with a held-out PR fixture and human labels before scoring.",
                    }
                )
                case_id += 1
    with OUT.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True) + "\n")
    print(f"wrote {len(rows)} fixture definitions to {OUT}")


if __name__ == "__main__":
    main()
