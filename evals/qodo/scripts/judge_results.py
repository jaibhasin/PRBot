#!/usr/bin/env python3
"""Run concurrent judging and write Qodo evaluation reports."""

from __future__ import annotations

import argparse
import json
import os
from concurrent.futures import ThreadPoolExecutor, as_completed

from common import (
    atomic_write_text,
    batch_dir,
    load_jsonl,
    read_json,
    stable_hash,
    write_json,
    write_jsonl,
)
from judge_scoring import (
    JUDGE_SCHEMA_VERSION,
    judge_case,
    percentage,
    summarize,
)


def render_summary_md(
    batch_id: str,
    model: str,
    rows: list[dict],
    summary: dict,
) -> str:
    lines = [
        f"# Batch {batch_id} summary",
        "",
        f"Judge model: `{model}`",
        "",
        "## All-issue Qodo score",
        "",
        f"- Cases: {summary['cases']}",
        f"- Ground-truth issues: {summary['ground_truth_total']}",
        f"- Published findings: {summary['published_total']}",
        f"- Precision: {percentage(summary['precision'])}",
        f"- Recall: {percentage(summary['recall'])}",
        f"- F1: {percentage(summary['f1'])}",
        f"- PRBot errors: {summary['errors']}",
        "",
        "## Category breakdown",
        "",
        "| Category | Ground truth | Matches | Recall |",
        "| --- | ---: | ---: | ---: |",
    ]
    for category, metrics in summary["by_category"].items():
        lines.append(
            f"| {category} | {metrics['ground_truth_total']} | "
            f"{metrics['true_positives']} | {percentage(metrics['recall'])} |"
        )
    lines.extend(
        [
            "",
            "## Compliance-source breakdown",
            "",
            f"- Ground-truth issues: {summary['compliance']['ground_truth_total']}",
            f"- Matches: {summary['compliance']['true_positives']}",
            f"- Recall: {percentage(summary['compliance']['recall'])}",
            "",
            "## Per case",
            "",
            "| Case | Ground truth | Published | Precision | Recall | F1 |",
            "| --- | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for row in rows:
        lines.append(
            f"| {row['case_id']} | {row['ground_truth_total']} | "
            f"{row['published_total']} | {percentage(row['precision'])} | "
            f"{percentage(row['recall'])} | {percentage(row['f1'])} |"
        )
    lines.append("")
    return "\n".join(lines)


def validate_inputs(
    selected: list[dict],
    categorized: dict[str, dict],
    prbot_rows: dict[str, dict],
) -> list[str]:
    invalid = []
    for case in selected:
        case_id = case["case_id"]
        if case_id not in categorized:
            invalid.append(f"{case_id}: missing categorization")
        elif case_id not in prbot_rows:
            invalid.append(f"{case_id}: missing PRBot output")
        elif prbot_rows[case_id].get("error"):
            invalid.append(f"{case_id}: {prbot_rows[case_id]['error']}")
    return invalid


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument(
        "--model",
        default=os.environ.get("PRBOT_EVAL_JUDGE_MODEL", "deepseek/deepseek-v4-flash"),
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=int(os.environ.get("PRBOT_EVAL_META_WORKERS", "4")),
    )
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    target = batch_dir(args.batch_id)
    selection = read_json(target / "selection.json")
    selected = selection["cases"]
    if args.limit > 0:
        selected = selected[: args.limit]
    categorized = {
        row["case_id"]: row
        for row in load_jsonl(target / "categorized.jsonl")
    }
    prbot_rows = {
        row["case_id"]: row
        for row in load_jsonl(target / "prbot_output.jsonl")
    }

    invalid = validate_inputs(selected, categorized, prbot_rows)
    if invalid:
        print("batch is incomplete and will not be scored:")
        for error in invalid:
            print(f"- {error}")
        return 1

    output = target / "judged.jsonl"
    existing_rows = load_jsonl(output) if output.exists() else []
    by_case = {row["case_id"]: row for row in existing_rows}
    fingerprints = {
        case["case_id"]: stable_hash(
            {
                "schema": JUDGE_SCHEMA_VERSION,
                "model": args.model,
                "categorized": categorized[case["case_id"]],
                "prbot": prbot_rows[case["case_id"]],
            }
        )
        for case in selected
    }
    pending = [
        case
        for case in selected
        if args.force
        or case["case_id"] not in by_case
        or by_case[case["case_id"]].get("judge_input_hash")
        != fingerprints[case["case_id"]]
    ]
    skipped = len(selected) - len(pending)
    if skipped:
        print(f"reusing {skipped} judged result(s)")

    failures = []
    workers = max(args.workers, 1)
    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = {
            executor.submit(
                judge_case,
                args.model,
                categorized[case["case_id"]],
                prbot_rows[case["case_id"]],
            ): case
            for case in pending
        }
        for future in as_completed(futures):
            case = futures[future]
            case_id = case["case_id"]
            try:
                row = future.result()
            except Exception as error:  # noqa: BLE001
                failures.append((case_id, str(error)))
                print(f"judging {case_id} failed: {error}")
                continue
            row["judge_input_hash"] = fingerprints[case_id]
            by_case[case_id] = row
            ordered = [
                by_case[item["case_id"]]
                for item in selection["cases"]
                if item["case_id"] in by_case
            ]
            write_jsonl(output, ordered)
            print(f"judged {case_id} with {args.model}")

    if failures:
        print(f"{len(failures)} judging task(s) failed")
        return 1
    rows = [by_case[case["case_id"]] for case in selected]
    summary = summarize(rows)
    summary["judge_model"] = args.model
    summary["judge_schema_version"] = JUDGE_SCHEMA_VERSION
    write_json(target / "metrics.json", summary)
    atomic_write_text(
        target / "SUMMARY.md",
        render_summary_md(args.batch_id, args.model, rows, summary),
    )
    print(json.dumps(summary, indent=2))
    print(f"wrote {target / 'SUMMARY.md'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
