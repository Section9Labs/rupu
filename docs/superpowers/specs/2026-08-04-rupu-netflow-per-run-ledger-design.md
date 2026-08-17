# Netflow per-run ledgers, inference-scoped capture, and ledger lifecycle (design)

**Date:** 2026-08-04
**Status:** Design — approved by matt (scope, per-run model, and dropping run-less capture agreed in conversation). Ready to plan.
**Scope:** `crates/rupu-netflow` (paths, sink resolution, read views), `crates/rupu-runtime` (`provider_factory`), `crates/rupu-cli` (`paths`, `run`, `session`, `workflow`, `cp`, a new `netflow prune`), `crates/rupu-orchestrator` (`step_factory`), `crates/rupu-mcp` + `crates/rupu-webhook` (origin construction), `crates/rupu-cp` (`api/netflow.rs` + the web views).
**Out of scope:** the microVM `Fidelity::Full` backend (still its own arc); egress *enforcement*; capturing non-HTTP egress (`git2`, `object_store`, the node WebSocket).

## 1. What shipped, and what it cannot do

The netflow arc (#578, #580) shipped a working subsystem: records, an append-only ledger, ASN enrichment, an instrumented client behind a `deny`-level choke-point lint, and CP views at run / project / global scope. Verified against `main`, three gaps remain.

**1a. Only two code paths capture anything.** `rupu_netflow::http::init` has exactly two production call sites — `crates/rupu-cli/src/cmd/run.rs:573` and `crates/rupu-cli/src/cmd/cp.rs:85`. Every other entry point resolves the default `NullSink`. In particular `rupu session` (a long-lived daemon), `rupu workflow run` (agent steps built at `crates/rupu-orchestrator/src/step_factory.rs:235`), autoflow, and `dispatch_agent` sub-agents capture **nothing**. A workflow run's Network tab is permanently empty.

**1b. `Origin::Mcp` and `Origin::Webhook` are never constructed.** Both variants exist in `ctx.rs` and are emitted by zero production sites. MCP and webhook egress is not captured at all.

**1c. The ledger grows without bound.** One file per workspace, appended forever, with no rotation, retention, or time-range query. `grep -riE "rotate|retention|prune"` over `crates/rupu-netflow/src` returns nothing.

## 2. Why 1a cannot be fixed by calling `init` in more places

`http::init` installs a **process-wide `OnceLock`** — first call wins, later calls are silently ignored. That is correct for `rupu run`, which is one run per process. It is wrong for exactly the paths that matter here:

- `rupu session` is an explicitly long-lived daemon (`crates/rupu-cli/src/cmd/session.rs:68`).
- `rupu cp serve` hosts many runs across many workspaces in one process.
- A workflow executes many agent steps in one process.

Calling `init` per run in those processes pins the **first** run's sink and routes every later run's flows into the first run's transcript and ledger. That is actively wrong attribution — strictly worse than the empty tab it would replace.

So the fix is not wiring; it is **replacing process-global sink resolution with per-run resolution**. That single change is the enabler for everything else in this spec.

## 3. Decisions (operator-locked)

### 3.1 Capture is scoped to inference-driven egress

The question this subsystem answers is *"what did the agent's work reach out to."* Housekeeping egress is not that. In scope: any path where an agent runs — `run`, `session`, `workflow run`, autoflow, and `dispatch_agent` sub-agents. Out of scope: `rupu update`, `rupu auth login`, and direct human-invoked commands like `rupu issues list`.

**Autoflow's provider path is unresolved and must be traced as the first step of Plan 1.** `crates/rupu-cli/src/cmd/autoflow_runtime.rs` contains no `build_for_provider*` call and no obvious subprocess spawn, so how it executes an agent is not established by inspection. Two outcomes, both acceptable: if it spawns `rupu run` subprocesses, each is one run per process and it inherits capture for free once `run` is wired; if it builds providers in-process, it is another call site needing the sink. Do not assume either — trace it, and record which it was.

### 3.2 Ledgers are per-run, and mirror transcripts exactly

A ledger belongs to one run, workflow, or session — the same lifecycle a transcript has. Layout mirrors `crates/rupu-cli/src/paths.rs`:

```rust
/// Pick the netflow directory. Project-local when
/// `<project>/.rupu/netflow/` exists; global default otherwise.
/// Byte-for-byte the shape of `transcripts_dir` — the two must not drift.
pub fn netflow_dir(global: &Path, project_root: Option<&Path>) -> PathBuf
```

with a per-run file `<netflow_dir>/{run_id}.jsonl`, and `archived_netflow_dir(dir) -> dir.join("archive")` alongside `archived_transcripts_dir`.

**A consequence worth stating: this closes the git-leak class structurally.** `transcripts_dir` returns the project-local path only when that directory *already exists*, falling back to global otherwise. So a repo that was never `rupu init`'d never gets a ledger written inside it at all — the case #580 patched at the `ensure_dir` layer. #580's self-ignoring `.gitignore` stays as belt-and-braces for projects that do have `.rupu/netflow/`.

### 3.3 Run-less flows are not recorded

Under a per-run model a flow with no run has no file. Rather than keep a daemon-level ledger for them, they are **not captured**: `cp serve`'s fleet traffic, the ASN refresh's own download, and the updater become invisible to this subsystem.

This is a deliberate trade. It deletes the global-ledger enumeration, the "CP's own ledger is unreachable by the CP's own API" defect class, and every scope-disclosure caveat about CP fleet traffic. The cost is that `rupu cp serve`'s outbound calls to fleet hosts are no longer recorded. Accepted.

### 3.4 Retention copies the transcript idiom

`rupu netflow prune --retention 30d` mirrors `rupu transcript prune`, reusing `crates/rupu-cli/src/cmd/retention.rs::parse_retention_duration`. No bespoke rotation logic, no size caps, no background compaction — per-run files plus an existing prune verb is the whole lifecycle story.

## 4. Per-run sink resolution

`provider_factory::build_for_provider_with_config` (`crates/rupu-runtime/src/provider_factory.rs:239`) is already the chokepoint every agent path funnels through — 8 call sites across `session.rs`, `run.rs`, `step_factory.rs`, `generate.rs`, `dispatch.rs` and `cp.rs`. Providers already capture their sink at construction (`AnthropicClient` holds its own `sink` field).

So the factory gains an explicit sink parameter and each caller supplies its run's sink. `build_for_provider` is a thin wrapper that delegates to `build_for_provider_with_config`, so the parameter lands on the latter and the wrapper forwards it — one chokepoint, not two. No task-locals, no `OnceLock`, no ambient state — the same *bind at construction* discipline that made attribution correct in the first place, extended one level up.

`http::init` / `http::sink()` are removed rather than deprecated. A process-global sink has no correct use once resolution is per-run, and leaving it in place is a live footgun: the next provider added would reach for it and silently reintroduce cross-run attribution.

## 5. What this deletes

This is mostly a subtraction, which is the point.

- **The ledger↔transcript merge in `get_run_netflow`.** It exists only because `Origin::Scm`, auth and `System` flows carry `run_id: None` and would otherwise vanish from run scope. With a per-run sink they land in that run's ledger by construction — **the file is the attribution** — so the merge, its dedupe, and the `run_id` filtering around it all go.
- **Global-ledger enumeration** in `read_all_workspaces_sync`, and the `$RUPU_HOME/netflow/flows.jsonl` special case.
- **The `pwd` fallback** in `run.rs`'s `netflow_root`, replaced by the shared `netflow_dir` resolution.
- **`http::init` / `http::sink()` / `http::complete`'s global lookup**, including the dead-but-public `complete` that still resolves the global.

## 6. Time-range query

Per-run files make this cheap: a run's ledger has a bounded time span, so a range query selects *files* before reading rows.

- `views.rs` gains a range-filtered read that takes `Option<DateTime<Utc>>` bounds and skips files whose run falls outside.
- The CP API takes `?from=`/`?to=` (RFC 3339) at every scope.
- The UI offers a relative picker (last hour / 24h / 7d / all) with an absolute from-to fallback. Relative is the default because *"what did this reach in the last hour"* is the question actually asked.

Absent bounds means everything, so existing callers are unaffected.

## 7. MCP and webhook origins are deleted, not constructed

**Revised after tracing the code.** The original intent was to construct `Origin::Mcp` and `Origin::Webhook` at their real call sites. There are none:

- **`rupu-mcp` makes no outbound HTTP whatsoever** — it has no `reqwest` dependency. It dispatches tool calls into `rupu-scm`'s `Registry`, and those connectors make the actual requests, already tagged `Origin::Scm`. Re-tagging them `Origin::Mcp` would be *less* accurate, not more: the call really is to GitHub's or GitLab's API, and that is what an operator auditing egress needs to see. The MCP layer is dispatch, not transport.
- **`rupu-webhook` is an inbound axum server.** It receives webhooks on `/webhook/{github,gitlab,linear,jira}` and makes no outbound calls. Its `reqwest` dependency is used only by its test files.

So both variants describe egress that structurally cannot occur. They are **removed from the `Origin` enum** rather than populated. The enum should enumerate what can happen, not what someone imagined might.

This also retires the standing CP note that filter chips must not be built from the enum — once the variants are gone, the enum becomes a truthful list again.

## 8. Error handling

Unchanged from the original design and non-negotiable: capture never breaks a request; loss is visible, never silent; `Fidelity` keeps "unobservable" distinguishable from "zero". Per-run ledgers do not relax any of these — a per-run writer that cannot open its file degrades to a logged no-op exactly as the workspace writer did.

## 9. Testing

- `netflow_dir` resolves project-local-when-present and global otherwise, proven against the same fixtures `transcripts_dir` uses.
- A run writes its flows to `{run_id}.jsonl` and **not** into any other run's file — the cross-run attribution bug this arc exists to prevent, asserted directly.
- Two concurrent runs in one process (the session/`cp serve` shape) each land in their own ledger. This is the test that would have failed under the `OnceLock`.
- A workflow run's agent step produces a ledger with a non-empty flow set.
- `netflow prune` removes ledgers older than the cutoff and leaves newer ones, mirroring the transcript prune tests.
- A range query returns only flows inside the bounds, and absent bounds returns everything.

## 10. Phasing

| Plan | Content |
|---|---|
| **1** | Per-run ledger + per-run sink: `netflow_dir`/`archived_netflow_dir`, sink through `provider_factory`, remove `http::init`/`sink()`, wire `run` / `session` / `workflow` / autoflow / `dispatch`, delete the merge and the global enumeration. |
| **2** | Remove `Origin::Mcp` and `Origin::Webhook` — neither crate makes outbound HTTP, so both describe egress that cannot occur. |
| **3** | `rupu netflow prune` + time-range filtering through views → API → UI. |

Plan 1 is the enabler and by far the largest; 2 and 3 are independent of each other and both depend on 1.
