#!/usr/bin/env python3
"""Judge PRBot findings against categorized Qodo ground truth using an LLM."""

from __future__ import annotations

import argparse
import json
import os

from common import (
    batch_dir,
    extract_json_object,
    load_jsonl,
    openrouter_chat,
    write_json,
    write_jsonl,
)

SYSTEM = """You are an independent judge for an AI PR review benchmark.
Compare PRBot findings to ground-truth issues.
Return JSON only:
{
  "matches":[{"finding_index":0,"issue_index":1,"confidence":0.0,"reason":"..."}],
  "false_positives":[{"finding_index":0,"reason":"..."}],
  "missed_functional":[{"issue_index":1,"reason":"..."}],
  "notes":"optional"
}
Matching rules:
- Prefer file path equality and overlapping/nearby lines.
- Semantic match on bug meaning counts even if wording differs.
- Style ground-truth issues are out of scope for recall; do not mark them missed_functional.
- A finding that only restates style noise is a false positive for this harness.
"""


def judge_case(model: str, categorized: dict, prbot_row: dict) -> dict:
    functional = [
        issue for issue in categorized.get("issues", []) if issue.get("category") == "functional"
    ]
    findings = prbot_row.get("findings", [])
    user = {
        "case_id": categorized["case_id"],
        "functional_ground_truth": [
            {
                "index": issue.get("index"),
                "title": issue.get("title"),
                "description": issue.get("description"),
                "file_path": issue.get("file_path"),
                "start_line": issue.get("start_line"),
                "end_line": issue.get("end_line"),
                "priority": issue.get("priority"),
            }
            for issue in functional
        ],
        "prbot_findings": [
            {
                "index": index,
                "path": finding.get("candidate", {}).get("path"),
                "line": finding.get("line"),
                "start_line": finding.get("start_line"),
                "title": finding.get("candidate", {}).get("title"),
                "body": finding.get("candidate", {}).get("body"),
                "priority": finding.get("candidate", {}).get("priority"),
                "category": finding.get("candidate", {}).get("category"),
                "file_level": finding.get("file_level"),
            }
            for index, finding in enumerate(findings)
        ],
    }
    raw = openrouter_chat(model, SYSTEM, str(user))
    parsed = extract_json_object(raw)
    matches = parsed.get("matches", [])
    false_positives = parsed.get("false_positives", [])
    missed = parsed.get("missed_functional", [])
    matched_findings = {int(item["finding_index"]) for item in matches if "finding_index" in item}
    matched_issues = {int(item["issue_index"]) for item in matches if "issue_index" in item}

    functional_total = len(functional)
    published_total = len(findings)
    true_positives = len(matched_findings)
    # Unmatched published findings count as false positives if judge omitted them.
    inferred_fp = [
        {"finding_index": index, "reason": "unmatched by judge"}
        for index in range(published_total)
        if index not in matched_findings
        and not any(int(item.get("finding_index", -1)) == index for item in false_positives)
    ]
    false_positives = list(false_positives) + inferred_fp
    missed_count = max(functional_total - len(matched_issues), 0)
    if not missed and missed_count:
        missed = [
            {"issue_index": issue.get("index"), "reason": "not matched by judge"}
            for issue in functional
            if int(issue.get("index", -1)) not in matched_issues
        ]

    precision = (true_positives / published_total) if published_total else 1.0
    recall = (len(matched_issues) / functional_total) if functional_total else 1.0
    return {
        "case_id": categorized["case_id"],
        "repository": categorized["repository"],
        "pr_number": categorized["pr_number"],
        "functional_total": functional_total,
        "style_total": categorized.get("style_count", 0),
        "published_total": published_total,
        "true_positives": true_positives,
        "false_positives_count": len(false_positives),
        "missed_functional_count": len(missed),
        "precision": round(precision, 4),
        "recall": round(recall, 4),
        "matches": matches,
        "false_positives": false_positives,
        "missed_functional": missed,
        "notes": parsed.get("notes", ""),
        "prbot_error": prbot_row.get("error"),
    }


