# Agentiflows Plan 3b-1 — the lead's coordination substrate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the (still solo) agentiflow lead a set of always-on, run-scoped **coordination tools** and **ambient collectors**, so it can write to a shared board, address mailboxes, and — the load-bearing fix — **bank findings into the pooled coverage scope the envelope's goal evaluator actually reads**. End state: a lead run carries `board.claim`/`board.release`/`board.post`/`board.read`/`msg.send` tools plus `report_finding`/`asset_mark` bound to `target_id(workspace, agentiflow_id)`, and each round folds in the lead's inbox (`MailboxCollector`) and standing board directives (`DirectiveCollector`). No agent is spawned yet — dispatch, the roster, and `generate_workflow` are Plan 3b-2; verification is 3c.

**Architecture:** Two seams. (1) `rupu-agent` gains a general tool-injection point — `AgentRunOpts.extra_tools: Vec<Arc<dyn rupu_tools::Tool>>`, inserted into the run's tool registry *after* the `agent_tools` `filter_to` and the coverage/MCP blocks, so injected tools are always-on platform tools (the `PermissionDecider` still gates each call) — exactly mirroring how Plan 1 added `collectors`. (2) `rupu-agentiflow` gains a `tools` module (the board/mailbox tools as `rupu_tools::Tool` impls over run-scoped `rupu-fleet` handles) and a `collectors` module (the Mailbox/Directive `TurnCollector` impls), and `run_agentiflow` constructs the board + mailboxes under the run dir and wires both into every round's lead run. `report_finding` reuses the runner's existing coverage-tool path — the only change is routing it to the pooled scope via `scope_name` + `tool_context.findings.engagement`.

**Why Seam A over Seam B (ruling, so a reviewer can challenge it):** The alternative was a `ToolContext.fleet: Option<Arc<dyn FleetServices>>` port in `rupu-tools` (~8 literal edits vs. ~150 for the `AgentRunOpts` field). Rejected because it pushes agentiflow-domain concepts (board/mailbox) into the generic `rupu-tools` public surface and splits the fleet tools away from their domain crate, whereas `extra_tools` keeps them in `rupu-agentiflow`, is a genuinely reusable injection point, and is byte-for-byte a no-op for every existing caller (empty `Vec`), exactly like `collectors`. The churn is mechanical and compile-checked. Spec §22's "the Tier-1/2 tools live in `rupu-tools`" is reconciled here: the `Tool` *trait* is in `rupu-tools`; the agentiflow tool *impls* live in `rupu-agentiflow`, the correct home for board/mailbox logic (same as `report_finding`/coverage tools living in `rupu-agent`, not `rupu-tools`).

