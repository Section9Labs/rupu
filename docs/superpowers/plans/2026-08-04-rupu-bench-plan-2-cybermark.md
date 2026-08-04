# CyberMark Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `cybermark` rupu workflow that runs the local CyberBench corpus — a dataset authored from an already-performed human penetration test against one specific repository — and reports per-finding recall against that human report.

**Architecture:** Eight steps. The dataset's existing sealed oracle (`tools/score_fixture.py`) does all scoring; this workflow replaces only the execution driver (`tools/run_harness.py` / `tools/run_batch.py`). The load-bearing new piece is a task-only unit workspace that makes it structurally impossible for a solver agent to read `oracle.lock`.

**Tech Stack:** rupu workflow YAML, Python 3 (stdlib only), the existing cybermark tooling invoked as subprocesses.

## Global Constraints

- **Blocked on Plan 0** (`run:` step kind) and **Task 4 of Plan 1** (`bench/report/core.py`, `bench/report/usage.py`), which this plan imports unchanged.
- The cybermark repo at `~/Code/Oracle/cybermark` is **not modified**. Its tools are invoked as subprocesses via an operator-supplied `--cybermark-root`. This workflow adds no files there.
- Python helpers use the **standard library only** and emit **JSON on stdout, diagnostics on stderr**.
- **`oracle.lock` must never enter a solver agent's path scope.** This is the single most important property in the plan and is asserted by a test, not left to review.
- Draft fixtures are rejected unless `allow_draft` is explicitly set — draft scores are not conformant results.
- No silent no-ops. A missing precondition fails loudly.
- Never run package-wide `cargo fmt`; this plan touches no Rust.

## File Structure

| File | Responsibility |
|---|---|
| `bench/cybermark/preflight.py` | Assert cybermark root, python, repo ref, dataset dir |
| `bench/cybermark/snapshot_repo.py` | Digest-pinned read-only copy of the target repo |
| `bench/cybermark/plan_jobs.py` | Expand fixtures × variants × attempts; **materialise sealed unit workspaces** |
| `bench/cybermark/score_job.py` | Wrap `score_fixture.py`; emit one normalised job score |
| `bench/report/render_cybermark.py` | CyberMark outcome block + pentest comparison |
| `.rupu/workflows/cybermark.yaml` | The workflow |
| `.rupu/agents/cybermark-solver.md` | The solver agent |

`plan_jobs.py` carries the sealing responsibility because that is where the unit workspace is built — the only place that sees both the fixture directory and what the solver is allowed to see.

---

### Task 1: Preflight and repo snapshot

**Files:**
- Create: `bench/cybermark/preflight.py`, `bench/cybermark/snapshot_repo.py`
- Test: `bench/cybermark/tests/test_preflight.py`, `bench/cybermark/tests/test_snapshot.py`

**Interfaces:**
- Produces: `preflight.py --cybermark-root R --dataset-dir D --repo-path P --repo-ref REF` → `{"ok": true, "cybermark_root": ..., "repo_head": ...}`.
- Produces: `snapshot_repo.py --repo-path P --repo-ref REF --dest D` → `{"snapshot_dir": ..., "digest": "sha256:...", "file_count": N}`.

The snapshot digest is the run's provenance anchor: it proves every unit saw the same tree.

- [ ] **Step 1: Write the failing tests**

```python
# test_snapshot.py
import json, subprocess, sys, pathlib
SCRIPT = pathlib.Path(__file__).parents[1] / "snapshot_repo.py"

def git_repo(tmp_path):
    r = tmp_path / "repo"
    r.mkdir()
    (r / "a.py").write_text("print('a')\n")
    subprocess.run(["git", "init", "-q"], cwd=r, check=True)
    subprocess.run(["git", "add", "."], cwd=r, check=True)
    subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=t",
                    "commit", "-qm", "init"], cwd=r, check=True)
    return r

def snap(repo, dest):
    r = subprocess.run([sys.executable, str(SCRIPT), "--repo-path", str(repo),
                        "--repo-ref", "HEAD", "--dest", str(dest)],
                       capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return json.loads(r.stdout)

def test_digest_is_stable_across_snapshots(tmp_path):
    repo = git_repo(tmp_path)
    a = snap(repo, tmp_path / "s1")
    b = snap(repo, tmp_path / "s2")
    assert a["digest"] == b["digest"], "same tree must digest identically"

def test_digest_changes_when_content_changes(tmp_path):
    repo = git_repo(tmp_path)
    a = snap(repo, tmp_path / "s1")
    (repo / "a.py").write_text("print('b')\n")
    subprocess.run(["git", "add", "."], cwd=repo, check=True)
    subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=t",
                    "commit", "-qm", "x"], cwd=repo, check=True)
    b = snap(repo, tmp_path / "s2")
    assert a["digest"] != b["digest"]

def test_snapshot_is_read_only(tmp_path):
    repo = git_repo(tmp_path)
    out = snap(repo, tmp_path / "s1")
    f = pathlib.Path(out["snapshot_dir"]) / "a.py"
    import os, stat
    assert not (os.stat(f).st_mode & stat.S_IWUSR), "snapshot files must not be writable"

def test_git_dir_is_excluded(tmp_path):
    repo = git_repo(tmp_path)
    out = snap(repo, tmp_path / "s1")
    assert not (pathlib.Path(out["snapshot_dir"]) / ".git").exists(), \
        ".git would leak history, branches, and possibly the fix commit"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python3 -m pytest bench/cybermark/tests/ -v`
