"""Shared helpers for the Qodo PR-Review-Bench harness."""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
QODO_ROOT = ROOT / "evals" / "qodo"
DATA_DIR = QODO_ROOT / "data"
BATCHES_DIR = QODO_ROOT / "batches"
PROGRESS_DIR = QODO_ROOT / "progress"
DATASET_URL = (
    "https://huggingface.co/datasets/Qodo/PR-Review-Bench/resolve/main/"
    "git_code_review_bench_100_w_open_prs.jsonl"
)
DATASET_PATH = DATA_DIR / "git_code_review_bench_100_w_open_prs.jsonl"
OPENROUTER_URL = os.environ.get(
    "OPENROUTER_URL", "https://openrouter.ai/api/v1/chat/completions"
)


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    rows = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError as error:
                raise ValueError(f"{path}:{line_number}: {error}") from error
    return rows


def write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=False) + "\n")


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def parse_pr_url(url: str) -> tuple[str, int]:
    # https://github.com/owner/repo/pull/123
    parts = url.rstrip("/").split("/")
    if len(parts) < 7 or parts[-2] != "pull":
        raise ValueError(f"unsupported PR URL: {url}")
    owner, repo, number = parts[-4], parts[-3], parts[-1]
    return f"{owner}/{repo}", int(number)


def case_id_for(row: dict[str, Any], index: int) -> str:
    repo = str(row.get("repo") or "unknown").replace("/", "-")
    _, number = parse_pr_url(row["pr_url_to_review"])
    return f"{repo}-pr{number}-{index:03d}"


def batch_dir(batch_id: str) -> Path:
    path = BATCHES_DIR / batch_id
    path.mkdir(parents=True, exist_ok=True)
    return path


def next_batch_id() -> str:
    existing = sorted(path.name for path in BATCHES_DIR.glob("batch-*") if path.is_dir())
    if not existing:
        return "batch-001"
    last = existing[-1]
    number = int(last.split("-")[-1]) + 1
    return f"batch-{number:03d}"


def openrouter_chat(
    model: str,
    system: str,
    user: str,
    *,
    temperature: float = 0.0,
) -> str:
    api_key = os.environ.get("OPENROUTER_API_KEY", "").strip()
    if not api_key:
        raise RuntimeError("OPENROUTER_API_KEY is required")
    payload = {
        "model": model,
        "temperature": temperature,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    }
    request = urllib.request.Request(
        OPENROUTER_URL,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://github.com/jaibhasin/PRBot",
            "X-Title": "PRBot Qodo Eval Harness",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=180) as response:
            body = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"OpenRouter HTTP {error.code}: {detail}") from error
    content = body["choices"][0]["message"]["content"]
    if not isinstance(content, str) or not content.strip():
        raise RuntimeError("OpenRouter returned empty content")
    return content.strip()


def extract_json_object(raw: str) -> Any:
    text = raw.strip()
    if text.startswith("```"):
        lines = text.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        text = "\n".join(lines).strip()
    return json.loads(text)


def prbot_version() -> str:
    cargo = ROOT / "Cargo.toml"
    for line in cargo.read_text(encoding="utf-8").splitlines():
        if line.startswith("version"):
            return line.split("=", 1)[1].strip().strip('"')
    return "unknown"
