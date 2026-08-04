#!/usr/bin/env python3
"""Assert every CyberGym precondition before a benchmark run starts.

Reports *all* missing preconditions in one pass, not just the first. An
operator setting CyberGym up should get the full list once rather than
discover them one failed run at a time.

Exits non-zero with actionable messages rather than letting a run proceed
and produce a partial, misleading result set — a benchmark that silently
skipped half its tasks would report a number that looks real and isn't.

Emits resolved config as JSON on stdout; diagnostics go to stderr. Nothing
is written to stdout unless every check passed, because a `run:` step binds
stdout and partial JSON would be consumed downstream as if it were valid.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path


class Checks:
    """Accumulates failures so they can all be reported together."""

    def __init__(self) -> None:
        self.problems: list[str] = []
        self.info: dict[str, object] = {}

    def fail(self, msg: str) -> None:
        self.problems.append(msg)

    def data_dir(self, path: str) -> None:
        d = Path(path)
        if not d.is_dir():
            self.fail(
                f"--data-dir {d} does not exist. Clone the dataset first:\n"
                f"    git lfs install && "
                f"git clone https://huggingface.co/datasets/sunblaze-ucb/cybergym"
            )
            return
        self.info["data_dir"] = str(d.resolve())

    def mask_map(self, path: str) -> None:
        m = Path(path)
        if not m.is_file():
            self.fail(f"--mask-map {m} does not exist or is not a file")
            return
        self.info["mask_map"] = str(m.resolve())

    def docker(self) -> None:
        if shutil.which("docker") is None:
            self.fail("docker is not on PATH; CyberGym needs Docker to build and run PoCs")
            return
        proc = subprocess.run(
            ["docker", "version", "--format", "{{.Server.Version}}"],
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0:
            self.fail(
                "the docker daemon is not reachable; start Docker Desktop and retry "
                f"(docker said: {proc.stderr.strip()})"
            )
            return
        self.info["docker_version"] = proc.stdout.strip()

    def cybergym(self) -> None:
        proc = subprocess.run(
            [
                sys.executable,
                "-c",
                "import cybergym, sys; "
                "sys.stdout.write(getattr(cybergym, '__version__', 'unknown'))",
            ],
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0:
            self.fail(
                "`cybergym` is not importable by this interpreter "
                f"({sys.executable}); run: pip3 install -e '.[dev,server]'"
            )
            return
        self.info["cybergym_version"] = proc.stdout.strip()

    def server(self, url: str, timeout: float = 5.0) -> None:
        try:
            with urllib.request.urlopen(url, timeout=timeout) as resp:
                resp.read(1)
        except Exception as e:  # noqa: BLE001 — any failure is a hard stop
            self.fail(
                f"the PoC server at {url} is not answering ({e}).\n"
                f"    Start it with: python3 -m cybergym.server "
                f"--host <docker-gateway> --port <port> ...\n"
                f"    Bind it to the Docker gateway (e.g. 172.17.0.1), NOT "
                f"localhost — containers cannot reach the host's localhost."
            )
            return
        self.info["server"] = url

    def firewall(self, enabled: bool) -> None:
        self.info["firewall"] = enabled
        if not enabled:
            return
        proc = subprocess.run(
            [sys.executable, "-m", "cybergym.firewall", "start"],
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0:
            self.fail(f"--firewall was requested but it failed to start: {proc.stderr.strip()}")


def main() -> None:
    p = argparse.ArgumentParser(description="CyberGym preflight checks.")
    p.add_argument("--data-dir", required=True)
    p.add_argument("--server", required=True)
    p.add_argument("--mask-map", required=True)
    p.add_argument(
        "--firewall",
        action="store_true",
        help="start the CyberGym squid-allowlist firewall so solver agents "
        "cannot reach the public internet to look up the CVE they are "
        "supposed to be discovering",
    )
    a = p.parse_args()

    c = Checks()
    c.data_dir(a.data_dir)
    c.mask_map(a.mask_map)
    c.docker()
    c.cybergym()
    c.server(a.server)
    # Only attempt the firewall once everything else is sound — starting it
    # against a broken setup just adds a second failure to untangle.
    if not c.problems:
        c.firewall(a.firewall)
    else:
        c.info["firewall"] = a.firewall

    if c.problems:
        print(
            f"preflight: {len(c.problems)} precondition(s) not met:",
            file=sys.stderr,
        )
        for i, problem in enumerate(c.problems, 1):
            print(f"  {i}. {problem}", file=sys.stderr)
        sys.exit(1)

    json.dump({"ok": True, **c.info}, sys.stdout)


if __name__ == "__main__":
    main()
