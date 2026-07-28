# Arc 4 — gate/action correctness

Fourth arc of the integrity-remediation program
(`docs/superpowers/specs/2026-07-24-rupu-integrity-remediation-charter.md`).
Covers **I-33 … I-44**.

Every item below was verified against the code before this plan was written. Four of the
twelve triage titles were wrong or mis-scoped and are corrected here — **do not work from
the triage titles**.

## Corrections to the triage (read first)

| ID | Triage said | Actually |
|---|---|---|
| **I-35** | "CP web reject (and host-connector, TUI, cancel)" | There is **no TUI** — the affected GUI is the GPUI desktop app `rupu-app`. And **SSH + tunnel connectors DO run the chain** (they re-enter the CLI). The real set is: web-local, `LocalHostConnector`, `HttpHostConnector`, `rupu-app`, and **both cancel paths**. |
| **I-39** | "old binaries die on new `events.jsonl`" | Nothing dies — every reader `filter_map`s errors and **silently skips the line**. The severe case is **`step_results.jsonl`**, where skipping a row silently loses a prior step's output on resume (steps re-run, template context loses values). `events.jsonl` is cosmetic by comparison. |
| **I-40** | "action steps show an empty transcript" | **False.** `execute_action_step` writes `ActionEmitted` **and** `ToolAudit`, and the viewer has a dedicated standalone-action fallback for `tool_audit` (`transcriptView.ts:364`) with a test at `transcriptView.test.ts:369`. The action node *does* render. What is lost is the **rendered `with:` args** plus `allowed`/`applied`/`reason`. |
| **I-41** | "`rupu workflow show` renders gate/action as linear" | Only the **steps table** does. The **graph is correct** (`git_graph.rs:278,308` emit `gate` / `action · <tool>`), and the graph is the primary view. |

**I-37 needs care:** its surrounding code was just rewritten by Arc 2 (I-25, commit
`7a2286d2`). The hint-only behavior is *deliberate and documented* at
`cmd/workflow.rs:1920-1946`. **Fixing I-37 inside the listing would directly re-break
I-25** — a listing must not have side effects. I-37 is properly read as "there is no
non-`cp serve` resume path for `on_timeout: approve`", which is the same shape as the
already-filed **I-80**. Treat it as documentation + a possible explicit opt-in command,
never as "make the listing resume".

## Global constraints

- `#![deny(clippy::all)]`; `unsafe_code` forbidden; thiserror in libs, anyhow in the CLI.
- **Never package-wide `cargo fmt`** — per-file only.
- **Never `git commit -a`/`-am`** — stage explicit paths. **Never `git stash`.**
- Known-red Rust baseline, DO NOT chase: 4 `linear_runner.rs` tests (filed as **I-4**,
  pre-existing on clean `main`); ANSI/colour failures across `output::printer`,
  `cli_auth`, `cli_autoflow`, `cli_session`, `cli_workflow`, `output_line_stream`;
  `init_with_samples` template drift; a self-terminating `cmd::session` test.
- Web suite is load-sensitive: healthy run ~12s. A multi-minute run with failures means
  contention — re-run before believing it.
- **Validation bar (charter §3.2):** observe the behavior at the consumer. For this arc
  that means real workflow runs and real persisted state, not parse assertions.
