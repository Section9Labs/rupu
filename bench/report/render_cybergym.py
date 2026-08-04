#!/usr/bin/env python3
"""Render a CyberGym benchmark report.

Every number here is computed by code. The `analyze` workflow step appends
prose to report.md afterwards and cannot alter a value — report.json is
written once, here, and is byte-identical before and after that step.

Writes into `--run-dir`:
    report.json     canonical, machine-readable
    report.md       human-readable, with a trailing empty `## Analysis`
    results.jsonl   one raw record per job
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from bench.report.core import (  # noqa: E402
    fmt_num,
    fmt_pct,
    markdown_table,
    mean,
    pass_at_k,
    percentiles,
    flip_rate,
    split_failure_classes,
    tally,
)
from bench.report.usage import add_usage, apply_pricing, sum_usage, ZERO  # noqa: E402

# Every outcome verify_job.py can emit. Listed explicitly so a taxonomy
# table always shows a 0 row rather than silently omitting a category.
OUTCOMES = ["success", "no_crash", "crashes_both", "malformed", "no_submission"]


def load(path: str):
    return json.loads(Path(path).read_text())


def build_results(jobs: list[dict], verdicts: list[dict]) -> list[dict]:
    """Join jobs to verdicts on job_id, into the shape core.py consumes."""
    by_id = {v["job_id"]: v for v in verdicts}
    results = []
    for job in jobs:
        v = by_id.get(job["job_id"])
        if v is None:
            # A job with no verdict never got verified — an infrastructure
            # problem, not a capability failure.
            v = {
                "outcome": "malformed",
                "failure_class": "infrastructure",
                "raw": {"reason": "no verdict recorded for this job"},
            }
        usage = sum_usage(job.get("transcript_path", ""))
        results.append(
            {
                "item_id": job["task_id"],
                "job_id": job["job_id"],
                "attempt": job["attempt"],
                "difficulty": job.get("difficulty", "unknown"),
                "success": v["outcome"] == "success",
                "outcome": v["outcome"],
                "failure_class": v["failure_class"],
                "usage": usage,
                "duration_ms": job.get("duration_ms"),
                "raw": v.get("raw", {}),
            }
        )
    results.sort(key=lambda r: (r["item_id"], r["attempt"]))
    return results


def by_difficulty(scored: list[dict], k: int) -> list[list]:
    rows = []
    for diff in sorted({r["difficulty"] for r in scored}):
        subset = [r for r in scored if r["difficulty"] == diff]
        items = len({r["item_id"] for r in subset})
        rows.append(
            [
                diff,
                items,
                fmt_pct(pass_at_k(subset, 1)),
                fmt_pct(pass_at_k(subset, k)),
            ]
        )
    return rows


def build_report(jobs: list[dict], verdicts: list[dict], provenance: dict) -> tuple[dict, list[dict]]:
    results = build_results(jobs, verdicts)
    scored, excluded = split_failure_classes(results)
    k = int(provenance.get("attempts", 1) or 1)

    total_usage = dict(ZERO)
    for r in results:
        total_usage = add_usage(total_usage, r["usage"])
    cost = apply_pricing(total_usage, provenance.get("pricing"))

    durations = [r["duration_ms"] for r in scored if isinstance(r["duration_ms"], (int, float))]

    report = {
        "schema_version": 1,
        "benchmark": "cybergym",
        "provenance": dict(sorted(provenance.items())),
        "units": {
            "total": len(results),
            "scored": len(scored),
            "items": len({r["item_id"] for r in scored}),
        },
        "outcome": {
            "pass_at_1": pass_at_k(scored, 1),
            "pass_at_k": pass_at_k(scored, k),
            "k": k,
            "failure_taxonomy": {
                o: tally(r["outcome"] for r in scored).get(o, 0) for o in OUTCOMES
            },
            "by_difficulty": {
                diff: {
                    "pass_at_1": pass_at_k(
                        [r for r in scored if r["difficulty"] == diff], 1
                    ),
                    "pass_at_k": pass_at_k(
                        [r for r in scored if r["difficulty"] == diff], k
                    ),
                }
                for diff in sorted({r["difficulty"] for r in scored})
            },
        },
        "reliability": {
            "flip_rate": flip_rate(scored),
        },
        "efficiency": {
            "tokens": total_usage,
            "cost_usd": cost["cost_usd"],
            "duration_ms": percentiles(durations, [50, 95]),
        },
        "excluded": {
            "infrastructure": len(excluded),
            "job_ids": sorted(r["job_id"] for r in excluded),
        },
    }
    return report, results


def render_markdown(report: dict) -> str:
    o = report["outcome"]
    u = report["units"]
    ex = report["excluded"]["infrastructure"]

    lines = [
        "# CyberGym benchmark report",
        "",
        "## Provenance",
        "",
        markdown_table(
            ["field", "value"],
            [[k, v] for k, v in sorted(report["provenance"].items())],
        ),
        "## Headline",
        "",
        markdown_table(
            ["metric", "value"],
            [
                ["items", u["items"]],
                ["units scored", u["scored"]],
                ["pass@1", fmt_pct(o["pass_at_1"])],
                [f"pass@{o['k']}", fmt_pct(o["pass_at_k"])],
                ["flip rate", fmt_pct(report["reliability"]["flip_rate"])],
            ],
        ),
    ]

    if ex:
        lines += [
            f"> **{ex} unit(s) excluded as infrastructure failures.** "
            f"Rates above are computed over the remaining {u['scored']} "
            f"unit(s). Infrastructure failures are not model failures.",
            "",
        ]
    else:
        lines += ["> 0 unit(s) excluded as infrastructure failures.", ""]

    lines += [
        "## Success is strict",
        "",
        "A PoC counts only when it crashes the **vulnerable** build and does "
        "**not** crash the patched one. `crashes_both` found a crash, but not "
        "this vulnerability.",
        "",
        "## Failure taxonomy",
        "",
        markdown_table(
            ["outcome", "units"],
            [[k, v] for k, v in sorted(o["failure_taxonomy"].items())],
        ),
        "## By difficulty",
        "",
        markdown_table(
            ["difficulty", "pass@1", f"pass@{o['k']}"],
            [
                [d, fmt_pct(v["pass_at_1"]), fmt_pct(v["pass_at_k"])]
                for d, v in sorted(o["by_difficulty"].items())
            ],
        ),
        "## Efficiency",
        "",
        markdown_table(
            ["metric", "value"],
            [
                ["input tokens", report["efficiency"]["tokens"]["input_tokens"]],
                ["output tokens", report["efficiency"]["tokens"]["output_tokens"]],
                ["cached tokens", report["efficiency"]["tokens"]["cached_tokens"]],
                ["turns", report["efficiency"]["tokens"]["turns"]],
                ["tool calls", report["efficiency"]["tokens"]["tool_calls"]],
                [
                    "cost (USD)",
                    "n/a (no price table declared)"
                    if report["efficiency"]["cost_usd"] is None
                    else fmt_num(report["efficiency"]["cost_usd"], 4),
                ],
                ["duration p50 (ms)", fmt_num(report["efficiency"]["duration_ms"]["p50"], 0)],
                ["duration p95 (ms)", fmt_num(report["efficiency"]["duration_ms"]["p95"], 0)],
            ],
        ),
        "## Analysis",
        "",
    ]
    return "\n".join(lines)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--jobs", required=True)
    p.add_argument("--verdicts", required=True)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--provenance", required=True)
    a = p.parse_args()

    jobs_doc = load(a.jobs)
    jobs = jobs_doc["jobs"] if isinstance(jobs_doc, dict) else jobs_doc
    verdicts_doc = load(a.verdicts)
    verdicts = verdicts_doc["verdicts"] if isinstance(verdicts_doc, dict) else verdicts_doc
    provenance = load(a.provenance)

    report, results = build_report(jobs, verdicts, provenance)

    run_dir = Path(a.run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    # sort_keys so the same inputs always render byte-identical output.
    (run_dir / "report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n"
    )
    (run_dir / "report.md").write_text(render_markdown(report))
    (run_dir / "results.jsonl").write_text(
        "".join(json.dumps(r, sort_keys=True) + "\n" for r in results)
    )
    print(json.dumps({"report_json": str(run_dir / "report.json")}))


if __name__ == "__main__":
    main()
