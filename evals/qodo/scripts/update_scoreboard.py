#!/usr/bin/env python3
"""Append a batch result row to the long-lived scoreboard."""

from __future__ import annotations

import argparse
from datetime import date

from common import PROGRESS_DIR, batch_dir, prbot_revision, prbot_version, read_json

SCOREBOARD = PROGRESS_DIR / "SCOREBOARD.md"
HEADER = (
    "# Qodo scoreboard\n\n"
    "| Date | PRBot version | Revision | Batch | Engine | Cases | Ground truth | "
    "Precision | Recall | F1 | Errors | Notes |\n"
    "| --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n"
)


def ensure_scoreboard() -> None:
    PROGRESS_DIR.mkdir(parents=True, exist_ok=True)
    if SCOREBOARD.exists() and "| F1 |" in SCOREBOARD.read_text(encoding="utf-8"):
        return
    SCOREBOARD.write_text(HEADER, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument("--engine", default="contextual")
    parser.add_argument("--notes", default="")
    args = parser.parse_args()
    ensure_scoreboard()
    target = batch_dir(args.batch_id)
    metrics = read_json(target / "metrics.json")
    if metrics.get("errors"):
        raise SystemExit("refusing to score an incomplete batch")
    row = (
        f"| {date.today().isoformat()} | {prbot_version()} | {prbot_revision()} | "
        f"{args.batch_id} | {args.engine} | "
        f"{metrics['cases']} | {metrics['ground_truth_total']} | "
        f"{metrics['precision']:.2%} | {metrics['recall']:.2%} | "
        f"{metrics['f1']:.2%} | {metrics['errors']} | "
        f"{args.notes or ''} |\n"
    )
    text = SCOREBOARD.read_text(encoding="utf-8")
    identity = (
        f"| {prbot_version()} | {prbot_revision()} | "
        f"{args.batch_id} | {args.engine} |"
    )
    if identity in text:
        print(f"scoreboard already contains {args.batch_id} for PRBot {prbot_version()}")
        return 0
    if not text.endswith("\n"):
        text += "\n"
    SCOREBOARD.write_text(text + row, encoding="utf-8")
    print(f"updated {SCOREBOARD}")
    print(row.strip())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
