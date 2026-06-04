# rupu agentic coverage harness — Slice B (reproducibility & run comparison)

**Status:** Draft
**Author:** matt
**Date:** 2026-06-02
**Surface:** workspace-level. Builds on Slice A. Touches `rupu-coverage` (diff engine, manifest type, determinism), the four run surfaces (`rupu-agent` / `rupu-orchestrator` / session / autoflow) for manifest capture and `rerun` dispatch, and `rupu-cli` for the new subcommands.
**Depends on:** Slice A (`docs/superpowers/specs/2026-05-23-rupu-coverage-harness-design.md`) — the three append-only JSONL ledgers, the effective-catalog snapshot, and `(workspace, scope_name) → target_id` derivation are prerequisites.

## Problem

Slice A made coverage *auditable*: for a given target, the ledgers record which files were touched, which `(concern, file)` cells were asserted, and what the verdict was — all stamped with `run_id` + `model` attribution. What Slice A deliberately left out is the ability to *reason across runs*.

The pain that motivated the whole harness is unaddressed without it. From the Slice A spec: matt runs the same security-review prompt against the same repo with the same model multiple times and gets substantially different findings each time. The variance is *not entirely bad* — different angles surface different bugs — but **the combination across runs is currently impossible to evaluate for completeness.** You can `rupu coverage audit` the accumulated state, but you cannot ask:

- "I ran this review twice. What did the second pass do *differently* — what did it newly cover, and where did its verdict disagree with the first?"
- "Re-run that exact review and show me what the model did differently this time."

There are two distinct sub-problems hiding here:

1. **Variance is unmeasured.** The ledgers hold every run's contribution, but there's no tool to *diff* one run against another. The variance matt observes is real signal, and it's sitting on disk unanalyzed.
2. **Variance is uncontrolled — partly by the harness itself.** Some run-to-run difference comes from the model's sampling (irreducible without provider-level seed control, which most providers don't offer). But some comes from the *harness*: if concern ordering, file ordering, or prompt interpolation shift between runs, the model isn't even seeing the same prompt, so you can't isolate model variance from scaffolding variance.

## Goals

- **Run-to-run diff.** Given two runs against the same target, report what changed: cell-coverage delta, verdict flips, findings appeared/disappeared, file-touch delta. Human-readable and JSON.
- **Reproducible prompt construction.** Everything the *harness* controls about what the model sees is byte-stable across runs, so the model becomes the only source of variance.
- **Faithful re-run.** Capture each run's defining inputs so a past run can be replayed turnkey, producing a new run against the same target — closing the `rerun → diff` loop.
- **Build on Slice A's substrate.** The diff is pure ledger analysis: no new instrumentation, no schema change to the three ledgers.
- **Honest about limits.** We do not claim byte-identical model outputs. Level-1 determinism covers prompt construction only; model sampling still varies, and the documentation says so.

## Non-goals (this slice)

- **Sampling-parameter control.** Threading `temperature` / `seed` / `top_p` through `LlmRequest` and every provider's request builder is deferred. `LlmRequest` exposes none of these today, and true reproducibility is unachievable across providers (Anthropic has no `seed`; OpenAI's is best-effort only). When the diff tooling proves the re-run workflow is something matt reaches for, sampling-param plumbing is the natural fast-follow. Called "Level 2" below.
- **Cross-model / cumulative diff selectors.** The diff engine is built around a *run selector* abstraction so `model:<name>` and `through:<run_id>` selectors slot in later, but v1 ships only `<run_id>`, `latest`, and `previous`.
- **Workflow / autoflow `rerun` dispatch.** Manifest *capture* happens on all four surfaces; v1 `rerun` *dispatch* targets the agent + session surfaces. Workflow/autoflow replay is a documented fast-follow on the same manifest (`rerun` of those surfaces returns a clear error, never a silent no-op).
- **Finding identity across runs.** Findings get unique ids per emission, so "the same finding in two runs" is matched heuristically (theme-based, see B-1). A stable finding-identity scheme is out of scope.
- **GUI surface in rupu-app.** Diff and rerun are CLI-only in this slice, consistent with Slice A.

## Design

Slice B is three independent plans, each delivering working software. They live in the existing `rupu-coverage` crate (no new crate) plus thin CLI wiring, mirroring how Slice A's audit module is structured (`pub use audit::generate::audit as run_audit`).

| Plan | Delivers | Touches | Depends on |
|------|----------|---------|------------|
| **B-1: Diff engine** (the star) | `rupu coverage diff` + `rupu coverage runs` | `rupu-coverage` (pure ledger read) + `rupu-cli` | nothing — works off existing ledgers |
| **B-2: Determinism (Level 1)** | byte-stable prompt construction | `rupu-coverage` catalog/render + file-list ordering | nothing |
| **B-3: Manifest + rerun** | `rupu coverage rerun` | `rupu-coverage` (manifest type) + run-path capture across 4 surfaces + `rupu-cli` | B-2 (so a replay renders faithfully) |

