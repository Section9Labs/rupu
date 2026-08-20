# rupu.app macOS — Phase 2: Read Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Activity and Run-detail placeholders with the real read path — normalized execution table with live tail, run detail (step graph, transcript feed, netflow + findings rails), read-only session detail — on top of testable snapshot/delta/re-snapshot store primitives.

**Architecture:** Per-screen `@Observable` stores compose three new RupuStore primitives (`PagedSnapshot`, `LiveReducer`, `StreamLifecycle`). Four differently-shaped CP list endpoints normalize into one client-side `ActivityRow`. Golden fixtures emitted from rupu-cp's real serde types gate every new Swift model. Two new RupuKit modules: `RupuActivity`, `RupuRunDetail`.

**Tech Stack:** SwiftUI (macOS 14+), Swift 6, Swift Testing; Rust (rupu-cp fixture tests). No third-party deps.

**Spec:** `docs/superpowers/specs/2026-08-20-rupu-macos-phase-2-read-path-design.md` (umbrella: `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md`; visual: `docs/macOS_design/HANDOFF.md` screens 2 & 8)

## Global Constraints

- Strict read-only: NO mutation controls render anywhere (spec §1.1). Gated runs show awaiting state only.
- No-silent-noop: a control without a wired endpoint does not render. Null discipline: unknown → `—` via `Fmt`, never 0 (partial sums `+`).
- Live tail and transcript streaming are LOCAL runs only; remote runs render REST blocks + literal note "Remote streaming lands with Fleet (Phase 5)" (spec §1.4).
- Saved views: NOT in this phase (spec §1.3).
- Re-snapshot-on-reconnect: a store must re-snapshot via REST before resuming SSE delta application after any reconnect (spec §2, `StreamLifecycle`).
- Per-block loading: `RunDetail`'s four blocks load/fail independently; one slow block never blanks the page.
- No third-party Swift deps; Swift Testing (`import Testing`, `@Test`); app target stays thin.
- Rust: workspace deps only; never package-wide `cargo fmt` (rustfmt only files you create/edit); `#![deny(clippy::all)]`.
- Never run bare `git stash pop`.
- All work on branch `feat/macos-phase2` (controller manages branching); one commit per task minimum.
- GUI rule: build+test green ≠ rendering green; matt runs the app before merge.

## Verified API facts (from source, 2026-08-20 — the plan argues from these)

- `GET /api/runs?offset&limit&host` → **bare JSON array** of `RunListRow` objects + injected `"host_id"`: `{id, workflow_name, status (snake_case RunStatus: pending|running|completed|failed|awaiting_approval|rejected|cancelled|paused), started_at (RFC3339), finished_at?, trigger, usage: UsageSummary, turns, duration_ms?, host_id?}`. `UsageSummary = {input_tokens, output_tokens, cached_tokens, total_tokens, cost_usd?, priced, runs}`.
- `GET /api/runs/workflows?offset&limit&lifecycle&host` → same `RunListRow` shape (lifecycle ∈ active|completed|failed).
- `GET /api/runs/agents?offset&limit&lifecycle&host` (in `api/run_streams.rs`) → array of `AgentRunRow`: `{run_id, source ("standalone"|"session"), agent?, session_id?, trigger_source?, status? (string), started_at? (string), transcript_path?, usage, turns, duration_ms?, host_id?}`.
- `GET /api/runs/autoflows/events?...` → array of `AutoflowEventRow`: `{event_id, cycle_id, at, kind, workflow?, issue_display_ref?, run_id?, status?, worker_name?, usage, turns?, duration_ms?, detail?, host_id?}` — the per-launch surface Activity uses for the autoflows kind.
- `GET /api/sessions?offset&limit&scope&host` → array of `SessionDto` + injected `"scope"` ("active"|"archived"), `"usage"`, `"host_id"`: `{session_id, agent_name, model, provider_name, status (raw JSON value), total_turns, total_tokens_in/out/cached, created_at, updated_at, active_run_id?, last_error?, target?, workspace_id}`.
- `GET /api/runs/:id?host` → `{run: RunRecord, steps: [StepResultRecord], usage: UsageSummary}`. `RunRecord` key fields for UI: `id, workflow_name, status, workspace_id, started_at, finished_at?, error_message?, awaiting: [{step_id, prompt?, since, ...}] (default []), active_step_id?, active_step_transcript_path?, parent_run_id?, permission_mode?, final_output?`. `StepResultRecord`: `{step_id, run_id, transcript_path, output, success, skipped, rendered_prompt, kind (StepKind snake_case), items[], findings[], iterations}`.
- `GET /api/runs/:id/graph?host` → `{run: RunRecord, workflow: {steps: [StepNodeDto]}, step_results: [StepResultRecord], units: [...], usage}`. `StepNodeDto`: `{id, kind ("step"|"for_each"|"parallel"|"panel"|"action"|"run"|"gate"), agent?, for_each?, parallel?: [{id, agent}], panelists?: [String], gate?: {max_iterations, until_severity, fix_with}, action?, approval_gate?: {auto_approve, has_on_reject, timeout_seconds?}}`. `units` elements: usually `{step_id, index, item, run_id, transcript_path, output, success (bool), finished_at, host?}` but events-synthesized units carry `success: null` — Swift decodes `success` as `Bool?`.
- `GET /api/transcript?path=<file.jsonl>&host` → `{events: [TranscriptEvent], summary: RunSummary?}`; missing file → `{"events":[],"summary":null}` (200). **`path` is the full transcript file path** (from `StepResultRecord.transcript_path` / `RunRecord.active_step_transcript_path` / unit rows / `AgentRunRow.transcript_path` / `SessionRunRow.transcript_path`).
- `TranscriptEvent` is **adjacently tagged**: `{"type":"<snake_case>","data":{...}}` (unlike the internally-tagged orchestrator CPEvent). Variants Phase 2 renders: `assistant_message {content, thinking?}`, `assistant_delta {content}`, `tool_call {call_id, tool, input}`, `tool_result {call_id, output, error?, duration_ms, structured?}`, `gate_requested {gate_id, prompt, decision?, decided_by?}`, `run_start {run_id, workspace_id, agent, provider, model, started_at, mode}`, `turn_start {turn_idx}`, `turn_end {turn_idx, tokens_in?, tokens_out?}`, `file_edit {path, kind, diff}`, `command_run {argv, cwd, exit_code, stdout_bytes, stderr_bytes}`, `usage {provider, model, served_model?, input_tokens, output_tokens, cached_tokens}`, `run_complete {run_id, status (ok|error|aborted), total_tokens, duration_ms, error?}`, `action_emitted`, `tool_audit`, `net_flow` — plus unknown-tolerant fallback.
- `GET /api/transcript/stream?path&host` → SSE; each `data:` frame is one `TranscriptEvent` JSON (same shape, no envelope); keep-alive comments every 15s.
- `GET /api/runs/:id/netflow?from&to` → `{flows: [FlowView], hosts: [HostRollup], window, dropped_total, asn_loaded}`. `FlowView` = flattened `FlowRecord` (`{id, ts, ctx: {run_id?, step_id?, agent?, workspace_id?, origin}, fidelity, method, scheme, host, port, path, status?, outcome (ok|http_error|transport_error|timeout), error?, bytes_out?, bytes_in?, body_complete, ttfb_ms?, duration_ms?, ...}`) + `asn?: {asn, org}`. `HostRollup` = `{host, port, calls, bytes_in?, bytes_out?, errors, p50_ms?, p95_ms?}`. **There is NO server-side "unexpected host" flag** — HANDOFF's "unexpected hosts loud" renders this phase as: rollup rows with `errors > 0` or any non-`ok` outcome in fail color; a true allowlist diff is future work (note in code comment).
- `GET /api/findings?run_id=<id>` → `{findings: [FindingOut], summary: {total, critical, high, medium, low, info}}`. `FindingOut` = `{ws_id, project, target_id, workflow_name?, permalink?}` + flattened `FindingRecord` `{id, file_path?, line_range? ([u32;2]), scope (line|file|repo), summary, severity (info|low|medium|high|critical), concern_id?, evidence: {code_excerpt?, rationale, references[]}, declared_by: {run_id, model, surface}, declared_at}`. `run_id` scoping includes fan-out sub-runs server-side.
- `GET /api/events?limit&before_ts&before_run&before_pos` → array of orchestrator CPEvent JSON with `ts` (i64 unix-ms) and `pos` (usize) injected into EVERY row unconditionally (verified events.rs:296-298) → **`CPEventRow.ts`/`.pos` become non-optional.**
- `GET /api/runs/:id/usage-timeline?host` → `[{turn, label, tokens_in, tokens_out, tokens_cached}]` (not rendered this phase; fixture only if free, else skip).
- No pagination envelope anywhere: paging is offset/limit in, bare array out; "end" = short page.

