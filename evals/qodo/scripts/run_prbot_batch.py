#!/usr/bin/env python3
"""Run PRBot --eval-json against each PR in a batch."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

from common import ROOT, batch_dir, load_jsonl, read_json, write_jsonl


def tail_text(value: str | bytes | None, length: int = 4000) -> str:
    if isinstance(value, bytes):
        value = value.decode("utf-8", errors="replace")
    return (value or "")[-length:]


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
    command = [
        str(prbot),
        "review",
        "--eval-json",
        "--repository",
        case["repository"],
        "--pr-number",
        str(case["pr_number"]),
        "--engine",
        engine,
    ]
    try:
        completed = subprocess.run(
            command,
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_sec,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        return {
            "case_id": case["case_id"],
            "repository": case["repository"],
            "pr_number": case["pr_number"],
            "pr_url_to_review": case["pr_url_to_review"],
            "engine": engine,
            "elapsed_seconds": round(time.time() - started, 2),
            "returncode": None,
            "stderr_tail": tail_text(error.stderr),
            "stdout_tail": tail_text(error.stdout),
            "error": f"prbot timed out after {timeout_sec} seconds",
        }
    except OSError as error:
        return {
            "case_id": case["case_id"],
            "repository": case["repository"],
            "pr_number": case["pr_number"],
            "pr_url_to_review": case["pr_url_to_review"],
            "engine": engine,
            "elapsed_seconds": round(time.time() - started, 2),
            "returncode": None,
            "stderr_tail": "",
            "error": f"could not start prbot: {error}",
        }
    elapsed = round(time.time() - started, 2)
    record = {
        "case_id": case["case_id"],
        "repository": case["repository"],
        "pr_number": case["pr_number"],
        "pr_url_to_review": case["pr_url_to_review"],
        "engine": engine,
        "elapsed_seconds": elapsed,
        "returncode": completed.returncode,
        "stderr_tail": tail_text(completed.stderr),
    }
    if completed.returncode != 0:
        record["error"] = f"prbot exited {completed.returncode}"
        record["stdout_tail"] = tail_text(completed.stdout)
        return record
    try:
        payload = extract_eval_payload(completed.stdout)
    except Exception as error:  # noqa: BLE001
        record["error"] = str(error)
        record["stdout_tail"] = tail_text(completed.stdout)
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
    parser.add_argument(
        "--workers",
        type=int,
        default=int(os.environ.get("PRBOT_EVAL_REVIEW_WORKERS", "3")),
    )
    parser.add_argument("--attempts", type=int, default=2)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    if not os.environ.get("GITHUB_TOKEN"):
        raise SystemExit("GITHUB_TOKEN is required to fetch public PR refs")
    if not os.environ.get("OPENROUTER_API_KEY"):
        raise SystemExit("OPENROUTER_API_KEY is required for eval reviews")

    target = batch_dir(args.batch_id)
    selection = read_json(target / "selection.json")
    cases = selection["cases"]
    if args.limit > 0:
        cases = cases[: args.limit]
    prbot = find_prbot_bin(args.prbot_bin)

    output = target / "prbot_output.jsonl"
    existing_rows = load_jsonl(output) if output.exists() else []
    by_case = {row["case_id"]: row for row in existing_rows}
    pending = [
        case
        for case in cases
        if args.force
        or case["case_id"] not in by_case
        or by_case[case["case_id"]].get("error")
    ]
    skipped = len(cases) - len(pending)
    if skipped:
        print(f"reusing {skipped} successful PRBot result(s)")

    def review_with_retries(case: dict) -> dict:
        row = {}
        for attempt in range(1, max(args.attempts, 1) + 1):
            row = run_one(prbot, case, args.engine, args.timeout_sec)
            row["attempt"] = attempt
            if not row.get("error"):
                return row
            print(f"review attempt {attempt} failed for {case['case_id']}: {row['error']}")
        return row

    selection_order = selection["cases"]
    workers = max(args.workers, 1)
    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = {
            executor.submit(review_with_retries, case): case
            for case in pending
        }
        for future in as_completed(futures):
            case = futures[future]
            case_id = case["case_id"]
            try:
                row = future.result()
            except Exception as error:  # noqa: BLE001
                row = {
                    "case_id": case_id,
                    "repository": case["repository"],
                    "pr_number": case["pr_number"],
                    "pr_url_to_review": case["pr_url_to_review"],
                    "engine": args.engine,
                    "error": f"review worker failed: {error}",
                }
            by_case[case_id] = row
            ordered = [
                by_case[item["case_id"]]
                for item in selection_order
                if item["case_id"] in by_case
            ]
            write_jsonl(output, ordered)
            status = "ok" if not row.get("error") else f"error: {row['error']}"
            print(f"{case_id}: {status} findings={len(row.get('findings', []))}")

    print(f"wrote {output}")
    failures = [
        case["case_id"]
        for case in cases
        if case["case_id"] not in by_case or by_case[case["case_id"]].get("error")
    ]
    if failures:
        print(f"{len(failures)} PRBot review(s) failed: {', '.join(failures)}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
