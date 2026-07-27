#!/usr/bin/env python3
"""Categorize Qodo ground-truth issues into functional vs style using an LLM."""

from __future__ import annotations

import argparse
import json
import os
from concurrent.futures import ThreadPoolExecutor, as_completed

from common import (
    batch_dir,
    extract_json_object,
    load_jsonl,
    openrouter_chat,
    stable_hash,
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
CATEGORIZE_SCHEMA_VERSION = 1


def categorize_input_hash(model: str, case: dict) -> str:
    """Fingerprint model, schema, and ground-truth issues for resume reuse."""
    return stable_hash(
        {
            "schema": CATEGORIZE_SCHEMA_VERSION,
            "model": model,
            "case_id": case["case_id"],
            "issues": case.get("issues", []),
        }
    )


def reusable_categorized_row(row: dict | None, expected_hash: str) -> bool:
    if not isinstance(row, dict):
        return False
    return row.get("categorize_input_hash") == expected_hash


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
        f"Classify these issues:\n{json.dumps(compact, ensure_ascii=False)}"
    )
    raw = openrouter_chat(model, SYSTEM, str(user))
    parsed = extract_json_object(raw)
    items = parsed.get("issues")
    if not isinstance(items, list):
        raise ValueError("categorizer response is missing an issues array")
    by_index = {
        int(item["index"]): item
        for item in items
        if isinstance(item, dict) and "index" in item
    }
    expected_indices = set(range(len(compact)))
    if set(by_index) != expected_indices or len(items) != len(expected_indices):
        raise ValueError(
            "categorizer must return exactly one result for every issue index"
        )
    categorized = []
    for index, issue in enumerate(case.get("issues", [])):
        meta = by_index[index]
        category = str(meta.get("category", "")).lower()
        if category not in {"functional", "style", "other"}:
            raise ValueError(f"invalid category for issue {index}: {category}")
        priority = str(meta.get("priority", "")).upper()
        if priority not in {"P0", "P1", "P2", "P3"}:
            raise ValueError(f"invalid priority for issue {index}: {priority}")
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
        "categorize_model": model,
        "categorize_schema_version": CATEGORIZE_SCHEMA_VERSION,
        "categorize_input_hash": categorize_input_hash(model, case),
        "categorize_raw": raw,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument(
        "--model",
        default=os.environ.get("PRBOT_EVAL_CATEGORIZE_MODEL", "deepseek/deepseek-v4-flash"),
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=int(os.environ.get("PRBOT_EVAL_META_WORKERS", "4")),
    )
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument(
        "--force",
        action="store_true",
        help="Regenerate frozen categorizations for the selected cases",
    )
    args = parser.parse_args()
    target = batch_dir(args.batch_id)
    ground_truth = load_jsonl(target / "ground_truth.jsonl")
    selected = ground_truth[: args.limit] if args.limit > 0 else ground_truth
    output = target / "categorized.jsonl"
    existing_rows = load_jsonl(output) if output.exists() else []
    by_case = {row["case_id"]: row for row in existing_rows}
    fingerprints = {
        case["case_id"]: categorize_input_hash(args.model, case) for case in selected
    }
    pending = [
        case
        for case in selected
        if args.force
        or not reusable_categorized_row(
            by_case.get(case["case_id"]), fingerprints[case["case_id"]]
        )
    ]
    skipped = len(selected) - len(pending)
    if skipped:
        print(f"reusing {skipped} frozen categorization(s)")

    failures = []
    workers = max(args.workers, 1)
    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = {
            executor.submit(categorize_case, args.model, case): case
            for case in pending
        }
        for future in as_completed(futures):
            case = futures[future]
            case_id = case["case_id"]
            try:
                row = future.result()
            except Exception as error:  # noqa: BLE001
                failures.append((case_id, str(error)))
                print(f"categorizing {case_id} failed: {error}")
                continue
            by_case[case_id] = row
            ordered = [
                by_case[item["case_id"]]
                for item in ground_truth
                if item["case_id"] in by_case
            ]
            write_jsonl(output, ordered)
            print(f"categorized {case_id} with {args.model}")

    if not pending and not output.exists():
        write_jsonl(output, [])
    if output.exists():
        print(f"wrote {output}")
    if failures:
        print(f"{len(failures)} categorization(s) failed")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
