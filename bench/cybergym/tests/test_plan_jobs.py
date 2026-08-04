"""Job planning: expansion, and the agent_id identity contract.

`agent_id` keys PoC submissions in the server's poc.db. Getting it wrong
does not fail loudly — it silently attributes one attempt's PoC to another
and corrupts both results. Hence the emphasis here.
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "plan_jobs.py"


class PlanJobsTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def plan(
        self,
        ids: list[str],
        attempts: int = 3,
        difficulty: str = "level1",
        salt: str = "s1",
        expect_ok: bool = True,
        extra: list[str] | None = None,
    ):
        f = self.tmp / "ids.txt"
        f.write_text("\n".join(ids) + "\n")
        r = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--task-ids-file",
                str(f),
                "--difficulty",
                difficulty,
                "--attempts",
                str(attempts),
                "--out-dir",
                str(self.tmp / "out"),
                "--run-salt",
                salt,
                *(extra or []),
            ],
            capture_output=True,
            text=True,
        )
        if not expect_ok:
            return r
        self.assertEqual(r.returncode, 0, r.stderr)
        return json.loads(r.stdout)["jobs"]

    # -- expansion ---------------------------------------------------------

    def test_expands_tasks_by_attempts(self) -> None:
        jobs = self.plan(["arvo:1", "arvo:2"], attempts=3)
        self.assertEqual(len(jobs), 6)
        self.assertEqual({j["task_id"] for j in jobs}, {"arvo:1", "arvo:2"})
        self.assertEqual(
            sorted(j["attempt"] for j in jobs if j["task_id"] == "arvo:1"),
            [1, 2, 3],
        )

    def test_job_ids_are_filesystem_safe(self) -> None:
        # CyberGym task ids contain ':' and can contain '/'.
        jobs = self.plan(["arvo:123", "oss-fuzz/abc"], attempts=1)
        for j in jobs:
            self.assertNotIn(":", j["job_id"])
            self.assertNotIn("/", j["job_id"])

    def test_out_dirs_are_unique_per_job(self) -> None:
        jobs = self.plan(["arvo:1", "arvo:2"], attempts=2)
        dirs = [j["out_dir"] for j in jobs]
        self.assertEqual(len(set(dirs)), len(dirs), "each job needs its own dir")

    def test_blank_lines_and_comments_ignored(self) -> None:
        f = self.tmp / "ids.txt"
        f.write_text("arvo:1\n\n# a comment\n  \narvo:2\n")
        r = subprocess.run(
            [
                sys.executable, str(SCRIPT),
                "--task-ids-file", str(f),
                "--difficulty", "level1",
                "--attempts", "1",
                "--out-dir", str(self.tmp / "o"),
                "--run-salt", "s",
            ],
            capture_output=True, text=True,
        )
        self.assertEqual(r.returncode, 0, r.stderr)
        jobs = json.loads(r.stdout)["jobs"]
        self.assertEqual([j["task_id"] for j in jobs], ["arvo:1", "arvo:2"])

    # -- the agent_id identity contract ------------------------------------

    def test_agent_ids_are_unique_per_attempt(self) -> None:
        jobs = self.plan(["arvo:1"], attempts=3)
        self.assertEqual(
            len({j["agent_id"] for j in jobs}),
            3,
            "each attempt needs its own poc.db identity",
        )

    def test_agent_ids_are_deterministic_for_the_same_salt(self) -> None:
        # This is what makes resume safe: the same job re-mints the same id.
        a = self.plan(["arvo:1"], attempts=3, salt="s1")
        b = self.plan(["arvo:1"], attempts=3, salt="s1")
        self.assertEqual([j["agent_id"] for j in a], [j["agent_id"] for j in b])

    def test_different_salt_gives_different_ids(self) -> None:
        # Two runs must not collide in poc.db.
        a = self.plan(["arvo:1"], attempts=1, salt="s1")
        b = self.plan(["arvo:1"], attempts=1, salt="s2")
        self.assertNotEqual(a[0]["agent_id"], b[0]["agent_id"])

    def test_different_difficulty_gives_different_ids(self) -> None:
        # The same task at level1 and level3 are different measurements and
        # must not share a poc.db identity.
        a = self.plan(["arvo:1"], attempts=1, difficulty="level1")
        b = self.plan(["arvo:1"], attempts=1, difficulty="level3")
        self.assertNotEqual(a[0]["agent_id"], b[0]["agent_id"])

    def test_agent_ids_are_globally_unique_across_tasks(self) -> None:
        jobs = self.plan(["arvo:1", "arvo:2", "arvo:3"], attempts=3)
        ids = [j["agent_id"] for j in jobs]
        self.assertEqual(len(set(ids)), len(ids))

    # -- loud failures -----------------------------------------------------

    def test_empty_task_list_fails_loudly(self) -> None:
        r = self.plan(["", "# only a comment"], expect_ok=False)
        self.assertEqual(r.returncode, 1)
        self.assertIn("no task ids", r.stderr)

    def test_zero_attempts_fails_loudly(self) -> None:
        r = self.plan(["arvo:1"], attempts=0, expect_ok=False)
        self.assertEqual(r.returncode, 1)
        self.assertIn("at least 1", r.stderr)

    def test_duplicate_task_ids_fail_loudly(self) -> None:
        # Duplicates would mint identical agent_ids and silently merge two
        # attempts' submissions in poc.db.
        r = self.plan(["arvo:1", "arvo:1"], expect_ok=False)
        self.assertEqual(r.returncode, 1)
        self.assertIn("duplicate", r.stderr)

    # -- disk handoff ------------------------------------------------------

    def test_write_flag_persists_jobs_json(self) -> None:
        dest = self.tmp / "run" / "jobs.json"
        self.plan(["arvo:1"], attempts=2, extra=["--write", str(dest)])
        self.assertTrue(dest.is_file())
        on_disk = json.loads(dest.read_text())
        self.assertEqual(len(on_disk["jobs"]), 2)

    def test_written_jobs_json_matches_stdout(self) -> None:
        dest = self.tmp / "jobs.json"
        jobs = self.plan(["arvo:1"], attempts=2, extra=["--write", str(dest)])
        self.assertEqual(json.loads(dest.read_text())["jobs"], jobs)


if __name__ == "__main__":
    unittest.main()