The narrative arc: **measure variance (B-1) → remove the harness's own variance (B-2) → faithfully reproduce a run (B-3) → diff the reproductions.**

---

### Plan B-1: the diff engine

#### Run selectors

A **run selector** resolves, against a target's ledgers, to a set of `run_id`s. v1 selectors:

| Selector | Resolves to |
|----------|-------------|
| `<run_id>` | that exact run |
| `latest` | the `run_id` with the most recent timestamp across the ledgers |
| `previous` | the second-most-recent `run_id` |

"Most recent" is computed from the maximum `at` / `declared_at` timestamp observed for each `run_id` across `files.jsonl`, `concerns.jsonl`, and `findings.jsonl`. Ties (same timestamp) break by `run_id` string order for stability. A selector that resolves to **zero** runs is an error (`no run matches '<selector>'`), distinct from a run that resolves but contributed nothing.

Future selectors — `model:<name>` (all runs by a model), `through:<run_id>` (cumulative state up to a run) — resolve to a *set* of run_ids and feed the same engine unchanged. That generality is the reason the unit is a "selector" rather than a bare id.

#### A run's contribution

For a selector's run-id set, the **derived verdict map** is: for each `(concern_id, file_path)` cell, the *last* assertion whose `run_id` is in the set (within-set supersession), exactly matching how `audit::generate` collapses within-run supersession in Slice A. The contribution also carries the set of touched file paths (from `files.jsonl`) and the findings (from `findings.jsonl`) attributed to those run_ids.

#### The four diff dimensions

`run_diff(base, compare)` over two contributions, where `base` is the earlier reference and `compare` is the run under inspection:

