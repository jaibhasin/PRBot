#!/usr/bin/env python3
"""Judge PRBot findings against categorized Qodo ground truth using an LLM."""

from __future__ import annotations

import json

from common import (
    extract_json_object,
    openrouter_chat,
)

JUDGE_SCHEMA_VERSION = 2

SYSTEM = """You are a judge for an AI pull-request review benchmark.
Match PRBot findings to ground-truth issues by meaning.
Return JSON only:
{
  "matches":[
    {
      "finding_index":0,
      "issue_index":1,
      "confidence":0.0,
      "reason":"short semantic match explanation"
    }
  ],
  "notes":"optional"
}
Rules:
- Use only pairs listed in eligible_pairs.
- The finding must identify the same concrete problem and impact as the ground truth.
- Similar topics or nearby code without the same defect do not match.
- Each finding and each issue may appear in at most one match.
- Do not invent indices.
"""


def normalize_path(value: object) -> str:
    path = str(value or "").strip().replace("\\", "/")
    while path.startswith("./"):
        path = path[2:]
    return path


def integer(value: object) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def issue_location(issue: dict) -> tuple[str, int, int] | None:
    path = normalize_path(issue.get("file_path"))
    start = integer(issue.get("start_line"))
    end = integer(issue.get("end_line"))
    if not path or (start is None and end is None):
        return None
    start = start if start is not None else end
    end = end if end is not None else start
    assert start is not None and end is not None
    return path, min(start, end), max(start, end)


def finding_location(finding: dict) -> tuple[str, int, int] | None:
    path = normalize_path(finding.get("candidate", {}).get("path"))
    end = integer(finding.get("line"))
    start = integer(finding.get("start_line"))
    if not path or (start is None and end is None):
        return None
    start = start if start is not None else end
    end = end if end is not None else start
    assert start is not None and end is not None
    return path, min(start, end), max(start, end)


def locations_overlap(
    issue: tuple[str, int, int],
    finding: tuple[str, int, int],
) -> bool:
    issue_path, issue_start, issue_end = issue
    finding_path, finding_start, finding_end = finding
    return (
        issue_path == finding_path
        and issue_start <= finding_end
        and finding_start <= issue_end
    )


def eligible_pairs(issues: list[dict], findings: list[dict]) -> set[tuple[int, int]]:
    pairs = set()
    for issue in issues:
        issue_index = integer(issue.get("index"))
        if issue_index is None:
            continue
        location = issue_location(issue)
        for finding_index, finding in enumerate(findings):
            finding_position = finding_location(finding)
            if location is None or (
                finding_position is not None
                and locations_overlap(location, finding_position)
            ):
                pairs.add((finding_index, issue_index))
    return pairs


def normalize_matches(
    parsed_matches: object,
    pairs: set[tuple[int, int]],
) -> tuple[list[dict], list[dict]]:
    candidates = parsed_matches if isinstance(parsed_matches, list) else []
    valid = []
    rejected = []
    for item in candidates:
        if not isinstance(item, dict):
            rejected.append({"match": item, "reason": "match is not an object"})
            continue
        finding_index = integer(item.get("finding_index"))
        issue_index = integer(item.get("issue_index"))
        pair = (finding_index, issue_index)
        if finding_index is None or issue_index is None or pair not in pairs:
            rejected.append({"match": item, "reason": "ineligible or invalid index pair"})
            continue
        try:
            confidence = float(item.get("confidence", 0.0))
        except (TypeError, ValueError):
            confidence = 0.0
        valid.append(
            {
                "finding_index": finding_index,
                "issue_index": issue_index,
                "confidence": max(0.0, min(confidence, 1.0)),
                "reason": str(item.get("reason", "")),
            }
        )

    valid.sort(key=lambda item: item["confidence"], reverse=True)
    accepted = []
    used_findings = set()
    used_issues = set()
    for item in valid:
        if item["finding_index"] in used_findings or item["issue_index"] in used_issues:
            rejected.append({"match": item, "reason": "duplicate finding or issue match"})
            continue
        used_findings.add(item["finding_index"])
        used_issues.add(item["issue_index"])
        accepted.append(item)
    accepted.sort(key=lambda item: item["finding_index"])
    return accepted, rejected


def metric_counts(
    ground_truth_total: int,
    published_total: int,
    true_positives: int,
) -> dict:
    true_positives = max(
        min(true_positives, ground_truth_total, published_total),
        0,
    )
    precision = true_positives / published_total if published_total else None
    recall = true_positives / ground_truth_total if ground_truth_total else None
    f1 = (
        2 * precision * recall / (precision + recall)
        if precision is not None and recall is not None and precision + recall
        else 0.0
        if precision is not None and recall is not None
        else None
    )
    return {
        "ground_truth_total": ground_truth_total,
        "published_total": published_total,
        "true_positives": true_positives,
        "false_positives": max(published_total - true_positives, 0),
        "false_negatives": max(ground_truth_total - true_positives, 0),
        "precision": round(precision, 4) if precision is not None else None,
        "recall": round(recall, 4) if recall is not None else None,
        "f1": round(f1, 4) if f1 is not None else None,
    }


