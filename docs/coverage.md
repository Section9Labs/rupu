# Agentic coverage harness

`rupu coverage` turns "review this repo for X" from an unauditable one-shot into
a **persistent, accumulating, comparable record** of what an agent examined, for
which criteria, and what it concluded.

## At a glance

When a model performs a survey task — "review this repo for vulnerabilities",
"find every place we touch the database", "audit for accessibility" — findings
vary run-to-run, and you can't tell *"no issues found"* apart from *"the model
never looked."* The coverage harness fixes that:

- **Coverage is explicit.** For every `(file × concern)` the agent assesses, it
  records a verdict — `clean` / `finding` / `examined` / `not_applicable` — with
  evidence. File reads/greps/edits are tracked automatically.
- **Runs accumulate.** A second pass (same model or a different one) picks up
  where the first left off; cross-model verdicts are attributed and merge-able.
- **Industry-anchored catalogs.** Ship OWASP Top 10, CWE Top 25, the full CWE
  weakness list, STRIDE, secrets, and more; include or extend them.
- **Surface-uniform.** Works the same whether the agent loop is driven by
  `rupu run`, a workflow, an autoflow cycle, or an interactive session.

On top of that foundation, the harness also lets you **measure and reproduce**
variance: diff two runs, and replay a run to compare it against the original.

## How it works

Coverage data for a *target* lives under `<workspace>/.rupu/coverage/<target_id>/`
as append-only JSONL plus a catalog snapshot:

| File | Contents |
|------|----------|
| `files.jsonl` | every file touch (read / grep / glob / edit / cmd), with attribution |
| `concerns.jsonl` | every `(concern, file) → verdict` assertion |
| `findings.jsonl` | every reported issue |
| `catalog.yaml` | the effective concern catalog, snapshotted at run start |
| `runs.jsonl` | one manifest per run (its defining inputs, for replay) |

`<target_id>` is derived deterministically from `(workspace, scope_name)`, so the
same agent against the same repo accumulates into one target across runs, while
different workspaces stay distinct. Every row carries `run_id` + `model` +
`surface` attribution, which is what makes cross-run and cross-model analysis
possible.

## Turning it on: the `concerns:` block

An agent activates the harness by declaring a `concerns:` block in its
frontmatter. The block is a list of catalog includes (with optional overrides,
filters, and render mode). When present, the runtime flattens the catalog,
snapshots it, injects the catalog into the system prompt, and registers the
coverage tools.

```yaml
---
name: security-assessor
permissionMode: readonly
concerns:
  - include: owasp-top10-2021
    mode: full
  - include: cwe-top25-2023
    mode: full
  - include: secrets-in-source
    mode: full
  - include: stride
    mode: full
  - include: cwe-software-development   # ~399 CWEs
    mode: index                          # one-line table, searched on demand
---
You are a security assessor. For each (file × concern) you assess, call
coverage_mark; for each issue, call report_finding…
```

**Render modes.** `full` inlines each concern's body into the prompt; `index`
renders a compact one-line-per-concern table that the agent searches on demand
(use it for large catalogs like the full CWE list so the prompt stays small);
`auto` (default) picks based on catalog size.

### Bundled catalogs

`rupu coverage templates list` prints them all:

```
owasp-top10-2021        owasp-api-top10-2023
cwe-top25-2023          cwe-software-development   cwe-research
stride                  secrets-in-source          code-smells
web-security-default    api-security-default
```

User catalogs may live in `.rupu/concerns/` (project) or `~/.rupu/concerns/`
(global) and are discovered by name; project overrides global overrides builtin.

### Coverage tools the agent gets