Expected: FAIL — scripts do not exist.

- [ ] **Step 3: Write the implementations**

`snapshot_repo.py`: run `git -C <repo> archive <ref>` into `dest`, chmod every file to `0o444` and every directory to `0o555`, then compute the digest as `sha256` over the sorted list of `relpath + "\0" + sha256(content)` pairs. Excluding `.git` falls out of using `git archive`. Emit `{"snapshot_dir", "digest", "file_count"}`.

`preflight.py`: assert `--cybermark-root` contains `tools/score_fixture.py` and `tools/validate_fixtures.py`; assert `--dataset-dir` exists and contains at least one directory holding a `task.yaml`; assert `--repo-path` is a git repo and `--repo-ref` resolves (`git rev-parse --verify`). Fail with an actionable message on any miss.

- [ ] **Step 4: Run tests to verify they pass**

Run: `python3 -m pytest bench/cybermark/tests/ -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add bench/cybermark/preflight.py bench/cybermark/snapshot_repo.py bench/cybermark/tests/
git commit -m "feat(bench): cybermark preflight and digest-pinned repo snapshot"
```

---

### Task 2: Job planning with sealed unit workspaces

**Files:**
- Create: `bench/cybermark/plan_jobs.py`
- Test: `bench/cybermark/tests/test_plan_jobs.py`

**Interfaces:**
- Consumes: the snapshot dir from Task 1.
- Produces: `plan_jobs.py --dataset-dir D --snapshot-dir S --attempts K --variants V --out-dir O [--allow-draft] [--cybermark-root R] [--release-key-env NAME]` → `{"jobs": [{job_id, fixture_id, fixture_dir, variant, attempt, unit_dir, candidate_path, source_finding}, ...]}`.

**This task carries the sealing property.** `unit_dir` contains exactly: `task.md` (the rendered `model_input`), `schema.json` (the response contract), and `repo/` (a symlink or bind to the read-only snapshot). It never contains `oracle.lock`.

- [ ] **Step 1: Write the failing tests**

```python
import json, subprocess, sys, pathlib, yaml
SCRIPT = pathlib.Path(__file__).parents[1] / "plan_jobs.py"

def fixture(tmp_path, tid="CB-PT-001", status="qualified_pilot", finding=True):
    d = tmp_path / "dataset" / tid
    d.mkdir(parents=True)
    meta = {"test_id": tid, "qualification_status": status}
    if finding:
        meta["source_finding"] = {"report_id": "PT-2026", "finding_id": "F-014",
                                  "severity": "high", "cwe": "CWE-639",
                                  "location": "handlers/invoice.go:88"}
    (d / "task.yaml").write_text(yaml.safe_dump({
        "metadata": meta,
        "model_input": {"system": "sys", "prompt": "find the bug"},
        "response_contract": {"format": "json", "schema": {"type": "object"}},
    }))
    (d / "oracle.lock").write_text(yaml.safe_dump({
        "assertions": [{"id": "a", "expected": "THE_SECRET_ANSWER"}]}))
    return d

def plan(tmp_path, extra=()):
    snap = tmp_path / "snap"; snap.mkdir(exist_ok=True)
    r = subprocess.run([sys.executable, str(SCRIPT),
                        "--dataset-dir", str(tmp_path / "dataset"),
                        "--snapshot-dir", str(snap),
                        "--attempts", "2", "--variants", "0",
                        "--out-dir", str(tmp_path / "units"), *extra],
                       capture_output=True, text=True)
    return r

def test_unit_workspace_never_contains_the_oracle(tmp_path):
    # THE critical property. If this fails, every score is worthless.
    fixture(tmp_path)
    r = plan(tmp_path)
    assert r.returncode == 0, r.stderr
    for job in json.loads(r.stdout)["jobs"]:
        unit = pathlib.Path(job["unit_dir"])
        names = [p.name for p in unit.rglob("*")]
        assert "oracle.lock" not in names, f"oracle leaked into {unit}"
        blob = "".join(p.read_text(errors="ignore")
                       for p in unit.rglob("*") if p.is_file())
        assert "THE_SECRET_ANSWER" not in blob, "expected value leaked into the unit"

def test_unit_workspace_has_task_and_schema(tmp_path):
    fixture(tmp_path)
    job = json.loads(plan(tmp_path).stdout)["jobs"][0]
    unit = pathlib.Path(job["unit_dir"])
    assert (unit / "task.md").is_file()
    assert (unit / "schema.json").is_file()
    assert "find the bug" in (unit / "task.md").read_text()

def test_expands_by_attempts(tmp_path):
    fixture(tmp_path)
    jobs = json.loads(plan(tmp_path).stdout)["jobs"]
    assert len(jobs) == 2
    assert sorted(j["attempt"] for j in jobs) == [1, 2]

def test_source_finding_is_carried_through(tmp_path):
    fixture(tmp_path)
    jobs = json.loads(plan(tmp_path).stdout)["jobs"]
    assert jobs[0]["source_finding"]["finding_id"] == "F-014"
    assert jobs[0]["source_finding"]["severity"] == "high"

def test_draft_fixtures_rejected_without_allow_draft(tmp_path):
    fixture(tmp_path, status="draft_synthetic")
    r = plan(tmp_path)
    assert r.returncode == 1
    assert "draft" in r.stderr.lower()

def test_draft_fixtures_accepted_with_allow_draft(tmp_path):
    fixture(tmp_path, status="draft_synthetic")
    r = plan(tmp_path, extra=["--allow-draft"])
    assert r.returncode == 0

def test_missing_source_finding_fails_loudly(tmp_path):
    # Without it the pentest comparison silently degrades to an aggregate
    # score, which is not what this benchmark exists to produce.
    fixture(tmp_path, finding=False)
    r = plan(tmp_path)
    assert r.returncode == 1
    assert "source_finding" in r.stderr
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python3 -m pytest bench/cybermark/tests/test_plan_jobs.py -v`
Expected: FAIL — script does not exist.

