"""Shared benchmark statistics. Consumed unchanged by both renderers.

Every function here is pure: results in, numbers out. No model is ever in
the path of a value that appears in a report.

The most important function is `split_failure_classes`. Infrastructure
failures — a Docker timeout, an OOM, a provider 529, a scorer crash — are
not the model failing at the task. Averaging them in is how benchmark
numbers quietly become wrong, so every rate in a report is computed over
the scored set only, with the excluded count stated on the report's face.
"""

from __future__ import annotations

import statistics
from collections import defaultdict
from typing import Any, Iterable, Sequence

# A result is a dict with at least: item_id, attempt, success, failure_class.
Result = dict[str, Any]

INFRASTRUCTURE = "infrastructure"


def split_failure_classes(results: Iterable[Result]) -> tuple[list[Result], list[Result]]:
    """Return `(scored, excluded)`.

    `excluded` is every result whose `failure_class` is `infrastructure`.
    Callers compute all rates over `scored` and report `len(excluded)`.
    """
    scored, excluded = [], []
    for r in results:
        (excluded if r.get("failure_class") == INFRASTRUCTURE else scored).append(r)
    return scored, excluded


def group_by_item(results: Iterable[Result]) -> dict[str, list[Result]]:
    """Group results by `item_id`, each group ordered by `attempt`."""
    groups: dict[str, list[Result]] = defaultdict(list)
    for r in results:
        groups[r["item_id"]].append(r)
    for rs in groups.values():
        rs.sort(key=lambda r: r.get("attempt", 0))
    return dict(groups)


def pass_at_k(results: Iterable[Result], k: int) -> float:
    """Fraction of items solved within their first `k` attempts.

    `pass_at_k(results, 1)` is pass@1. Returns 0.0 for an empty set rather
    than raising — an empty benchmark scores zero, it is not an error here
    (the renderer reports the unit count alongside).
    """
    groups = group_by_item(results)
    if not groups:
        return 0.0
    solved = sum(
        1 for rs in groups.values() if any(r["success"] for r in rs[:k])
    )
    return solved / len(groups)


def flip_rate(results: Iterable[Result]) -> float:
    """Fraction of items that pass on some attempts and fail on others.

    A report that states only a mean hides this, and it is frequently the
    most informative statistic in the data: a 60% pass rate made of stable
    passes is a very different capability claim from one made of coin
    flips.
    """
    groups = group_by_item(results)
    if not groups:
        return 0.0
    flipped = 0
    for rs in groups.values():
        outcomes = {bool(r["success"]) for r in rs}
        if len(outcomes) > 1:
            flipped += 1
    return flipped / len(groups)


def stddev_by_item(results: Iterable[Result], field: str = "score") -> dict[str, float]:
    """Per-item standard deviation of `field` across attempts.

    Items with fewer than two attempts, or with a missing/None value, are
    omitted rather than reported as 0.0 — "no variance measured" and "no
    variance" are different claims.
    """
    out: dict[str, float] = {}
    for item_id, rs in group_by_item(results).items():
        vals = [r.get(field) for r in rs]
        vals = [v for v in vals if isinstance(v, (int, float))]
        if len(vals) >= 2:
            out[item_id] = statistics.stdev(vals)
    return out


def mean(values: Sequence[float]) -> float | None:
    """Arithmetic mean, or None for an empty sequence.

    None rather than 0.0 on purpose: "nothing was measured" must not read
    as "the measurement was zero".
    """
    vals = [v for v in values if isinstance(v, (int, float))]
    return sum(vals) / len(vals) if vals else None


def percentiles(values: Sequence[float], ps: Sequence[int]) -> dict[str, float]:
    """Nearest-rank percentiles, keyed `p50`, `p95`, ..."""
    vals = sorted(v for v in values if isinstance(v, (int, float)))
    if not vals:
        return {f"p{p}": 0.0 for p in ps}
    out = {}
    for p in ps:
        # Nearest-rank: index = ceil(p/100 * N) - 1, clamped.
        idx = max(0, min(len(vals) - 1, -(-p * len(vals) // 100) - 1))
        out[f"p{p}"] = float(vals[idx])
    return out


def tally(values: Iterable[str]) -> dict[str, int]:
    """Count occurrences, sorted by key so output is byte-stable."""
    counts: dict[str, int] = defaultdict(int)
    for v in values:
        counts[v] += 1
    return dict(sorted(counts.items()))


def markdown_table(headers: Sequence[str], rows: Sequence[Sequence[Any]]) -> str:
    """Render a GitHub-flavoured markdown table.

    Rows are emitted in the order given — callers sort first so the output
    is byte-stable across runs.
    """
    if not rows:
        return "_(none)_\n"
    head = "| " + " | ".join(str(h) for h in headers) + " |"
    sep = "|" + "|".join("---" for _ in headers) + "|"
    body = "\n".join(
        "| " + " | ".join("" if c is None else str(c) for c in row) + " |"
        for row in rows
    )
    return f"{head}\n{sep}\n{body}\n"


def fmt_pct(x: float | None) -> str:
    """Format a 0..1 fraction as a percentage, or `n/a` for None."""
    return "n/a" if x is None else f"{x * 100:.1f}%"


def fmt_num(x: float | None, places: int = 1) -> str:
    return "n/a" if x is None else f"{x:.{places}f}"
