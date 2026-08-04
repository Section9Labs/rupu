"""Token accounting walked from rupu transcripts."""

import json
import tempfile
import unittest
from pathlib import Path

from bench.report.usage import add_usage, apply_pricing, sum_usage


class SumUsage(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def write(self, events) -> Path:
        p = self.tmp / "run.jsonl"
        p.write_text("\n".join(json.dumps(e) for e in events))
        return p

    def test_sums_usage_events_across_turns(self) -> None:
        t = self.write([
            {"type": "Usage", "input_tokens": 100, "output_tokens": 50, "cached_tokens": 10},
            {"type": "ToolCall", "name": "bash"},
            {"type": "Usage", "input_tokens": 200, "output_tokens": 80, "cached_tokens": 0},
        ])
        u = sum_usage(t)
        self.assertEqual(u["input_tokens"], 300)
        self.assertEqual(u["output_tokens"], 130)
        self.assertEqual(u["cached_tokens"], 10)
        self.assertEqual(u["turns"], 2)
        self.assertEqual(u["tool_calls"], 1)

    def test_handles_externally_tagged_events(self) -> None:
        # rupu's Event enum can serialize as {"Usage": {...}}.
        t = self.write([{"Usage": {"input_tokens": 5, "output_tokens": 7, "cached_tokens": 0}}])
        u = sum_usage(t)
        self.assertEqual(u["input_tokens"], 5)
        self.assertEqual(u["turns"], 1)

    def test_missing_transcript_returns_zeros_not_an_error(self) -> None:
        # A refused unit legitimately has no transcript; one absent file
        # must not fail a 600-unit report.
        u = sum_usage(self.tmp / "absent.jsonl")
        self.assertEqual(u["input_tokens"], 0)
        self.assertEqual(u["turns"], 0)

    def test_malformed_lines_are_skipped_not_fatal(self) -> None:
        p = self.tmp / "run.jsonl"
        p.write_text(
            '{"type":"Usage","input_tokens":1,"output_tokens":1,"cached_tokens":0}\n'
            "{ this is not json\n"
            '{"type":"Usage","input_tokens":2,"output_tokens":2,"cached_tokens":0}\n'
        )
        u = sum_usage(p)
        self.assertEqual(u["input_tokens"], 3, "one bad line must not cost the rest")


class Pricing(unittest.TestCase):
    def test_pricing_applied_per_million(self) -> None:
        u = {"input_tokens": 1_000_000, "output_tokens": 500_000, "cached_tokens": 0}
        c = apply_pricing(u, {"input_per_million": 3.0, "output_per_million": 15.0})
        self.assertAlmostEqual(c["cost_usd"], 3.0 + 7.5)

    def test_absent_price_table_yields_none_not_zero(self) -> None:
        # "$0.00 spent" would state a fact that was never measured.
        c = apply_pricing({"input_tokens": 1, "output_tokens": 1, "cached_tokens": 0}, None)
        self.assertIsNone(c["cost_usd"])

    def test_partial_price_table_yields_none(self) -> None:
        c = apply_pricing(
            {"input_tokens": 1, "output_tokens": 1, "cached_tokens": 0},
            {"input_per_million": 3.0},
        )
        self.assertIsNone(c["cost_usd"])

    def test_add_usage(self) -> None:
        a = {"input_tokens": 1, "output_tokens": 2, "cached_tokens": 3, "turns": 1, "tool_calls": 0}
        self.assertEqual(add_usage(a, a)["input_tokens"], 2)
        self.assertEqual(add_usage(a, a)["turns"], 2)


if __name__ == "__main__":
    unittest.main()