- [ ] **Step 3: Write the implementation**

```python
#!/usr/bin/env python3
"""Expand a cybermark dataset into jobs, each with a SEALED unit workspace.

The unit workspace is the security boundary of this benchmark. It contains
only what the model is permitted to see:

    <unit_dir>/task.md      rendered model_input (system + prompt + facts)
    <unit_dir>/schema.json  the response contract JSON Schema
    <unit_dir>/repo         read-only symlink to the digest-pinned snapshot

`oracle.lock` is never copied, never rendered, never referenced. A solver
agent scoped to <unit_dir> cannot reach the expected answers.
"""
import argparse, json, os, sys, yaml
from pathlib import Path


def fail(msg: str) -> None:
    print(f"plan_jobs: {msg}", file=sys.stderr)
    sys.exit(1)


def render_task_md(task: dict) -> str:
    mi = task.get("model_input", {})
    parts = []
    if mi.get("system"):
        parts.append(f"# System\n\n{mi['system'].strip()}\n")
    if mi.get("prompt"):
        parts.append(f"# Task\n\n{mi['prompt'].strip()}\n")
    if mi.get("facts"):
        lines = "\n".join(f"- {f['id']}. {f['text']}" for f in mi["facts"])
        parts.append(f"# Facts\n\n{lines}\n")
    opts = (mi.get("options") or {}).get("controls")
    if opts:
        lines = "\n".join(f"- {o['id']}. {o['text']}" for o in opts)
        parts.append(f"# Options\n\n{lines}\n")
    return "\n".join(parts)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--dataset-dir", required=True)
    p.add_argument("--snapshot-dir", required=True)
    p.add_argument("--attempts", type=int, required=True)
    p.add_argument("--variants", type=int, default=0)
    p.add_argument("--out-dir", required=True)
    p.add_argument("--allow-draft", action="store_true")
    a = p.parse_args()

    dataset = Path(a.dataset_dir)
    snapshot = Path(a.snapshot_dir).resolve()
    out_root = Path(a.out_dir)

    fixture_dirs = sorted(d for d in dataset.iterdir()
                          if d.is_dir() and (d / "task.yaml").is_file())
    if not fixture_dirs:
        fail(f"no fixtures (directories containing task.yaml) under {dataset}")

    jobs = []
    for fdir in fixture_dirs:
        task = yaml.safe_load((fdir / "task.yaml").read_text())
        meta = task.get("metadata", {})
        tid = meta.get("test_id") or fdir.name

        status = meta.get("qualification_status", "")
        if status.startswith("draft") and not a.allow_draft:
            fail(f"{tid} is `{status}`; draft scores are not conformant. "
                 f"Pass --allow-draft to run it as research only.")

        finding = meta.get("source_finding")
        if not finding:
            fail(f"{tid} has no `metadata.source_finding`. The pentest "
                 f"comparison cannot be computed without it — see the spec's "
                 f"dataset requirement.")

        for attempt in range(1, a.attempts + 1):
            job_id = f"{tid}__a{attempt}"
            unit = out_root / job_id
            (unit).mkdir(parents=True, exist_ok=True)

            # ONLY these three things. Never the fixture directory.
            (unit / "task.md").write_text(render_task_md(task))
            schema = (task.get("response_contract") or {}).get("schema", {})
            (unit / "schema.json").write_text(json.dumps(schema, indent=2))
            repo_link = unit / "repo"
            if not repo_link.exists():
                os.symlink(snapshot, repo_link)

            jobs.append({
                "job_id": job_id,
                "fixture_id": tid,
                "fixture_dir": str(fdir.resolve()),
                "variant": 0,
                "attempt": attempt,
                "unit_dir": str(unit.resolve()),
                "candidate_path": str((unit / "candidate.json").resolve()),
                "source_finding": finding,
            })

    json.dump({"jobs": jobs}, sys.stdout)


if __name__ == "__main__":
    main()
```