## File structure

```
crates/rupu-cp/tests/macos_fixtures.rs        # Modify: add pub-type fixtures + transcript-event exhaustiveness guard
crates/rupu-cp/src/api/run_streams.rs         # Modify: #[cfg(test)] fixture tests for private AgentRunRow/AutoflowEventRow
crates/rupu-cp/src/api/sessions.rs            # Modify: #[cfg(test)] fixture test for SessionDto (+injections)
crates/rupu-cp/src/api/netflow.rs             # Modify: #[cfg(test)] fixture test for NetflowResponse
crates/rupu-cp/src/api/findings.rs            # Modify: #[cfg(test)] fixture test for FindingsResponse
apps/rupu-macos/Fixtures/*.json               # New golden files (list below)
RupuKit/Sources/RupuAPI/ListRows.swift        # RunListRow/AgentRunRow/AutoflowEventRow/SessionRow models
RupuKit/Sources/RupuAPI/RunDetailModels.swift # RunRecord/StepResultRecord/RunDetailResponse/GraphResponse/StepNodeDto/UnitRow
RupuKit/Sources/RupuAPI/TranscriptModels.swift# TranscriptEvent + RunSummary
RupuKit/Sources/RupuAPI/NetflowModels.swift   # NetflowResponse/FlowView/HostRollup
RupuKit/Sources/RupuAPI/FindingsModels.swift  # FindingsResponse/FindingOut/Severity
RupuKit/Sources/RupuAPI/CPClient.swift        # Modify: new endpoint methods
RupuKit/Sources/RupuAPI/Models.swift          # Modify: CPEventRow ts/pos non-optional
RupuKit/Sources/RupuAPI/SSE.swift             # Modify: generic JSONEventStream<T> + backoff-reset-on-connect
RupuKit/Sources/RupuStore/ActivityRow.swift   # Normalized row + per-kind mapping
RupuKit/Sources/RupuStore/Primitives/PagedSnapshot.swift
RupuKit/Sources/RupuStore/Primitives/LiveReducer.swift
RupuKit/Sources/RupuStore/Primitives/StreamLifecycle.swift
RupuKit/Sources/RupuStore/ActivityStore.swift
RupuKit/Sources/RupuStore/RunDetailStore.swift
RupuKit/Sources/RupuStore/SessionDetailStore.swift
RupuKit/Sources/RupuStore/Route.swift         # Modify: runDetail/sessionDetail cases
RupuKit/Sources/RupuActivity/ActivityScreen.swift, ActivityTable.swift, FilterBar.swift
RupuKit/Sources/RupuRunDetail/GraphLayout.swift, StepGraphView.swift, TranscriptFeed.swift,
                              RunDetailScreen.swift, RailViews.swift, SessionDetailScreen.swift
RupuKit/Sources/RupuShell/RootView.swift      # Modify: route wiring for the two new screens
RupuKit/Package.swift                         # Modify: new targets RupuActivity, RupuRunDetail (+ test targets)
```

New fixtures: `run_list_row.json` (array of 2: running + awaiting_approval), `agent_run_rows.json`, `autoflow_event_rows.json`, `session_rows.json`, `run_detail.json`, `run_graph.json` (one node of EVERY StepNodeDto kind incl. gate + action + run), `transcript_events.json` (every rendered variant), `netflow_run.json`, `findings_run.json`, `event_rows.json` (CPEvent + ts/pos).

---

### Task 1: Rust fixture emitters for every Phase 2 shape