def summarize(rows: list[dict]) -> dict:
    functional_total = sum(row["functional_total"] for row in rows)
    published_total = sum(row["published_total"] for row in rows)
    true_positives = sum(row["true_positives"] for row in rows)
    matched_issues = sum(row["functional_total"] - row["missed_functional_count"] for row in rows)
    return {
        "cases": len(rows),
        "functional_total": functional_total,
        "published_total": published_total,
        "true_positives": true_positives,
        "precision": round((true_positives / published_total) if published_total else 1.0, 4),
        "recall": round((matched_issues / functional_total) if functional_total else 1.0, 4),
        "errors": sum(1 for row in rows if row.get("prbot_error")),
        "style_total": sum(row.get("style_total", 0) for row in rows),
    }


def render_summary_md(batch_id: str, model: str, rows: list[dict], summary: dict) -> str:
    lines = [
        f"# Batch {batch_id} summary",
        "",
        f"Judge model: `{model}`",
        "",
        "## Aggregate",
        "",
        f"- Cases: {summary['cases']}",
        f"- Functional ground-truth issues: {summary['functional_total']}",
        f"- Style issues ignored for recall: {summary['style_total']}",
        f"- Published findings: {summary['published_total']}",
        f"- Precision: {summary['precision']:.2%}",
        f"- Functional recall: {summary['recall']:.2%}",
        f"- PRBot errors: {summary['errors']}",
        "",
        "## Per case",
        "",
        "| Case | Functional | Published | Precision | Recall | Error |",
        "| --- | ---: | ---: | ---: | ---: | --- |",
    ]
    for row in rows:
        lines.append(
            f"| {row['case_id']} | {row['functional_total']} | {row['published_total']} | "
            f"{row['precision']:.2%} | {row['recall']:.2%} | {row.get('prbot_error') or ''} |"
        )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument(
        "--model",
        default=os.environ.get("PRBOT_EVAL_JUDGE_MODEL", "anthropic/claude-sonnet-4.6"),
    )
    args = parser.parse_args()
    target = batch_dir(args.batch_id)
    categorized = {row["case_id"]: row for row in load_jsonl(target / "categorized.jsonl")}
    prbot_rows = {row["case_id"]: row for row in load_jsonl(target / "prbot_output.jsonl")}

    judged = []
    for case_id, truth in categorized.items():
        prbot_row = prbot_rows.get(case_id)
        if prbot_row is None:
            judged.append(
                {
                    "case_id": case_id,
                    "functional_total": truth.get("functional_count", 0),
                    "style_total": truth.get("style_count", 0),
                    "published_total": 0,
                    "true_positives": 0,
                    "false_positives_count": 0,
                    "missed_functional_count": truth.get("functional_count", 0),
                    "precision": 1.0,
                    "recall": 0.0,
                    "matches": [],
                    "false_positives": [],
                    "missed_functional": [],
                    "notes": "missing prbot output",
                    "prbot_error": "missing prbot output",
                }
            )
            continue
        print(f"judging {case_id} with {args.model}")
        if prbot_row.get("error"):
            judged.append(
                {
                    "case_id": case_id,
                    "repository": truth["repository"],
                    "pr_number": truth["pr_number"],
                    "functional_total": truth.get("functional_count", 0),
                    "style_total": truth.get("style_count", 0),
                    "published_total": 0,
                    "true_positives": 0,
                    "false_positives_count": 0,
                    "missed_functional_count": truth.get("functional_count", 0),
                    "precision": 1.0,
                    "recall": 0.0,
                    "matches": [],
                    "false_positives": [],
                    "missed_functional": [],
                    "notes": "",
                    "prbot_error": prbot_row.get("error"),
                }
            )
            continue
        judged.append(judge_case(args.model, truth, prbot_row))

    summary = summarize(judged)
    write_jsonl(target / "judged.jsonl", judged)
    write_json(target / "metrics.json", summary)
    (target / "SUMMARY.md").write_text(
        render_summary_md(args.batch_id, args.model, judged, summary),
        encoding="utf-8",
    )
    print(json.dumps(summary, indent=2))
    print(f"wrote {target / 'SUMMARY.md'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