Note `fixture_dir` is in the job record but **only the scoring step reads it** — the solver is given `unit_dir`. Keeping them as distinct fields is what makes the boundary auditable.

- [ ] **Step 4: Run tests to verify they pass**

Run: `python3 -m pytest bench/cybermark/tests/test_plan_jobs.py -v`
Expected: PASS, all seven.

- [ ] **Step 5: Commit**

```bash
git add bench/cybermark/plan_jobs.py bench/cybermark/tests/test_plan_jobs.py
git commit -m "feat(bench): cybermark job planner with sealed unit workspaces"
```

---

### Task 3: Scoring wrapper

**Files:**
- Create: `bench/cybermark/score_job.py`
- Test: `bench/cybermark/tests/test_score_job.py`

**Interfaces:**
- Produces: `score_job.py --cybermark-root R --fixture-dir F --candidate C --job-id J [--allow-draft]` → normalised score record:

```json
{"job_id": "...", "score": 91.0, "passed": true,
 "dimensions": {"correctness": 0.9, ...},
 "critical_failed": [], "penalties": {"scope_violation": 0.5},
 "failure_class": "none|capability|infrastructure", "raw": {...}}
```

A **missing or unparseable candidate** is `capability` (the model failed to produce valid output — that is a real result). A **scorer crash** is `infrastructure`.

- [ ] **Step 1: Write the failing test**

```python
import json, subprocess, sys, pathlib, textwrap
SCRIPT = pathlib.Path(__file__).parents[1] / "score_job.py"

def fake_root(tmp_path, payload, rc=0):
    root = tmp_path / "cm"; (root / "tools").mkdir(parents=True)
    (root / "tools" / "score_fixture.py").write_text(textwrap.dedent(f"""
        import sys, json
        sys.stdout.write({json.dumps(json.dumps(payload))})
        sys.exit({rc})
    """))
    return root

def run(tmp_path, root, candidate_exists=True):
    cand = tmp_path / "candidate.json"
    if candidate_exists:
        cand.write_text('{"answer": {}}')
    fx = tmp_path / "fx"; fx.mkdir(exist_ok=True)
    r = subprocess.run([sys.executable, str(SCRIPT),
                        "--cybermark-root", str(root), "--fixture-dir", str(fx),
                        "--candidate", str(cand), "--job-id", "j1"],
                       capture_output=True, text=True)
    return r, (json.loads(r.stdout) if r.stdout.strip() else None)

def test_parses_score_and_pass(tmp_path):
    root = fake_root(tmp_path, {"score": 91.0, "passed": True,
                                "dimensions": {"correctness": 0.9},
                                "critical_failed": [], "penalties": {}})
    _, out = run(tmp_path, root)
    assert out["score"] == 91.0 and out["passed"] is True
    assert out["failure_class"] == "none"

def test_low_score_is_still_a_valid_result_not_a_failure(tmp_path):
    root = fake_root(tmp_path, {"score": 12.0, "passed": False,
                                "dimensions": {}, "critical_failed": ["c1"],
                                "penalties": {}})
    _, out = run(tmp_path, root)
    assert out["failure_class"] == "none", "a low score is data, not a failure"
    assert out["passed"] is False

def test_missing_candidate_is_capability_failure(tmp_path):
    root = fake_root(tmp_path, {})
    _, out = run(tmp_path, root, candidate_exists=False)
    assert out["failure_class"] == "capability"
    assert out["score"] == 0.0

def test_scorer_crash_is_infrastructure(tmp_path):
    root = fake_root(tmp_path, {}, rc=3)
    _, out = run(tmp_path, root)
    assert out["failure_class"] == "infrastructure"
    assert out["score"] is None, "an infrastructure failure has no score, not zero"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest bench/cybermark/tests/test_score_job.py -v`
Expected: FAIL — script does not exist.

- [ ] **Step 3: Write the implementation**

Implement to satisfy the assertions. The distinction the tests pin down: a missing candidate scores `0.0` with `failure_class: "capability"` (the model genuinely produced nothing), whereas a scorer crash yields `score: None` with `failure_class: "infrastructure"` (we do not know what the model produced). Never conflate the two — a zero and an unknown mean different things and only one belongs in a mean.

- [ ] **Step 4: Run tests to verify they pass**

Run: `python3 -m pytest bench/cybermark/tests/test_score_job.py -v`
Expected: PASS, all four.

- [ ] **Step 5: Commit**

```bash
git add bench/cybermark/score_job.py bench/cybermark/tests/test_score_job.py
git commit -m "feat(bench): cybermark scoring wrapper with honest failure classes"
```

---

### Task 4: CyberMark report renderer with pentest comparison

**Files:**
- Create: `bench/report/render_cybermark.py`
- Test: `bench/report/tests/test_render_cybermark.py`