1. **Cell-coverage delta** — over `(concern_id, file_path)` cells:
   - *newly_asserted*: cells in `compare` not in `base` (the compare run examined a cell the base run didn't).
   - *no_longer_asserted*: cells in `base` not in `compare` (coverage the compare run dropped relative to base).
2. **Verdict flips** — cells present in *both* contributions whose status differs. Each flip records `(concern_id, file_path, base_status, compare_status)`. The `clean → finding` transition is flagged as high-signal (a later run found something an earlier run called clean).
3. **Findings appeared / disappeared** — findings carry per-emission ids, so cross-run identity is matched on the **`(concern_id, theme_key)` primitive already used by `audit::serendipitous`** (`theme_key` = first six words of the summary, lowercased). *appeared*: themes in `compare` absent from `base`. *disappeared*: themes in `base` absent from `compare`. The report labels this matching *theme-based, best-effort* so it never implies exact identity.
4. **File-touch delta** — *newly_touched* / *no_longer_touched*: file paths in one contribution's touch set but not the other.

All four are group-by + set-difference over data already on disk. The result is a `RunDiff` struct:

```rust
pub struct RunDiff {
    pub base_runs: Vec<String>,      // resolved run_ids for the base selector
    pub compare_runs: Vec<String>,   // resolved run_ids for the compare selector
    pub newly_asserted: Vec<CellRef>,
    pub no_longer_asserted: Vec<CellRef>,
    pub verdict_flips: Vec<VerdictFlip>,
    pub findings_appeared: Vec<FindingThemeRef>,
    pub findings_disappeared: Vec<FindingThemeRef>,
    pub newly_touched: Vec<String>,
    pub no_longer_touched: Vec<String>,
}
```

Rendered as a human table and `--format json`, mirroring `AuditReport`'s dual output. Vectors are sorted deterministically (by `concern_id` then `file_path`, or by path) so output is stable.

#### `rupu coverage runs`

The companion that makes diffing usable: lists the runs on a target so you can find ids. One row per `run_id` with `started_at`, `model`, `surface`, and contribution counts (cells asserted, findings, files touched). Sourced from the ledgers in v1 (and enriched by the B-3 manifest once present, e.g. `agent_name`).

---

### Plan B-2: determinism (Level 1)

Three sources of *harness* variance to pin, then a guarantee mechanism.

1. **Concern ordering** — already deterministic: `flatten` builds from `BTreeMap<String, Concern>` keyed by id, and `render::render_prompt_section` iterates `catalog.concerns` in that order. The work is *proving* and *locking* it: a byte-stability test (render the same catalog twice → identical) plus an `insta` snapshot of the rendered section.
2. **File ordering** — anywhere the harness emits file *lists* into the model's view, chiefly the `coverage_remaining`-style tool output listing in-scope-but-unasserted files. Sort by path before rendering. The audit's derived views already use `BTreeSet` (sorted); this targets the live tool surface.
3. **Nondeterministic interpolation** — audit the prompt-construction path for the three usual culprits: wall-clock timestamps, RNG, and `HashMap` iteration order. Any `HashMap` feeding a rendered string becomes a `BTreeMap` or is sorted before render.

**Guarantee mechanism:** a `determinism` test that assembles the full coverage-injected prompt section from a fixed catalog + fixed ledger state *twice* and asserts byte-equality. That test is the contract — it fails loudly if anyone later reintroduces variance.

**Explicit boundary:** Level 1 makes prompt *construction* deterministic. Model output still varies; the documentation and the `rerun` command output say so plainly. This honesty is precisely why B-1's diff exists — to measure the variance Level 1 cannot remove.

---

### Plan B-3: run manifest + `rerun`

#### Manifest file

`.rupu/coverage/<target_id>/runs.jsonl` — append-only, one `RunManifest` per run, consistent with the three existing JSONL ledgers.

#### `RunManifest` — the defining inputs to reconstruct a run

```rust
pub struct RunManifest {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub surface: Surface,            // workflow | agent | autoflow | session
    pub agent_name: String,
    pub provider: String,
    pub model: String,
    pub permission_mode: String,     // ask | bypass | readonly
    pub user_prompt: String,         // the turn's input message
    pub concerns: ConcernsBlock,     // resolved block (template refs + overrides),
                                     // NOT the flattened snapshot — replay re-resolves
                                     // the same catalog and stays small
    pub scope_name: String,
    pub workspace_path: PathBuf,     // with scope_name, recomputes the same target_id
    pub run_shaping: RunShaping,     // effort/thinking, context window — the params
                                     // already threaded through the run
}
```

#### Capture

A manifest row is appended at run start, at the seam where each surface already initializes its coverage target (the same place attribution is wired into the ledger). Writing one row is cheap; capture happens on **all four surfaces** so every run is replay-describable, even before `rerun` supports dispatching it.

#### `rupu coverage rerun <target_id> <run_id>`

1. Read that run's manifest from `runs.jsonl`.
2. **Reconstruct an invocation** — a pure library function in `rupu-coverage` that turns a `RunManifest` into an invocation spec. Keeping reconstruction in the library (not the CLI) honors the thin-CLI architecture rule.
3. **Dispatch** through the existing run machinery for the manifest's surface, producing a **new `run_id` appended to the same target**, so its contribution accumulates and is immediately diffable.
4. Print the new run id and suggest `rupu coverage diff <run_id> <new_run_id>`.

**Scope honesty (per the "no mock features" rule):** manifest *capture* is real on all four surfaces. v1 `rerun` *dispatch* targets the **agent + session** surfaces — the interactive ones matt actually re-runs. A `rerun` of a workflow/autoflow run returns a clear `rerun of <surface> runs not yet supported` error, never a silent no-op. Workflow/autoflow replay is a documented fast-follow on the same manifest.

---

### Error handling

- `diff` / `rerun` with an unknown `run_id` → `no run with id '<run_id>' on target '<target_id>'`.
- `diff` with a selector that resolves to zero runs → `no run matches '<selector>'` (distinct from a resolved-but-empty contribution, which renders as a normal "no changes" diff).
- `rerun` of an unsupported surface → `rerun of <surface> runs not yet supported` (names the surface).
- `rerun` whose manifest is absent (run predates B-3) → `no manifest for run '<run_id>' (runs before Slice B are not replayable)`.

## CLI surface

```
rupu coverage diff <target_id> [<base> <compare>]   # default: previous latest
rupu coverage runs <target_id>                       # list runs to find ids
rupu coverage rerun <target_id> <run_id>             # replay; appends a new run
```

All support `--format json|table`, matching the existing `rupu coverage audit` output convention. `diff` with no run arguments is the zero-argument "did my last pass add anything?" check (`previous` vs `latest`).

## Components

### Changes to `rupu-coverage`

- `src/diff/` — `types.rs` (`RunDiff`, `CellRef`, `VerdictFlip`, `FindingThemeRef`) + `generate.rs` (`run_diff`, selector resolution). Re-exported at the crate root as `run_diff`, mirroring `run_audit`.
- `src/ledger/manifest.rs` — `RunManifest`, `RunShaping`, and append/read helpers over `runs.jsonl`.
- `src/rerun.rs` — the pure `manifest → invocation spec` reconstruction function.
- Determinism (B-2): adjustments in `catalog/render.rs` and the live file-list tool output; new determinism + snapshot tests.

### Changes to the run surfaces

- `rupu-agent` / `rupu-orchestrator` / session / autoflow: append a `RunManifest` at run start where the coverage target is initialized. `rerun` dispatch (B-3, v1) wires the reconstructed invocation back through the agent + session entry points.

### Changes to `rupu-cli`

- `cmd/coverage.rs`: `diff`, `runs`, `rerun` subcommands — arg parsing + delegation to the `rupu-coverage` library functions. No business logic in the CLI.

## Testing

- **B-1:** diff engine over synthetic two-run ledgers with known flips / appears / disappears; selector resolution (`latest` / `previous` / explicit id, plus the zero-match error); CLI json + human smoke (mirrors the existing audit CLI test).
- **B-2:** byte-stability test (render twice → identical) + `insta` snapshot of the rendered section; file-list ordering test.
- **B-3:** manifest write → read round-trip; the reconstruct function builds the expected invocation spec *without* dispatching; the unsupported-surface and missing-manifest error paths.

## Alternatives considered

1. **A new `rupu-diff` crate.** Rejected: the diff is pure analysis over the Slice A ledgers and shares all their types. It belongs in `rupu-coverage` next to `audit`, exactly as `run_audit` lives there.
2. **Matching findings by a content hash for exact cross-run identity.** Rejected for v1: summaries are free-text and vary in wording run-to-run, so a hash is brittle. The theme-based primitive is already used for serendipitous clustering and is honestly labeled best-effort. A stable finding-identity scheme can come later if demand is real.
3. **Storing the flattened catalog snapshot in the manifest** instead of the `ConcernsBlock`. Rejected: the snapshot is large and would drift from how the agent was *configured*. Storing the block keeps the manifest small and makes replay re-resolve the catalog the same way the original run did.
4. **Forcing `temperature = 0` as the determinism story.** Rejected for this slice: it requires provider-wide plumbing `LlmRequest` doesn't have, can't deliver true reproducibility across providers, and would suppress the very output diversity matt values. Level 1 (prompt-construction determinism) plus diff tooling is the right altitude; sampling control is a deliberate Level-2 fast-follow.

## Risks

- **Finding match fuzziness.** Theme-based matching can over- or under-merge findings whose wording drifts. Mitigated by labeling the dimension best-effort and keeping cell-coverage + verdict-flips (which *are* exact) as the primary diff signal.
- **Manifest faithfulness.** A replay is only as faithful as the captured inputs. If a run's behavior depended on something not in the manifest (e.g. external repo state that changed), the rerun diff reflects that drift — which is itself useful signal, but the docs note replay reproduces *inputs*, not the world.
- **Determinism regressions.** New prompt-construction code could reintroduce variance. Mitigated by the byte-stability contract test failing loudly.
- **Partial `rerun` surface coverage.** v1 not supporting workflow/autoflow replay could surprise. Mitigated by an explicit named error (no silent no-op) and clear docs.

## Acceptance

- `rupu coverage diff <target> previous latest` against a target with ≥2 runs reports cell-coverage delta, verdict flips (with `clean → finding` flagged), findings appeared/disappeared, and file-touch delta, in both human and JSON form.
- `rupu coverage diff` with no run arguments defaults to `previous` vs `latest`.
- A selector matching no run errors clearly; a resolved-but-unchanged pair renders a clean "no changes" diff.
- `rupu coverage runs <target>` lists every run with id, timestamp, model, surface, and contribution counts.
- Rendering the coverage prompt section twice from identical inputs is byte-identical (contract test passes); the file-list tool output is path-sorted.
- Every run on every surface appends a `RunManifest` to `runs.jsonl`.
- `rupu coverage rerun <target> <run_id>` for an agent/session run replays its inputs, appends a new run to the same target, and prints a ready-to-run `diff` command; a workflow/autoflow `rerun` returns the explicit "not yet supported" error; a missing-manifest `rerun` returns the explicit "not replayable" error.

## Out of scope (future slices)

- **Level 2 — sampling-parameter control.** Thread `temperature` / `seed` through `LlmRequest` and provider request builders, exposed as `--reproducible` / `--temperature`. The natural fast-follow once the rerun→diff loop proves valuable.
- **Cross-model and cumulative diff selectors** (`model:<name>`, `through:<run_id>`) — the engine is built to accept them; the CLI surface and resolution land later.
- **Workflow / autoflow `rerun` dispatch** — manifests already capture these surfaces; only the replay dispatch is deferred.
- **Stable finding identity** — a content-addressed or agent-assigned finding key enabling exact cross-run finding tracking.
- **rupu-app diff/rerun view** — graphical surface for run comparison, Slice C/D-adjacent.