**Files:**
- Modify: `crates/rupu-cp/tests/macos_fixtures.rs`
- Modify: `crates/rupu-cp/src/api/run_streams.rs`, `src/api/sessions.rs`, `src/api/netflow.rs`, `src/api/findings.rs` (append `#[cfg(test)] mod tests` each — private types can't be built from integration tests; the in-module pattern is established in `src/api/host_info.rs`)
- Create (generated): the ten fixture files listed above
- Modify: `Makefile` (`macos-fixtures` target: add `REGEN_FIXTURES=1 cargo test -p rupu-cp fixture_is_current` so the in-module tests regen too — the shared substring `fixture_is_current` in every test name is the filter contract)

**Interfaces:**
- Consumes: existing `check_fixture` helper pattern (write `rendered + "\n"` on REGEN_FIXTURES, else assert `on_disk.trim_end() == rendered`).
- Produces: the ten checked-in JSON files; every fixture test name ends in `fixture_is_current`.

- [ ] **Step 1: Public-type fixtures in `tests/macos_fixtures.rs`.** Append tests constructing and checking: `run_list_row.json` — build two `rupu_cp`-public `RunListRow`... **NOTE:** `RunListRow` is `pub` in `crate::api::runs`; if the module path is not exported, mirror the handler instead: construct the row, serialize, then `obj.insert("host_id", json!("local"))` exactly as `list_runs` does. Sample values: row 1 `{id:"run-01", workflow_name:"nightly-health", status: RunStatus::Running, started_at: fixed 2026-08-20T12:00Z, finished_at: None, trigger:"cron", usage: UsageSummary{input_tokens:1000, output_tokens:200, cached_tokens:0, total_tokens:1200, cost_usd:Some(0.12), priced:true, runs:1}, turns:4, duration_ms:None}`; row 2 status `AwaitingApproval`, finished None, trigger "manual", host_id "mini". Also `transcript_events.json`: a `Vec<rupu_transcript::Event>` covering EVERY variant listed in "Verified API facts" (adjacently tagged output), each with all-Some optionals somewhere and a None counterpart where the variant has optionals. Also `event_rows.json`: take 3 orchestrator `Event`s (RunStarted, StepCompleted, RunCompleted), serialize, insert `"ts": 1755691200000i64` and `"pos": 0/1/2` — mirroring `recent_events`. Also an **exhaustiveness guard** over `rupu_transcript::Event` (match with no wildcard, same pattern as the existing orchestrator-Event guard) so new transcript variants break compile here.
- [ ] **Step 2: In-module fixture tests.** In `run_streams.rs` tests: build one `AgentRunRow` (all Options Some; source "session") + one with Nones (source "standalone"), serialize as array → `agent_run_rows.json`; two `AutoflowEventRow` (one `run_launched` with run_id/turns/duration, one `cycle_failed` with detail, no run_id) → `autoflow_event_rows.json`. In `sessions.rs` tests: one `SessionDto`, serialize, insert `"scope":"active"`, `"usage"`, `"host_id":"local"` exactly as the handler does → `session_rows.json` (array of 1). In `netflow.rs` tests: a `NetflowResponse` with one `FlowView` (outcome `Ok`, asn Some) + one (outcome `TransportError`, error Some, asn None) and two `HostRollup`s (one with `errors: 3`) → `netflow_run.json`. In `findings.rs` tests: `FindingsResponse` with two `FindingOut` (severities `critical` + `info`, one with file_path+line_range, one repo-scope without) → `findings_run.json`. In `runs.rs` tests (or the integration file if `RunRecord`/`StepResultRecord` re-exports allow): `run_detail.json` = `json!({"run": RunRecord{...status: AwaitingApproval, awaiting: [AwaitingGate{step_id:"gate", prompt:Some("deploy?"), ...}], active_step_id: Some("gate"), ...}, "steps": [one successful StepResultRecord with kind Linear + one with kind Panel/findings], "usage": ...})`; `run_graph.json` = the graph handler's shape with `workflow.steps` containing SEVEN nodes, one per kind (`step`,`for_each`,`parallel`,`panel`,`gate`,`action`,`run`) with their kind-specific fields per the verified shapes, `units` containing one checkpoint-shaped unit (`success: true`) and one synthesized unit (`success: null` — hand-built `json!` to lock the mismatch), plus `step_results`.
- [ ] **Step 3: Regenerate + verify** — `make macos-fixtures` writes all ten; then WITHOUT the env var run `cargo test -p rupu-cp fixture_is_current` and `cargo test -p rupu-cp --test macos_fixtures`: all PASS. Inspect each file once (shape sanity: transcript events have `type`+`data`, event_rows have flat `type`+`ts`+`pos`).
- [ ] **Step 4: Clippy + fmt scoped** — `cargo clippy -p rupu-cp --all-targets -- -D warnings` clean for your additions; rustfmt only edited regions stay scoped (check `git diff`).
- [ ] **Step 5: Commit** — `feat(macos): Phase 2 golden fixtures — list rows, run detail, graph, transcript, netflow, findings`

### Task 2: Swift models + CPClient methods (list rows, sessions, CPEventRow tightening)

**Files:**
- Create: `RupuKit/Sources/RupuAPI/ListRows.swift`
- Modify: `RupuKit/Sources/RupuAPI/Models.swift` (CPEventRow), `CPClient.swift`
- Test: `RupuKit/Tests/RupuAPITests/ListRowDecodingTests.swift`

**Interfaces:**
- Consumes: fixtures from Task 1; existing `CPClient.get`, `Fixtures.data(_:)`.
- Produces (exact, later tasks compile against):
  - `public struct APIRunListRow: Decodable, Equatable, Sendable { id, workflowName, status: String, startedAt: String, finishedAt: String?, trigger: String, usage: APIUsageSummary, turns: UInt64, durationMS: UInt64?, hostID: String? }`
  - `public struct APIUsageSummary: Decodable, Equatable, Sendable { inputTokens, outputTokens, cachedTokens, totalTokens: UInt64; costUSD: Double?; priced: Bool; runs: UInt64 }`
  - `public struct APIAgentRunRow: Decodable, Equatable, Sendable { runID: String, source: String, agent: String?, sessionID: String?, triggerSource: String?, status: String?, startedAt: String?, transcriptPath: String?, usage: APIUsageSummary, turns: UInt64, durationMS: UInt64?, hostID: String? }`
  - `public struct APIAutoflowEventRow: Decodable, Equatable, Sendable { eventID, cycleID, at, kind: String; workflow, issueDisplayRef, runID, status, workerName, detail: String?; usage: APIUsageSummary; turns: UInt64?; durationMS: UInt64?; hostID: String? }`
  - `public struct APISessionRow: Decodable, Equatable, Sendable { sessionID, agentName, model, providerName: String; totalTurns: UInt32; totalTokensIn, totalTokensOut, totalTokensCached: UInt64; createdAt, updatedAt: String; activeRunID, lastError, target: String?; workspaceID: String; scope: String?; usage: APIUsageSummary?; hostID: String? }` (session `status` is deliberately NOT decoded this phase — it's a raw JSON value of varying shape; `activeRunID`/`lastError` carry the UI signal)
  - `CPEventRow`: `ts: Int64`, `pos: Int` (non-optional; decoding a row without them THROWS).
  - `CPClient` methods: `func runs(offset: Int, limit: Int) async throws -> [APIRunListRow]`, `workflowRuns(offset:limit:)`, `agentRuns(offset:limit:) -> [APIAgentRunRow]`, `autoflowEvents(offset:limit:) -> [APIAutoflowEventRow]`, `sessions(offset:limit:) -> [APISessionRow]` — all snake_case CodingKeys explicit.

- [ ] **Step 1: Failing decode tests** — decode each of the four list fixtures; assert representative fields (row 2 of run_list_row has `status == "awaiting_approval"`, `hostID == "mini"`; agent row 1 `source == "session"`; autoflow `cycle_failed` row has `runID == nil`, `detail != nil`; session row `scope == "active"`). Plus `CPEventRow` against `event_rows.json`: all three decode, `ts == 1755691200000`, `pos` 0/1/2; and a row WITHOUT ts must fail to decode (inline JSON literal, `#expect(throws:)`).
- [ ] **Step 2: RED** (`make macos-test`), **implement**, **GREEN** — status stays `String` at the API layer (normalization is the store's job, Task 4).
- [ ] **Step 3: CPClient methods** + URLProtocol-stub test for one of them (path + query assertion for `runs(offset:limit:)` → `/api/runs?offset=0&limit=50`).
- [ ] **Step 4: Commit** — `feat(macos): Phase 2 list-row models + CPClient list endpoints; CPEventRow ts/pos non-optional`

### Task 3: Swift models — run detail, graph, transcript, netflow, findings + generic SSE

**Files:**
- Create: `RupuKit/Sources/RupuAPI/RunDetailModels.swift`, `TranscriptModels.swift`, `NetflowModels.swift`, `FindingsModels.swift`
- Modify: `RupuKit/Sources/RupuAPI/SSE.swift`, `CPClient.swift`
- Test: `RupuKit/Tests/RupuAPITests/DetailDecodingTests.swift`, `TranscriptDecodingTests.swift`

**Interfaces:**
- Consumes: Task 1 fixtures; `SSELineParser` (unchanged).
- Produces:
  - `public struct APIRunDetail: Decodable, Sendable { run: APIRunRecord, steps: [APIStepResult], usage: APIUsageSummary }`
  - `public struct APIRunRecord: Decodable, Sendable` — decode ONLY the UI fields listed in Verified API facts (id, workflowName, status, workspaceID, startedAt, finishedAt?, errorMessage?, awaiting: [APIAwaitingGate] defaulting to [], activeStepID?, activeStepTranscriptPath?, parentRunID?, permissionMode?, finalOutput?) — `decodeIfPresent` throughout; extra server fields ignored by design.
  - `public struct APIAwaitingGate: Decodable, Sendable { stepID: String, prompt: String?, since: String }`
  - `public struct APIStepResult: Decodable, Sendable { stepID, runID, transcriptPath, output: String; success, skipped: Bool; kind: String; iterations: UInt32 }` (kind defaults "linear"; iterations defaults 0)
  - `public struct APIRunGraph: Decodable, Sendable { run: APIRunRecord, workflow: APIStepDag, stepResults: [APIStepResult], units: [APIUnitRow], usage: APIUsageSummary }`; `APIStepDag { steps: [APIStepNode] }`; `public struct APIStepNode: Decodable, Sendable { id: String, kind: String, agent: String?, forEach: String?, parallel: [APISubStep]?, panelists: [String]?, gate: APIPanelGate?, action: String?, approvalGate: APIApprovalGate? }`; `APISubStep {id, agent}`; `APIPanelGate {maxIterations: UInt32, untilSeverity: String, fixWith: String}`; `APIApprovalGate {autoApprove: Bool, hasOnReject: Bool, timeoutSeconds: UInt64?}`; `public struct APIUnitRow: Decodable, Sendable { stepID: String, index: Int, runID: String?, transcriptPath: String, success: Bool?, host: String? }` (`success: Bool?` — locks the null-vs-bool server mismatch).
  - `public enum TranscriptEvent: Decodable, Equatable, Sendable` — adjacently tagged decoder (`type` + `data` containers): cases exactly per Verified API facts + `.unknown(type: String)`; `public struct APIRunSummary: Decodable, Sendable { runID, agent, provider, model, status: String; totalTokens: UInt64; durationMS: UInt64; error: String?; firstAssistantText: String? }`; `public struct APITranscriptPage: Decodable, Sendable { events: [TranscriptEvent], summary: APIRunSummary? }`
  - `public struct APINetflow: Decodable, Sendable { flows: [APIFlow], hosts: [APIHostRollup], droppedTotal: UInt64, asnLoaded: Bool }`; `APIFlow` (flattened — decode from the SAME container: id, ts, method, scheme, host, port: UInt16, path, status: UInt16?, outcome: String, error: String?, bytesIn/Out: UInt64?, durationMS: UInt64?, ctx: APIFlowCtx {runID?, stepID?, agent?}, asn: APIAsn? {asn: UInt32, org})`; `APIHostRollup {host, port: UInt16, calls: UInt64, bytesIn/Out: UInt64?, errors: UInt64, p50MS/p95MS: UInt64?}`
  - `public struct APIFindings: Decodable, Sendable { findings: [APIFinding], summary: APIFindingsSummary }`; `APIFinding` (flattened FindingOut+FindingRecord: id, summary, severity: String, scope: String, filePath: String?, lineRange: [UInt32]?, project: String, workflowName: String?, permalink: String?, rationale (from evidence.rationale via nested decode), declaredAt: String)`; `APIFindingsSummary {total, critical, high, medium, low, info: Int}`
  - **Generic SSE**: `public final class JSONEventStream<T: Decodable & Sendable>: Sendable { public init(url: URL, token: String?, session: URLSession = .shared, onConnectionChange: (@Sendable (Bool) -> Void)? = nil); public func events() -> AsyncStream<T> }` — the existing `EventStreamClient` becomes `public typealias EventStreamClient = JSONEventStream<CPEvent>` (all Phase 1 call sites compile unchanged; parser + backoff + logging move verbatim). **Carry-over fix folded in:** backoff resets when a connection is successfully established (`didSignalConnected`), not only on first decoded frame.
  - `CPClient` methods: `runDetail(id: String, host: String?) async throws -> APIRunDetail`, `runGraph(id:host:) -> APIRunGraph`, `transcript(path: String, host: String?) -> APITranscriptPage`, `runNetflow(id:) -> APINetflow`, `runFindings(id:) -> APIFindings` (via `/api/findings?run_id=`), `sessionDetail(id:) -> APISessionRow`, `sessionRuns(id:) -> [APISessionRunRow]` with `public struct APISessionRunRow: Decodable, Sendable { runID: String, prompt: String, transcriptPath: String, status: String?, startedAt: String?, completedAt: String?, tokensIn, tokensOut: UInt64, durationMS: UInt64, error: String? }`.

- [ ] **Step 1: Failing tests** — decode `run_detail.json` (awaiting gate present, prompt "deploy?"), `run_graph.json` (**assert all seven node kinds decode with their kind-specific payloads**; synthesized unit decodes with `success == nil`), `transcript_events.json` (every variant decodes, none `.unknown`; a made-up `{"type":"future","data":{}}` decodes `.unknown`), `netflow_run.json` (transport_error flow has `error != nil`; rollup errors == 3), `findings_run.json` (severity strings; repo-scope finding has `filePath == nil`).
- [ ] **Step 2: RED → implement → GREEN.** The adjacently-tagged decoder: top-level container reads `type: String`, then `nestedContainer(keyedBy:forKey:.data)` per case.
- [ ] **Step 3: SSE generalization** — mechanical rename to `JSONEventStream<T>` + typealias; existing 8 parser tests + suite stay green untouched; add one test: `SSELineParser` output decoded as `TranscriptEvent` via the generic path (feed an `assistant_message` frame string through the parser and decode).
- [ ] **Step 4: Run** `make macos-test` (all green incl. Phase 1 suites — the typealias proves source compatibility), commit — `feat(macos): detail/graph/transcript/netflow/findings models + generic JSONEventStream`

### Task 4: Store primitives + ActivityRow normalization

**Files:**
- Create: `RupuKit/Sources/RupuStore/Primitives/PagedSnapshot.swift`, `LiveReducer.swift`, `StreamLifecycle.swift`, `RupuKit/Sources/RupuStore/ActivityRow.swift`
- Test: `RupuKit/Tests/RupuStoreTests/PagedSnapshotTests.swift`, `ActivityRowTests.swift`, `StreamLifecycleTests.swift`

**Interfaces:**
- Consumes: Task 2/3 API models; `JSONEventStream`, `CPEvent`.
- Produces:
  - `public enum BlockState<T: Sendable>: Sendable { case loading, content(T), empty, failed(String) }` with `public var value: T?`
  - `@MainActor @Observable public final class PagedSnapshot<Row: Sendable & Identifiable> { public private(set) var rows: [Row]; public private(set) var state: BlockState<Void>; public private(set) var exhausted: Bool; public init(pageSize: Int = 50, fetch: @escaping @Sendable (_ offset: Int, _ limit: Int) async throws -> [Row]); public func refresh() async; public func loadMore() async }` — refresh replaces rows with page 0; a short page sets `exhausted`; failures set `.failed` keeping stale rows visible.
  - `public enum ActivityKindTag: String, Sendable { case agent, workflow, autoflow, session }`
  - `public enum ActivityStatus: Equatable, Sendable { case pending, running, completed, failed, awaiting, rejected, cancelled, paused, unknown(String); public var tone: RunTone }` (tone: running→.run, completed→.done, failed/rejected→.fail, awaiting→.waiting, paused/cancelled→.pause, pending/unknown→.pause) with `public static func normalize(_ raw: String?) -> ActivityStatus` — snake_case run statuses map 1:1; agent/autoflow string statuses ("ok"→completed, "error"→failed, "aborted"→cancelled, "running"→running); nil → `.unknown("—")`.
  - `public struct ActivityRow: Identifiable, Equatable, Sendable { public let id: String; kind: ActivityKindTag; subject: String; project: String?; host: String; trigger: String?; status: ActivityStatus; durationMS: UInt64?; costUSD: Double?; startedAt: Date?; navigation: Navigation; public enum Navigation: Equatable, Sendable { case run(id: String, host: String?), session(id: String), none } }` + mapping inits: `init(_ r: APIRunListRow)` (kind .workflow, subject workflow_name, navigation .run, startedAt via ISO8601 parse), `init(_ r: APIAgentRunRow)` (subject = agent ?? "agent run", navigation .run(runID)), `init(_ r: APIAutoflowEventRow)` (subject = workflow ?? kind; navigation = runID.map { .run } ?? .none; id = eventID), `init(_ r: APISessionRow)` (kind .session, subject = agentName, project = workspaceID, navigation .session, status: activeRunID != nil ? .running : (lastError != nil ? .failed : .completed))`. Date parsing helper `parseISO(_ s: String?) -> Date?` handles RFC3339 with fractional seconds.
  - `public enum ActivityDelta: Equatable, Sendable { case statusPatch(runID: String, status: ActivityStatus, durationMS: UInt64?), newRun(runID: String), none }` with `public static func reduce(_ e: CPEvent) -> ActivityDelta` (LiveReducer.swift): runStarted→newRun; runCompleted→statusPatch(normalize(status)); runFailed→failed; runPaused→paused; runResumed→running; stepAwaitingApproval→awaiting; everything else→none.
  - `@MainActor @Observable public final class StreamLifecycle { public enum Freshness: Equatable, Sendable { case idle, live, stale } ; public private(set) var freshness: Freshness; public init(); public func start<T: Decodable & Sendable>(stream: JSONEventStream<T>, resnapshot: @escaping @Sendable () async -> Void, apply: @escaping @MainActor (T) -> Void); public func stop() }` — semantics under test: on connect after a previous disconnect (i.e. any reconnect), `resnapshot()` completes BEFORE the next `apply`; first connect does not resnapshot (the store's initial load already did); `stop()` cancels the consumer + connection; deallocation-safe (weak-self loop per the Phase 1 HealthMonitor pattern).

- [ ] **Step 1: Failing tests.** `PagedSnapshot`: fake fetch closure over a 120-item array → refresh gives 50, loadMore→100, loadMore→120 + exhausted; fetch throwing on page 1 → `.failed` with prior rows intact; refresh after failure recovers. `ActivityRow`: decode the four fixtures via Task-2 models and map — assert subject/kind/navigation/status per row (incl. autoflow `cycle_failed` → `.none` navigation, session with activeRunID → .running). `normalize`: table-driven over all raw statuses. `StreamLifecycle`: drive it with a scripted fake — since `JSONEventStream` is a final class with a network loop, test via a seam: add `internal init(frames: AsyncStream<T>, onConnectionChange:...)`-style injection OR extract the protocol `EventStreaming` — choose the smallest seam: `public protocol EventStreaming<Element>: Sendable { associatedtype Element: Decodable & Sendable; func events() -> AsyncStream<Element>; var onConnectionChange: (@Sendable (Bool) -> Void)? { get set } }`, conform `JSONEventStream`, have `StreamLifecycle.start` accept `any EventStreaming<T>` and tests inject a `FakeStream` that scripts connect→frames→disconnect→connect→frames; assert resnapshot-before-apply ordering via an actor-backed event log.
- [ ] **Step 2: RED → implement → GREEN** (`make macos-test`).
- [ ] **Step 3: Commit** — `feat(macos): store primitives (PagedSnapshot/LiveReducer/StreamLifecycle) + ActivityRow normalization`

### Task 5: ActivityStore

**Files:**
- Create: `RupuKit/Sources/RupuStore/ActivityStore.swift`
- Modify: `RupuKit/Sources/RupuStore/Route.swift` (add cases), `AppModel.swift` (route plumbing only if needed)
- Test: `RupuKit/Tests/RupuStoreTests/ActivityStoreTests.swift`

**Interfaces:**
- Consumes: everything from Task 4; `CPClient` list methods (Task 2); `BackendController.client()`/`eventStream()`.
- Produces:
  - `Route` gains `.runDetail(id: String, host: String?)` and `.sessionDetail(id: String)`; `SidebarItem` mapping: both map to `.runs` for sidebar-highlight purposes (get); they are pushed states — `AppModel` gains `public func navigateBack()` returning to the stored `lastActivityRoute` (default `.activity(.all)`).
  - `@MainActor @Observable public final class ActivityStore { public var kind: RunKindFilter (drives which sources load); public var statusFilter: Set<ActivityStatus>; public var liveTail: Bool; public private(set) var rows: [ActivityRow] (merged, startedAt desc, status-filtered); public private(set) var state: BlockState<Void>; public private(set) var pendingNewRuns: Int; public private(set) var freshness: StreamLifecycle.Freshness; public init(client: CPClient, stream: any EventStreaming<CPEvent>); public func activate(kind: RunKindFilter) async; public func deactivate(); public func loadMore() async; public func applyPendingRefresh() async }`
  - Source mapping: `.agents`→agentRuns, `.workflows`→workflowRuns, `.autoflows`→autoflowEvents, `.sessions`→sessions, `.all`→all four (each its own `PagedSnapshot`; merged view sorts by startedAt desc, `nil` dates last). Delta handling: `statusPatch` mutates matching row in place; `newRun` increments `pendingNewRuns` when `liveTail == false`, else debounced (500ms) page-0 refresh of run-bearing sources (no row synthesis — an event doesn't carry enough to build an honest row). `applyPendingRefresh()` = refresh + zero the counter. Reconnect resnapshot = refresh page 0 of active sources.

- [ ] **Step 1: Failing tests** — fake client closures + `FakeStream`: (a) activate(.all) merges the four fixture-derived source arrays sorted by date; (b) statusFilter {.failed} filters the merged view; (c) with liveTail false, a `runStarted` event → `pendingNewRuns == 1`, rows unchanged; `applyPendingRefresh` refetches and zeroes; (d) with liveTail true, `runCompleted` for a visible row patches its status to `.completed` without refetch; (e) `deactivate()` stops the stream (FakeStream records cancellation); (f) reconnect (scripted disconnect+connect) triggers a refresh before further deltas apply.
- [ ] **Step 2: RED → implement → GREEN.** Route/AppModel changes extend the existing routing tests (sidebar one-state contract must still pass; add: `.runDetail` keeps sidebar highlight on `.runs`, `navigateBack()` restores the prior activity filter).
- [ ] **Step 3: Commit** — `feat(macos): ActivityStore — merged multi-source snapshot, live patching, no-jump pill semantics`

### Task 6: RupuActivity screen

**Files:**
- Modify: `RupuKit/Package.swift` (add `RupuActivity` target dep: RupuAPI/RupuStore/RupuDesign; add to RupuKit product + RupuShell deps)
- Create: `RupuKit/Sources/RupuActivity/ActivityScreen.swift`, `ActivityTable.swift`, `FilterBar.swift`
- Modify: `RupuKit/Sources/RupuShell/RootView.swift` (route `.activity` → `ActivityScreen`; placeholder dies)
- Test: existing store tests cover logic; no view unit tests (visual validation)

**Interfaces:**
- Consumes: `ActivityStore`, `ActivityRow`, RupuDesign (`MicroLabel`, `PanelStyle`, `Color.status`, `Fmt`), `AppModel` routing.
- Produces: `public struct ActivityScreen: View { public init(model: AppModel, backend: BackendController) }` — owns the store lifecycle (`.task` activate / `.onDisappear` deactivate).

- [ ] **Step 1: FilterBar** — kind segmented control bound THROUGH `model.route` (`.activity(kind)` — same state as sidebar, no parallel source of truth); additive status chips (all `ActivityStatus` cases except unknown; chip fill = `Color.status(tone)` at 12% opacity, ring 30%); live-tail `Toggle` (switch style); when `pendingNewRuns > 0`, a pill button "`\(n)` new runs" (brand tint) calling `applyPendingRefresh()`.
- [ ] **Step 2: ActivityTable** — SwiftUI `Table(rows)`: columns Status (glyph dot in `Color.status(tone)` + MicroLabel text) · Kind (MicroLabel) · Subject (flexible, `.rupuInk`) · Project (`row.project ?? "—"`, `.rupuDim`) · Host · Trigger (`?? "—"`) · Dur (`Fmt.duration` or `—`) · Cost (`costUSD` formatted "$0.12" or `—`, `Font.numeral`) · Started (relative, `Font.numeral`). Row background: `.awaiting` rows get `Color.status(.waiting).opacity(0.04)`. Row tap: switch `row.navigation` — `.run` → `model.route = .runDetail(id:host:)` (recording lastActivityRoute), `.session` → `.sessionDetail(id:)`, `.none` → not clickable (no hover affordance).
- [ ] **Step 3: States** — `.loading` (initial): panel chrome + progress; `.failed`: message + Retry button (calls refresh — a wired control, allowed); `.empty`: MicroLabel "NO EXECUTIONS IN RANGE". Staleness: when `freshness == .stale`, a dim MicroLabel "STREAM STALE — RECONNECTING" above the table.
- [ ] **Step 4: Wire RootView**, run `make macos-test && make macos-build`; `make macos-run` against the local CP (`rupu cp serve --bind 127.0.0.1:7420` or app-spawned) with real run history — screenshot table with data.
- [ ] **Step 5: Commit** — `feat(macos): Activity screen — filter bar, merged execution table, live pill states`

### Task 7: Graph layout + StepGraphView

**Files:**
- Modify: `RupuKit/Package.swift` (add `RupuRunDetail` target + `RupuRunDetailTests`)
- Create: `RupuKit/Sources/RupuRunDetail/GraphLayout.swift`, `StepGraphView.swift`
- Test: `RupuKit/Tests/RupuRunDetailTests/GraphLayoutTests.swift`

**Interfaces:**
- Consumes: `APIRunGraph`/`APIStepNode`/`APIStepResult`/`APIUnitRow` (Task 3), `CPEvent` (live overlay), RupuDesign tokens.
- Produces:
  - `public enum NodeState: Equatable, Sendable { case done(success: Bool), running, gatePending, pending, skipped }`
  - `public struct GraphNodeVM: Identifiable, Equatable, Sendable { public let id: String; kindLabel: String; agentLabel: String?; state: NodeState; laneCount: Int; unitProgress: (done: Int, total: Int)? }` (Equatable via manual `==` since tuple)
  - `public func layoutGraph(nodes: [APIStepNode], results: [APIStepResult], units: [APIUnitRow], liveStates: [String: NodeState]) -> [GraphNodeVM]` — PURE. Per node: `liveStates[id]` wins; else a matching result → `.done(success:)` or `.skipped`; else if any earlier node isn't done → `.pending`; gate node with no result and run awaiting → `.gatePending` (caller passes it via liveStates from `RunRecord.awaiting`). `laneCount` = parallel/panelists count, else 1. `unitProgress` for for_each/parallel from units (done = units where success != nil).
  - `public struct StepGraphView: View { public init(nodes: [GraphNodeVM]) }` — horizontal `ScrollView`: node capsules joined by 1px `.rupuBorderStrong` connectors; done ✓ glyph (`Color.status(.done)`, fail ✗ `.fail`), running = pulsing `.run` dot, gatePending = pulsing `.waiting` ring, pending = dashed `.rupuBorder` stroke, skipped = dimmed. Pulses wrapped in `@Environment(\.accessibilityReduceMotion)` guard. Kind label as MicroLabel under each node; unitProgress renders "3/6" in `Font.numeral`.

- [ ] **Step 1: Failing layout tests** — feed the `run_graph.json` fixture decoded via Task 3: seven nodes → assert per-kind `kindLabel`, gate node with liveStates `["approve": .gatePending]` → gatePending; results mark earlier nodes done; nodes after the gate → pending; for_each node's unitProgress (1,2) from the fixture's one-null-one-true units. Edge case: empty results + empty liveStates → first node pending (not running — we don't guess).
- [ ] **Step 2: RED → implement layout → GREEN.** Then the view (no unit test).
- [ ] **Step 3: Commit** — `feat(macos): step graph — pure layout function + horizontal graph view`

### Task 8: RunDetailStore + RunDetailScreen

**Files:**
- Create: `RupuKit/Sources/RupuStore/RunDetailStore.swift`, `RupuKit/Sources/RupuRunDetail/RunDetailScreen.swift`, `TranscriptFeed.swift`, `RailViews.swift`
- Modify: `RupuKit/Sources/RupuShell/RootView.swift` (route case)
- Test: `RupuKit/Tests/RupuStoreTests/RunDetailStoreTests.swift`

**Interfaces:**
- Consumes: Tasks 3/4/7 products; `BackendController` (client + event stream factory — needs a run-scoped stream: add `public func runEventStream(runID: String) -> JSONEventStream<CPEvent>?` and `public func transcriptStream(path: String) -> JSONEventStream<TranscriptEvent>?` to `BackendController` (URLs `/api/events/stream?run=` and `/api/transcript/stream?path=`, token passthrough)).
- Produces:
  - `@MainActor @Observable public final class RunDetailStore { public private(set) var detail: BlockState<APIRunDetail>; graph: BlockState<APIRunGraph>; netflow: BlockState<APINetflow>; findings: BlockState<APIFindings>; public private(set) var liveStates: [String: NodeState]; public private(set) var transcript: [TranscriptEvent]; public private(set) var transcriptTailActive: Bool; public private(set) var focusedTranscriptPath: String?; public let isRemote: Bool; public init(runID: String, host: String?, client: CPClient, backend: BackendController); public func activate() async; public func deactivate(); public func focusStep(_ stepID: String) async }`
  - Semantics: `activate()` fires the four REST loads concurrently (independent BlockStates). Local runs additionally start the run-scoped CPEvent stream via `StreamLifecycle` — deltas update `liveStates` (stepStarted→running, stepCompleted→done, stepAwaitingApproval→gatePending, stepSkipped→skipped) and a terminal run event (runCompleted/runFailed) stops the stream + refreshes `detail`/`findings`/`netflow` once. `focusStep`: resolves the step's transcript_path (results, else units, else activeStepTranscriptPath), loads `transcript(path:)` snapshot, then (local + step running) tails `/api/transcript/stream` appending events; switching focus tears the old tail down. Initial focus = `activeStepID` ?? last step with a result. Remote (`isRemote`): REST only; `transcriptTailActive` stays false.

- [ ] **Step 1: Failing store tests** (fake client closures returning fixture-decoded values + FakeStream): (a) activate populates all four blocks; one failing block → `.failed` while others `.content`; (b) `stepAwaitingApproval` event → `liveStates["gate"] == .gatePending`; (c) terminal `runCompleted` → stream stopped (FakeStream cancelled) + detail refetched (call-count == 2); (d) focusStep loads the right path and appends tail events in order; (e) deactivate cancels everything; (f) remote store never opens a stream.
- [ ] **Step 2: RED → implement → GREEN.**
- [ ] **Step 3: Screen composition** — `RunDetailScreen(model:backend:runID:host:)`: header (back chevron → `model.navigateBack()`, breadcrumb "Activity ▸ \(workflowName)", status pill `Color.status(tone)`, facts row: started/duration/tokens/cost via `Fmt`); awaiting banner when `detail.value?.run.awaiting` non-empty — `Color.status(.waiting)` tinted panel, gate prompt text, MicroLabel "AWAITING APPROVAL — ACTIONS ARRIVE IN PHASE 3" (honest, not a dead button); `StepGraphView(nodes: layoutGraph(...))` with `liveStates` merged (gatePending injected from `awaiting` when stream absent); `TranscriptFeed(events:)` — `assistant_message`/`assistant_delta` prose blocks (`.rupuInk`, thinking collapsed behind a disclosure), `tool_call`+`tool_result` paired collapsed rows (`Font.identifier`, JSON pretty-printed in `.rupuSurface` inner card, error text `.fail`), `gate_requested` rows with 2px `Color.status(.waiting)` left edge + decision line when present, `command_run`/`file_edit` one-line summaries, `run_complete` terminal row; unknown → skipped. Rails (right column, 280pt): facts card; netflow card — `HostRollup` rows (`host:port`, calls, p95), rows with `errors > 0` in `Color.status(.fail)` (code comment: server has no unexpected-host flag; error-based highlighting until an allowlist exists — spec §"Verified facts"); findings card — severity 2px left edge `Color.severity(...)`, summary line, count badges from `summary`. Remote runs: stream slots render MicroLabel "REMOTE STREAMING LANDS WITH FLEET (PHASE 5)".
- [ ] **Step 4: Wire route in RootView.** `make macos-test && make macos-build`; run against local CP with a real workflow run (live: graph pulses, transcript tails, terminal settles). Screenshot.
- [ ] **Step 5: Commit** — `feat(macos): run detail — store, screen, transcript feed, netflow/findings rails`

### Task 9: Session detail

**Files:**
- Create: `RupuKit/Sources/RupuStore/SessionDetailStore.swift`, `RupuKit/Sources/RupuRunDetail/SessionDetailScreen.swift`
- Modify: `RupuKit/Sources/RupuShell/RootView.swift` (route case)
- Test: `RupuKit/Tests/RupuStoreTests/SessionDetailStoreTests.swift`

**Interfaces:**
- Consumes: `APISessionRow`, `APISessionRunRow`, `TranscriptFeed`, `CPClient.sessionDetail/sessionRuns/transcript`.
- Produces: `@MainActor @Observable public final class SessionDetailStore { public private(set) var session: BlockState<APISessionRow>; runs: BlockState<[APISessionRunRow]>; transcript: [TranscriptEvent]; public init(sessionID: String, client: CPClient); public func activate() async; public func focusRun(_ run: APISessionRunRow) async }` — transcript loads the focused run's `transcriptPath` snapshot (newest run by default); read-only, no tail (sessions' live story is Phase 3). Screen: metadata header (agent/model/provider, token totals via `Fmt`, MicroLabel "READ-ONLY — SEND ARRIVES IN PHASE 3"), child-run list (rows navigate to `.runDetail`), `TranscriptFeed` reuse.

- [ ] **Step 1: Failing tests** — fixture-driven: activate populates session + runs; focusRun loads that run's transcript; a runs row with error shows in the list model. RED → implement → GREEN.
- [ ] **Step 2: Screen + RootView wiring; build; commit** — `feat(macos): read-only session detail`

### Task 10: Polish, docs, integration smoke

**Files:**
- Modify: `RupuKit/Sources/RupuShell/*` (matt's Phase 1 button-polish notes — controller supplies the concrete list at dispatch time), `CLAUDE.md` (module map: add RupuActivity/RupuRunDetail; Phase 2 plan to Read-first), umbrella spec §8 parity ledger (mark Phase 2 modules' dispositions: `runs`, `run_streams`, `transcript`, `sessions` (read), `graph`, `netflow` (per-run), `findings` (per-run) → shipped; `run_resolve`, `usage-timeline` → deferred-with-tracking)
- Test: full gates

**Interfaces:** consumes everything; produces the shippable branch.

- [ ] **Step 1: Button polish** — apply the controller-supplied list of matt's UI notes (small, shell-scoped).
- [ ] **Step 2: Docs** — CLAUDE.md + spec ledger edits per Files above.
- [ ] **Step 3: Full gates** — `make macos-test && make macos-build && cargo test -p rupu-cp && make lint` (pre-existing environmental failures excluded per the known list).
- [ ] **Step 4: Integration smoke (documented in report)** — with the app attached to a local CP: run `rupu run` / a workflow against a scratch project; verify live: new-runs pill increments (liveTail off) → applyPendingRefresh shows the row; open run detail mid-run → graph node pulses → completes; transcript tail appends; terminal state settles; netflow + findings rails populate after completion; session list shows an existing session, detail renders its transcript. Screenshot each.
- [ ] **Step 5: Commit** — `feat(macos): Phase 2 polish, docs, parity-ledger update`

---

## Self-review notes

- Spec coverage: §1 decisions → global constraints + Tasks 6/8/9 honest-state renders; §2 primitives → Task 4 (+5/8 composition); §3 Activity → Tasks 5-6; §4 Run detail → Tasks 7-8; §5 sessions → Task 9; §6 fixtures + carry-overs → Tasks 1-3 (CPEventRow non-optional T2, backoff-reset T3, stream teardown = StreamLifecycle.stop on deactivate T4/5/8, button polish T10); §7 testing → per-task TDD + T10 smoke.
- Known intentional deviations from HANDOFF encoded above: no saved views (spec §1.3); "unexpected hosts" rendered as error-based highlighting (no server flag — verified); Activity has no server-side Project column for run kinds (renders `—`, null discipline).
- Type consistency pass: `ActivityStatus.tone` ↔ RupuDesign `RunTone` (`.waiting` case name per Phase 1); `EventStreaming` protocol introduced in T4 is what T5/T8 stores accept; `BlockState` defined once in T4, used T8/T9.