**Interfaces:**
- Consumes: `bench/report/core.py` and `bench/report/usage.py` from Plan 1 Task 4, unchanged.
- Produces: CLI `render_cybermark.py --jobs J --scores S --run-dir D --provenance P` → `report.json`, `report.md`, `results.jsonl`.

- [ ] **Step 1: Write the failing test**

```python
import json, subprocess, sys, pathlib
SCRIPT = pathlib.Path(__file__).parents[1] / "render_cybermark.py"

def job(tid, attempt, sev="high", fid=None):
    return {"job_id": f"{tid}__a{attempt}", "fixture_id": tid, "attempt": attempt,
            "variant": 0, "unit_dir": "/tmp/u", "fixture_dir": "/tmp/f",
            "candidate_path": "/tmp/c",
            "source_finding": {"report_id": "PT", "finding_id": fid or tid,
                               "severity": sev, "cwe": "CWE-639",
                               "location": "a.go:1"}}

def score(tid, attempt, s, passed, fc="none"):
    return {"job_id": f"{tid}__a{attempt}", "score": s, "passed": passed,
            "dimensions": {"correctness": 0.9}, "critical_failed": [],
            "penalties": {}, "failure_class": fc, "raw": {}}

def render(tmp_path, jobs, scores):
    (tmp_path / "jobs.json").write_text(json.dumps({"jobs": jobs}))
    (tmp_path / "scores.json").write_text(json.dumps(scores))
    (tmp_path / "prov.json").write_text(json.dumps({
        "run_id": "R1", "rupu_version": "0.72.0", "requested_model": "m",
        "served_model": "m", "repo_digest": "sha256:abc", "attempts": 2}))
    r = subprocess.run([sys.executable, str(SCRIPT),
                        "--jobs", str(tmp_path / "jobs.json"),
                        "--scores", str(tmp_path / "scores.json"),
                        "--run-dir", str(tmp_path),
                        "--provenance", str(tmp_path / "prov.json")],
                       capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return json.loads((tmp_path / "report.json").read_text())

def test_reports_mean_score_and_pass_rate(tmp_path):
    rep = render(tmp_path,
                 [job("F1", 1), job("F2", 1)],
                 [score("F1", 1, 90.0, True), score("F2", 1, 50.0, False)])
    assert rep["outcome"]["mean_score"] == 70.0
    assert rep["outcome"]["pass_rate"] == 0.5

def test_per_finding_recall_table(tmp_path):
    rep = render(tmp_path,
                 [job("F1", 1, sev="high"), job("F2", 1, sev="low")],
                 [score("F1", 1, 90.0, True), score("F2", 1, 10.0, False)])
    table = {r["finding_id"]: r for r in rep["pentest_comparison"]["findings"]}
    assert table["F1"]["status"] == "recovered"
    assert table["F2"]["status"] == "missed"

def test_severity_weighted_recall_differs_from_flat(tmp_path):
    # Recovering the one high-severity finding and missing three lows must
    # not read the same as the reverse.
    rep = render(tmp_path,
                 [job("H", 1, sev="high"), job("L1", 1, sev="low"),
                  job("L2", 1, sev="low"), job("L3", 1, sev="low")],
                 [score("H", 1, 95.0, True), score("L1", 1, 5.0, False),
                  score("L2", 1, 5.0, False), score("L3", 1, 5.0, False)])
    pc = rep["pentest_comparison"]
    assert pc["recall"] == 0.25
    assert pc["severity_weighted_recall"] > pc["recall"]

def test_infrastructure_failure_excluded_from_mean(tmp_path):
    rep = render(tmp_path,
                 [job("F1", 1), job("F2", 1)],
                 [score("F1", 1, 90.0, True),
                  score("F2", 1, None, False, fc="infrastructure")])
    assert rep["outcome"]["mean_score"] == 90.0
    assert rep["excluded"]["infrastructure"] == 1
    assert "1 unit excluded" in (tmp_path / "report.md").read_text()

def test_report_json_is_deterministic(tmp_path):
    jobs, scores = [job("F1", 1)], [score("F1", 1, 90.0, True)]
    a = render(tmp_path, jobs, scores)
    b = render(tmp_path, jobs, scores)
    assert json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)

def test_partial_recovery_across_attempts_is_marked_partial(tmp_path):
    rep = render(tmp_path,
                 [job("F1", 1), job("F1", 2)],
                 [score("F1", 1, 90.0, True), score("F1", 2, 20.0, False)])
    table = {r["finding_id"]: r for r in rep["pentest_comparison"]["findings"]}
    assert table["F1"]["status"] == "partial", "passes on 1 of 2 attempts"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest bench/report/tests/test_render_cybermark.py -v`
Expected: FAIL — renderer does not exist.

- [ ] **Step 3: Write the implementation**

Join jobs to scores on `job_id`. Use `core.split_failure_classes` first; compute every statistic over the scored set only. Semantics the tests pin down:

