"""Token and cost accounting, walked from rupu transcripts.

`StepResultRecord` carries no usage fields, so usage is summed from each
unit's transcript JSONL — `ItemResultRecord` records a per-unit
`transcript_path` plus the item JSON, which is what lets a token count
attribute to an exact eval item.

rupu emits one `Usage` event per assistant turn, with `input_tokens`,
`output_tokens`, `cached_tokens`, and the requested-vs-served model
recorded separately.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

ZERO = {
    "input_tokens": 0,
    "output_tokens": 0,
    "cached_tokens": 0,
    "turns": 0,
    "tool_calls": 0,
}


def _event_type(rec: dict[str, Any]) -> str:
    """rupu transcript events are tagged either `type` or as a single key."""
    if "type" in rec:
        return str(rec["type"])
    # Externally-tagged enum form: {"Usage": {...}}
    if len(rec) == 1:
        return next(iter(rec))
    return ""


def _payload(rec: dict[str, Any]) -> dict[str, Any]:
    if "type" in rec:
        return rec
    if len(rec) == 1:
        inner = next(iter(rec.values()))
        return inner if isinstance(inner, dict) else {}
    return {}


def sum_usage(transcript_path: str | Path) -> dict[str, int]:
    """Sum every `Usage` event in one transcript.

    A missing transcript returns zeros rather than raising: a unit that was
    refused before dispatch legitimately has none, and a whole report should
    not fail over it. Malformed lines are skipped for the same reason —
    one bad line must not cost the other 599 units' accounting.
    """
    p = Path(transcript_path)
    if not p.is_file():
        return dict(ZERO)

    totals = dict(ZERO)
    for line in p.read_text(errors="ignore").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(rec, dict):
            continue

        kind = _event_type(rec)
        body = _payload(rec)
        if kind == "Usage":
            totals["turns"] += 1
            for field in ("input_tokens", "output_tokens", "cached_tokens"):
                v = body.get(field)
                if isinstance(v, int):
                    totals[field] += v
        elif kind in ("ToolCall", "ToolUse"):
            totals["tool_calls"] += 1

    return totals


def add_usage(a: dict[str, int], b: dict[str, int]) -> dict[str, int]:
    """Element-wise sum of two usage dicts."""
    return {k: a.get(k, 0) + b.get(k, 0) for k in ZERO}


def apply_pricing(
    usage: dict[str, int], price_table: dict[str, float] | None
) -> dict[str, float | None]:
    """Cost in USD from a price table, or `None` when none is declared.

    `None` rather than 0.0 on purpose. Reporting "$0.00 spent" when no
    price was declared states a fact that was never measured; cybermark's
    own harness profiles leave `pricing:` unset for preview models
    precisely so an unpriced run cannot be mistaken for a free one.
    """
    if not price_table:
        return {"cost_usd": None}
    inp = price_table.get("input_per_million")
    out = price_table.get("output_per_million")
    if inp is None or out is None:
        return {"cost_usd": None}
    cost = (
        usage.get("input_tokens", 0) / 1_000_000 * float(inp)
        + usage.get("output_tokens", 0) / 1_000_000 * float(out)
    )
    return {"cost_usd": round(cost, 6)}
