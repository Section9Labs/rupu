#!/usr/bin/env python3
"""Collect per-job verdict files into the single verdicts.json the renderer reads.

A `run:` step has no stdin (the executor sets it to null) and passing 600
verdicts as one argv string would risk ARG_MAX, so the fan-out's units each
write `<job_id>.json` and this globs them.

Every planned job must produce a verdict. A job with no file on disk is
reported as an infrastructure failure rather than dropped — a silently
missing verdict would be scored as a capability miss, understating the
model for what is actually a harness problem.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def collect(verdict_dir: Path, jobs: list[dict]) -> list[dict]:
    verdicts = []
    for job in jobs:
        f = verdict_dir / f"{job['job_id']}.json"
        if not f.is_file():
            verdicts.append(
                {
                    "job_id": job["job_id"],
                    "agent_id": job.get("agent_id", ""),
                    "outcome": "malformed",
                    "failure_class": "infrastructure",
                    "raw": {"reason": f"no verdict file at {f}"},
                }
            )
            continue
        try:
            verdicts.append(json.loads(f.read_text()))
        except json.JSONDecodeError as e:
            verdicts.append(
                {
                    "job_id": job["job_id"],
                    "agent_id": job.get("agent_id", ""),
                    "outcome": "malformed",
                    "failure_class": "infrastructure",
                    "raw": {"parse_error": str(e), "path": str(f)},
                }
            )
    return verdicts


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--verdict-dir", required=True)
    p.add_argument("--jobs", required=True, help="jobs.json, the authoritative job list")
    p.add_argument("--out", required=True)
    a = p.parse_args()

    jobs_doc = json.loads(Path(a.jobs).read_text())
    jobs = jobs_doc["jobs"] if isinstance(jobs_doc, dict) else jobs_doc
    verdicts = collect(Path(a.verdict_dir), jobs)

    dest = Path(a.out)
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps(verdicts, indent=2, sort_keys=True))

    missing = sum(1 for v in verdicts if v["failure_class"] == "infrastructure")
    if missing:
        print(
            f"collect_verdicts: {missing} of {len(jobs)} job(s) had no usable "
            f"verdict; recorded as infrastructure failures",
            file=sys.stderr,
        )
    json.dump({"verdicts": len(verdicts), "missing": missing, "path": str(dest)}, sys.stdout)


if __name__ == "__main__":
    main()
