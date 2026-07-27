#!/usr/bin/env python3
"""Score human-adjudicated PRBot review results against the release gate."""

import argparse
import json
import sys
from collections import Counter
from pathlib import Path


def parse_args():
    """
    Parse command-line arguments for the evaluation tool.
    
    Returns:
        argparse.Namespace: Parsed results file path and small-sample gate option.
    """
    parser = argparse.ArgumentParser()
    parser.add_argument("results", type=Path)
    parser.add_argument("--allow-small-sample", action="store_true")
    return parser.parse_args()


def load_results(path):
    """
    Load review result objects from a JSON Lines file.
    
    Parameters:
    	path (Path): Path to the input file. Blank lines and lines beginning with `#` are ignored.
    
    Returns:
    	list: Parsed JSON objects from the file.
    
    Raises:
    	ValueError: If a non-comment, non-empty line contains invalid JSON.
    """
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


def safe_ratio(numerator, denominator):
    """
    Calculate a ratio, using 1.0 when the denominator is zero.
    
    Parameters:
        numerator: The value to divide.
        denominator: The divisor.
    
    Returns:
        The quotient, or 1.0 when the denominator is zero.
    """
    return numerator / denominator if denominator else 1.0


def score(cases):
    """
    Compute aggregate review-quality metrics from case results.
    
    Parameters:
    	cases (iterable): Case results containing expected and published findings, hunks, and model-call data.
    
    Returns:
    	dict: Metrics including precision, critical recall, anchor accuracy, coverage, duplicate rate, unauthorized model-call count, and partial false-clean count.
    """
    published = [finding for case in cases for finding in case["published_findings"]]
    actionable = sum(bool(finding["actionable"]) for finding in published)
    anchor_valid = sum(bool(finding["anchor_valid"]) for finding in published)
    expected_critical = {
        (case["case_id"], finding["id"])
        for case in cases
        for finding in case["expected_findings"]
        if finding["priority"] in {"P0", "P1"}
    }
    found_critical = {
        (case["case_id"], finding["expected_id"])
        for case in cases
        for finding in case["published_findings"]
        if finding.get("expected_id")
    }
    fingerprints = [
        (case["case_id"], finding["fingerprint"])
        for case in cases
        for finding in case["published_findings"]
    ]
    duplicate_count = sum(count - 1 for count in Counter(fingerprints).values() if count > 1)
    eligible_hunks = sum(case["eligible_hunks"] for case in cases)
    assigned_hunks = sum(case["assigned_hunks"] for case in cases)
    unauthorized_calls = sum(case.get("unauthorized_model_calls", 0) for case in cases)
    false_clean = sum(
        case.get("reported_clean", False) and case["assigned_hunks"] < case["eligible_hunks"]
        for case in cases
    )
    return {
        "cases": len(cases),
        "precision": safe_ratio(actionable, len(published)),
        "critical_recall": safe_ratio(
            len(expected_critical & found_critical), len(expected_critical)
        ),
        "anchor_accuracy": safe_ratio(anchor_valid, len(published)),
        "coverage": safe_ratio(assigned_hunks, eligible_hunks),
        "duplicate_rate": safe_ratio(duplicate_count, len(published)),
        "unauthorized_model_calls": unauthorized_calls,
        "false_clean_partial_runs": false_clean,
    }


def passes_gate(metrics, allow_small_sample):
    """
    Determine whether aggregate review metrics satisfy the release gate.
    
    Parameters:
        metrics (dict): Aggregate metrics to evaluate against the release thresholds.
        allow_small_sample (bool): Whether to bypass the minimum requirement of 50 cases.
    
    Returns:
        bool: `true` if all release-gate conditions are satisfied, `false` otherwise.
    """
    return all(
        [
            allow_small_sample or metrics["cases"] >= 50,
            metrics["precision"] >= 0.90,
            metrics["critical_recall"] >= 0.75,
            metrics["anchor_accuracy"] == 1.0,
            metrics["coverage"] >= 0.99,
            metrics["duplicate_rate"] < 0.01,
            metrics["unauthorized_model_calls"] == 0,
            metrics["false_clean_partial_runs"] == 0,
        ]
    )


def main():
    """
    Run the evaluation command and report whether the results pass the release gate.
    
    Returns:
    	int: Exit status: `0` if the gate passes, `1` if it fails, or `2` if the results file cannot be loaded or parsed.
    """
    args = parse_args()
    try:
        cases = load_results(args.results)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 2
    metrics = score(cases)
    metrics["passes"] = passes_gate(metrics, args.allow_small_sample)
    print(json.dumps(metrics, indent=2, sort_keys=True))
    return 0 if metrics["passes"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
