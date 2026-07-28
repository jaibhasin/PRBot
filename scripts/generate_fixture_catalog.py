#!/usr/bin/env python3
"""Generate pending fixture catalog stubs without clobbering adjudicated rows."""

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

PROTECTED = {"ready", "adjudicated", "retired"}


def load_existing() -> dict[str, dict]:
    if not OUT.exists():
        return {}
    rows = {}
    for line in OUT.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        rows[row["case_id"]] = row
    return rows


def main() -> None:
    existing = load_existing()
    preserved = {
        case_id: row
        for case_id, row in existing.items()
        if row.get("status") in PROTECTED
    }
    rows = list(preserved.values())
    case_id = 1
    while len(rows) < 50:
        for language in LANGUAGES:
            for kind, priority, summary in KINDS:
                if len(rows) >= 50:
                    break
                candidate_id = f"{language}-{case_id:03d}-{kind}"
                case_id += 1
                if candidate_id in preserved:
                    continue
                if candidate_id in existing and existing[candidate_id].get("status") in PROTECTED:
                    continue
                rows.append(
                    {
                        "case_id": candidate_id,
                        "language": language,
                        "kind": kind,
                        "priority": priority,
                        "summary": summary,
                        "status": "pending_adjudication",
                        "repository": None,
                        "pr_number": None,
                        "expected_findings": [],
                        "notes": "Replace with a held-out PR fixture and human labels before scoring.",
                    }
                )
    rows = sorted(rows, key=lambda row: row["case_id"])[:50]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with OUT.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True) + "\n")
    print(f"wrote {len(rows)} fixture definitions to {OUT} ({len(preserved)} preserved)")


if __name__ == "__main__":
    main()