def has_rule(issue: dict) -> bool:
    value = str(issue.get("rule_name") or "").strip().upper()
    return value not in {"", "NONE", "NULL"}


def metrics_for_issues(
    issues: list[dict],
    findings: list[dict],
    matches: list[dict],
) -> dict:
    issue_indices = {
        index
        for issue in issues
        if (index := integer(issue.get("index"))) is not None
    }
    true_positives = sum(
        1 for match in matches if match["issue_index"] in issue_indices
    )
    return metric_counts(len(issue_indices), len(findings), true_positives)


def judge_case(model: str, categorized: dict, prbot_row: dict) -> dict:
    issues = categorized.get("issues", [])
    findings = prbot_row.get("findings", [])
    pairs = eligible_pairs(issues, findings)
    user = {
        "case_id": categorized["case_id"],
        "ground_truth": [
            {
                "index": issue.get("index"),
                "title": issue.get("title"),
                "description": issue.get("description"),
                "file_path": issue.get("file_path"),
                "start_line": issue.get("start_line"),
                "end_line": issue.get("end_line"),
                "problematic_code_snippet": issue.get("problematic_code_snippet"),
                "rule_name": issue.get("rule_name"),
                "category": issue.get("category"),
            }
            for issue in issues
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
        "eligible_pairs": [
            {"finding_index": finding_index, "issue_index": issue_index}
            for finding_index, issue_index in sorted(pairs)
        ],
    }
    raw = openrouter_chat(model, SYSTEM, json.dumps(user, ensure_ascii=False))
    parsed = extract_json_object(raw)
    matches, rejected_matches = normalize_matches(parsed.get("matches"), pairs)
    matched_findings = {item["finding_index"] for item in matches}
    matched_issues = {item["issue_index"] for item in matches}
    issue_by_index = {
        integer(issue.get("index")): issue
        for issue in issues
        if integer(issue.get("index")) is not None
    }
    false_positives = [
        {"finding_index": index, "reason": "no validated ground-truth match"}
        for index in range(len(findings))
        if index not in matched_findings
    ]
    missed = [
        {"issue_index": index, "reason": "no validated PRBot finding"}
        for index in sorted(issue_by_index)
        if index not in matched_issues
    ]
    categories = sorted(
        {str(issue.get("category", "other")).lower() for issue in issues}
        | {"functional", "style", "other"}
    )
    by_category = {
        category: metrics_for_issues(
            [
                issue
                for issue in issues
                if str(issue.get("category", "other")).lower() == category
            ],
            findings,
            matches,
        )
        for category in categories
    }
    return {
        "case_id": categorized["case_id"],
        "repository": categorized["repository"],
        "pr_number": categorized["pr_number"],
        **metrics_for_issues(issues, findings, matches),
        "by_category": by_category,
        "compliance": metrics_for_issues(
            [issue for issue in issues if has_rule(issue)],
            findings,
            matches,
        ),
        "matches": matches,
        "rejected_matches": rejected_matches,
        "false_positive_details": false_positives,
        "missed_ground_truth": missed,
        "semantic_only_ground_truth": sum(
            1 for issue in issues if issue_location(issue) is None
        ),
        "judge_model": model,
        "judge_schema_version": JUDGE_SCHEMA_VERSION,
        "judge_notes": parsed.get("notes", ""),
        "judge_raw": raw,
        "prbot_error": prbot_row.get("error"),
    }


def summarize(rows: list[dict]) -> dict:
    overall = metric_counts(
        sum(row["ground_truth_total"] for row in rows),
        sum(row["published_total"] for row in rows),
        sum(row["true_positives"] for row in rows),
    )
    categories = sorted(
        {
            category
            for row in rows
            for category in row.get("by_category", {})
        }
    )
    by_category = {
        category: metric_counts(
            sum(row["by_category"][category]["ground_truth_total"] for row in rows),
            sum(row["by_category"][category]["published_total"] for row in rows),
            sum(row["by_category"][category]["true_positives"] for row in rows),
        )
        for category in categories
    }
    compliance = metric_counts(
        sum(row["compliance"]["ground_truth_total"] for row in rows),
        sum(row["compliance"]["published_total"] for row in rows),
        sum(row["compliance"]["true_positives"] for row in rows),
    )
    return {
        "cases": len(rows),
        **overall,
        "by_category": by_category,
        "compliance": compliance,
        "errors": sum(1 for row in rows if row.get("prbot_error")),
        "semantic_only_ground_truth": sum(
            row.get("semantic_only_ground_truth", 0) for row in rows
        ),
    }


def percentage(value: float | None) -> str:
    return "N/A" if value is None else f"{value:.2%}"
