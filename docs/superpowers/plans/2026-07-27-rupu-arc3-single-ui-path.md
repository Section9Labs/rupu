# Arc 3 — single UI path

Third arc of the integrity-remediation program
(`docs/superpowers/specs/2026-07-24-rupu-integrity-remediation-charter.md`).
Closes **I-28, I-29, I-30, I-31**. **I-32 is re-triaged out of this arc** — see below.

**Operator decision (settled, do not revisit):** the `next` UIs are proven and are
now the only UIs. matt has used them exclusively; the visual gate is cleared. The
"classic" renderers go.

## What reconnaissance changed about this arc

Three assumptions in the original triage were wrong. The plan below reflects the code,
not the triage.

1. **`CpConfig` carries `#[serde(default, deny_unknown_fields)]`**
   (`crates/rupu-config/src/policy_config.rs:18`). Hard-deleting the two keys would make
   every existing `config.toml` that sets them fail to deserialize — and because seven
   call paths use `.unwrap_or_default()`, that failure silently discards the operator's
   **entire** config. This is precisely the defect Arc 1 hit when `[retry]` was deleted
   (see I-9…I-20's closure). **Use the established deprecation shim instead of deleting.**

2. **No component file becomes fully dead.** The classic/next split is implemented as
   inline early-returns and ternaries *inside shared files* — deliberately, since several
   plans mandated "classic stays byte-identical". Only the two hooks are deleted outright.
   The rest is 20 files edited in place. The earlier "33 files, 52 branch sites" estimate
   understated the file count (63 including tests and docs) and overstated the deletions.

3. **I-32 is not a defect.** Its title lumps `branch` in with `split`/`join`, but
   `branch`'s `vhex` silhouette is deliberate, documented, and its height was *measured in
   headless Chrome* (`lib/workflowLayout.ts:32-49`) — refinement work, not placeholder
   work. `fanout`/`fanin` genuinely are declared provisional art
   (`kindVisuals.ts:48-51`, `nodeShapes.ts:353-358`, `:381-384`), but they render real
   5-vertex geometry with a safe content rect and tested handle anchors
   (`nodeShapes.test.ts:96,231,236,269-290,348`) — nothing renders as a blank box or a
   fallback rect. Per the charter's two-file split, "final art pending" is a deferred
   *feature*, not a thing that claims to work and doesn't. **Move I-32 to `TODO.md`,
   scoped to `fanout`/`fanin` only, and record the re-triage reason in `ISSUES.md`.**

## Global constraints

- `#![deny(clippy::all)]`; `unsafe_code` forbidden; workspace deps only.
- **Never run package-wide `cargo fmt`** — per-file only; `main` is fmt-dirty.
- Web: `cd crates/rupu-cp/web && npm run build && npx vitest run`.
- Known-red Rust baseline, DO NOT chase: 4 `linear_runner.rs` tests (filed as **I-4**,
  verified pre-existing on clean `main`); ANSI/colour-detection failures across
  `output::printer`, `cli_auth`, `cli_autoflow`, `cli_session`, `cli_workflow`,
  `output_line_stream`; `init_with_samples` template drift; a self-terminating
  `cmd::session` test. This worktree runs Homebrew Rust 1.95 against a pinned 1.88.
- **Validation bar (charter §3.2):** observe behavior at the consumer. For a deletion arc
  that means: the UI still renders and behaves as the `next` UI did, the test suite proves
  the surviving path, and a config carrying the old keys still loads.
- Branch: `arc3/single-ui-path`, **stacked on `arc2/safety`** (PR #543) because both edit
  `ISSUES.md`. Set the PR base to `arc2/safety`, not `main`, and say so in the PR body —
  a previous stacked PR in this repo merged into its base by accident.

---

### Task 1: I-28/I-29 (config half) — deprecate both keys, do not delete them

**Files:**
- Modify: `crates/rupu-config/src/policy_config.rs`, and the deprecation-warning site
  (`crates/rupu-config/src/config.rs:148-160`, `warn_deprecated_keys`)
- Test: `crates/rupu-config/tests/parse.rs:88-112` (four existing tests)

**Read `crates/rupu-config/src/config.rs:29-45` first** — the `[retry]` shim is the
established pattern in this codebase and the reference implementation for this task.

Replace the two `String` fields with the shim shape: retain the keys so
`deny_unknown_fields` still accepts them, mark them `skip_serializing` so they are not
written back, ignore their values, and emit a one-line deprecation warning naming each key
when present. Remove `default_agent_authoring_ui` / `default_workflow_editor_ui` and the
two `impl Default` entries.

- [ ] **Step 1: Write the failing test first.** The binding test is *not* "the field is
  gone" — it is **"a `config.toml` that sets `[cp].agent_authoring_ui = "next"` still
  parses, and every other `[cp]` key in that same file survives with its value intact."**
  Write it for both keys, and assert a sibling key (e.g. `gate_sweep_interval_secs`) keeps
  a non-default value — that is the assertion that would have caught the Arc 1 `[retry]`
  regression. Add a second test asserting the deprecation warning fires.
- [ ] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN**
- [ ] **Step 5:** Rewrite the four tests at `parse.rs:88-112` as accepted-as-no-op tests
  rather than deleting them.
- [ ] **Step 6: Commit** — `refactor(config): deprecate [cp].agent_authoring_ui and .workflow_editor_ui (I-28, I-29)`

`crates/rupu-cp/src/api/config.rs` needs **no** edit: it serializes `CpConfig` generically
at `:90`, with no per-field code.

---

### Task 2: I-28 (web half) — delete the classic agent-authoring UI

**Files:**
- Delete: `web/src/hooks/useAgentAuthoringUi.ts`, `web/src/hooks/useAgentAuthoringUi.test.ts`
- Modify: `pages/Agents.tsx`, `pages/AgentDetail.tsx`
- Tests: `pages/NewAgentModal.test.tsx`, `pages/Agents.test.tsx`, `pages/AgentDetail.test.tsx`

Delete, with the `next` arm becoming unconditional:
- `pages/Agents.tsx:129` — the `agentUi !== 'next'` arm (`setCreateOpen(true)`); keep
  `navigate('/agents/new')`. Then delete `function NewAgentModal` (`:194-371`, ~178 lines),
  its render at `:185`, and the `createOpen` state at `:33`.
- `pages/AgentDetail.tsx:217` — the `editing && !next` arm and its raw `<CodeEditor>`
  Cancel/Save block (`:229-250`); keep `<AgentBuilder>` (`:218-228`).

**Do not delete `components/CodeEditor.tsx` or `CodeHighlight.tsx`** — both are imported
from six other places including the next path and the non-editing agent view.
`api.generateAgent` also survives (`AgentNew.tsx:68`).

- [ ] **Step 1:** Delete the classic tests — `NewAgentModal.test.tsx` `describe` at `:52`
  and `it` at `:82`; `Agents.test.tsx:59-79`; `AgentDetail.test.tsx:246-255`.
  **Keep `NewAgentModal.test.tsx`'s `describe` at `:126`** (it asserts the surviving
  navigate-to-`/agents/new` behavior) and drop its `localStorage.setItem` seed at `:129`.
  Do not delete `NewAgentModal.test.tsx` wholesale — that would lose the kept test.
- [ ] **Step 2:** Delete the classic code. **Step 3:** `npm run build && npx vitest run` green.
- [ ] **Step 4:** Confirm by reading `App.tsx:77` that the `/agents/new` route is registered
  unconditionally (recon says it is) — report if not.
- [ ] **Step 5: Commit** — `refactor(cp-web): delete the classic agent-authoring UI (I-28)`

---

### Task 3: I-29 (web half) — delete the classic workflow-editor branches

**Files (all under `web/src/components/workflow-editor/`):** `WorkflowEditor.tsx`,
`WorkflowEditorGraph.tsx`, `NodePalette.tsx`, `WorkflowSettingsForm.tsx`, `StepForm.tsx`,
`nodes/EditableStepNode.tsx`, plus `pages/WorkflowDetail.tsx` and `web/src/styles.css`.

Leave the *prop plumbing* in place for now — Task 5 removes it. This task removes the
branch **logic** only, making every `next` arm unconditional.

Branch sites (classic side deleted in each):
- `WorkflowEditor.tsx` — `:161` default tab, `:492` SplitPane-vs-collapsible source,
  `:537` palette container, `:546` source-pane id, `:557` hide-source button,
  `:580/:582/:586` rail sizing, `:588` resize separator, `:604` Blocks tab, `:614` tab order
- `WorkflowEditorGraph.tsx` — `:645` edge-memo gate, `:653` marker colour, `:678`
  `edge.animated`, `:687` branch stroke, `:695-704` then/else chips, `:707-720` plain-edge
  styling, `:980` `wfx-canvas`, `:1074` palette portal, `:1120` background, `:1122`
  Background variant
- `NodePalette.tsx` — `:352` item set, `:354` connector groups, `:559` early return, then
  delete the float-dock JSX (`:596-638`) and the `KIND_COLOR` map (`:33-48`, whose own
  comment at `:30` calls it "classic-only fixed hex")
- `WorkflowSettingsForm.tsx` — keep the `next` early return at `:49`; delete everything after it
- `StepForm.tsx` — `:110`/`:127` `NEXT_ONLY_KINDS` filtering (all kinds now offered),
  `:517/:627/:708/:720/:1316` `size`, `:1184` and `:1585` Approval prompt → `ExpressionField`
- `nodes/EditableStepNode.tsx` — keep the `next` early return at `:365`; delete the classic
  card JSX after it, and the `?? 'classic'` at `:353`

**Careful, not mechanical:** `NodePalette`'s `variant="rail"` block (~`:180-560`) is gated by
`variant`, *not* by the flag. Evaluate its two internal `workflowEditorUi === 'next'`
conditions by hand.

- [ ] **Step 1:** Delete/collapse the classic tests listed in the recon inventory for these
  files (`NodePalette.test.tsx`, `WorkflowEditor.test.tsx`, `WorkflowSettingsForm.test.tsx`,
  `StepForm.test.tsx`, `EditableStepNode.test.tsx`, `WorkflowEditorGraph.test.tsx`).
  `StepForm.test.tsx:514` is a *mixed* next/classic assertion — **update**, don't delete.
- [ ] **Step 2:** Delete the classic branches. **Step 3:** build + vitest green.
- [ ] **Step 4: Commit** — `refactor(cp-web): delete the classic workflow-editor branches (I-29)`

---

### Task 4: I-30 — collapse the run-graph dual path

**Files:** `components/RunGraph.tsx`, the six node components under `components/graph/`
(`StepNode`, `GateNode`, `ActionNode`, `ParallelNode`, `PanelLoopNode`, `FanoutNode`),
`web/src/styles.css`, `components/RunGraph.edges.test.tsx`, `components/graph/*.test.tsx`.

The run graph does not pick between two renderers — it picks between two paint schemes
inside one. Collapse it:
- `RunGraph.tsx:74` — remove `ui` from `NodeData`; `:148` — stop pushing it into node data;
  `:106` — drop the `useWorkflowEditorUi()` call; `:167-188` — delete the classic edge arm,
  keep `:190-215` (`runKindAccent` + `rg-edge-flow`)
- Each of the six node files: delete the `const next = data.ui === 'next'` derivation and
  inline the next branch (line refs in the recon inventory)
- `styles.css:219-231` — delete `.rg-edge-active` / `.rg-edge-await` (classic-only per the
  comment at `:234-236`); keep `.rg-edge-flow`

**Keep `components/graph/kindBridge.ts`** — it becomes the unconditional source of node and
edge colour. Also keep `components/graph/stepStyle.ts` (used by both paths *and* by
`components/run/StepTranscriptBrowser.tsx:9`).

- [ ] **Step 1:** `RunGraph.edges.test.tsx` — delete the classic test (`:78-98`), the
  `uiMock` (`:23,69`) and the `vi.mock` (`:24-25`). `graph/ContainerNodes.test.tsx`
  (`:106-111,140-145,172-177,200-219`) asserts *classic ≠ next* throughout — collapse each
  to a next-only assertion rather than deleting. `graph/StepNode.test.tsx:25-26` deletes.
- [ ] **Step 2:** Collapse. **Step 3:** build + vitest green.
- [ ] **Step 4: Commit** — `refactor(cp-web): collapse the run-graph classic/next dual paths (I-30)`

---

### Task 5: I-31 — remove the hook, the type, and all prop plumbing

Only after Tasks 3 and 4 have removed every consumer of the value.

- Delete `web/src/hooks/useWorkflowEditorUi.ts` and `useWorkflowEditorUi.test.ts`
- Remove the `workflowEditorUi` prop and its `WorkflowEditorUi` type import from the 13
  files listed in the recon inventory (`NodePalette`, `WorkflowEditorGraph`,
  `WorkflowEditor`, `WorkflowSettingsForm`, `StepForm`, `nodes/EditableStepNode`, and the
  six `components/graph/` nodes), and the threading at `pages/WorkflowDetail.tsx:441`/`:83`
- Strip the prop from every test harness and the ~90 `workflowEditorUi="next"` props
- Confirm both localStorage keys (`rupu.cp.agentUi`, `rupu.cp.workflowEditorUi`) have no
  remaining reader or writer

- [ ] **Step 1:** Remove. **Step 2:** build + vitest green.
- [ ] **Step 3: Verify the keys are gone** —
  `grep -rn "agentUi\|workflowEditorUi\|agent_authoring_ui\|workflow_editor_ui" crates/rupu-cp/web/src crates/rupu-config/src`
  must return only the Task 1 deprecation shim. Paste the output into the report.
- [ ] **Step 4: Commit** — `refactor(cp-web): remove both UI hooks and their localStorage overrides (I-31)`

---

### Task 6: I-32 re-triage + docs + close-out

- [ ] **Step 1: Re-triage I-32.** Move it to `TODO.md` as a deferred feature, scoped to
  `fanout`/`fanin` only (not `branch`). In `ISSUES.md` mark the row `moved → TODO.md` with
  the reason: the shapes render real, tested geometry and are declared provisional *art*,
  which is a deferred feature rather than a defect. Never delete the row.
- [ ] **Step 2: `CLAUDE.md:41`** — drop the `[cp].workflow_editor_ui = "next"` gating clause
  from the rupu-orchestrator bullet; the kindBridge sentence stays true and stays.
  Update `ISSUES.md:754-761`'s config-key inventory for the two deprecated keys.
  **Leave the 18 historical plan/spec files alone** — they are dated records of decisions
  taken at the time, not live documentation, and rewriting them would falsify the record.
- [ ] **Step 3: Full verification** — `cargo build --workspace`, `cargo test -p rupu-config`,
  `npm run build`, `npx vitest run`. Compare any Rust failure against the known-red baseline.
- [ ] **Step 4:** Confirm I-28…I-31 each sit in `## Fixed` with a validation naming what was
  observed, and I-32 is marked moved. No row silently deleted.
- [ ] **Step 5:** Open the PR **against base `arc2/safety`**.

---

## Deferred out of this arc

- **I-32** → `TODO.md` (final art for the `fanout`/`fanin` silhouettes).
- **The 18 historical plan/spec files** referencing the two flags are left as-is by design;
  they record decisions accurately for their date.
