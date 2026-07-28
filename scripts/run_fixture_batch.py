#!/usr/bin/env python3
"""Run ready fixture PRs through prbot --eval-json and emit draft evaluate.py rows."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load_catalog(path: Path) -> list[dict]:
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            rows.append(json.loads(line))
    return rows


def draft_row(case: dict, payload: dict) -> dict:
    outcome = payload.get("outcome", {})
    findings = payload.get("findings", [])
    published = []
    for finding in findings:
        candidate = finding.get("candidate", {})
        published.append(
            {
                "expected_id": None,
                "actionable": True,
                "anchor_valid": bool(finding.get("line")) and not finding.get("file_level", False),
                "fingerprint": finding.get("fingerprint"),
                "path": candidate.get("path"),
                "title": candidate.get("title"),
                "priority": candidate.get("priority"),
            }
        )
    expected = case.get("expected_findings") or []
    return {
        "case_id": case["case_id"],
        "language": case.get("language", "unknown"),
        "eligible_hunks": outcome.get("eligible_hunks", 0),
        "assigned_hunks": outcome.get("assigned_hunks", 0),
        "reported_clean": len(findings) == 0 and outcome.get("status") == "complete",
        "unauthorized_model_calls": 0,
        "expected_findings": expected,
        "published_findings": published,
        "notes": "Draft row from --eval-json. Set expected_id/actionable/anchor_valid before adjudication.",
    }


def run_case(case: dict, binary: Path) -> dict:
    repository = case.get("repository")
    pr_number = case.get("pr_number")
    if not repository or not pr_number:
        raise SystemExit(f"{case['case_id']} is missing repository/pr_number")
    env = os.environ.copy()
    env["PRBOT_EVAL_JSON"] = "1"
    command = [
        str(binary),
        "review",
        "--eval-json",
        "--repository",
        repository,
        "--pr-number",
        str(pr_number),
    ]
    completed = subprocess.run(
        command,
        cwd=ROOT,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    # Eval payload is the last JSON object printed.
    lines = [line for line in completed.stdout.splitlines() if line.strip().startswith("{")]
    if not lines:
        raise SystemExit(f"no JSON payload for {case['case_id']}: {completed.stdout}")
    return json.loads(lines[-1])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--catalog", type=Path, default=ROOT / "evals/fixtures/catalog.jsonl")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/prbot")
    parser.add_argument("--status", default="ready")
    args = parser.parse_args()

    cases = [
        case
        for case in load_catalog(args.catalog)
        if case.get("status") == args.status
    ]
    if not cases:
        print(f"no cases with status={args.status}", file=sys.stderr)
        return 0

    drafts = []
    for case in cases:
        print(f"running {case['case_id']}...", file=sys.stderr)
        payload = run_case(case, args.binary)
        drafts.append(draft_row(case, payload))

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w", encoding="utf-8") as handle:
        for row in drafts:
            handle.write(json.dumps(row, sort_keys=True) + "\n")
    print(f"wrote {len(drafts)} draft rows to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
