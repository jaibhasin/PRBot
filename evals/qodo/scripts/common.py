"""Shared helpers for the Qodo PR-Review-Bench harness."""

from __future__ import annotations

import json
import os
import random
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from contextlib import contextmanager
from hashlib import sha256
from pathlib import Path
from typing import Any

import fcntl

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
    text = "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows)
    atomic_write_text(path, text)


def write_json(path: Path, value: Any) -> None:
    atomic_write_text(path, json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def atomic_write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.",
        suffix=".tmp",
    )
    temporary_path = Path(temporary)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary_path, path)
    except BaseException:
        temporary_path.unlink(missing_ok=True)
        raise


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def prbot_row_error(row: dict[str, Any]) -> str | None:
    """Return why a PRBot eval row is not scorable, or None when it is.

    Exit code 0 with outcome.status=failed and empty findings must not be
    treated as a successful zero-finding review.
    """
    existing = row.get("error")
    if existing:
        return str(existing)
    outcome = row.get("outcome")
    if not isinstance(outcome, dict):
        return "missing PRBot outcome"
    status = outcome.get("status")
    if status in {"failed", "skipped"}:
        failed = outcome.get("failed_bundles") or []
        detail = f": {', '.join(str(item) for item in failed)}" if failed else ""
        return f"PRBot outcome status={status}{detail}"
    if outcome.get("coverage_complete") is not True:
        label = status if status is not None else "unknown"
        return f"PRBot coverage incomplete (status={label})"
    return None


def stable_hash(value: Any) -> str:
    serialized = json.dumps(
        value,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    )
    return sha256(serialized.encode("utf-8")).hexdigest()


def file_sha256(path: Path) -> str:
    digest = sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


# Bump when resume-cache fields change so old rows are recomputed.
REVIEW_CACHE_SCHEMA = 1
DEFAULT_REVIEW_MODEL = "deepseek/deepseek-v4-flash"
DEFAULT_VERIFICATION_MODEL = "deepseek/deepseek-v4-flash"


def env_or_default(name: str, default: str) -> str:
    value = os.environ.get(name)
    if value is None or value.strip() == "":
        return default
    return value.strip()


def prbot_review_input_hash(prbot_bin: Path, engine: str) -> str:
    """Fingerprint the binary and review settings that affect PRBot output.

    Eval review workers are excluded: they only change scheduling, not a single
    PR's review result. Internal max_concurrency is included because it can
    change tool/agent interleaving under budget pressure.
    """
    return stable_hash(
        {
            "schema": REVIEW_CACHE_SCHEMA,
            "engine": engine,
            "binary_sha256": file_sha256(prbot_bin),
            "review_model": env_or_default(
                "PRBOT_REVIEW_MODEL", DEFAULT_REVIEW_MODEL
            ),
            "verification_model": env_or_default(
                "PRBOT_VERIFICATION_MODEL", DEFAULT_VERIFICATION_MODEL
            ),
            "max_review_minutes": env_or_default("PRBOT_MAX_REVIEW_MINUTES", "15"),
            "max_input_tokens": env_or_default("PRBOT_MAX_INPUT_TOKENS", "500000"),
            "max_cost_usd": env_or_default("PRBOT_MAX_COST_USD", "3.0"),
            "max_concurrency": env_or_default("PRBOT_MAX_CONCURRENCY", "8"),
            "max_comments": env_or_default("PRBOT_MAX_COMMENTS", "12"),
        }
    )


def reusable_prbot_row(row: dict[str, Any] | None, expected_hash: str) -> bool:
    if not isinstance(row, dict):
        return False
    if prbot_row_error(row):
        return False
    return row.get("review_input_hash") == expected_hash


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
    body = None
    attempts = max(int(os.environ.get("PRBOT_EVAL_HTTP_ATTEMPTS", "4")), 1)
    for attempt in range(attempts):
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
            with global_request_slot():
                with urllib.request.urlopen(request, timeout=180) as response:
                    body = json.loads(response.read().decode("utf-8"))
            break
        except urllib.error.HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")
            retryable = error.code == 429 or 500 <= error.code < 600
            if not retryable or attempt + 1 == attempts:
                raise RuntimeError(f"OpenRouter HTTP {error.code}: {detail}") from error
            retry_after = error.headers.get("Retry-After")
            delay = float(retry_after) if retry_after and retry_after.isdigit() else None
        except (TimeoutError, urllib.error.URLError) as error:
            if attempt + 1 == attempts:
                raise RuntimeError(f"OpenRouter request failed: {error}") from error
            delay = None
        if delay is None:
            delay = min(2**attempt, 30) + random.uniform(0.0, 0.5)
        time.sleep(delay)
    if body is None:
        raise RuntimeError("OpenRouter request completed without a response")
    if not isinstance(body, dict):
        raise RuntimeError("OpenRouter returned a non-object response")
    choices = body.get("choices")
    if not isinstance(choices, list) or not choices:
        raise RuntimeError("OpenRouter response is missing choices")
    choice = choices[0]
    if not isinstance(choice, dict) or not isinstance(choice.get("message"), dict):
        raise RuntimeError("OpenRouter response is missing a message")
    content = choice["message"].get("content")
    if not isinstance(content, str) or not content.strip():
        raise RuntimeError("OpenRouter returned empty content")
    return content.strip()


@contextmanager
def global_request_slot():
    directory_value = os.environ.get("PRBOT_GLOBAL_CONCURRENCY_DIR", "").strip()
    if not directory_value:
        yield
        return
    limit = max(int(os.environ.get("PRBOT_GLOBAL_CONCURRENCY_LIMIT", "12")), 1)
    directory = Path(directory_value)
    directory.mkdir(parents=True, exist_ok=True)
    while True:
        for index in range(limit):
            handle = (directory / f"slot-{index:03d}.lock").open("a+", encoding="utf-8")
            try:
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                handle.close()
                continue
            try:
                yield
            finally:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
                handle.close()
            return
        time.sleep(0.05)


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


def prbot_revision() -> str:
    completed = subprocess.run(
        ["git", "rev-parse", "--short=12", "HEAD"],
        cwd=ROOT,
        capture_output=True,
        check=True,
        text=True,
    )
    return completed.stdout.strip()
