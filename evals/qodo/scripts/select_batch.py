#!/usr/bin/env python3
"""Select the next batch of PRs from the Qodo dataset."""

from __future__ import annotations

import argparse
import random

from common import (
    DATASET_PATH,
    batch_dir,
    case_id_for,
    load_jsonl,
    next_batch_id,
    parse_pr_url,
    write_json,
    write_jsonl,
    BATCHES_DIR,
)


def used_urls() -> set[str]:
    urls: set[str] = set()
    for selection in BATCHES_DIR.glob("batch-*/selection.json"):
        data = __import__("json").loads(selection.read_text(encoding="utf-8"))
        for item in data.get("cases", []):
            urls.add(item["pr_url_to_review"])
    return urls


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--size", type=int, default=10)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--batch-id", default="")
    parser.add_argument(
        "--prefer-langs",
        default="rust,typescript,javascript,python,go",
        help="Comma-separated language hints used for soft prioritization",
    )
    args = parser.parse_args()
    if not DATASET_PATH.exists():
        raise SystemExit("dataset missing; run download_dataset.py first")

    rows = load_jsonl(DATASET_PATH)
    already = used_urls()
    available = [row for row in rows if row["pr_url_to_review"] not in already]
    if len(available) < args.size:
        raise SystemExit(
            f"only {len(available)} unused PRs left; need {args.size}"
        )

    prefer = {item.strip().lower() for item in args.prefer_langs.split(",") if item.strip()}
    rng = random.Random(args.seed)

    def score(row: dict) -> tuple[int, float]:
        repo = str(row.get("repo", "")).lower()
        preferred = any(lang in repo for lang in prefer)
        # Prefer fewer style-heavy issue counts slightly by random tie-break.
        return (0 if preferred else 1, rng.random())

    available.sort(key=score)
    chosen = available[: args.size]
    batch_id = args.batch_id or next_batch_id()
    target = batch_dir(batch_id)

    cases = []
    ground_truth = []
    for index, row in enumerate(chosen, 1):
        repository, pr_number = parse_pr_url(row["pr_url_to_review"])
        case_id = case_id_for(row, index)
        cases.append(
            {
                "case_id": case_id,
                "repo": row.get("repo"),
                "repository": repository,
                "pr_number": pr_number,
                "pr_url_to_review": row["pr_url_to_review"],
                "num_of_issues": row.get("num_of_issues", len(row.get("issues", []))),
            }
        )
        ground_truth.append(
            {
                "case_id": case_id,
                "repository": repository,
                "pr_number": pr_number,
                "pr_url_to_review": row["pr_url_to_review"],
                "issues": row.get("issues", []),
            }
        )

    write_json(
        target / "selection.json",
        {
            "batch_id": batch_id,
            "seed": args.seed,
            "size": args.size,
            "cases": cases,
        },
    )
    write_jsonl(target / "ground_truth.jsonl", ground_truth)
    print(f"selected {len(cases)} PRs into {target}")
    for case in cases:
        print(f"- {case['case_id']}: {case['pr_url_to_review']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
