#!/usr/bin/env python3
"""Record reproducibility metadata for a completed Qodo eval run."""

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
from datetime import datetime, timezone
from pathlib import Path

from common import DATASET_PATH, ROOT, batch_dir, load_jsonl, read_json, write_json


def file_hash(path: Path) -> str | None:
    if not path.exists():
        return None
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_output(*args: str) -> str:
    completed = subprocess.run(
        ["git", *args],
        cwd=ROOT,
        capture_output=True,
        check=True,
        text=True,
    )
    return completed.stdout.strip()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument("--engine", required=True)
    parser.add_argument("--categorize-model", required=True)
    parser.add_argument("--judge-model", required=True)
    parser.add_argument("--review-workers", type=int, required=True)
    parser.add_argument("--meta-workers", type=int, required=True)
    parser.add_argument("--limit", type=int, default=0)
    args = parser.parse_args()

    target = batch_dir(args.batch_id)
    selection = read_json(target / "selection.json")
    selected = selection["cases"]
    if args.limit > 0:
        selected = selected[: args.limit]
    categorized_path = target / "categorized.jsonl"
    categorized = load_jsonl(categorized_path) if categorized_path.exists() else []
    prbot_path = target / "prbot_output.jsonl"
    prbot_rows = load_jsonl(prbot_path) if prbot_path.exists() else []
    selected_ids = {case["case_id"] for case in selected}
    reviewed_heads = {
        row["case_id"]: row.get("outcome", {}).get("reviewed_sha")
        for row in prbot_rows
        if row["case_id"] in selected_ids
    }
    binary = ROOT / "target" / "release" / "prbot"
    metadata = {
        "batch_id": args.batch_id,
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "smoke_run": args.limit > 0,
        "case_ids": [case["case_id"] for case in selected],
        "engine": args.engine,
        "models": {
            "review": os.environ.get(
                "PRBOT_REVIEW_MODEL",
                "deepseek/deepseek-v4-flash",
            ),
            "verification": os.environ.get(
                "PRBOT_VERIFICATION_MODEL",
                "deepseek/deepseek-v4-flash",
            ),
            "categorize_requested": args.categorize_model,
            "categorize_frozen": sorted(
                {
                    row.get("categorize_model", "legacy-unrecorded")
                    for row in categorized
                    if row["case_id"] in selected_ids
                }
            ),
            "judge": args.judge_model,
        },
        "concurrency": {
            "review_workers": args.review_workers,
            "prbot_max_concurrency": int(
                os.environ.get("PRBOT_MAX_CONCURRENCY", "4")
            ),
            "meta_workers": args.meta_workers,
        },
        "prbot_git_commit": git_output("rev-parse", "HEAD"),
        "prbot_git_dirty": bool(git_output("status", "--porcelain")),
        "prbot_binary_sha256": file_hash(binary),
        "dataset_sha256": file_hash(DATASET_PATH),
        "reviewed_heads": reviewed_heads,
    }
    write_json(target / "run_metadata.json", metadata)
    print(f"wrote {target / 'run_metadata.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
