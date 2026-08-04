#!/usr/bin/env python3
"""Expand a CyberGym task-id list into one job record per (task, attempt).

`agent_id` is derived deterministically from (task, difficulty, attempt,
run_salt) so a resumed run reuses the same identity in the server's poc.db,
while a fresh run with a new salt never collides with a previous run's
submissions. Collisions would silently attribute one run's PoC to another
and corrupt both results.

Emits `{"jobs": [...]}` on stdout; also writes jobs.json when --write is
given, because the report renderer reads it from disk.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path


def read_task_ids(path: str) -> list[str]:
    """One id per line. Blank lines and `#` comments are ignored."""
    raw = Path(path).read_text().splitlines()
    return [
        line.strip()
        for line in raw
        if line.strip() and not line.strip().startswith("#")
    ]


def mint_agent_id(task_id: str, difficulty: str, attempt: int, run_salt: str) -> str:
    """Deterministic per (task, difficulty, attempt, salt).

    The server keys PoC submissions by this, so it must be stable across a
    resume (same inputs => same id) and distinct across runs (new salt =>
    new id).
    """
    key = f"{task_id}|{difficulty}|{attempt}|{run_salt}"
    return hashlib.sha256(key.encode()).hexdigest()[:32]


def safe_job_id(task_id: str, attempt: int) -> str:
    """A filesystem-safe job id. CyberGym task ids contain ':' and '/'."""
    slug = task_id.replace(":", "_").replace("/", "_")
    return f"{slug}__a{attempt}"


def build_jobs(
    task_ids: list[str],
    difficulty: str,
    attempts: int,
    out_root: Path,
    run_salt: str,
) -> list[dict]:
    jobs = []
    for task_id in task_ids:
        for attempt in range(1, attempts + 1):
            job_id = safe_job_id(task_id, attempt)
            jobs.append(
                {
                    "job_id": job_id,
                    "task_id": task_id,
                    "difficulty": difficulty,
                    "attempt": attempt,
                    "out_dir": str(out_root / job_id),
                    "agent_id": mint_agent_id(task_id, difficulty, attempt, run_salt),
                }
            )
    return jobs


def main() -> None:
    p = argparse.ArgumentParser(description="Expand CyberGym tasks into jobs.")
    p.add_argument("--task-ids-file", required=True)
    p.add_argument("--difficulty", required=True)
    p.add_argument("--attempts", type=int, required=True)
    p.add_argument("--out-dir", required=True)
    p.add_argument(
        "--run-salt",
        required=True,
        help="distinguishes this run's poc.db identities from any previous "
        "run's; reuse it to resume, change it for a fresh run",
    )
    p.add_argument(
        "--write",
        help="also write jobs.json here (the report renderer reads it from disk)",
    )
    a = p.parse_args()

    if a.attempts < 1:
        print("plan_jobs: --attempts must be at least 1", file=sys.stderr)
        sys.exit(1)

    task_ids = read_task_ids(a.task_ids_file)
    if not task_ids:
        print(
            f"plan_jobs: {a.task_ids_file} contains no task ids "
            f"(blank lines and # comments are ignored)",
            file=sys.stderr,
        )
        sys.exit(1)

    duplicates = {t for t in task_ids if task_ids.count(t) > 1}
    if duplicates:
        # Two jobs for the same (task, attempt) would mint the SAME agent_id
        # and collide in poc.db, silently merging two attempts' submissions.
        print(
            f"plan_jobs: duplicate task ids in {a.task_ids_file}: "
            f"{sorted(duplicates)}",
            file=sys.stderr,
        )
        sys.exit(1)

    jobs = build_jobs(
        task_ids, a.difficulty, a.attempts, Path(a.out_dir), a.run_salt
    )
    payload = {"jobs": jobs}

    if a.write:
        dest = Path(a.write)
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(json.dumps(payload, indent=2, sort_keys=True))

    json.dump(payload, sys.stdout)


if __name__ == "__main__":
    main()
