"""Unit tests for the Qodo evaluation harness."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))

import categorize_gt  # noqa: E402
import common  # noqa: E402
import judge_results  # noqa: E402
import judge_scoring  # noqa: E402
import run_prbot_batch  # noqa: E402


class CommonTests(unittest.TestCase):
    def test_atomic_jsonl_replaces_complete_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rows.jsonl"
            common.write_jsonl(path, [{"case_id": "first"}])
            common.write_jsonl(path, [{"case_id": "second"}])
            self.assertEqual(common.load_jsonl(path), [{"case_id": "second"}])
            self.assertEqual(list(path.parent.glob("*.tmp")), [])

    def test_stable_hash_ignores_dictionary_order(self):
        self.assertEqual(
            common.stable_hash({"a": 1, "b": 2}),
            common.stable_hash({"b": 2, "a": 1}),
        )


class CategorizeTests(unittest.TestCase):
    def case(self):
        return {
            "case_id": "case-1",
            "repository": "owner/repo",
            "pr_number": 1,
            "pr_url_to_review": "https://github.com/owner/repo/pull/1",
            "issues": [
                {
                    "title": "Bug",
                    "description": "Runtime failure",
                    "file_path": "src/main.rs",
                }
            ],
        }

    def test_categorizer_requires_every_issue(self):
        with patch.object(
            categorize_gt,
            "openrouter_chat",
            return_value='{"issues":[]}',
        ):
            with self.assertRaisesRegex(ValueError, "exactly one result"):
                categorize_gt.categorize_case("model", self.case())

    def test_categorizer_preserves_raw_result_and_model(self):
        response = json.dumps(
            {
                "issues": [
                    {
                        "index": 0,
                        "category": "functional",
                        "priority": "P1",
                        "rationale": "runtime impact",
                    }
                ]
            }
        )
        with patch.object(categorize_gt, "openrouter_chat", return_value=response):
            result = categorize_gt.categorize_case("v4-flash", self.case())
        self.assertEqual(result["categorize_model"], "v4-flash")
        self.assertEqual(result["categorize_raw"], response)
        self.assertEqual(result["functional_count"], 1)

    def test_parallel_main_freezes_completed_categorizations(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            cases = []
            for index in range(5):
                case = self.case()
                case["case_id"] = f"case-{index}"
                cases.append(case)
            common.write_jsonl(target / "ground_truth.jsonl", cases)
            response = json.dumps(
                {
                    "issues": [
                        {
                            "index": 0,
                            "category": "functional",
                            "priority": "P1",
                            "rationale": "runtime impact",
                        }
                    ]
                }
            )
            arguments = [
                "categorize_gt.py",
                "--batch-id",
                "batch-test",
                "--workers",
                "4",
            ]
            with (
                patch.object(categorize_gt, "batch_dir", return_value=target),
                patch.object(sys, "argv", arguments),
                patch.object(
                    categorize_gt,
                    "openrouter_chat",
                    return_value=response,
                ) as chat,
            ):
                self.assertEqual(categorize_gt.main(), 0)
                self.assertEqual(categorize_gt.main(), 0)
            self.assertEqual(chat.call_count, 5)
            self.assertEqual(
                len(common.load_jsonl(target / "categorized.jsonl")),
                5,
            )


class JudgeTests(unittest.TestCase):
    def test_location_gate_requires_same_path_and_overlap(self):
        issues = [
            {
                "index": 0,
                "file_path": "src/main.rs",
                "start_line": 10,
                "end_line": 12,
            }
        ]
        findings = [
            {"candidate": {"path": "src/main.rs"}, "line": 11},
            {"candidate": {"path": "src/main.rs"}, "line": 20},
            {"candidate": {"path": "src/other.rs"}, "line": 11},
        ]
        self.assertEqual(judge_scoring.eligible_pairs(issues, findings), {(0, 0)})

    def test_unlocated_ground_truth_uses_semantic_matching(self):
        issues = [{"index": 0, "file_path": None}]
        findings = [
            {"candidate": {"path": "src/main.rs"}, "line": 11},
            {"candidate": {"path": "src/other.rs"}, "line": 20},
        ]
        self.assertEqual(
            judge_scoring.eligible_pairs(issues, findings),
            {(0, 0), (1, 0)},
        )

    def test_match_normalization_rejects_invalid_and_duplicate_indices(self):
        matches, rejected = judge_scoring.normalize_matches(
            [
                {"finding_index": 0, "issue_index": 0, "confidence": 0.8},
                {"finding_index": 1, "issue_index": 0, "confidence": 0.9},
                {"finding_index": 1, "issue_index": 1, "confidence": 0.7},
                {"finding_index": 99, "issue_index": 99, "confidence": 1.0},
            ],
            {(0, 0), (1, 0), (1, 1)},
        )
        self.assertEqual(
            [(item["finding_index"], item["issue_index"]) for item in matches],
            [(1, 0)],
        )
        self.assertEqual(len(rejected), 3)

    def test_metrics_never_exceed_one(self):
        metrics = judge_scoring.metric_counts(1, 1, 2)
        self.assertEqual(metrics["precision"], 1.0)
        self.assertEqual(metrics["recall"], 1.0)

    def test_empty_precision_is_not_applicable(self):
        metrics = judge_scoring.metric_counts(1, 0, 0)
        self.assertIsNone(metrics["precision"])
        self.assertEqual(metrics["recall"], 0.0)
        self.assertIsNone(metrics["f1"])

    def test_judge_case_derives_consistent_metrics(self):
        categorized = {
            "case_id": "case-1",
            "repository": "owner/repo",
            "pr_number": 1,
            "issues": [
                {
                    "index": 0,
                    "title": "Bug",
                    "description": "Runtime failure",
                    "file_path": "src/main.rs",
                    "start_line": 10,
                    "end_line": 10,
                    "category": "functional",
                }
            ],
        }
        prbot = {
            "findings": [
                {
                    "candidate": {
                        "path": "src/main.rs",
                        "title": "Bug",
                        "body": "Runtime failure",
                    },
                    "line": 10,
                }
            ]
        }
        response = json.dumps(
            {
                "matches": [
                    {
                        "finding_index": 0,
                        "issue_index": 0,
                        "confidence": 0.9,
                        "reason": "same failure",
                    }
                ]
            }
        )
        with patch.object(judge_scoring, "openrouter_chat", return_value=response):
            result = judge_scoring.judge_case("v4-flash", categorized, prbot)
        self.assertEqual(result["precision"], 1.0)
        self.assertEqual(result["recall"], 1.0)
        self.assertEqual(result["f1"], 1.0)

    def test_parallel_judging_is_resumable(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            cases = [
                {
                    "case_id": f"case-{index}",
                    "repository": "owner/repo",
                    "pr_number": index,
                }
                for index in range(1, 6)
            ]
            common.write_json(
                target / "selection.json",
                {"batch_id": "batch-test", "cases": cases},
            )
            common.write_jsonl(
                target / "categorized.jsonl",
                [
                    {
                        **case,
                        "issues": [
                            {
                                "index": 0,
                                "category": "functional",
                                "file_path": "src/main.rs",
                                "start_line": 1,
                                "end_line": 1,
                            }
                        ],
                    }
                    for case in cases
                ],
            )
            common.write_jsonl(
                target / "prbot_output.jsonl",
                [
                    {
                        **case,
                        "findings": [
                            {
                                "candidate": {"path": "src/main.rs"},
                                "line": 1,
                            }
                        ],
                    }
                    for case in cases
                ],
            )

            def judged(_model, categorized, _prbot):
                metrics = judge_scoring.metric_counts(1, 1, 1)
                return {
                    "case_id": categorized["case_id"],
                    "repository": categorized["repository"],
                    "pr_number": categorized["pr_number"],
                    **metrics,
                    "by_category": {"functional": metrics},
                    "compliance": judge_scoring.metric_counts(0, 1, 0),
                    "semantic_only_ground_truth": 0,
                    "prbot_error": None,
                }

            arguments = [
                "judge_results.py",
                "--batch-id",
                "batch-test",
                "--workers",
                "4",
            ]
            with (
                patch.object(judge_results, "batch_dir", return_value=target),
                patch.object(sys, "argv", arguments),
                patch.object(judge_results, "judge_case", side_effect=judged) as judge,
            ):
                self.assertEqual(judge_results.main(), 0)
                self.assertEqual(judge_results.main(), 0)
            self.assertEqual(judge.call_count, 5)
            metrics = common.read_json(target / "metrics.json")
            self.assertEqual(metrics["cases"], 5)
            self.assertEqual(metrics["f1"], 1.0)


class ReviewOutputTests(unittest.TestCase):
    def test_timeout_output_bytes_are_json_safe(self):
        self.assertEqual(run_prbot_batch.tail_text(b"output"), "output")

    def test_extracts_last_eval_payload(self):
        stdout = (
            'log {"ignored":true}\n'
            '{"outcome":{"status":"complete"},"findings":[]}\n'
        )
        payload = run_prbot_batch.extract_eval_payload(stdout)
        self.assertEqual(payload["findings"], [])

    def test_parallel_batch_writes_results_in_selection_order(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            executable = target / "fake-prbot"
            executable.write_text(
                "#!/bin/sh\n"
                "printf '%s\\n' "
                """'{"outcome":{"status":"complete"},"findings":[]}'\n""",
                encoding="utf-8",
            )
            executable.chmod(0o755)
            cases = [
                {
                    "case_id": f"case-{index}",
                    "repository": "owner/repo",
                    "pr_number": index,
                    "pr_url_to_review": f"https://github.com/owner/repo/pull/{index}",
                }
                for index in range(1, 6)
            ]
            common.write_json(
                target / "selection.json",
                {"batch_id": "batch-test", "cases": cases},
            )
            arguments = [
                "run_prbot_batch.py",
                "--batch-id",
                "batch-test",
                "--prbot-bin",
                str(executable),
                "--workers",
                "3",
            ]
            with (
                patch.object(run_prbot_batch, "batch_dir", return_value=target),
                patch.object(sys, "argv", arguments),
                patch.dict(
                    os.environ,
                    {"GITHUB_TOKEN": "token", "OPENROUTER_API_KEY": "key"},
                ),
            ):
                self.assertEqual(run_prbot_batch.main(), 0)
            rows = common.load_jsonl(target / "prbot_output.jsonl")
            self.assertEqual(
                [row["case_id"] for row in rows],
                [case["case_id"] for case in cases],
            )


if __name__ == "__main__":
    unittest.main()
