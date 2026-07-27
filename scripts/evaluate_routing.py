#!/usr/bin/env python3
"""Score specialist-routing results against labelled expectations."""

import argparse
import json
import sys
from pathlib import Path


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("results", type=Path)
    return parser.parse_args()


def load_results(path):
    cases = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            try:
                cases.append(json.loads(line))
            except json.JSONDecodeError as error:
                raise ValueError(f"{path}:{line_number}: {error}") from error
    return cases


def ratio(numerator, denominator):
    return numerator / denominator if denominator else 1.0


def score(cases):
    expected_count = 0
    actual_count = 0
    matched_count = 0
    critical_expected = 0
    critical_matched = 0
    for case in cases:
        expected = set(case["expected_agents"])
        actual = set(case["actual_agents"])
        matched = expected & actual
        expected_count += len(expected)
        actual_count += len(actual)
        matched_count += len(matched)
        if case["priority"] in {"P0", "P1"}:
            critical_expected += len(expected)
            critical_matched += len(matched)
    metrics = {
        "cases": len(cases),
        "precision": ratio(matched_count, actual_count),
        "recall": ratio(matched_count, expected_count),
        "critical_recall": ratio(critical_matched, critical_expected),
    }
    metrics["passes"] = (
        metrics["precision"] >= 0.90
        and metrics["recall"] >= 0.95
        and metrics["critical_recall"] == 1.0
    )
    return metrics


def main():
    args = parse_args()
    try:
        cases = load_results(args.results)
    except (OSError, ValueError, KeyError) as error:
        print(error, file=sys.stderr)
        return 2
    metrics = score(cases)
    print(json.dumps(metrics, indent=2, sort_keys=True))
    return 0 if metrics["passes"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