- Branch `arc4/gate-action-correctness`, **stacked on `arc3/single-ui-path`** (PR #544),
  which is itself stacked on `arc2/safety` (#543). Set the PR base accordingly.

---

### Task 1: I-33 — a templated scalar must reach a typed tool parameter (P0)

**This is the arc's real P0.** The `action:` feature's headline use case — "comment on
issue #N", where N comes from an input or a trigger event — is **literally unexpressible
today**.

**Why:** `validate_action_step` (`workflow.rs:1319-1370`) checks *keys only*, by design
("VALUES are not checked — they may be minijinja templates rendered at runtime").
`render_action_args` (`runner.rs:4528-4563`) renders a string leaf to
`serde_json::Value::String` **unconditionally**. The dispatcher then does a typed serde
parse — e.g. `CommentIssueArgs { number: u64, .. }` (`crates/rupu-mcp/src/tools/issues.rs:28`)
— which rejects `"42"`. There is no coercion anywhere (grep for `deserialize_with` /
`as_u64` in `crates/rupu-mcp/src`: zero hits). Aggravating: even a declared `type: int`
input cannot help, because `StepContext.inputs` is `BTreeMap<String, String>`
(`templates.rs:56`) and `render_step_prompt` returns `String` regardless of source.

Scope: every integer param in the catalog — `issues.get/comment/update_state.number`
(u64), `scm.prs.get/diff/comment.number` (u32), every `.limit`. **Every existing test
templates only string fields**; the repo's own fixture uses a literal `number: 7`
(`tests/action_step.rs:259`).

- [ ] **Step 1: Write the failing test first.** In `crates/rupu-orchestrator/tests/action_step.rs`,
  using the existing fake-connector + `ToolDispatcher` harness: a workflow with
  `inputs: { number: { type: int } }` and a step
  `action: issues.comment` / `with: { project: "acme/w", number: "{{ inputs.number }}", body: "hi" }`.
  Assert the fake connector **received `42` as a number** and the run succeeded.
  Pre-fix this fails with `invalid type: string "42", expected u64`.
  Add a negative test: a non-numeric template into a numeric field still fails, with a
  clear error naming the step and parameter.
- [ ] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN**

  **Implementation:** coerce in `render_action_args` using the tool's `input_schema`,
  which is already reachable (`rupu_mcp::tools::tool_catalog()`, the same source
  `validate_action_step` uses). For each `with:` key, look up its declared JSON-schema
  type; when the rendered leaf is a `String` **and the whole leaf was a template** and the
  schema says `integer`/`number`/`boolean`, parse it and emit the typed `Value`. If the
  parse fails, return a `RunWorkflowError` naming the step, the parameter, the expected
  type and the offending value — do **not** silently pass the string through to a generic
  serde error.

  Two judgment points, decide and record: (a) only coerce when the schema declares the
  type — never guess from the value's shape, or `"007"` and version strings will be
  mangled; (b) a *partial* template (`"issue-{{ n }}"`) into a numeric field is an author
  error — fail with the clear message rather than coercing.
- [ ] **Step 5:** Consider moving type-checking to parse time so authors fail fast. If the
  template makes that impossible (it does for the value itself), at minimum reject a
  `with:` leaf that is a **literal** of the wrong type at parse time. State what you did.
- [ ] **Step 6: Commit** — `fix(orchestrator): coerce templated action args to their schema type (I-33)`

---

### Task 2: I-34 — an action step's output must be indexable (P0)

An action step's `output` is a JSON **string** end to end: the dispatcher returns
`Result<String, _>` (`crates/rupu-mcp/src/dispatcher.rs:26`), `StepResult.output` is
`String` (`runner.rs:4664`), and it reaches minijinja verbatim as `StepOutput.output:
String` (`templates.rs:141`). **`grep -rn "add_filter" crates/` returns zero hits
repo-wide** — the only customization is `env.add_function("read_file", …)`
(`templates.rs:428-458`). minijinja is pinned with `features = ["json"]`, which adds
**only `tojson`** — there is no inverse filter in minijinja at all.

So `{{ steps.<action>.output }}` cannot be indexed, and there is no escape hatch. Action
steps are effectively **write-only**: you can call a tool but cannot consume its result.
(Contrast: gates got a bespoke escape hatch — `gate_decision()` pre-parses gate JSON into
`steps.<id>.decision` at `runner.rs:4490`. Nothing equivalent exists for actions.)

- [ ] **Step 1: Write the failing test.** A two-step workflow: an action step whose fake
  connector returns `{"number": 7, "title": "x"}`, then a step whose prompt is
  `{{ steps.act.output | fromjson | attr('title') }}` (or the idiomatic
  `{{ (steps.act.output | fromjson).title }}` — use whichever minijinja supports; verify).
  Assert the rendered prompt contains `x`.
- [ ] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN**

  Register a `fromjson` filter next to the existing `add_function("read_file", …)` in
  `templates.rs`. On invalid JSON, produce a clear minijinja error naming the filter —
  not a panic, and not silent `undefined`.
- [ ] **Step 5: Also expose the parsed form directly.** Consider `steps.<id>.json` mirroring
  the gate `decision` precedent, so the common case needs no filter. If you add it, test
  it; if you decide against, record why.
- [ ] **Step 6: Document** the filter in `docs/workflow-format.md` alongside the action-step
  docs. An undocumented filter is a silent feature.
- [ ] **Step 7: Commit** — `feat(orchestrator): add a fromjson filter so action output is indexable (I-34)`

---

### Task 3: I-35 — every reject and cancel path must run the `on_reject` chain (P0)

`run_reject_cleanup` is not merely "cleanup" — it is **the only caller of
`emit_gate_result`**, so a path that skips it records no gate decision at all (this is
also why [[I-36]] exists).

Verified per-path (do not re-derive; **do** re-verify line numbers, they drift):

| Path | file:line | Runs chain today |
|---|---|---|
| CLI `workflow reject` | `cmd/workflow.rs:2967` | ✅ |
| CLI `approve` on a timed-out reject gate | `cmd/workflow.rs:2364` | ✅ |
| `cp serve` sweep | `cmd/cp.rs:888` | ✅ |
| SSH connector | `host/ssh.rs:1204` | ✅ (shells the CLI) |
| Tunnel connector | `host/tunnel.rs:230` → `cmd/node.rs:490` | ✅ (spawns the CLI) |
| **CP web `POST /api/runs/:id/reject`** | `crates/rupu-cp/src/api/runs.rs:205` | ❌ |
| **`LocalHostConnector::reject_run`** | `host/local.rs:213` | ❌ |
| **`HttpHostConnector::reject_run`** | `host/http.rs:288` | ❌ |
| **`rupu-app` reject** | `window/mod.rs:287` → `executor/in_process.rs:336` | ❌ |
| **`RunStore::cancel` on an awaiting run** | `runs.rs:2062` (`self.reject(...)`) | ❌ — drives **both** CLI `workflow cancel` and CP web `/cancel` |

**Neither cp-serve worker covers the gap** — verified: the resume worker filters on
`resume_requested_at.is_some()` (`runs.rs:1946`), which a reject never sets; and the gate
sweep only matches `AwaitingApproval` / `Running` / `Pending`, so an already-terminal
`Rejected` run hits `_ => {}` (`cmd/cp.rs:948`) forever. The library documents the gap
itself at `runs.rs:1627-1634`.

**Architectural constraint — respect it.** `rupu-cp` is deliberately read-only
("record-in-CP, resume-in-cp-serve"): it must **not** grow a workflow runtime. So the fix
for the web/local/http paths is *not* to call `run_reject_cleanup` inside `rupu-cp`.

- [ ] **Step 1: Choose the mechanism and say why.** The shape that fits the existing
  architecture is a **marker**, exactly like the resume path: have the reject record a
  "cleanup requested" marker on the `RunRecord`, and have the `cp serve` sweep grow an arm
  that picks up rejected-with-pending-cleanup runs and executes the chain — reusing
  `build_reject_cleanup_opts` + `run_reject_cleanup`, and clearing the marker on success.
  This keeps `rupu-cp` read-only and gives the desktop app and both cancel paths the same
  fix for free. If you find a better shape, argue for it in the report before implementing.
- [ ] **Step 2: Write the failing test** — reject via the **web API handler** (not the CLI) a
  run whose gate has an `on_reject` chain that creates a marker file; run the sweep; assert
  the marker file exists and the gate decision was recorded. Pre-fix the file never appears.
  Add a second test for `RunStore::cancel` on an `AwaitingApproval` run.
- [ ] **Step 3: RED**, **Step 4: implement**, **Step 5: GREEN**
- [ ] **Step 6:** Ensure the `rupu-app` path benefits (it goes through `in_process.rs:336`
  → `run_store.reject`). If the marker lives in `RunStore::reject`, it does automatically —
  confirm and state so.
- [ ] **Step 7: Note the operator consequence.** This makes cleanup for web/app/cancel
  rejects depend on `cp serve`, exactly like **I-80**. Cross-reference I-80 rather than
  filing a duplicate, and make sure I-80's write-up mentions the widened scope.
- [ ] **Step 8: Commit** — `fix(orchestrator,cp): run on_reject cleanup for web, app and cancel rejects (I-35)`

---

### Task 4: I-36 + I-38 — gate decision provenance (merge these)

They are one gap: **the gate audit record cannot tell you who decided, or whether a human
decided at all.**

- **I-36:** `cmd/workflow.rs:2967` short-circuits on `Some(0)` — an empty `on_reject`
  chain skips `run_reject_cleanup`, the only `emit_gate_result` caller, so **no gate
  decision row is written**. `RunStore::reject_gate` keeps the reason (in `error_message`)
  and the time (`finished_at`) but explicitly discards the actor: `let _ = approver;`
  (`runs.rs:1707`). `approve_gate` does the same (`runs.rs:1590`) with a comment claiming
  "identity recorded in transcript via runner re-entry" — **that comment is not backed by
  code**; `emit_gate_result` has no approver field.
- **I-38:** the sweep resolves an `on_timeout: approve` gate by spawning
  `rupu workflow approve` (`cmd/cp.rs:821`), which lands in the normal approve path and
  emits `via: "human"` (`runner.rs:1997`). A machine approval is indistinguishable from an
  operator's. Reject *does* thread `"timeout"` correctly — the asymmetry is the bug.

- [ ] **Step 1: Write failing tests** — (a) reject a gate with an **empty** `on_reject` and
  assert a gate decision row exists with the reason and the actor; (b) let a gate
  auto-approve on timeout via the sweep and assert `via == "timeout"`, not `"human"`.
- [ ] **Step 2: RED**, **Step 3: implement**
  - Emit the gate decision unconditionally on reject, not only when a chain exists.
  - Add an `approver`/`actor` field to the decision JSON and stop discarding it in
    `approve_gate`/`reject_gate`. Fix or delete the false comment at `runs.rs:1590`.
  - Thread the timeout provenance through the approve path (the sweep already knows; the
    CLI's `resolve_approve_gate` detects it and only `println!`s — `cmd/workflow.rs:2298`).
- [ ] **Step 4: GREEN. Step 5: Commit** — `fix(orchestrator): record gate decision provenance on every path (I-36, I-38)`

---

### Task 5: I-39 — re-scope to `step_results.jsonl`, then fix

**Correct the title first** (in `ISSUES.md`): nothing "dies". Every reader is tolerant and
**silently skips the line** — `api/events.rs:286`, `api/graph.rs:209`,
`transcript_tail.rs:112`, `executor/file_tail.rs:47,88`, `agent/runner.rs:1964,3003,3306`,
and `RunStore::read_step_results` (`runs.rs:867`, with the comment *"Skip malformed rows
rather than failing the read"*).

**The real defect is worse than the filed one.** For `step_results.jsonl`, skipping a row
means an older binary resuming a run **silently loses a prior step's output** — the
template context loses values and steps re-run, with no error. `StepKind` has 10 variants
and no `#[serde(other)]` (`runs.rs:415-469`); `#[serde(default)]` on
`StepResultRecord.kind` (`runs.rs:486`) covers only a *missing* field, not an unknown
value. The `Split`/`Join`/`Loop` doc comments (`runs.rs:426-465`) narrate **three separate
rounds** of exactly this variant-addition, so this has already bitten repeatedly.

`#[serde(other)]` is viable — serde supports it for the **last** unit variant of an
externally-tagged enum.

- [ ] **Step 1: Write the failing test** — a `step_results.jsonl` row with
  `"kind": "some_future_kind"` must still deserialize, preserve **every other field**
  (`output`, `step_id`, `success`), and surface as an `Unknown` kind rather than being
  dropped. Assert the *output survives* — that is the actual harm.
- [ ] **Step 2: RED**, **Step 3: implement** — add `#[serde(other)] Unknown` as the last
  variant; make sure every `match` on `StepKind` handles it sensibly (printer dispatch
  should fall back to the linear rendering, not panic).
- [ ] **Step 4: GREEN. Step 5:** Do the same for the `events.jsonl` `Event` enum only if
  it is cheap; it is cosmetic by comparison. Say what you decided.
- [ ] **Step 6: Commit** — `fix(orchestrator): unknown StepKind no longer silently drops a step result (I-39)`

---

### Task 6: I-42, I-43, I-44 — three small, independent correctness fixes

- **I-42** (two-line fix): the two `when:`-skip sites omit `kind`, so a skipped gate/action
  persists as `Linear`. `runner.rs:1948-1959` (scheduler) and `runner.rs:3961-3971` (linear
  loop) both use `..Default::default()`; the two *adjacent* skip sites (`runner.rs:1828`
  prune, `runner.rs:3898` branch-not-taken) correctly set
  `kind: step_kind_for_run_record(step)`. Copy that. Test: a `when: false` gate persists
  `kind: "approval_gate"`.
- **I-43**: the sweep claims a lease (`claim_resume`, `cmd/cp.rs:797`) then releases it
  **unconditionally** after spawning — including on spawn failure (`cmd/cp.rs:862-865`).
  Next tick re-claims and re-spawns. No marker, no backoff, no attempt counter; the only
  thing that stops it is the child successfully flipping status, which the sweep never
  verifies. A permanently-failing approve re-spawns **forever** at 60s. Fix: do not clear
  the claim when the spawn itself failed, and/or record an attempt counter with backoff.
  Test: a spawn that fails does not produce an unbounded re-spawn on subsequent ticks.
- **I-44**: `fire_notify_hooks` discards `execute_action_step`'s `Ok(StepResult)`
  (`runner.rs:4749`) while that function unconditionally writes a ULID-named transcript
  (`runner.rs:4617`). Because `continue_on_error: true` is passed, even a failed hook
  returns `Ok`, so **every** notify hook orphans one `.jsonl` referenced by nothing,
  invisible to `show-run` and the CP, never collected. Fix: persist the notify step's
  result (with its `.notify` id) so the transcript is reachable, or don't write a
  transcript for notify hooks. **Prefer persisting** — the audit trail is the point of
  notify hooks. Test: after a gate with a notify hook parks, the transcript path is
  referenced by a persisted `StepResult`.

- [ ] Each with a failing test first. Separate commits:
  - `fix(orchestrator): preserve step kind when a when: guard skips a step (I-42)`
  - `fix(cli): stop the gate sweep re-spawning approve forever on failure (I-43)`
  - `fix(orchestrator): notify hook transcripts are no longer orphaned (I-44)`

---

### Task 7: I-40, I-41, I-37 — re-scope, then do the honest small fix

- **I-40 — retitle** to *"the CP transcript never shows an action step's rendered
  arguments"*. The filed title is factually wrong and would send an implementer down the
  wrong path. The action node **does** render via the `tool_audit` standalone fallback
  (`transcriptView.ts:364`, tested at `transcriptView.test.ts:369`). What's missing is the
  `action_emitted` payload: the rendered `with:` args plus `allowed`/`applied`/`reason`.
  **Two existing regression tests deliberately assert `action_emitted` stays ignored
  (`transcriptView.test.ts:284,301`)** — they must be revised deliberately, with the
  comment at `transcriptView.ts:423` (which calls the shape "dead/legacy") corrected, since
  it is now false for `action_emitted`.
- **I-41 — re-scope** to *"the steps table lacks gate/action arms"*. The graph is correct
  (`git_graph.rs:278,308`) and is the primary view. Add `action` and gate arms to
  `workflow_step_table_summary` (`cmd/workflow.rs:1447-1535`) so a gate row shows
  `KIND=gate` and an action row shows `KIND=action` with the tool name in PRIMARY instead
  of a blank column.
- **I-37 — document, do not "fix" in the listing.** Fixing it there would re-break I-25
  (Arc 2). Document that `on_timeout: approve` requires `cp serve`, in the same place
  I-80's dependency is documented, and cross-reference the two. If an explicit opt-in
  command is wanted, that is a new feature — file it in `TODO.md`, do not build it here.

---

### Task 8: Arc close-out

- [ ] Full verification: `cargo build --workspace`; `cargo test -p rupu-orchestrator -p rupu-cli -p rupu-mcp -p rupu-cp`; web `npm run build && npx vitest run`. Compare failures against the known-red baseline.
- [ ] Every I-33…I-44 either in `## Fixed` with a validation naming what was observed, or still in triage with a recorded reason. No row silently deleted. Retitled rows must show the corrected title.
- [ ] Open the PR against base `arc3/single-ui-path`.

## Deferred out of this arc

- An explicit opt-in resume command for `on_timeout: approve` without `cp serve` (I-37) → `TODO.md`.
- Widening I-80's write-up to note that web/app/cancel reject cleanup now also depends on the sweep.
