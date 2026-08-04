"""Preflight must fail LOUDLY on every missing precondition.

The property under test throughout: a failure exits non-zero AND writes
nothing to stdout. A `run:` step binds stdout, so partial JSON on a failed
preflight would be bound downstream as if the check had passed.
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "preflight.py"


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args], capture_output=True, text=True
    )


class PreflightFailsLoudly(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def test_missing_data_dir_fails_loudly(self) -> None:
        r = run(
            "--data-dir",
            str(self.tmp / "nope"),
            "--server",
            "http://127.0.0.1:1",
            "--mask-map",
            str(self.tmp / "m.json"),
        )
        self.assertEqual(r.returncode, 1)
        self.assertIn("data-dir", r.stderr.lower())
        self.assertEqual(r.stdout.strip(), "", "no partial JSON on failure")

    def test_missing_data_dir_message_names_the_remedy(self) -> None:
        # An operator who hits this should not have to go read the README.
        r = run(
            "--data-dir",
            str(self.tmp / "nope"),
            "--server",
            "http://127.0.0.1:1",
            "--mask-map",
            str(self.tmp / "m.json"),
        )
        self.assertIn("git lfs", r.stderr)

    def test_missing_mask_map_fails_loudly(self) -> None:
        (self.tmp / "data").mkdir()
        r = run(
            "--data-dir",
            str(self.tmp / "data"),
            "--server",
            "http://127.0.0.1:1",
            "--mask-map",
            str(self.tmp / "absent.json"),
        )
        self.assertEqual(r.returncode, 1)
        self.assertIn("mask-map", r.stderr)
        self.assertEqual(r.stdout.strip(), "")

    def test_unreachable_server_fails_loudly(self) -> None:
        # Every check runs, so this assertion no longer depends on whether
        # docker happens to be installed on the machine running the tests.
        (self.tmp / "data").mkdir()
        (self.tmp / "m.json").write_text("{}")
        r = run(
            "--data-dir",
            str(self.tmp / "data"),
            "--server",
            "http://127.0.0.1:1",  # nothing listens here
            "--mask-map",
            str(self.tmp / "m.json"),
        )
        self.assertEqual(r.returncode, 1)
        self.assertIn("server", r.stderr.lower())
        self.assertEqual(r.stdout.strip(), "")

    def test_server_message_warns_about_localhost_binding(self) -> None:
        # Binding the server to localhost is the single most common
        # CyberGym setup mistake: containers cannot reach it.
        (self.tmp / "data").mkdir()
        (self.tmp / "m.json").write_text("{}")
        r = run(
            "--data-dir",
            str(self.tmp / "data"),
            "--server",
            "http://127.0.0.1:1",
            "--mask-map",
            str(self.tmp / "m.json"),
        )
        self.assertIn("gateway", r.stderr.lower())


class PreflightReportsEveryProblemAtOnce(unittest.TestCase):
    """An operator should get the whole list, not one item per failed run."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def test_multiple_failures_are_all_reported(self) -> None:
        # Both the data dir AND the mask map are missing here.
        r = run(
            "--data-dir",
            str(self.tmp / "nope"),
            "--server",
            "http://127.0.0.1:1",
            "--mask-map",
            str(self.tmp / "absent.json"),
        )
        self.assertEqual(r.returncode, 1)
        self.assertIn("data-dir", r.stderr)
        self.assertIn("mask-map", r.stderr)
        self.assertIn("server", r.stderr.lower())
        self.assertRegex(r.stderr, r"precondition\(s\) not met")

    def test_problems_are_numbered(self) -> None:
        r = run(
            "--data-dir",
            str(self.tmp / "nope"),
            "--server",
            "http://127.0.0.1:1",
            "--mask-map",
            str(self.tmp / "absent.json"),
        )
        self.assertIn("1.", r.stderr)
        self.assertIn("2.", r.stderr)


class PreflightEmitsParseableConfig(unittest.TestCase):
    def test_success_path_is_the_only_stdout_write(self) -> None:
        # Cannot reach success here without docker + cybergym + a live
        # server, so assert the contract structurally.
        src = SCRIPT.read_text()
        self.assertEqual(
            src.count("json.dump("), 1, "exactly one stdout write, on success"
        )
        self.assertIn("file=sys.stderr", src)


if __name__ == "__main__":
    unittest.main()