**Tech Stack:** Rust 2021; `rupu-agent` (`AgentRunOpts`, `run_agent_full`, `ToolRegistry`, `TurnCollector`, `coverage_tools`); `rupu-tools` (`Tool`/`ToolOutput`/`ToolError`/`ToolContext`); `rupu-fleet` (`Board`/`Mailbox` + types — **new dep for `rupu-agentiflow`**); `rupu-coverage` (`ActiveSet`, `FindingWriteOptions`, `CoveragePaths`, `target_id`); `async-trait` (new workspace dep for `rupu-agentiflow`, for the `Tool` impls); `serde_json`/`chrono`/`tracing`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` — §8 (collector pipeline, esp. §8.5 Mailbox/Directive collectors), §9 (board + mailboxes), §13 (tool taxonomy — Tier 1 always-on), §14 (`verified` findings on the board that goals read). Read it alongside. Builds directly on Plan 3a (`run_agentiflow`, `RunAgentLeadDriver`, `LeadConfig`) and Plan 1 (`rupu-fleet`, the `TurnCollector` pipeline).

## Global Constraints

- **Hexagonal / ports.** `rupu-agent` stays unaware of agentiflows: `extra_tools` is typed only as `Vec<Arc<dyn rupu_tools::Tool>>`. The fleet tool/collector impls live in `rupu-agentiflow`. The lead is still reached only via `LeadDriver`.
- **Spec §13 Tier-1 semantics.** Injected tools are *always-on*: inserted after the `agent_tools` `filter_to`, so a restrictive agent `tools:`/`actions:` list cannot strip them (platform tools, like builtins). The per-call `PermissionDecider` still runs (unchanged); the lead runs under `BypassDecider`.
- **No new restrictive gates (operator directive).** No `capabilities:` frontmatter field (the spec's §13 capability gate is dropped — confirmed in 3a). The lead's tool access is the envelope's choice via `LeadConfig`; Tier-3 for any agent is still the existing `tools:` opt-in.
- **Workspace deps only;** `[lints] workspace = true`; no `unsafe`; `cargo clippy -p rupu-agent --all-targets -- -D warnings` and `-p rupu-agentiflow` clean; `edition`/`rust-version`/`version` via `.workspace = true`; `thiserror` for library errors.
- **Don't break anything.** `extra_tools` defaults to an empty `Vec` at every existing `AgentRunOpts` site (byte-for-byte no-op, exactly like `collectors` in Plan 1). Every existing `rupu-agent`/`rupu-orchestrator`/`rupu-agentiflow` test stays green.
- **Fail-closed stays fail-closed.** Nothing here widens the lead's tool access beyond what the envelope injects; a tool that cannot reach its store returns a `ToolError`/`ToolOutput.error`, never a panic and never a silent success.

## Scope boundary

3b-1 includes ONLY: the `extra_tools` injection point; the board + mailbox tools; `report_finding`/`asset_mark` routed to the pooled scope; the Mailbox + Directive collectors; and wiring all of these into the lead's run. It does NOT include: agent/workflow **dispatch**, `join`, process isolation, the orphan reaper, `generate_workflow`, the **roster** (`agents.list`/`workflows.list`/`catalog.search`), the `RosterCollector`, the read-only `goal.status`/`budget.status`/`coverage.status` pull-tools, or the `StatusCollector` (all 3b-2 — they pair with fleet observability and the digest is already rendered into the round prompt); nor the verify path (3c) or the CLI/CP surfaces (Plan 4). 3b-1 is exercised through `run_agentiflow(…)` with a `MockProvider` scripted to call the new tools.

## File Structure

In `crates/rupu-agent/`:
- `src/runner.rs` — modify: add `pub extra_tools: Vec<std::sync::Arc<dyn rupu_tools::Tool>>` to `AgentRunOpts`; insert them into the registry after the MCP block.
- *(~150 `AgentRunOpts { … }` literals across the workspace)* — add `extra_tools: Vec::new(),` (mechanical sweep; see Task 1).

In `crates/rupu-agentiflow/`:
- `Cargo.toml` — add `rupu-fleet` and `async-trait` (workspace) deps.
- `src/tools.rs` — **new**: `FleetToolCtx` (run-scoped handle: `Arc<Board>`, `Arc<Mailbox>`, participant id, held-claims map) + the `BoardClaim`/`BoardRelease`/`BoardPost`/`BoardRead`/`MsgSend` tools; `fleet_tools(ctx) -> Vec<Arc<dyn Tool>>`.
- `src/collectors.rs` — **new**: `MailboxCollector` + `DirectiveCollector` (`TurnCollector` impls); `lead_collectors(...) -> Vec<Arc<dyn TurnCollector>>`.
- `src/lead.rs` — modify: `LeadConfig` gains `extra_tools`, `collectors`, `scope_name`, `findings_engagement`; `run_round` threads them into `AgentRunOpts`.
- `src/run.rs` — modify: create `board/` + `mailboxes/` under the run dir; build the fleet tools + collectors; ensure the lead's effective `agent_tools` contains `report_finding`; pass everything into `LeadConfig`.
- `src/lib.rs` — modify: `mod tools; mod collectors;` + re-exports.

---

## Task 1: `AgentRunOpts.extra_tools` — a general always-on tool-injection point

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (the `AgentRunOpts` struct + the registry assembly in `run_agent_inner`).
- Modify: every other `AgentRunOpts { … }` literal in the workspace (add `extra_tools: Vec::new(),`).

**Interfaces:**
- Produces: `AgentRunOpts.extra_tools: Vec<std::sync::Arc<dyn rupu_tools::Tool>>`. Semantics: each is `registry.insert(tool.name().into(), tool.clone())` **after** the builtin `filter_to`, the coverage block, and the MCP block, so injected tools are present regardless of `agent_tools` and win a name collision. Call-time `PermissionDecider` is unchanged.

- [ ] **Step 1: Failing test — an injected tool is callable even under an empty `agent_tools`**

In `crates/rupu-agent/src/runner.rs` tests (reuse the existing `MockProvider`/`ScriptedTurn` harness and `BypassDecider`):
```rust
#[tokio::test]
async fn extra_tools_are_registered_and_bypass_the_agent_tools_filter() {
    // A trivial always-on tool.
    #[derive(Debug)]
    struct Ping;
    #[async_trait::async_trait]
    impl rupu_tools::Tool for Ping {
        fn name(&self) -> &'static str { "ping" }
        fn description(&self) -> &'static str { "returns pong" }
        fn input_schema(&self) -> serde_json::Value { serde_json::json!({"type":"object","properties":{}}) }
        async fn invoke(&self, _input: serde_json::Value, _ctx: &rupu_tools::ToolContext)
            -> Result<rupu_tools::ToolOutput, rupu_tools::ToolError> {
            Ok(rupu_tools::ToolOutput { stdout: "pong".into(), ..Default::default() })
        }
    }
    let provider = MockProvider::new(vec![
        // turn 1: call ping; turn 2: stop.
        ScriptedTurn::tool_call("ping", serde_json::json!({})),
        ScriptedTurn::text("done"),
    ]);
    let mut opts = base_test_opts(provider);     // helper that builds a minimal AgentRunOpts
    opts.agent_tools = Some(vec![]);              // empty allowlist: NO builtins, NO MCP
    opts.extra_tools = vec![std::sync::Arc::new(Ping)];
    let exit = run_agent_full(opts).await;
    let run = exit.result.expect("run ok");
    assert!(run.final_messages.iter().any(|m| text_of(m).contains("pong")),
        "the injected tool ran despite an empty agent_tools: {:?}", run.final_messages);
}
```
If `base_test_opts` / `ScriptedTurn::tool_call` / `text_of` don't already exist in the test module, add the smallest helpers next to the existing mock-provider tests (there are several `AgentRunOpts`-building tests already — copy one).

- [ ] **Step 2: Run it — expect a compile error** (`extra_tools` field doesn't exist yet). `cargo test -p rupu-agent --lib extra_tools_are_registered 2>&1 | head`.

- [ ] **Step 3: Add the field.** In the `AgentRunOpts` struct (after `collectors`, before `recovery`, to keep related Vecs together):
```rust
    /// Pre-built, run-scoped tools injected into this run's registry regardless
    /// of `agent_tools` (always-on platform tools; the `PermissionDecider` still
    /// gates each call). Empty for ordinary runs; the agentiflow envelope uses it
    /// for the lead's board/mailbox tools.
    pub extra_tools: Vec<std::sync::Arc<dyn rupu_tools::Tool>>,
