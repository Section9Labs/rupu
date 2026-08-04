"""Shared benchmark statistics.

The load-bearing test here is the infrastructure/capability split: it is
the difference between a benchmark number that means something and one
that quietly doesn't.
"""

import unittest

from bench.report.core import (
    flip_rate,
    group_by_item,
    markdown_table,
    mean,
    pass_at_k,
    percentiles,
    split_failure_classes,
    stddev_by_item,
    tally,
)


def r(item, attempt, ok, fc="none", score=None):
    d = {"item_id": item, "attempt": attempt, "success": ok, "failure_class": fc}
    if score is not None:
        d["score"] = score
    return d


class PassAtK(unittest.TestCase):
    def test_pass_at_1_uses_only_the_first_attempt(self) -> None:
        results = [r("a", 1, False), r("a", 2, True), r("b", 1, True), r("b", 2, True)]
        self.assertEqual(pass_at_k(results, 1), 0.5)

    def test_pass_at_k_counts_an_item_solved_on_any_attempt(self) -> None:
        results = [r("a", 1, False), r("a", 2, True), r("b", 1, False), r("b", 2, False)]
        self.assertEqual(pass_at_k(results, 2), 0.5)

    def test_pass_at_k_respects_attempt_order_not_list_order(self) -> None:
        # Attempt 2 succeeded; pass@1 must still be 0.
        results = [r("a", 2, True), r("a", 1, False)]
        self.assertEqual(pass_at_k(results, 1), 0.0)
        self.assertEqual(pass_at_k(results, 2), 1.0)

    def test_empty_is_zero_not_an_error(self) -> None:
        self.assertEqual(pass_at_k([], 1), 0.0)


class FlipRate(unittest.TestCase):
    def test_finds_items_that_pass_sometimes(self) -> None:
        results = [
            r("a", 1, True), r("a", 2, False),    # flips
            r("b", 1, True), r("b", 2, True),     # stable pass
            r("c", 1, False), r("c", 2, False),   # stable fail
        ]
        self.assertAlmostEqual(flip_rate(results), 1 / 3)

    def test_single_attempt_items_never_flip(self) -> None:
        self.assertEqual(flip_rate([r("a", 1, True), r("b", 1, False)]), 0.0)


class FailureClassSplit(unittest.TestCase):
    def test_infrastructure_failures_are_excluded_from_rates(self) -> None:
        # THE property that keeps benchmark numbers honest.
        results = [r("a", 1, True), r("b", 1, False, fc="infrastructure")]
        scored, excluded = split_failure_classes(results)
        self.assertEqual(len(scored), 1)
        self.assertEqual(len(excluded), 1)
        self.assertEqual(
            pass_at_k(scored, 1), 1.0, "a docker timeout must not count as a miss"
        )

    def test_capability_failures_are_kept(self) -> None:
        results = [r("a", 1, True), r("b", 1, False, fc="capability")]
        scored, excluded = split_failure_classes(results)
        self.assertEqual(len(scored), 2)
        self.assertEqual(excluded, [])
        self.assertEqual(pass_at_k(scored, 1), 0.5)

    def test_without_the_split_the_number_would_be_wrong(self) -> None:
        # Demonstrates the failure mode the split prevents.
        results = [r("a", 1, True), r("b", 1, False, fc="infrastructure")]
        naive = pass_at_k(results, 1)
        scored, _ = split_failure_classes(results)
        self.assertEqual(naive, 0.5)
        self.assertEqual(pass_at_k(scored, 1), 1.0)


class Stats(unittest.TestCase):
    def test_stddev_by_item_needs_two_attempts(self) -> None:
        out = stddev_by_item([r("a", 1, True, score=90)])
        self.assertNotIn("a", out, "one sample measures no variance")

    def test_stddev_by_item_computed_across_attempts(self) -> None:
        out = stddev_by_item([r("a", 1, True, score=90), r("a", 2, True, score=70)])
        self.assertAlmostEqual(out["a"], 14.142135, places=4)

    def test_mean_of_empty_is_none_not_zero(self) -> None:
        self.assertIsNone(mean([]))

    def test_percentiles(self) -> None:
        p = percentiles([1, 2, 3, 4, 100], [50, 95])
        self.assertEqual(p["p50"], 3)
        self.assertEqual(p["p95"], 100)

    def test_percentiles_of_empty_are_zero(self) -> None:
        self.assertEqual(percentiles([], [50])["p50"], 0.0)

    def test_tally_is_sorted_for_stable_output(self) -> None:
        self.assertEqual(
            list(tally(["b", "a", "b"]).items()), [("a", 1), ("b", 2)]
        )


class Rendering(unittest.TestCase):
    def test_markdown_table(self) -> None:
        out = markdown_table(["a", "b"], [[1, 2], [3, 4]])
        self.assertIn("| a | b |", out)
        self.assertIn("| 1 | 2 |", out)

    def test_empty_table_says_so(self) -> None:
        self.assertIn("none", markdown_table(["a"], []))

    def test_group_by_item_orders_by_attempt(self) -> None:
        g = group_by_item([r("a", 3, True), r("a", 1, True), r("a", 2, True)])
        self.assertEqual([x["attempt"] for x in g["a"]], [1, 2, 3])


if __name__ == "__main__":
    unittest.main()
