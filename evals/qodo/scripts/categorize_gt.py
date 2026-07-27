#!/usr/bin/env python3
"""Categorize Qodo ground-truth issues into functional vs style using an LLM."""

from __future__ import annotations

import argparse
import os

from common import (
    batch_dir,
    extract_json_object,
    load_jsonl,
    openrouter_chat,
    write_jsonl,
)

SYSTEM = """You classify injected code-review ground-truth issues.
Return JSON only with this shape:
{"issues":[{"index":0,"category":"functional|style|other","priority":"P0|P1|P2|P3","rationale":"short reason"}]}
Rules:
- functional: correctness, security, reliability, concurrency, resource leaks, API contract breaks, logic bugs
- style: formatting, quotes, semicolons, naming conventions, import order, lint-only preferences
- other: docs-only or unclear
Prefer functional when an issue has concrete runtime impact.
"""


def categorize_case(model: str, case: dict) -> dict:
    compact = []
    for index, issue in enumerate(case.get("issues", [])):
        compact.append(
            {
                "index": index,
                "title": issue.get("title"),
                "description": issue.get("description"),
                "file_path": issue.get("file_path"),
                "rule_name": issue.get("rule_name"),
            }
        )
    user = (
        f"Case {case['case_id']} for {case['pr_url_to_review']}\n"
        f"Classify these issues:\n{compact}"
    )
    raw = openrouter_chat(model, SYSTEM, str(user))
    parsed = extract_json_object(raw)
    by_index = {
        int(item["index"]): item
        for item in parsed.get("issues", [])
        if "index" in item
    }
    categorized = []
    for index, issue in enumerate(case.get("issues", [])):
        meta = by_index.get(index, {})
        category = str(meta.get("category", "other")).lower()
        if category not in {"functional", "style", "other"}:
            category = "other"
        priority = str(meta.get("priority", "P2")).upper()
        if priority not in {"P0", "P1", "P2", "P3"}:
            priority = "P2"
        categorized.append(
            {
                **issue,
                "index": index,
                "category": category,
                "priority": priority,
                "categorize_rationale": meta.get("rationale", ""),
            }
        )
    return {
        "case_id": case["case_id"],
        "repository": case["repository"],
        "pr_number": case["pr_number"],
        "pr_url_to_review": case["pr_url_to_review"],
        "issues": categorized,
        "functional_count": sum(1 for item in categorized if item["category"] == "functional"),
        "style_count": sum(1 for item in categorized if item["category"] == "style"),
        "other_count": sum(1 for item in categorized if item["category"] == "other"),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument(
        "--model",
        default=os.environ.get("PRBOT_EVAL_CATEGORIZE_MODEL", "deepseek/deepseek-v4-flash"),
    )
    args = parser.parse_args()
    target = batch_dir(args.batch_id)
    ground_truth = load_jsonl(target / "ground_truth.jsonl")
    rows = []
    for case in ground_truth:
        print(f"categorizing {case['case_id']} with {args.model}")
        rows.append(categorize_case(args.model, case))
    write_jsonl(target / "categorized.jsonl", rows)
    print(f"wrote {target / 'categorized.jsonl'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