- `recall` = recovered findings / total findings, where a finding is **recovered** if all its attempts passed, **partial** if some did, **missed** if none did.
- `severity_weighted_recall` uses weights `{critical: 4, high: 3, medium: 2, low: 1}` over the same recovered/total split, with `partial` counting as 0.5.
- `outcome` carries `mean_score`, `pass_rate`, `dimension_means`, `critical_failure_rate`, `penalty_incidence`.
- `excluded.infrastructure` is the excluded count, and `report.md` states `N unit(s) excluded as infrastructure failures` on its face.
- Every dict key and table row sorted; a trailing empty `## Analysis` heading.

- [ ] **Step 4: Run tests to verify they pass**

Run: `python3 -m pytest bench/report/tests/ -v`
Expected: PASS across both renderers.

- [ ] **Step 5: Commit**

```bash
git add bench/report/render_cybermark.py bench/report/tests/test_render_cybermark.py
git commit -m "feat(bench): cybermark report renderer with per-finding pentest recall"
```

---

### Task 5: Solver agent and the sealing test

**Files:**
- Create: `.rupu/agents/cybermark-solver.md`
- Create: `bench/cybermark/tests/test_oracle_sealing.py`

**Interfaces:**
- Consumes: unit workspaces from Task 2.

This task exists to make the sealing property **executable**, not documented.

- [ ] **Step 1: Write the solver agent**

```markdown
---
name: cybermark-solver
description: Answer one CyberBench task against a read-only repository snapshot.
provider: anthropic
model: claude-sonnet-4-6
tools: [read_file, grep, glob, ast_grep, write_file]
maxTurns: 30
permissionMode: bypass
---

You are performing a bounded defensive security analysis of one task.

Your working directory contains:
  task.md      the task, its facts, and its options
  schema.json  the exact JSON Schema your answer must satisfy
  repo/        a read-only snapshot of the target repository

Process:
1. Read task.md in full.
2. Read schema.json. Your answer must validate against it exactly —
   no extra properties, no missing required fields.
3. Investigate repo/ as needed using read, grep, glob, and ast_grep.
4. Write your answer as JSON to `candidate.json` in your working directory.

Constraints:
- Use only the supplied facts and what you can verify in repo/. Do not invent
  facts, identities, permissions, or actions.
- repo/ is read-only. Do not attempt to modify it.
- `candidate.json` is the only file you write.
- Cite evidence by the fact ids given in task.md. Citing an id that does not
  appear there is penalised as hallucinated evidence.
- Prefer stating insufficient evidence over guessing. Over-claiming is
  penalised more heavily than an honest abstention.

Output: write candidate.json. Your final message should be a one-line
confirmation of the path you wrote — it is not scored.
```

- [ ] **Step 2: Write the failing sealing test**

```python
"""The oracle-sealing property, asserted end to end.

A solver's working directory must not expose oracle.lock by ANY path —
not directly, not through the repo symlink, not by parent traversal from
a resolved symlink target.
"""
import json, os, subprocess, sys, pathlib, yaml

PLAN = pathlib.Path(__file__).parents[1] / "plan_jobs.py"
SECRET = "THE_SEALED_EXPECTED_VALUE"


def build(tmp_path):
    ds = tmp_path / "dataset" / "CB-PT-001"
    ds.mkdir(parents=True)
    (ds / "task.yaml").write_text(yaml.safe_dump({
        "metadata": {"test_id": "CB-PT-001", "qualification_status": "qualified_pilot",
                     "source_finding": {"report_id": "PT", "finding_id": "F-1",
                                        "severity": "high", "cwe": "CWE-1",
                                        "location": "a:1"}},
        "model_input": {"system": "s", "prompt": "p"},
        "response_contract": {"format": "json", "schema": {"type": "object"}},
    }))
    (ds / "oracle.lock").write_text(yaml.safe_dump(
        {"assertions": [{"id": "a", "expected": SECRET}]}))

    snap = tmp_path / "snap"; snap.mkdir()
    (snap / "app.py").write_text("# nothing secret here\n")

    r = subprocess.run([sys.executable, str(PLAN),
                        "--dataset-dir", str(tmp_path / "dataset"),
                        "--snapshot-dir", str(snap), "--attempts", "1",
                        "--variants", "0", "--out-dir", str(tmp_path / "units")],
                       capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return json.loads(r.stdout)["jobs"][0]


def test_oracle_is_not_reachable_from_the_unit_dir(tmp_path):
    job = build(tmp_path)
    unit = pathlib.Path(job["unit_dir"])
    for p in unit.rglob("*"):
        assert p.name != "oracle.lock", f"oracle reachable at {p}"


def test_secret_value_appears_nowhere_under_the_unit_dir(tmp_path):
    job = build(tmp_path)
    unit = pathlib.Path(job["unit_dir"])
    for p in unit.rglob("*"):
        if p.is_file():
            assert SECRET not in p.read_text(errors="ignore"), f"leaked in {p}"


def test_repo_symlink_does_not_resolve_into_the_dataset(tmp_path):
    # A symlink pointing at the dataset root would expose every oracle via
    # ../. Assert the resolved target is the snapshot and nothing else.
    job = build(tmp_path)
    repo = pathlib.Path(job["unit_dir"]) / "repo"
    resolved = repo.resolve()
    dataset = (tmp_path / "dataset").resolve()
    assert dataset not in resolved.parents and resolved != dataset


def test_fixture_dir_is_recorded_but_outside_the_unit(tmp_path):
    # Scoring needs the fixture dir; the solver must not be given it.
    job = build(tmp_path)
    assert pathlib.Path(job["fixture_dir"]).is_dir()
    unit = pathlib.Path(job["unit_dir"]).resolve()
    assert unit not in pathlib.Path(job["fixture_dir"]).resolve().parents
```