```

- [ ] **Step 4: Insert them in the registry.** In `run_agent_inner`, immediately AFTER the MCP block (after the `for spec in rupu_mcp::tool_catalog()` loop closes, i.e. just before `let tool_defs = registry.to_tool_definitions();`):
```rust
    // Always-on injected tools (agentiflow Tier-1): registered after the
    // agent_tools filter and the coverage/MCP blocks, so a restrictive agent
    // tools:/actions: list cannot strip them. A name collision favors the
    // injected tool. The per-call PermissionDecider still runs.
    for tool in &opts.extra_tools {
        registry.insert(tool.name().to_string(), tool.clone());
    }
```

- [ ] **Step 5: Run the new test — expect PASS.** `cargo test -p rupu-agent --lib extra_tools_are_registered -- --nocapture`.

- [ ] **Step 6: Sweep every other `AgentRunOpts` literal.** Add `extra_tools: Vec::new(),` to every `AgentRunOpts { … }` in the workspace. This is the identical mechanical change Plan 1 made for `collectors` — a missing site is a hard compile error, so the compiler is the checklist. Find them:
```bash
rg -n 'AgentRunOpts \{' --glob '!target/**' | wc -l   # ~150 across ~45 files
rg -l 'AgentRunOpts \{' --glob '!target/**'
```
Put the new field next to `collectors: Vec::new(),` in each. Known clusters: `crates/rupu-agent/src/runner.rs` (tests), `crates/rupu-orchestrator/src/runner.rs` (~34), `crates/rupu-orchestrator/tests/it/*`, `crates/rupu-agentiflow/src/lead.rs` (Task 5 will set it; leave `Vec::new()` for now), `crates/rupu-cli/src/cmd/{run,session,dispatch}.rs`, and the various `tests/it` modules. Do NOT change behavior anywhere — every site gets `Vec::new()`.

- [ ] **Step 7: Build the workspace + run the touched crates' tests.**
```bash
cargo build --workspace 2>&1 | tail -20
cargo test -p rupu-agent 2>&1 | tail -5
cargo test -p rupu-orchestrator 2>&1 | tail -5
cargo clippy -p rupu-agent --all-targets -- -D warnings 2>&1 | tail -5
```
Expected: green; the only behavioral change is the new (empty-by-default) field.

- [ ] **Step 8: Commit.**
```bash
git add -A && git commit -m "feat(agent): AgentRunOpts.extra_tools — always-on injected run tools"
```

---

## Task 2: Fleet coordination tools (`rupu-agentiflow::tools`)

**Files:**
- Modify: `crates/rupu-agentiflow/Cargo.toml` (add `rupu-fleet`, `async-trait`).
- Create: `crates/rupu-agentiflow/src/tools.rs`.
- Modify: `crates/rupu-agentiflow/src/lib.rs` (`mod tools;` + re-export `FleetToolCtx`, `fleet_tools`).

**Interfaces:**
- Consumes: `rupu_fleet::{Board, Mailbox, BoardPost, PostKind, Directive, FleetMessage, ClaimOutcome, ClaimGuard}`; `rupu_tools::{Tool, ToolOutput, ToolError, ToolContext}`.
- Produces:
  - `pub struct FleetToolCtx { board: Arc<Board>, mailbox: Arc<Mailbox>, participant: String, claims: Arc<Mutex<HashMap<String, ClaimGuard>>>, msg_cap: usize }` with `pub fn new(board: Arc<Board>, mailbox: Arc<Mailbox>, participant: impl Into<String>) -> Self` (default `msg_cap = 256`).
  - `pub fn fleet_tools(ctx: Arc<FleetToolCtx>) -> Vec<Arc<dyn Tool>>` → the five tools below, each holding `ctx.clone()`.

Design notes carried from the research dossier:
- `Board::claim(key, owner, ttl) -> ClaimOutcome{Granted(ClaimGuard)|Denied{holder}}`; the `ClaimGuard` **releases on drop**, so a stateless tool must *retain* the guard. `FleetToolCtx.claims` holds granted guards keyed by work-unit; `board.release` drops the guard by removing it from the map; `board.claim` of a key already in the map re-reports it as held by this participant.
- `Board::post(&BoardPost)`, `read_posts() -> Vec<BoardPost>`, `read_directives() -> Vec<Directive>`. `BoardPost{author,ts,kind:PostKind,body,addressed_to:Option<String>}`.
- `Mailbox::send(to, &FleetMessage, cap)`, `FleetMessage{from,ts,body}`. Addressing `to ∈ {participant id, role, "parent", "lead", "broadcast"}` is just the inbox key; v1 writes exactly the key given.
- All tool invocations are infallible at the turn level: a store error becomes `Ok(ToolOutput { error: Some(msg), .. })` (the model sees the error and can react), never `Err` (which aborts the run). Reserve `Err(ToolError::InvalidInput(..))` for unparseable arguments.
- `ts` strings are `chrono::Utc::now().to_rfc3339()`; `author`/`from` are `ctx.participant`.

- [ ] **Step 1: Cargo deps.** In `crates/rupu-agentiflow/Cargo.toml`, under `[dependencies]`, add (workspace-pinned, no versions):
```toml
rupu-fleet = { workspace = true }
async-trait = { workspace = true }
```
Confirm both are declared in the root `Cargo.toml` `[workspace.dependencies]` (both are — `rupu-fleet` since Plan 1, `async-trait` is used across the workspace). `cargo build -p rupu-agentiflow` after.

- [ ] **Step 2: Failing test — `board.post` then `board.read` round-trips through a real `Board`.**

Create `crates/rupu-agentiflow/src/tools.rs` with a `#[cfg(test)] mod tests` holding:
```rust
#[tokio::test]
async fn board_post_then_read_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(FleetToolCtx::new(
        Arc::new(Board::new(dir.path().join("board"))),
        Arc::new(Mailbox::new(dir.path().join("mailboxes"))),
        "lead",
    ));
    let tools = fleet_tools(ctx.clone());
    let post = tools.iter().find(|t| t.name() == "board.post").unwrap();
    let read = tools.iter().find(|t| t.name() == "board.read").unwrap();
    let tc = rupu_tools::ToolContext::default();
    post.invoke(serde_json::json!({"kind":"observation","body":"found an open port"}), &tc)
        .await.unwrap();
    let out = read.invoke(serde_json::json!({}), &tc).await.unwrap();
    assert!(out.error.is_none(), "{out:?}");
    assert!(out.stdout.contains("found an open port"), "{}", out.stdout);
    assert!(out.stdout.contains("lead"), "author shown: {}", out.stdout);
}
```

- [ ] **Step 3: Implement `FleetToolCtx` + the five tools.** Each tool is a unit-ish struct holding `Arc<FleetToolCtx>`, `#[derive(Debug)]`, `#[async_trait] impl Tool`. Schemas are minimal `serde_json::json!` objects. Sketch (write all five):
```rust
use async_trait::async_trait;
use rupu_fleet::{Board, BoardPost, ClaimOutcome, Directive, FleetMessage, Mailbox, PostKind};
use rupu_tools::{Tool, ToolContext, ToolError, ToolOutput};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DEFAULT_CLAIM_TTL: Duration = Duration::from_secs(3600);

pub struct FleetToolCtx {
    board: Arc<Board>,
    mailbox: Arc<Mailbox>,
    participant: String,
    claims: Arc<Mutex<HashMap<String, rupu_fleet::ClaimGuard>>>,
    msg_cap: usize,
}
impl FleetToolCtx {
    pub fn new(board: Arc<Board>, mailbox: Arc<Mailbox>, participant: impl Into<String>) -> Self {
        Self { board, mailbox, participant: participant.into(),
               claims: Arc::new(Mutex::new(HashMap::new())), msg_cap: 256 }
    }
    fn now() -> String { chrono::Utc::now().to_rfc3339() }
}

// board.claim  { "work_unit": "host:1.1.2.2" }  -> "granted" | "held by <holder>"
struct BoardClaim(Arc<FleetToolCtx>);
#[async_trait]
impl Tool for BoardClaim {
    fn name(&self) -> &'static str { "board.claim" }
    fn description(&self) -> &'static str {
        "Atomically claim a work unit so no other participant duplicates it. Returns granted or the current holder."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","required":["work_unit"],
            "properties":{"work_unit":{"type":"string"}}})
    }
    async fn invoke(&self, input: serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let key = input.get("work_unit").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("work_unit (string) required".into()))?;
        let c = &self.0;
        if c.claims.lock().unwrap().contains_key(key) {
            return Ok(ToolOutput { stdout: format!("granted (already held by you): {key}"), ..Default::default() });
        }
        match c.board.claim(key, &c.participant, DEFAULT_CLAIM_TTL) {
            Ok(ClaimOutcome::Granted(guard)) => {
                c.claims.lock().unwrap().insert(key.to_string(), guard);
                Ok(ToolOutput { stdout: format!("granted: {key}"), ..Default::default() })
            }
            Ok(ClaimOutcome::Denied { holder }) =>
                Ok(ToolOutput { stdout: format!("denied: {key} held by {holder}"), ..Default::default() }),
            Err(e) => Ok(ToolOutput { error: Some(format!("board.claim failed: {e}")), ..Default::default() }),
        }
    }
}
// board.release { "work_unit": "..." } -> drops the guard (c.claims.lock().unwrap().remove(key))
// board.post    { "kind": "observation|question|answer|vote|note", "body": "...", "addressed_to"?: "..." }
//   -> parse kind via a small str->PostKind match (default note on unknown, but prefer InvalidInput on a bad enum),
//      c.board.post(&BoardPost { author: c.participant.clone(), ts: now(), kind, body, addressed_to })
// board.read    { "addressed_to"?: "...", "limit"?: n } -> c.board.read_posts(), newest-last, render "<ts> <author> [<kind>]: <body>"
// msg.send      { "to": "...", "body": "..." } -> c.mailbox.send(to, &FleetMessage{from:c.participant,ts:now(),body}, c.msg_cap)

pub fn fleet_tools(ctx: Arc<FleetToolCtx>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(BoardClaim(ctx.clone())),
        Arc::new(BoardRelease(ctx.clone())),
        Arc::new(BoardPostTool(ctx.clone())),
        Arc::new(BoardRead(ctx.clone())),
        Arc::new(MsgSend(ctx)),
    ]
}
```
Write the four elided tools fully, following the `BoardClaim` shape. For `board.post`, map the `kind` string to `PostKind` and return `ToolError::InvalidInput` on an unrecognized value (fail-closed, not a silent `Note`).

- [ ] **Step 4: More tests.** Add: `board_claim_is_exclusive_and_releasable` (claim a key → granted; a *second* `FleetToolCtx` on the same board claiming it → denied with holder; first releases → second now granted); `msg_send_then_drain` (send to `"worker-1"`, then `Mailbox::drain("worker-1")` returns it); `board_post_rejects_unknown_kind` (`kind:"bogus"` → `ToolError::InvalidInput`). Run: `cargo test -p rupu-agentiflow --lib tools::`.

- [ ] **Step 5: lib.rs.** Add `mod tools;` and `pub use tools::{fleet_tools, FleetToolCtx};`. `cargo clippy -p rupu-agentiflow --all-targets -- -D warnings`.

- [ ] **Step 6: Commit.**
```bash
git add -A && git commit -m "feat(agentiflow): board + mailbox coordination tools over rupu-fleet"
```

---

## Task 3: Route `report_finding`/`asset_mark` to the pooled agentiflow scope

**Files:** Modify `crates/rupu-agentiflow/src/lead.rs` (`LeadConfig` + `run_round`), `crates/rupu-agentiflow/src/run.rs` (set the new `LeadConfig` fields).

**Interfaces:**
- Produces: `LeadConfig` gains `pub scope_name: Option<String>` and `pub findings_engagement: Option<Arc<rupu_coverage::ActiveSet>>`. `run_round` sets `AgentRunOpts.scope_name = self.cfg.scope_name.clone()` and `AgentRunOpts.tool_context.findings = self.cfg.findings_engagement.clone().map(|e| rupu_coverage::FindingWriteOptions { engagement: Some(e), ..Default::default() })`, and ensures the lead's effective `agent_tools` contains `"report_finding"`.

**Why this task exists (the dossier's headline finding):** `report_finding` writes to `CoveragePaths::new(workspace, target_id(workspace, scope_name.unwrap_or(agent_name)))`. 3a left `scope_name: None`, so the lead's findings would land under `target_id(workspace, <lead agent name>)` — a *different* target than the pooled `target_id(workspace, <agentiflow id>)` that `run_agentiflow` passes to the envelope's `GoalEvaluator`/evidence pool (`run.rs` builds `CoveragePaths::new(&workspace, &target_id(&workspace, &id))`). Without this fix, a lead that reports findings can never satisfy a finding-goal. Setting `scope_name = Some(run_id)` makes the two targets identical.

Confirm the exact field set of `rupu_coverage::FindingWriteOptions` by reading it (grep `pub struct FindingWriteOptions` under `crates/rupu-coverage/src/`); construct it with `engagement: Some(active)` and otherwise its `Default`. The runner registers `report_finding` when `concerns.is_some()` OR (`concerns.is_none()` AND `agent_tools` contains `"report_finding"`), and additionally registers `asset_mark` when `tool_context.findings.engagement` is `Some` — so setting the engagement gives the lead both finding and asset write tools, both profile-typed by the active set.

- [ ] **Step 1: Failing test — a lead finding lands in the pooled scope.**

In `crates/rupu-agentiflow/src/run.rs` tests (reuse the 3a `opts(&fx, def_with(..), id)` harness + a scripted provider), script the lead to call `report_finding` once with a minimal valid full-profile finding for the active profile, then stop. After `run_agentiflow` returns, assert the finding is readable from the pooled target:
```rust
#[test]
fn a_lead_finding_lands_in_the_pooled_scope() {
    let fx = fixture();
    // provider scripted to emit one report_finding tool call with a valid report, then stop.
    let def = def_with("round: { ceiling: { rounds: 1 } }");
    let id = "af_finding";
    run_agentiflow(opts_scripted_report_finding(&fx, def, id)).unwrap();
    let pooled = rupu_coverage::CoveragePaths::new(
        &fx.workspace, &rupu_coverage::target_id(&fx.workspace, id));
    let findings = std::fs::read_to_string(&pooled.findings).unwrap_or_default();
    assert!(!findings.trim().is_empty(), "the lead's finding is in the pooled scope ledger");
}
```
(Build `opts_scripted_report_finding` from the existing 3a `opts(...)` helper by swapping the mock-provider script for one that emits the `report_finding` tool call. Reuse the finding JSON shape from `rupu-coverage`'s own `report_finding` tests — do NOT invent assessment-like content; a trivially valid `{scope:"repo", report:{…minimal…}}` is enough. If the full-profile schema is onerous to satisfy in a unit test, set `findings_engagement`'s options to the `summary` profile for this test via a dedicated `FindingWriteOptions` profile field, or script the summary schema — pick whichever the real `FindingWriteOptions` exposes.)

- [ ] **Step 2: Run it — expect FAIL** (finding absent from the pooled scope, because `scope_name` is `None` / `report_finding` not granted).

- [ ] **Step 3: `LeadConfig` fields.** Add to `LeadConfig`:
```rust
    /// Coverage scope the lead's `report_finding`/`asset_mark` write to. Set to
    /// the agentiflow id so findings pool under the same target the envelope's
    /// goal evaluator reads (`target_id(workspace, id)`).
    pub scope_name: Option<String>,
    /// The run's resolved engagement profile set; enables `asset_mark` and makes
    /// findings profile-typed. `None` = no engagement (bare findings).
    pub findings_engagement: Option<std::sync::Arc<rupu_coverage::ActiveSet>>,
```

- [ ] **Step 4: Thread them in `run_round`.** Replace the relevant `AgentRunOpts` fields:
```rust
    scope_name: self.cfg.scope_name.clone(),
    tool_context: rupu_tools::ToolContext {
        workspace_path: self.cfg.workspace_path.clone(),
        findings: self.cfg.findings_engagement.clone()
            .map(|e| rupu_coverage::FindingWriteOptions { engagement: Some(e), ..Default::default() }),
        ..Default::default()
    },
```
And ensure `report_finding` is granted: the envelope controls `self.cfg.agent_tools`, so Task 5 adds `"report_finding"` to it. (Belt-and-braces: do not rely on `concerns`; this run sets `concerns: None`.)

- [ ] **Step 5: Wire from `run.rs` (minimal, completed in Task 5).** In `run_agentiflow`, set `scope_name: Some(id.clone())` and `findings_engagement: Some(Arc::new(active.clone()))` on the `LeadConfig`, and add `"report_finding"` to the lead's `agent_tools` if absent. Confirm `active` is cloneable into an `Arc` (it is an `ActiveSet`); if `run_agentiflow` already consumes `active` by move into the envelope, clone it before the move.

- [ ] **Step 6: Run the test — expect PASS.** `cargo test -p rupu-agentiflow --lib a_lead_finding_lands_in_the_pooled_scope`. Then the whole crate + clippy.

- [ ] **Step 7: Commit.**
```bash
git add -A && git commit -m "fix(agentiflow): lead findings write to the pooled agentiflow scope"
```

---

## Task 4: Mailbox + Directive collectors (`rupu-agentiflow::collectors`)

**Files:** Create `crates/rupu-agentiflow/src/collectors.rs`; modify `src/lib.rs` (`mod collectors;` + re-export).

**Interfaces:**
- Consumes: `rupu_agent::{TurnCollector, TurnContext, Injection, InjectionKind, Cadence}`; `rupu_fleet::{Board, Mailbox}`.
- Produces:
  - `pub struct MailboxCollector { mailbox: Arc<Mailbox>, participant: String }` — `TurnCollector`; `collect` drains this participant's inbox (and, by convention, the `"broadcast"` inbox), emitting one `Injection { kind: Message, cadence: Once, priority: 200, source: "mailbox:<participant>" }` per message. `Once` because inbox messages must be delivered exactly once and persisted.
  - `pub struct DirectiveCollector { board: Arc<Board>, participant: String, role: Option<String> }` — `TurnCollector`; `collect` reads board directives addressed to this participant / its role / all, emitting `Injection { kind: Directive, cadence: EveryTurn, priority: 230, source: "directive:board" }`. `EveryTurn` because a standing directive should be re-asserted each turn until lifted (directives are not consumed).
  - `pub fn lead_collectors(mailbox: Arc<Mailbox>, board: Arc<Board>, participant: impl Into<String>) -> Vec<Arc<dyn TurnCollector>>`.

Dossier facts: `TurnCollector` is **sync** (`fn collect(&self, ctx: &TurnContext) -> Vec<Injection>`); the pipeline runs it under `spawn_blocking` before each model call; a panicking collector degrades to empty; `Once` injections are appended to `messages` (persisted); `wrap_injection` already frames every injection as data-not-instruction, so collectors return raw attributed `content` and must NOT add their own authority framing. `TurnContext.participant` is the agent name.

- [ ] **Step 1: Failing tests.**
```rust
#[test]
fn mailbox_collector_drains_inbox_as_once_messages() {
    let dir = tempfile::tempdir().unwrap();
    let mb = Arc::new(Mailbox::new(dir.path()));
    mb.send("lead", &rupu_fleet::FleetMessage { from: "worker-1".into(),
        ts: "t".into(), body: "found RCE on 1.1.2.2".into() }, 64).unwrap();
    let c = MailboxCollector { mailbox: mb.clone(), participant: "lead".into() };
    let injs = c.collect(&ctx("lead"));
    assert_eq!(injs.len(), 1);
    assert_eq!(injs[0].cadence, Cadence::Once);
    assert!(injs[0].content.contains("found RCE on 1.1.2.2"));
    // drained: a second collect returns nothing.
    assert!(c.collect(&ctx("lead")).is_empty());
}

#[test]
fn directive_collector_reasserts_every_turn() {
    let dir = tempfile::tempdir().unwrap();
    let board = Arc::new(Board::new(dir.path()));
    board.put_directive(&rupu_fleet::Directive { author: "lead".into(), ts: "t".into(),
        body: "focus on the auth module".into(), addressed_to: None }).unwrap();
    let c = DirectiveCollector { board: board.clone(), participant: "lead".into(), role: None };
    assert_eq!(c.collect(&ctx("lead")).len(), 1);
    assert_eq!(c.collect(&ctx("lead"))[0].cadence, Cadence::EveryTurn); // not consumed
}
```
(`ctx(p)` builds a `TurnContext { run_id:"r".into(), codename:None, participant:p.into(), turn_index:0 }`.)

- [ ] **Step 2: Run — expect FAIL / compile error.**

- [ ] **Step 3: Implement.** The `MailboxCollector::collect` calls `self.mailbox.drain(&self.participant)` (+ optionally `drain("broadcast")`), maps each `FleetMessage` to an `Injection` whose `content` is `format!("from {} at {}: {}", m.from, m.ts, m.body)`. `DirectiveCollector::collect` calls `self.board.read_directives()`, filters `addressed_to ∈ {None, Some(participant), Some(role)}`, maps each to an `Injection` with `content = format!("standing directive from {}: {}", d.author, d.body)`. Addressing filter helper shared. No authority framing in `content` (the pipeline wraps it).

- [ ] **Step 4: Run — expect PASS.** `cargo test -p rupu-agentiflow --lib collectors::`.

- [ ] **Step 5: lib.rs + clippy.** `mod collectors;` + `pub use collectors::{lead_collectors, MailboxCollector, DirectiveCollector};`.

- [ ] **Step 6: Commit.**
```bash
git add -A && git commit -m "feat(agentiflow): mailbox + directive turn-collectors for the lead"
```

---

## Task 5: Wire the board, tools, and collectors into the lead's run

**Files:** Modify `crates/rupu-agentiflow/src/run.rs` (`run_agentiflow`), `crates/rupu-agentiflow/src/lead.rs` (`LeadConfig` gains `extra_tools`/`collectors`; `run_round` passes them).

**Interfaces:**
- Produces: `LeadConfig` gains `pub extra_tools: Vec<Arc<dyn rupu_tools::Tool>>` and `pub collectors: Vec<Arc<dyn rupu_agent::TurnCollector>>`; `run_round` sets `AgentRunOpts.extra_tools = self.cfg.extra_tools.clone()` and `.collectors = self.cfg.collectors.clone()` (clone the `Arc`s into every round). `run_agentiflow` constructs the board/mailboxes under the run dir, builds the tools + collectors bound to them + participant `"lead"`, and populates the new `LeadConfig` fields (plus Task 3's `scope_name`/`findings_engagement`).

- [ ] **Step 1: `LeadConfig` fields + `run_round` threading.** Add the two `Vec` fields; in `run_round` replace `extra_tools: Vec::new(),` and `collectors: Vec::new(),` with the cloned config vecs.

- [ ] **Step 2: Run-dir board/mailboxes + construction (in `run_agentiflow`, inside the worker thread, after `std::fs::create_dir_all(run_dir.join("lead"))`):**
```rust
    let board = std::sync::Arc::new(rupu_fleet::Board::new(run_dir.join("board")));
    let mailbox = std::sync::Arc::new(rupu_fleet::Mailbox::new(run_dir.join("mailboxes")));
    let fleet_ctx = std::sync::Arc::new(crate::tools::FleetToolCtx::new(
        board.clone(), mailbox.clone(), "lead"));
    let extra_tools = crate::tools::fleet_tools(fleet_ctx);
    let collectors = crate::collectors::lead_collectors(mailbox.clone(), board.clone(), "lead");
```
(`Board::new`/`Mailbox::new` create their dirs lazily on first write; no explicit `create_dir_all` needed, but add it if the tests show otherwise.)

- [ ] **Step 3: Populate `LeadConfig`.** Set `extra_tools`, `collectors`, `scope_name: Some(id.clone())`, `findings_engagement: Some(Arc::new(active.clone()))`, and ensure `agent_tools` contains `"report_finding"` (push it if absent — do this on the `lead.agent_tools` the caller provided). Clone `active` before it is moved into the envelope.

- [ ] **Step 4: Integration test — the full round-trip.** In `run.rs` tests, script the lead across two rounds (`ceiling: { rounds: 2 }`): round 0 calls `board.post` and `report_finding`; between rounds, the test cannot interleave, so instead pre-seed a directive before the run and assert it reaches the lead. Concretely, script the provider to, on its first turn, emit a `board.post`, then stop the round; assert after the run that: (a) `run_dir/board` contains the post (read via a fresh `Board::read_posts`), (b) the pooled-scope finding exists (Task 3's assertion), and (c) a `Directive` pre-seeded onto the board before the run appears in the lead's round-0 prompt/history (search `final`-ish transcript or the persisted `lead.r0.jsonl` for the directive body). Keep assertions on observable store state + transcript content, not on internal counters.

- [ ] **Step 5: Full crate + clippy.**
```bash
cargo test -p rupu-agentiflow 2>&1 | tail -6
cargo clippy -p rupu-agentiflow --all-targets -- -D warnings 2>&1 | tail -4
```

- [ ] **Step 6: Commit.**
```bash
git add -A && git commit -m "feat(agentiflow): wire board/mailbox tools + collectors into the lead run"
```

---

## Self-Review

- **Spec coverage:** §8.5 Mailbox/Directive collectors (Task 4) ✓; §9.1 board claim/post/read/directive (Task 2) ✓; §9.2 mailbox `msg.send` (Task 2) ✓; §13 Tier-1 always-on semantics (Task 1 insert-after-filter_to) ✓; §14 `verified` findings on the board that goals read — findings now reach the pooled scope the evaluator reads (Task 3) ✓. Deferred by design (stated in Scope boundary): §10 dispatch/isolation, §12/§13 Tier-2 `dispatch`/`generate_workflow`, the roster + `*.status` pull-tools, `StatusCollector`/`RosterCollector`.
- **Type consistency:** `extra_tools: Vec<Arc<dyn rupu_tools::Tool>>` identical in `AgentRunOpts` (Task 1) and `LeadConfig` (Task 5); `collectors: Vec<Arc<dyn rupu_agent::TurnCollector>>` matches the existing `AgentRunOpts.collectors` type; `findings_engagement: Option<Arc<ActiveSet>>` matches `FindingWriteOptions.engagement`.
- **Placeholder scan:** the only "read the real struct" instruction is `FindingWriteOptions`'s exact field set (Task 3 Step 3/5) — the implementer confirms it from source; the semantic (engagement = the run's `ActiveSet`, default otherwise) is fixed. The elided tools in Task 2 Step 3 are named with their exact schemas + behavior and must be written out in full.
- **Fail-closed:** `board.post` rejects an unknown `kind` (Task 2); tool store errors surface as `ToolOutput.error`, never panics; `extra_tools` default empty everywhere (Task 1).

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-10-05-rupu-agentiflows-plan-3b1-lead-coordination.md`. Two execution options:

1. **Subagent-Driven (recommended)** — a fresh subagent per task, task review between, broad review at the end.
2. **Inline Execution** — execute tasks in this session with checkpoints.

3b-2 (non-blocking process-isolated dispatch + `join` + reaper + the roster + `generate_workflow` + profile propagation) is the next plan and depends on this one (its claim-dedup uses this board; it watches units via these collectors).
