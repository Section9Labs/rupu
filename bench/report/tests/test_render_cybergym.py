"""CyberGym report rendering.

Determinism and the infrastructure-exclusion disclosure are the two
properties worth defending: a report whose numbers shift between identical
runs cannot be compared over time, and one that hides its exclusions reads
as more complete than it is.
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "render_cybergym.py"
REPO = Path(__file__).resolve().parents[3]


def v(job_id, outcome, fc):
    return {"job_id": job_id, "agent_id": "x", "outcome": outcome,
            "failure_class": fc, "raw": {}}


class RenderTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def render(self, verdicts, jobs=None, provenance=None, difficulty="level1"):
        jobs = jobs or [
            {
                "job_id": x["job_id"],
                "task_id": x["job_id"].split("__a")[0],
                "difficulty": difficulty,
                "attempt": int(x["job_id"].split("__a")[1]),
                "agent_id": "x",
                "out_dir": str(self.tmp),
            }
            for x in verdicts
        ]
        (self.tmp / "jobs.json").write_text(json.dumps({"jobs": jobs}))
        (self.tmp / "verdicts.json").write_text(json.dumps(verdicts))
        (self.tmp / "prov.json").write_text(json.dumps(provenance or {
            "run_id": "R1", "rupu_version": "0.72.0",
            "requested_model": "m", "served_model": "m",
            "difficulty": difficulty, "attempts": 2,
        }))
        r = subprocess.run(
            [sys.executable, str(SCRIPT),
             "--jobs", str(self.tmp / "jobs.json"),
             "--verdicts", str(self.tmp / "verdicts.json"),
             "--run-dir", str(self.tmp),
             "--provenance", str(self.tmp / "prov.json")],
            capture_output=True, text=True, cwd=str(REPO),
        )
        self.assertEqual(r.returncode, 0, r.stderr)
        return json.loads((self.tmp / "report.json").read_text())

    # -- outcome -----------------------------------------------------------

    def test_reports_pass_at_1_and_pass_at_k(self) -> None:
        rep = self.render([
            v("t1__a1", "no_crash", "capability"), v("t1__a2", "success", "none"),
            v("t2__a1", "no_crash", "capability"), v("t2__a2", "no_crash", "capability"),
        ])
        self.assertEqual(rep["outcome"]["pass_at_1"], 0.0)
        self.assertEqual(rep["outcome"]["pass_at_k"], 0.5)

    def test_failure_taxonomy_is_broken_out(self) -> None:
        rep = self.render([
            v("t1__a1", "crashes_both", "capability"),
            v("t2__a1", "no_submission", "capability"),
        ])
        tax = rep["outcome"]["failure_taxonomy"]
        self.assertEqual(tax["crashes_both"], 1)
        self.assertEqual(tax["no_submission"], 1)

    def test_taxonomy_shows_zero_rows_not_missing_keys(self) -> None:
        # A missing category reads as "not applicable"; a 0 reads as "none".
        rep = self.render([v("t1__a1", "success", "none")])
        for outcome in ("success", "no_crash", "crashes_both",
                        "malformed", "no_submission"):
            self.assertIn(outcome, rep["outcome"]["failure_taxonomy"])

    def test_crashes_both_does_not_count_as_success(self) -> None:
        rep = self.render([v("t1__a1", "crashes_both", "capability")])
        self.assertEqual(rep["outcome"]["pass_at_1"], 0.0)

    # -- infrastructure exclusion -----------------------------------------

    def test_infrastructure_failures_excluded_and_stated(self) -> None:
        rep = self.render([
            v("t1__a1", "success", "none"),
            v("t2__a1", "malformed", "infrastructure"),
        ])
        self.assertEqual(rep["outcome"]["pass_at_1"], 1.0)
        self.assertEqual(rep["excluded"]["infrastructure"], 1)
        md = (self.tmp / "report.md").read_text()
        self.assertIn("1 unit(s) excluded", md,
                      "the exclusion must be on the face of the report")

    def test_zero_exclusions_is_still_stated(self) -> None:
        # Silence would be ambiguous: did nothing get excluded, or was the
        # exclusion count simply not reported?
        self.render([v("t1__a1", "success", "none")])
        self.assertIn("0 unit(s) excluded", (self.tmp / "report.md").read_text())

    def test_excluded_job_ids_are_listed(self) -> None:
        rep = self.render([
            v("t1__a1", "success", "none"),
            v("t2__a1", "malformed", "infrastructure"),
        ])
        self.assertEqual(rep["excluded"]["job_ids"], ["t2__a1"])

    def test_job_with_no_verdict_is_infrastructure_not_a_miss(self) -> None:
        jobs = [{"job_id": "t1__a1", "task_id": "t1", "difficulty": "level1",
                 "attempt": 1, "agent_id": "x", "out_dir": str(self.tmp)}]
        rep = self.render([], jobs=jobs)
        self.assertEqual(rep["excluded"]["infrastructure"], 1)

    # -- determinism -------------------------------------------------------

    def test_report_json_is_byte_identical_across_runs(self) -> None:
        verdicts = [v("t1__a1", "success", "none"), v("t1__a2", "no_crash", "capability")]
        self.render(verdicts)
        first = (self.tmp / "report.json").read_text()
        self.render(verdicts)
        self.assertEqual(first, (self.tmp / "report.json").read_text())

    def test_results_jsonl_has_one_line_per_job(self) -> None:
        self.render([v("t1__a1", "success", "none"),
                     v("t1__a2", "no_crash", "capability")])
        lines = (self.tmp / "results.jsonl").read_text().strip().splitlines()
        self.assertEqual(len(lines), 2)
        for line in lines:
            self.assertIn("failure_class", json.loads(line))

    # -- report surface ----------------------------------------------------

    def test_markdown_ends_with_an_empty_analysis_heading(self) -> None:
        # The analyst agent appends here; the renderer owns everything above.
        self.render([v("t1__a1", "success", "none")])
        self.assertTrue((self.tmp / "report.md").read_text().rstrip().endswith("## Analysis"))

    def test_unpriced_run_says_na_not_zero(self) -> None:
        self.render([v("t1__a1", "success", "none")])
        self.assertIn("n/a (no price table declared)", (self.tmp / "report.md").read_text())

    def test_provenance_is_carried_into_the_report(self) -> None:
        rep = self.render([v("t1__a1", "success", "none")])
        self.assertEqual(rep["provenance"]["requested_model"], "m")
        self.assertEqual(rep["provenance"]["served_model"], "m")


if __name__ == "__main__":
    unittest.main()
