#!/usr/bin/env python3
"""Append a batch result row to the long-lived scoreboard."""

from __future__ import annotations

import argparse
from datetime import date

from common import PROGRESS_DIR, batch_dir, prbot_version, read_json

SCOREBOARD = PROGRESS_DIR / "SCOREBOARD.md"


def ensure_scoreboard() -> None:
    PROGRESS_DIR.mkdir(parents=True, exist_ok=True)
    if SCOREBOARD.exists():
        return
    SCOREBOARD.write_text(
        "# Qodo scoreboard\n\n"
        "| Date | PRBot version | Batch | Engine | Cases | Functional GT | Precision | Recall | Errors | Notes |\n"
        "| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument("--engine", default="contextual")
    parser.add_argument("--notes", default="")
    args = parser.parse_args()
    ensure_scoreboard()
    target = batch_dir(args.batch_id)
    metrics = read_json(target / "metrics.json")
    row = (
        f"| {date.today().isoformat()} | {prbot_version()} | {args.batch_id} | {args.engine} | "
        f"{metrics['cases']} | {metrics['functional_total']} | "
        f"{metrics['precision']:.2%} | {metrics['recall']:.2%} | {metrics['errors']} | "
        f"{args.notes or ''} |\n"
    )
    text = SCOREBOARD.read_text(encoding="utf-8")
    if not text.endswith("\n"):
        text += "\n"
    SCOREBOARD.write_text(text + row, encoding="utf-8")
    print(f"updated {SCOREBOARD}")
    print(row.strip())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
