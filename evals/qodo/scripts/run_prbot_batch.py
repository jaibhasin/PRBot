#!/usr/bin/env python3
"""Run PRBot --eval-json against each PR in a batch."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from pathlib import Path

from common import ROOT, batch_dir, load_jsonl, read_json, write_jsonl


def find_prbot_bin(explicit: str) -> Path:
    if explicit:
        path = Path(explicit)
        if not path.exists():
            raise SystemExit(f"prbot binary not found: {path}")
        return path
    candidates = [
        ROOT / "target" / "release" / "prbot",
        ROOT / "target" / "debug" / "prbot",
    ]
    for path in candidates:
        if path.exists():
            return path
    raise SystemExit("build prbot first: cargo build --release")


def extract_eval_payload(stdout: str) -> dict:
    text = stdout.strip()
    # Prefer the last JSON object in stdout.
    decoder = json.JSONDecoder()
    last = None
    index = 0
    while index < len(text):
        start = text.find("{", index)
        if start < 0:
            break
        try:
            value, end = decoder.raw_decode(text[start:])
        except json.JSONDecodeError:
            index = start + 1
            continue
        if isinstance(value, dict) and "findings" in value and "outcome" in value:
            last = value
        index = start + end
    if last is None:
        raise RuntimeError("could not parse eval JSON payload from prbot stdout")
    return last


def run_one(prbot: Path, case: dict, engine: str, timeout_sec: int) -> dict:
    env = os.environ.copy()
    env["GITHUB_REPOSITORY"] = case["repository"]
    env["PRBOT_PR_NUMBER"] = str(case["pr_number"])
    env["PRBOT_ENGINE"] = engine
    env["PRBOT_EVAL_JSON"] = "1"
    if not env.get("GITHUB_TOKEN"):
        raise SystemExit("GITHUB_TOKEN is required to fetch public PR refs")
    if not env.get("OPENROUTER_API_KEY"):
        raise SystemExit("OPENROUTER_API_KEY is required for eval reviews")

    started = time.time()
    completed = subprocess.run(
        [
            str(prbot),
            "review",
            "--eval-json",
            "--repository",
            case["repository"],
            "--pr-number",
            str(case["pr_number"]),
            "--engine",
            engine,
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout_sec,
        check=False,
    )
    elapsed = round(time.time() - started, 2)
    record = {
        "case_id": case["case_id"],
        "repository": case["repository"],
        "pr_number": case["pr_number"],
        "pr_url_to_review": case["pr_url_to_review"],
        "engine": engine,
        "elapsed_seconds": elapsed,
        "returncode": completed.returncode,
        "stderr_tail": completed.stderr[-4000:],
    }
    if completed.returncode != 0:
        record["error"] = f"prbot exited {completed.returncode}"
        record["stdout_tail"] = completed.stdout[-4000:]
        return record
    try:
        payload = extract_eval_payload(completed.stdout)
    except Exception as error:  # noqa: BLE001
        record["error"] = str(error)
        record["stdout_tail"] = completed.stdout[-4000:]
        return record
    record["outcome"] = payload.get("outcome")
    record["findings"] = payload.get("findings", [])
    return record


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument("--engine", default="contextual")
    parser.add_argument("--prbot-bin", default="")
    parser.add_argument("--timeout-sec", type=int, default=900)
    parser.add_argument("--limit", type=int, default=0, help="Optional cap for smoke tests")
    args = parser.parse_args()

    target = batch_dir(args.batch_id)
    selection = read_json(target / "selection.json")
    cases = selection["cases"]
    if args.limit > 0:
        cases = cases[: args.limit]
    prbot = find_prbot_bin(args.prbot_bin)

    rows = []
    for case in cases:
        print(f"reviewing {case['case_id']} ({case['pr_url_to_review']})")
        row = run_one(prbot, case, args.engine, args.timeout_sec)
        rows.append(row)
        status = "ok" if "findings" in row and "error" not in row else f"error: {row.get('error')}"
        print(f"  -> {status} findings={len(row.get('findings', []))}")
        write_jsonl(target / "prbot_output.jsonl", rows)
    print(f"wrote {target / 'prbot_output.jsonl'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