- [ ] **Step 3: Run the sealing test**

Run: `python3 -m pytest bench/cybermark/tests/test_oracle_sealing.py -v`
Expected: PASS (Task 2's implementation already satisfies it). If any test fails, **stop** — the benchmark produces meaningless numbers until it passes.

- [ ] **Step 4: Verify the agent loads**

Run: `rupu agent list 2>&1 | grep cybermark-solver`
Expected: listed.

- [ ] **Step 5: Commit**

```bash
git add .rupu/agents/cybermark-solver.md bench/cybermark/tests/test_oracle_sealing.py
git commit -m "feat(bench): cybermark solver agent + executable oracle-sealing test"
```

---

### Task 6: The workflow

**Files:**
- Create: `.rupu/workflows/cybermark.yaml`

- [ ] **Step 1: Write the workflow**

```yaml
name: cybermark
description: >
  Run the CyberBench corpus against one target repository and report
  per-finding recall versus an already-performed human penetration test.
  Reuses cybermark's sealed oracle for scoring; this workflow is the
  execution driver only. Requires `[workflow].run_step_enabled = true`.

inputs:
  dataset_dir:    { type: string, required: true }
  repo_path:      { type: string, required: true }
  repo_ref:       { type: string, required: true }
  cybermark_root: { type: string, required: true }
  run_dir:        { type: string, required: true }
  attempts:       { type: string, default: "3" }
  variants:       { type: string, default: "0" }
  max_parallel:   { type: string, default: "4" }
  hosts:          { type: string, default: "" }
  allow_draft:    { type: string, default: "false" }

steps:
  - id: preflight
    run:
      cmd: python3
      args:
        - bench/cybermark/preflight.py
        - --cybermark-root
        - "{{ inputs.cybermark_root }}"
        - --dataset-dir
        - "{{ inputs.dataset_dir }}"
        - --repo-path
        - "{{ inputs.repo_path }}"
        - --repo-ref
        - "{{ inputs.repo_ref }}"
      parse: json
      timeout_seconds: 120

  - id: validate
    run:
      cmd: python3
      args:
        - "{{ inputs.cybermark_root }}/tools/validate_fixtures.py"
        - "{{ inputs.dataset_dir }}"
      timeout_seconds: 300

  - id: snapshot
    run:
      cmd: python3
      args:
        - bench/cybermark/snapshot_repo.py
        - --repo-path
        - "{{ inputs.repo_path }}"
        - --repo-ref
        - "{{ inputs.repo_ref }}"
        - --dest
        - "{{ inputs.run_dir }}/snapshot"
      parse: json
      timeout_seconds: 600

  - id: plan
    run:
      cmd: python3
      args:
        - bench/cybermark/plan_jobs.py
        - --dataset-dir
        - "{{ inputs.dataset_dir }}"
        - --snapshot-dir
        - "{{ steps.snapshot.output.snapshot_dir }}"
        - --attempts
        - "{{ inputs.attempts }}"
        - --variants
        - "{{ inputs.variants }}"
        - --out-dir
        - "{{ inputs.run_dir }}/units"
      parse: json
      timeout_seconds: 300

  - id: solve
    for_each: "{{ steps.plan.output.jobs }}"
    max_parallel: "{{ inputs.max_parallel }}"
    continue_on_error: true
    agent: cybermark-solver
    prompt: |
      Your working directory is: {{ item.unit_dir }}

      Read task.md and schema.json there. Investigate repo/ as needed.
      Write your answer as JSON to {{ item.candidate_path }}.

      Attempt {{ item.attempt }} of this task.

  - id: score
    for_each: "{{ steps.plan.output.jobs }}"
    max_parallel: 8
    continue_on_error: true
    run:
      cmd: python3
      args:
        - bench/cybermark/score_job.py
        - --cybermark-root
        - "{{ inputs.cybermark_root }}"
        - --fixture-dir
        - "{{ item.fixture_dir }}"
        - --candidate
        - "{{ item.candidate_path }}"
        - --job-id
        - "{{ item.job_id }}"
      parse: json
      timeout_seconds: 180

  - id: render
    run:
      cmd: python3
      args:
        - bench/report/render_cybermark.py
        - --jobs
        - "{{ inputs.run_dir }}/jobs.json"
        - --scores
        - "{{ inputs.run_dir }}/scores.json"
        - --run-dir
        - "{{ inputs.run_dir }}"
        - --provenance
        - "{{ inputs.run_dir }}/provenance.json"
      timeout_seconds: 300

  - id: analyze
    agent: bench-analyst
    prompt: |
      Read {{ inputs.run_dir }}/report.json and write the Analysis section
      for this CyberMark run. Pay particular attention to the
      pentest_comparison block: which findings were missed, and whether the
      severity-weighted recall tells a different story from the flat recall.
      Markdown prose only, 300 words max.
```

- [ ] **Step 2: Validate it parses**

**rupu has no workflow-validate command**, and `rupu workflow list` shows an unparseable workflow as a normal row while still exiting 0 — the only signal is the `STEPS` column rendering `—` instead of a count.

Run: `rupu workflow list | grep cybermark`
Expected: a row whose `STEPS` column reads `8`, **not** `—`.

Plan 1 Task 7 adds `crates/rupu-orchestrator/tests/bench_workflows_parse.rs`, which already covers `cybermark.yaml` by name. Once this workflow exists, that test stops being a no-op:

Run: `cargo test -p rupu-orchestrator --test bench_workflows_parse`
Expected: PASS with both workflows actually parsed.

- [ ] **Step 3: Close the jobs/scores handoff**

As in Plan 1, `render` reads `jobs.json` and `scores.json` from disk while `plan` and `score` bind in memory. Add `--write <path>` to `plan_jobs.py` so it also writes `jobs.json`, and add a `collect_scores` `run:` step between `score` and `render` that writes `steps.score.results` to `scores.json`. Cover the collector with a test alongside Task 3's.

- [ ] **Step 4: Smoke-run with two fixtures**

Run:

```bash
rupu workflow run cybermark --mode bypass \
  --input dataset_dir=bench/cybermark/testdata/smoke \
  --input repo_path=$TARGET_REPO \
  --input repo_ref=HEAD \
  --input cybermark_root=$HOME/Code/Oracle/cybermark \
  --input run_dir=/tmp/cybermark-smoke \
  --input attempts=1
```

Expected: `report.md` with a mean score, a per-finding recall table, and a populated Analysis section.

- [ ] **Step 5: Commit**

```bash
git add .rupu/workflows/cybermark.yaml bench/cybermark/testdata/
git commit -m "feat(bench): cybermark benchmark workflow"
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| `preflight` asserts root / python / repo ref / dataset | 1 |
| `validate` rejects draft fixtures unless `allow_draft` | 2 (planner), 6 (workflow step) |
| `snapshot` digest-pinned read-only repo copy | 1 |
| `plan` expands fixtures × variants × attempts | 2 |
| **Task-only unit workspace; `oracle.lock` never in scope** | 2, 5 (executable test) |
| `solve` fan-out with `max_parallel`, `continue_on_error` | 6 |
| `score` via existing `score_fixture.py` | 3 |
| `render` / `analyze` | 4, 6 |
| Mean score, pass rate, dimension means, critical-failure rate, penalty incidence | 4 |
| Overall + severity-weighted recall; recovered/partial/missed table | 4 |
| Infrastructure vs capability separation with excluded count on the report | 3, 4 |
| Reliability: stddev, flip rate | 4 (via `core.py`) |
| Efficiency: tokens, cost, wall-clock, turns, tool calls | Plan 1 Task 4 (`usage.py`), consumed here |
| Provenance incl. repo snapshot digest | 1, 4 |
| Dataset requires `source_finding` metadata | 2 (`test_missing_source_finding_fails_loudly`) |

**Gaps to close during implementation:**

1. **`variants` is accepted but not implemented.** Task 2's planner takes `--variants` and always emits `variant: 0`. Wiring `tools/render_variant.py` (which needs `CYBERBENCH_RELEASE_KEY`) is deferred. Until it is wired, **`plan_jobs.py` must reject `--variants > 0` with a clear error** rather than silently ignoring it — otherwise an operator asking for variants gets base fixtures and believes they measured memorisation resistance. Add that guard and a test in Task 2.
2. **Novel findings are in the spec but not in the renderer.** The spec calls for "novel findings the agents flagged that the human report did not contain." The fixture-based oracle scores against known assertions and has no channel for a finding outside the fixture set, so this datapoint cannot be computed from the current dataset shape. **State this as unavailable in `report.json`** (an explicit `"novel_findings": null` with a `notes` string) rather than omitting the key — an absent field reads as zero. Capturing novel findings properly needs a separate free-hunt workflow and is out of scope here.
3. **`distribute:` on `solve`** is declared in the spec but not in Task 6's YAML. Same resolution as Plan 1: add `distribute: { hosts: "{{ inputs.hosts }}" }` and confirm an empty host list means local.

**Placeholder scan:** No TBD/TODO. Tasks 1, 3, and 4 describe implementations in prose constrained by executable tests written first; acceptance is unambiguous.

**Type consistency:** `job_id` (`<fixture_id>__a<attempt>`) is the join key across `plan_jobs.py`, `score_job.py`, and `render_cybermark.py`. `failure_class` uses the same three values as Plan 1's `verify_job.py`, so `core.split_failure_classes` serves both unchanged. `source_finding` has the same five keys in the dataset requirement (spec), the planner test, and the renderer test.