When `concerns:` is set, these tools are injected automatically (you do **not**
list them in the agent's `tools:`):

| Tool | Purpose |
|------|---------|
| `coverage_mark` | record a `(concern, file)` verdict + evidence |
| `report_finding` | record an issue — a complete `report` under the `full` profile (default), or `summary` / `severity` / `evidence` under `summary`; see [Finding reports](#finding-reports) |
| `coverage_remaining` | list in-scope files still lacking an assertion |
| `coverage_status` | summary of assessed-vs-gap progress |
| `coverage_concerns_search` / `coverage_concerns_detail` | search / fetch full bodies for index-mode catalogs |

## Finding reports

A finding is recorded once, as structured data. That structured record is the
source of truth for the finding: rendered views and exports are built on it (see
the "Not built yet" note at the end of this section for what exists today).

### Profiles

| Profile | What `report_finding` / `findings.record` accepts |
|---------|----------------------------------------------------|
| `full` (default) | A complete `report` object. `summary`, `severity`, and `evidence` are **rejected**: rupu derives them (`summary` ← `title`, `severity` ← `rating.risk_rating`, `evidence.rationale` ← `root_cause`). |
| `summary` | The lightweight `summary` / `severity` / `evidence` record. A `report` is **refused**, never silently dropped. |

Pick a profile with the agent's `findingsProfile` frontmatter, a workflow's
`defaults.findings_profile`, a step's `findings_profile`, or `rupu run
--findings-profile` for a standalone run. Precedence is
step (or the flag) → workflow defaults → agent `findingsProfile` → `full`
(see `docs/agent-format.md` and `docs/workflow-format.md`). Remote workflow
units (`host:` / `distribute:`) get the same resolution: the step or default
value travels to the host as `--findings-profile`. The chosen profile
is stored on each finding record; records written before profiles existed read
back as `summary`.

> **Upgrading:** the built-in default is `full`, so an existing agent that
> records thin findings is rejected until it declares `findingsProfile: summary`
> or its prompt is updated to send a `report`. Agents outside the repo (for
> example under `~/.rupu/agents/`) need that one-line change.

### What `full` requires

Every field of the report is required except `cwe` (it may be an empty list),
`artifacts`, and `verification`. The rules below are enforced at write time; a
rejected call returns **every** problem at once, each with its field path, so
the agent can fix them all in one retry. (A structurally malformed JSON argument
surfaces as a single parse error instead.) When a full-profile run can record
findings (the agent has a `concerns:` block or `report_finding` in `tools:`),
finding-writing guidance is also appended to its system prompt, so the agent
needs no external reporting-standard file.

`artifacts` is described under [Artifacts](#artifacts) below. `verification` is
optional and is normally left to rupu or a verifier rather than the agent that
wrote the finding: `{status: unverified|confirmed|disputed|inconclusive,
by_run?, notes?}`.

- Required strings must be non-empty after trimming.
- Ratings (`impact`, `risk_rating`, `risk_factor`) are `Low`/`Medium`/`High`/`Critical`;
  `likelihood` is `Low`/`Medium`/`High`. `cwe` is a list of `CWE-<n>` ids and may be empty.
- `evidence` needs at least one claim and `replication_steps` at least one step.
- `file` values are workspace-relative and `lines` is `[start, end]` with `1 <= start <= end`.
- `cross_references[].finding_id` must be an existing finding id.
- The serialized report must fit `[findings].report_max_bytes` (default 256 KiB),
  so a pasted log cannot swell the ledger. The limit is checked again after
  artifact directories are expanded.

### Sentinels

Where information genuinely cannot be determined, a field uses a sentinel
instead of being omitted or guessed. On the structured fields (tickets, call
chain, patch, CI/CD detection, regression test, cross references) a sentinel is
accepted only in its exact form, and only the ones listed for that field:

| Sentinel | Used for |
|----------|----------|
| `Unknown` | owner, product, affected component, source repository, attack vector, CVSS score, tickets |
| `Not Applicable` | source repository, when no source-controlled code is involved |
| `None Provided` | tickets, when none are mentioned |
| `None` | cross references, when there is no related finding |
| `Not Provided — <justification>` (em dash) | call chain, recommended patch, CI/CD detection, regression test; the justification must be non-empty |

`Unknown` is **not** accepted for the patch, CI/CD detection, or regression
test: provide the item or say why it could not be produced.

### Artifacts

`report.artifacts[].path` lists proof-of-concept files (scripts, outputs,
harnesses) as workspace-relative paths. At write time rupu hashes each one:

- A file up to `[findings].artifact_max_bytes` (default 500 MiB) is copied into a
  content-addressed store at `<RUPU_HOME>/findings/artifacts/<aa>/<sha256>` and
  recorded `stored: copied`. Identical content is stored once across runs and
  projects, and the store outlives the workspace.
- A larger file is recorded `stored: external` with its path, size, and sha256.
- A directory expands to the files inside it, each handled by the same rule.
  Symlinks inside a directory are skipped.
- A path that escapes the workspace, does not exist, or names something other
  than a regular file (a device, socket, or the like) rejects the finding, so a
  typo is not silently dropped.
- A remote workflow unit (`host:` / `distribute:`) runs `report_finding` on the
  host, so its artifacts go into **that host's** store and are recorded
  `stored: copied` with no `host`. Recording them as `stored: external` with
  `host` set, and pulling them into the coordinator's store on first view, is
  specified but not built yet.

### Configuration

```toml
[findings]
artifact_max_bytes = 524288000   # copy cap per artifact file (default 500 MiB)
report_max_bytes = 262144        # serialized report budget (default 256 KiB)
ticket_patterns = ["ABC-[0-9]+"] # extra hints appended to the full-profile guidance
```

`ticket_patterns` lets an organisation say which reference formats count as
existing tickets. Nothing organisation-specific ships in rupu. The keys are also
listed in [configuration.md](configuration.md#findings).

### `rupu findings schema`

```
rupu findings schema                 Print the embedded draft-07 JSON Schema of a finding report
rupu findings schema --advertised    Print the simplified copy used in tool definitions
```

The schema is embedded in the binary and kept in lockstep with the validator by
a test, so external prompts and tools can be generated from rupu rather than
maintained by hand.

> Not built yet: Markdown / HTML / PDF export, the web report page, and the
> macOS views arrive in later plans. Today a `full` report is stored on the
> finding record and its artifacts are stored as described above.

## CLI

All inspection commands take the global `--format table|json|csv` flag
(`table` is the default; structured commands support `table`/`json`, tabular
ones also `csv`).

```
rupu coverage list                          List targets under .rupu/coverage/
rupu coverage templates {list, show}        List bundled catalogs / print one's concerns
rupu coverage catalog <target>              Print the effective catalog snapshot
rupu coverage show <target>                 Derived view: files touched + assertions + findings
rupu coverage audit <target>                Full report: per-concern coverage, gaps, cross-model, serendipitous findings
rupu coverage gap <target>                  Just the gaps (in-scope files lacking an assertion)
rupu coverage runs <target>                 List the runs recorded against a target
rupu coverage diff <target> [base compare]  What changed between two runs (defaults: previous latest)
rupu coverage rerun <target> <run_id>       Replay an agent run, appending a new run to the same target
```

Find a target id with `rupu coverage list`, then inspect:

```bash
rupu coverage audit a1b2c3d4e5f6          # human report
rupu --format json coverage audit a1b2c3d4e5f6   # machine-readable
rupu --format csv  coverage runs  a1b2c3d4e5f6
```

## The rerun → diff loop

Model output varies run-to-run — sometimes usefully (different angles surface
different bugs), but you could never evaluate *the combination* for
completeness. Now you can:

```bash
rupu run security-assessor "assess this repo"   # run 1
rupu coverage runs <target>                      # grab run 1's id
rupu coverage rerun <target> <run_id>            # replay it → run 2, same target
rupu coverage diff <target> <run_id> latest      # what did run 2 do differently?
```

`diff` reports four dimensions: **cell-coverage delta** (newly / no-longer
asserted), **verdict flips** (with `clean → finding` flagged), **findings
appeared / disappeared**, and **file-touch delta**.

> v1 `rerun` dispatch covers the **agent** surface; session / workflow / autoflow
> runs are captured but `rerun` returns an explicit "not yet supported" error.
> A replay re-resolves provider/model/concerns from the agent's current
> frontmatter (it goes through `rupu run`), so those manifest fields are a record
> of the original run, not replay inputs.

## Determinism

Everything the harness controls about what the model sees — concern ordering,
the catalog snapshot, and the live file list — is byte-stable and independent of
the order catalog inputs are declared in. That makes the model the *only* source
of run-to-run variance, which is exactly what `diff` measures. The harness does
**not** claim byte-identical model output: prompt *construction* is
deterministic; sampling is not.

## See also

- `docs/agent-format.md` — full agent frontmatter schema (incl. `concerns:` and `findingsProfile`)
- `docs/workflow-format.md` — workflow `findings_profile` (step and `defaults`)
- `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md` — the finding report design
- `docs/agent-authoring.md` — writing good agents
- Slice specs/plans under `docs/superpowers/{specs,plans}/` (search `coverage-harness`)
