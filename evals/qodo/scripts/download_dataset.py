#!/usr/bin/env python3
"""Download the Qodo PR-Review-Bench JSONL used by this harness."""

from __future__ import annotations

import argparse
import urllib.request

from common import DATASET_PATH, DATASET_URL, DATA_DIR


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    if DATASET_PATH.exists() and not args.force:
        print(f"already present: {DATASET_PATH}")
        return 0
    print(f"downloading {DATASET_URL}")
    urllib.request.urlretrieve(DATASET_URL, DATASET_PATH)
    print(f"wrote {DATASET_PATH} ({DATASET_PATH.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
