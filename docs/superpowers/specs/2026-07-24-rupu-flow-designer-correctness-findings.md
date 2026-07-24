# Flow Designer (`next`) — correctness deep-dive: findings & proposed direction

**Date:** 2026-07-24
**Status:** Findings complete; architecture direction pending operator sign-off.
**Scope audited:** `crates/rupu-cp/web/src/components/workflow-editor/**`, `src/lib/workflowGraph.ts`, `src/lib/workflowLayout.ts`, `src/lib/workflowMeta.ts`, `src/pages/WorkflowDetail.tsx`, and the backend contract in `crates/rupu-orchestrator/src/workflow.rs` + `crates/rupu-cp/src/api/workflows.rs`. Three independent audits (form/graph/YAML sync; 400-flood + node-loss reproduction; full field-by-field schema contract).

## 0. Headline

1. **The YAML "language" is correct.** The editor's model is a *faithful subset* of the backend schema — no wrong-shaped keys, no wrong nesting. **All 15 real `.rupu/workflows/*.yaml` round-trip cleanly and pass `Workflow::parse`.** Existing content is not being corrupted by the schema itself.
2. **Every bug is in the editor's edit → serialize → reconcile loop**, and every one is a symptom of the same disease: **the canvas and the YAML are allowed to drift apart.**
3. **All of it is pre-existing** — from when branch routing (#503) and action authoring (#516) first shipped. **None of it is a regression from the node-shapes work or the #521 run-graph merge** (verified per-commit; those touched only rendering/geometry and the read-only run graph).

## 1. Root causes (the disease, five faces)

**RC1 — Two sources of truth for branch routing.** A branch's connections live in *both* `graph.edges` (the canvas lines, tagged `branch:'then'|'else'`) *and* the node's `thenTargets`/`elseTargets` arrays. Of the five sites that mutate routing, only two (`applyConnect`, `applyRemoveEdges`) keep both in lockstep. The serializer emits from the arrays only. (Audit 1.)

**RC2 — `commit()` can half-apply an edit.** `WorkflowEditor.tsx:320-345`: `setGraph(next)` updates the canvas *unconditionally*, but `onYamlChange` is skipped when `graphToWorkflowObject` returns `{error}` (a cycle). No error is surfaced. The canvas now silently diverges from the YAML, and the next reconcile rebuilds from the stale YAML — deleting every node/edit since the divergence. (Audit 2b.)

**RC3 — The reconcile "safety net" is dead for graph-driven edits.** The echo guard (`lastSeenYaml`, `WorkflowEditor.tsx:266-290`) is set *before* the edit echoes back, so `reconcileFromYaml`/`yamlToGraph` — the only code that re-derives edges from data — never runs for a form/canvas edit. It only fires when you hand-edit the raw YAML pane. (Audit 1.)

**RC4 — Serialization is lossy/partial at the edges.** Truthy/`undefined` guards omit keys the backend requires, and some fields are dropped entirely. (Audits 2, 3.)

**RC5 — No validity gate on emit.** Every keystroke re-serializes and POSTs to `/validate` regardless of state (`WorkflowDetail.tsx` debounced 400ms). Incomplete-but-in-progress nodes therefore 400 constantly — the "flood." (Audit 3 §9.)

## 2. Issue register (prioritized)

### P0 — data integrity (silent loss / corruption / the reported symptoms)
| # | Symptom | Root cause | Site |
|---|---|---|---|
| P0.1 | **Nodes disappear / steps break.** Branch Then/Else picker offers ancestors → pick one → cycle → `commit` half-applies → next reconcile nukes everything since. | RC2 (+ no cycle guard in the form picker) | `WorkflowEditor.tsx:320-345`; `StepForm.tsx:603` (`candidates` = every other node) |
| P0.2 | **Branch "select true" draws no line.** Then/Else checkboxes write `thenTargets` only; edges never re-derived (RC3). YAML is correct, canvas isn't. | RC1 + RC3 | `StepForm.tsx:605-616`; `WorkflowEditor.tsx:352-364` |
| P0.3 | **A fresh `action` node silently becomes a `step`.** `if (d.action)` omits the key when unset → re-parses as kind `step` → the parallelogram becomes a rectangle on next reconcile. | RC4 | `workflowGraph.ts:448` vs `:193-199` |
| P0.4 | **Panel `when:` (and `continue_on_error`/`actions`) dropped on save.** The panel emitter arm never emits them. | RC4 | `workflowGraph.ts:424-438` |
| P0.5 | **Action-shaped `on_reject` row corrupts on any edit** — adds `agent`/`prompt` beside `action` → `ActionMutuallyExclusive` 400. | RC4 (form assumes agent/prompt shape) | `StepForm.tsx:702-704, 781-816` |

### P0/P1 — the 400 flood (guaranteed rejections reachable from the UI)
| # | Trigger | Emitted | Backend rule |
|---|---|---|---|
| F.1 | Fresh `branch`, no condition | `branch: {}` | `Branch.condition` required — *raw serde error, not the intended friendly `BranchEmptyCondition`* |
| F.2 | Panel "Enable gate", unfilled | `panel.gate: {}` | `PanelGate`'s 3 fields all required |
| F.3 | "Add sub-step", unfilled | `parallel: [{id}]` | `SubStep.agent`/`prompt` required |
| F.4 | (soft) `max_parallel: 0` | `max_parallel: 0` | `InvalidMaxParallel` (no client min-guard) |
| F.5 | (soft) severity typo | `..._or_above: "midium"` | `Severity` enum (free-text field, no `<select>`) |

Note: F.1–F.3 are *expected during authoring* — the real fix is (a) not flooding the console/validate with in-progress states, and (b) emitting a friendly error, not a raw serde one.

### P1 — desync / invisible state
- **Node deletion leaves dangling branch targets** in surviving nodes' `thenTargets`/`elseTargets` (saves a reference to a deleted step). `applyDelete` scrubs edges but not the arrays. (Audit 1 §3.)
- **Kind-switch away from `branch`** leaves orphaned `branch`-tagged edges pointing at a handle that no longer exists. (Audit 1 §4.)
- **Kind-switch `step`↔`for_each` drops `agent`/`prompt`** even though both kinds use them. (Audit 3 §8.3.)
- **Leftover `approval:` on a `branch`/`panel`** is emitted but invisible in the form (no way to see/clear without switching back to `step`). (Audit 3 §8.4.)

### P2 — authoring gaps (present-but-not-editable; not corruption)
- `Approval.notify` — parsed & preserved, **zero editing UI**.
- `Autoflow.source` / `Autoflow.priority` — preserved, no control.
- `with:` params are **string-only** in the editor — a numeric/bool/list value shows blank and narrows to a string if edited.
- `Panel`/`Branch`/`Approval` sub-schemas have **no passthrough bag** (unlike Step + top-level) — harmless today (fully covered), a latent blind spot if the backend schema grows.

## 3. Proposed direction — one source of truth

The operator's instinct is correct and it dissolves RC1–RC3 at once: **stop storing anything twice; derive the views.**

The open decision is *what* the single source is:

**Option A — Canonical model as source (recommended).** The parsed workflow model is authoritative. The YAML text and the canvas are *both* derived projections. Concretely: **edges become a pure derived view** (recomputed from node data on every edit, exactly as `yamlToGraph` already does on load), there is **exactly one mutate→serialize→re-derive path** (no half-apply), and the serializer is made **total and lossless** (round-trip-stable for every kind). The YAML file remains the authoritative *artifact* that runs — it is just always generated from the model, never independently edited into a divergent state. Hand-edits to the YAML pane parse back into the model (as today), so YAML stays a first-class input.
- *Pros:* kills RC1/RC2/RC3 structurally; node positions and comments-on-hand-edit are manageable (positions already preserved via `reconcileGraph`); invalid intermediate states stay on the canvas without nuking it.
- *Cons:* requires making serialization total + round-trip-tested for all kinds (P0.3/P0.4/F.1–F.3 fixes are prerequisites, not optional).

**Option B — Raw YAML text as source.** The YAML string is literally the only state; the canvas is re-parsed from text on every edit.
- *Pros:* one artifact, matches "the yaml file" literally; canvas and YAML can never disagree by construction.
- *Cons:* node **positions aren't in the YAML** → every edit would re-run auto-layout (nodes jump) unless positions are stored in a side-channel anyway (which re-introduces a second source); half-typed YAML can't parse → the canvas "pauses" constantly mid-edit; canonical re-dump loses comments on every graph edit. In practice this needs a position side-channel, which is Option A wearing a different hat.

Both make the YAML the thing that runs and the thing you can hand-edit. Option A is the robust engineering form of the same intent; Option B's honest version collapses into A once positions are accounted for.

## 4. Fix plan (shape, pending direction)

1. **Edges are derived, never stored.** One `deriveEdges(nodes)` (chain + branch arms) run on every commit; delete `graph.edges` as independent state. → P0.2, P1 dangling/orphan edges.
2. **`commit()` cannot half-apply.** Either it produces valid YAML and updates both views, or it surfaces the error and does not mutate the canvas. Add a cycle guard to the branch target picker so P0.1 is unreachable in the first place. → P0.1.
3. **Total, round-trip-tested serialization.** Emit modeled keys even when the backend requires them (or block emit on that node's validity); fix action-key omission, panel `when`, sub-step required fields; a round-trip test per kind (`yamlToGraph → graphToWorkflowObject → parse` stable) as the regression net. → P0.3, P0.4, F.1–F.3.
4. **Friendly validation, no flood.** Gate the `/validate` POST on the graph being non-transient (or debounce + suppress the console noise + map raw serde errors to friendly ones). → RC5, F.1.
5. **Close the authoring gaps** (severity `<select>`, `max_parallel` min-guard, `with:` typed values, notify UI, on_reject kind-awareness) as a follow-on. → P1/P2.
6. **Guardrails:** passthrough bags for `Panel`/`Branch`/`Approval`; kind-switch preserves shared fields.

## 5. Out of scope of this doc
- The branch **node shape** refinement (operator approved option A, "chevron rectangle," but it must be made visually distinct from the `for_each` hexagon) — tracked separately.
