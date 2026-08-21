# rupu.app macOS — Phase 3: Write Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the app an operator console — Launcher sheet, gate Approve/Reject, cancel/pause/resume, session send, archive/restore — under the honest pending-state mutation contract, plus the Phase 2 carry-overs (navigation stack, stopTail token, statusOverrides pruning).

**Architecture:** `CPClient` grows a POST surface with bidirectional golden fixtures (responses decoded by Swift; request bodies deserialized by rupu-cp tests). A `PendingAction` RupuStore primitive tracks `(entity, verb)` action state with a per-verb confirmation table — immediate verbs confirm from the mutation response's own run record; marker verbs (approve/resume) confirm from observed status transitions via Phase 2's live machinery. New `RupuLauncher` module; mutations land in the existing screens.

**Tech Stack:** SwiftUI (macOS 14+), Swift 6, Swift Testing; Rust fixture tests in rupu-cp. No third-party deps.

**Spec:** `docs/superpowers/specs/2026-08-21-rupu-macos-phase-3-write-path-design.md` (umbrella: `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md`; HANDOFF screen 9)

## Global Constraints

- **Pending-state contract (spec §1)**: POST 200 ⇒ `pending`; confirmation per the verb table in Task 3; 30s pending ⇒ dim "still pending — server sweep may be delayed"; POST failure ⇒ inline error + retry. NEVER an optimistic flip.
- No dead verbs: Cancel/Pause/Resume/Send render only when the current status makes them meaningful; Approve/Reject only on parked gates. No silent-noop controls anywhere.
- Gate targeting is ALWAYS explicit: approve/reject send `?gate=<step_id>` (we always know the step id from `awaiting[]`); the 409 `AmbiguousGate` path should be unreachable from this app — treat it as a surfaced error if it ever occurs.
- Launcher host chips populate progressively; the sheet never blocks on a slow host; offline chips disabled. Fan-out is CLIENT-SIDE (no server endpoint — verified): one launch per selected healthy host via the body's `host` field.
- 501 responses (server without launcher runtime — plain rupu-cp instead of `cp serve`) surface as "server lacks launch runtime — start with `rupu cp serve`" (honest, not generic).
- Null discipline; PendingAction keys shared across screens; tone rule: bypass mode always loud (`Color.status(.fail)`).
- Rust: workspace deps only; clippy clean; never package-wide `cargo fmt` (rustfmt only files you create; scoped edits elsewhere).
- Timing-sensitive store tests use condition-polling (`pollUntil`/`expectEventually` — pattern exists in ActivityStoreTests), never fixed-sleep-then-assert (CI runners are slow; this bit us in Phase 2).
- `make macos-test` + `make macos-build` are the gates; matt runs the app before merge. Never bare `git stash pop`.

## Verified API facts (extracted from source 2026-08-21 — the plan argues from these)

- **Approve**: `POST /api/runs/:id/approve?host=&gate=<step_id>`; optional body `{"mode": "ask"|"bypass"|"readonly"}` (bodyless OK). Local response = `{run: RunRecord, steps: [...], usage: {...}, host_id: "local"}`; remote = `{ok: true, host_id}`. **Marker-only** — the record's status does NOT flip in the response; a `cp serve` background worker resumes it (confirmation = observed transition).
- **Reject**: same route shape; body `{"reason": String?}` — the `Json` wrapper is REQUIRED (send `{}` when no reason). **Immediate** — response record reflects the rejection.
- **Cancel**: optional body `{"reason": String?}`; **immediate** (status flips in response; live pid TERM'd). 409 `AlreadyTerminal`.
- **Pause**: no body; **immediate** + marker for detached runners. 409 when not running. **Resume**: no body; **marker-only**, 501 without launcher runtime, 409 when not `paused`.
- **Archive/Restore**: no body; response `{ok: true, id, archived: bool}`; immediate filesystem move. Remote variants return `{ok, host_id, id, archived}`.
- Errors arrive as `{"error": "<message>"}` with 400/404/409/500/501; 409 approve variants include `AmbiguousGate` (message lists candidates) and `GateNotFound`.
- **Agent run**: `POST /api/agents/:name/run`, body all-optional `{prompt?, mode?, target?, working_dir?, host?}` → `{run_id, host_id}`. 501 without `agent_launcher`. **Agent session**: `POST /api/agents/:name/session`, same body shape → `{session_id, host_id}`.
- **Workflow run**: `POST /api/workflows/:name/run`, body `{inputs: {String:String} (default {}), mode?, target?, working_dir?, host?}` → `{run_id, host_id}`. 501 without launcher. **Validate**: `POST /api/workflows/validate`, body `{raw: String}` → `{ok:true}` or 400 with a SINGLE message string (not a structured list).
- **Session send**: `POST /api/sessions/:id/send?host=`, body `{prompt: String}` (non-empty after trim; else 400) → `{run_id, host_id}`. 404 unknown session; 409 `"session <id> is stopped"`; NO API-level guard against a concurrent turn — the UI disables Send while pending.
- **`host` placement differs**: QUERY param on run/session control routes; BODY field on agents-run/session, workflows-run. Session-send takes it as QUERY.
- **No `POST /api/host_fanout`** — module is an internal read-path helper. Client-side fan-out only.
- **`GET /api/agents`** → `[AgentDto]`: `{name, slug, description?, provider?, model?, effort?, max_tokens?, tools: [String], scope: String, scope_kind ("global"|"project"), scope_id?, usage: UsageSummary, run_count, last_run: String?}` (last_run always present, null when none).
- **`GET /api/workflows`** → `[WorkflowDto]`: `{name, scope, scope_kind, scope_id?, usage, run_count, last_run?, autoflow_enabled: bool?}` (no model field).
- **`GET /api/workflows/:name`** → `{workflow: Workflow, yaml: String, usage, scope, scope_kind, scope_id?}` where `workflow.inputs` is `{<name>: {type: String (default "string"), required: bool, default?: <yaml scalar>, values?: [String]}}` — the Launcher's declared-input rows source.
- **`GET /api/tools`** → `{tools: [{name, description, input_schema, kind ("read"|"write")}]}`.
- **`GET /api/hosts`** → `[HostView]`: adds `version?`, `capabilities?`, `active_run_count: Int` (the chips' load figure — derived per request, no slots concept), `last_seen_at?`, `base_url?`; `status ∈ online|offline|stale`.
- Every write accepts remote proxying via its `host` (responses then `{ok/run_id/session_id, host_id}` without the local record).

## File structure

```
crates/rupu-cp/tests/macos_fixtures.rs        # Modify: response fixtures for write shapes + request-body ROUND-TRIP tests
crates/rupu-cp/src/api/{agents,workflows,hosts,tools}.rs  # Modify: in-module fixture tests for private DTOs
apps/rupu-macos/Fixtures/*.json               # New goldens (list in Task 1)
RupuKit/Sources/RupuAPI/CPClient.swift        # Modify: generic post + write methods
RupuKit/Sources/RupuAPI/WriteModels.swift     # Request/response models for writes
RupuKit/Sources/RupuAPI/DefinitionModels.swift# AgentDefinition/WorkflowDefinition/WorkflowDetail/InputDef/ToolSpec; HostView extension
RupuKit/Sources/RupuStore/Primitives/PendingAction.swift
RupuKit/Sources/RupuStore/AppModel.swift      # Modify: route stack
RupuKit/Sources/RupuStore/{RunDetailStore,ActivityStore,SessionDetailStore}.swift  # Modify: mutations + carry-overs
RupuKit/Sources/RupuStore/LauncherStore.swift
RupuKit/Sources/RupuLauncher/LauncherSheet.swift, DefinitionPicker.swift, HostChips.swift, InputsForm.swift
RupuKit/Sources/RupuRunDetail/RunDetailScreen.swift (banner buttons, header verbs, overflow), SessionDetailScreen.swift (send box)
RupuKit/Sources/RupuActivity/ActivityTable.swift (inline gate actions)
RupuKit/Sources/RupuShell/{RootView,ShellToolbar}.swift  # Modify: sheet presentation, ⌘N, route stack back
RupuKit/Package.swift                          # Modify: RupuLauncher target (+tests)
```

---

### Task 1: Bidirectional golden fixtures for the write path

**Files:**
- Modify: `crates/rupu-cp/tests/macos_fixtures.rs`
- Modify: `crates/rupu-cp/src/api/agents.rs`, `src/api/workflows.rs`, `src/api/hosts.rs`, `src/api/tools.rs` (append `#[cfg(test)] mod tests` blocks; in-module pattern is established)
- Create (generated): `apps/rupu-macos/Fixtures/{agent_defs.json, workflow_defs.json, workflow_detail.json, tools.json, hosts.json, launch_responses.json, run_control_response.json, api_errors.json}` and request fixtures `apps/rupu-macos/Fixtures/requests/{approve_body.json, reject_body.json, cancel_body.json, agent_run_body.json, session_start_body.json, workflow_launch_body.json, validate_body.json, send_body.json}`
- Modify: `Makefile` only if the `fixture_is_current` filter doesn't already cover new test names (it does if names end in `fixture_is_current` — keep that convention; request round-trip tests end in `request_fixture_roundtrips`; add that filter to `macos-fixtures`)

**Interfaces:**
- Consumes: the Verified API facts above; existing `check_fixture` helper pattern.
- Produces: the fixture files; every response-fixture test name contains `fixture_is_current`; request tests named `*_request_fixture_roundtrips` and included in `macos-fixtures` regen.

- [ ] **Step 1: Response fixtures.** In-module tests (private DTOs): `agents.rs` → `agent_defs.json` (two `AgentDto`s: one global all-Some incl. tools list, one project-scoped with Nones); `workflows.rs` → `workflow_defs.json` (one with `autoflow_enabled: Some(true)`, one None) and `workflow_detail.json` — build via the same `json!({"workflow": Workflow::parse(SAMPLE_YAML)…})` shape `load_detail` emits, where SAMPLE_YAML declares two inputs (one `required: true` with `values`, one defaulted) — lock the `InputDef` wire shape; `hosts.rs` → `hosts.json` (online local with active_run_count 2; online ssh host with version; offline tunnel host); `tools.rs` → `tools.json` (one read + one write tool). Integration file: `launch_responses.json` = `[{"run_id":"run-01","host_id":"local"},{"session_id":"ses-01","host_id":"local"},{"ok":true,"host_id":"mini"}]` (hand-built `json!`, mirrors handlers); `run_control_response.json` = the `run_response()` shape (reuse the Task-1-Phase-2 RunRecord constructor with status `cancelled` + `"host_id":"local"` injected); `api_errors.json` = `[{"error":"run x not found"},{"error":"ambiguous gate: candidates [a, b]"},{"error":"session s is stopped"}]`.
- [ ] **Step 2: Request round-trip tests.** For each request fixture: the rupu-cp test READS the checked-in JSON and `serde_json::from_str::<RealBodyType>(…)` asserts it deserializes with the expected field values (e.g. `approve_body.json` = `{"mode":"bypass"}` → `ApproveBody{mode:Some("bypass")}`; `workflow_launch_body.json` = `{"inputs":{"branch":"main"},"mode":"ask","host":"mini"}`; `send_body.json` = `{"prompt":"hello"}`). REGEN mode writes the canonical JSON from a constructed body via `serde_json::to_string_pretty` where the type derives Serialize — for Deserialize-only body types, keep the fixture hand-authored in the test as a raw string constant written on REGEN. This locks the app's request shape server-side: if a body struct changes, the round-trip fails.
- [ ] **Step 3: Regen + assert-mode green** — `make macos-fixtures`; then without env: `cargo test -p rupu-cp fixture_is_current` and `cargo test -p rupu-cp request_fixture_roundtrips` PASS. Clippy scoped clean. Commit — `feat(macos): Phase 3 bidirectional fixtures — write bodies, launch responses, definitions, hosts, tools`

### Task 2: CPClient write surface + definition models

**Files:**
- Create: `RupuKit/Sources/RupuAPI/WriteModels.swift`, `DefinitionModels.swift`
- Modify: `RupuKit/Sources/RupuAPI/CPClient.swift` (generic `post`), `Models.swift` (extend APIHostRow)
- Test: `RupuKit/Tests/RupuAPITests/WriteModelsTests.swift`, `DefinitionDecodingTests.swift`, `CPClientWriteTests.swift`

**Interfaces:**
- Consumes: Task 1 fixtures; existing `CPClient.get`, `StubURLProtocol`, `Fixtures` loader.
- Produces (exact):
  - `public struct LaunchResponse: Decodable, Equatable, Sendable { public let runID: String?; public let sessionID: String?; public let ok: Bool?; public let hostID: String }` (decodes all three response variants; explicit CodingKeys).
  - `public struct RunControlResponse: Decodable, Sendable { public let run: APIRunRecord?; public let ok: Bool?; public let hostID: String?; public let archived: Bool? }` — decodes both the local record-bearing shape and the `{ok, …}` shapes from one struct (all optional; helper `public var confirmedStatus: String?` = `run?.status`).
  - Encodable request bodies (explicit snake_case CodingKeys): `ApproveBody{mode: String?}`, `RejectBody{reason: String?}`, `CancelBody{reason: String?}`, `AgentLaunchBody{prompt, mode, target, workingDir, host: String?}`, `WorkflowLaunchBody{inputs: [String:String], mode, target, workingDir, host: String?}`, `ValidateBody{raw: String}`, `SendBody{prompt: String}`.
  - `CPClient` methods: `approveRun(id:host:gate:body:) -> RunControlResponse`, `rejectRun(id:host:gate:body:)`, `cancelRun(id:host:body:)`, `pauseRun(id:host:)`, `resumeRun(id:host:)`, `archiveRun(id:host:)`, `restoreRun(id:host:)` (all → RunControlResponse); `launchAgentRun(name:body:) -> LaunchResponse`, `startAgentSession(name:body:)`, `runWorkflow(name:body:)`, `sendToSession(id:host:body:)` (→ LaunchResponse); `validateWorkflow(body:) -> Bool` (true on 200; 400 throws `.http` whose body message is the single validation error); `agentDefinitions() -> [AgentDefinition]`, `workflowDefinitions() -> [WorkflowDefinition]`, `workflowDetail(name:) -> WorkflowDetail`, `tools() -> [ToolSpec]`.
  - `DefinitionModels`: `AgentDefinition{name, slug, description?, provider?, model?, effort?, maxTokens?, tools: [String], scope, scopeKind, scopeID?, runCount, lastRun?}`, `WorkflowDefinition{name, scope, scopeKind, scopeID?, runCount, lastRun?, autoflowEnabled?}`, `WorkflowDetail{name, inputs: [String: WorkflowInputDef], yaml}` (custom decode: name from `workflow.name`, inputs from `workflow.inputs`), `WorkflowInputDef{type: String, required: Bool, default: String?, values: [String]?}` (default coerced to String via JSONValue-tolerant decode), `ToolSpec{name, description, kind}`.
  - `APIHostRow` gains `version: String?`, `activeRunCount: Int?` (`active_run_count`), `lastSeenAt: String?`.
  - `CPClient.post<B: Encodable, T: Decodable>(_ path:, query:, body: B?) async throws -> T` — Content-Type application/json, same error mapping as `get` PLUS: 501 → `CPError.http(status: 501, body:)` (stores special-case the message).

- [ ] **Step 1: Failing tests** — decode every Task-1 fixture (workflow_detail asserts both inputs incl. required+values; hosts assert activeRunCount 2 / offline status; launch_responses all three variants; run_control_response `confirmedStatus == "cancelled"`); ENCODE tests: each request body encodes to byte-equal JSON of its request fixture (sorted-keys encoder for stability); CPClientWriteTests via stub: `approveRun` hits `/api/runs/r1/approve?gate=g1` with `{"mode":"bypass"}` body; `runWorkflow` body carries host field; `sendToSession` sends `?host=` as QUERY and prompt in body; 501 maps to `.http(501, …)`.
- [ ] **Step 2: RED → implement → GREEN** (`make macos-test`). Commit — `feat(macos): CPClient write surface, definition models, bidirectional shape tests`

### Task 3: PendingAction primitive

**Files:**
- Create: `RupuKit/Sources/RupuStore/Primitives/PendingAction.swift`
- Test: `RupuKit/Tests/RupuStoreTests/PendingActionTests.swift`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `public enum ActionVerb: String, Sendable, Hashable { case approve, reject, cancel, pause, resume, archive, restore, send, launch }`
  - `public struct ActionKey: Hashable, Sendable { public let entityID: String; public let verb: ActionVerb; public init(_ entityID: String, _ verb: ActionVerb) }`
  - `public enum ActionState: Equatable, Sendable { case idle, pending(since: Date), confirmed, failed(String) }`
  - `@MainActor @Observable public final class PendingActions { public init(now: @escaping () -> Date = Date.init); public func state(_ key: ActionKey) -> ActionState; public func begin(_ key: ActionKey); public func fail(_ key: ActionKey, _ message: String); public func confirm(_ key: ActionKey); public func resolve(runID: String, observedStatus: ActivityStatus); public func isStale(_ key: ActionKey, timeout: TimeInterval = 30) -> Bool; public func clear(_ key: ActionKey) }`
  - Confirmation table encoded in `resolve(runID:observedStatus:)`: approve/resume confirm when status ∉ {awaiting, paused respectively} AND ≠ its prior blocking state (approve: leaves `.awaiting`; resume: leaves `.paused`); cancel → `.cancelled`; pause → `.paused`; reject → `.rejected` or `.cancelled`. archive/restore/send/launch confirm via `confirm(_:)` directly (their effects are response-visible or navigation-visible, not status transitions).
  - Injected `now` for deterministic staleness tests.

- [ ] **Step 1: Failing tests** — begin→pending; fail→failed w/ message; confirm; `resolve` table-driven per verb (approve+running→confirmed; approve+awaiting→still pending; cancel+cancelled→confirmed; pause+paused→confirmed; resume+running→confirmed); staleness with injected clock (29s not stale, 31s stale); clear; two keys same entity different verbs independent.
- [ ] **Step 2: RED → implement → GREEN.** Commit — `feat(macos): PendingAction primitive with per-verb confirmation table`

### Task 4: Carry-overs — navigation stack, stopTail teardown token, statusOverrides pruning

**Files:**
- Modify: `RupuKit/Sources/RupuStore/AppModel.swift`, `Route.swift` (stack), `RunDetailStore.swift` (teardown token), `ActivityStore.swift` (pruning)
- Modify: `RupuKit/Sources/RupuShell/RootView.swift` + `RupuRunDetail/*Screen.swift` back-chevrons (all call the same `navigateBack()` — verify no other back paths)
- Test: extend `RoutingTests.swift`, `RunDetailStoreTests.swift`, `ActivityStoreTests.swift`

**Interfaces:**
- Consumes: existing Route cases; existing stores.
- Produces:
  - `AppModel`: `public private(set) var routeStack: [Route]`; `public func navigate(to route: Route)` (pushes current route, sets new); `public func navigateBack()` (pops; empty stack falls back to `.activity(lastKind)`); setting `route` directly remains legal for tab-level switches (sidebar) and CLEARS the stack (a sidebar jump is a new context). `selectedSidebarItem` semantics unchanged.
  - RunDetailStore: `private var tailTeardownGeneration: Int` — bumped in `stopTail()`; `reloadTranscriptSnapshot` captures it before awaiting and discards its result if stale; the tail's `apply` closure also checks generation so a straggling append after stopTail is discarded (closes the parked final-review residual by construction).
  - ActivityStore: `statusOverrides` pruned per-key inside `recompute()` — an override whose runID appears in the freshly merged rows WITH the same status is removed (server caught up); full refresh still clears all.

- [ ] **Step 1: Failing tests** — stack: navigate(to:) ×2 then back×2 returns through both (session→runDetail→back lands on sessionDetail — the quirk matt flagged, now fixed); sidebar route set clears stack; empty-stack back falls to activity. stopTail: scripted tail with an in-flight apply racing stopTail + terminal snapshot → no append after the snapshot (condition-polled). Pruning: patch a row → refresh page containing the same status → override gone (dict exposed as `internal` for the test or asserted via behavior: a later recompute without the override source keeps server status).
- [ ] **Step 2: RED → implement → GREEN; update all back-chevron call sites to `navigateBack()` and row-activation sites to `navigate(to:)`.** Commit — `feat(macos): navigation stack, stopTail teardown token, statusOverrides pruning`

### Task 5: Run mutations — stores + Run detail/Activity UI

**Files:**
- Modify: `RupuKit/Sources/RupuStore/RunDetailStore.swift`, `ActivityStore.swift` (mutation methods + shared `PendingActions`)
- Modify: `RupuKit/Sources/RupuStore/BackendController.swift` (owns the app-wide `public let pendingActions = PendingActions()` so screens share keys)
- Modify: `RupuKit/Sources/RupuRunDetail/RunDetailScreen.swift`, `RupuKit/Sources/RupuActivity/ActivityTable.swift`
- Test: `RunDetailStoreTests.swift`, `ActivityStoreTests.swift` additions

**Interfaces:**
- Consumes: Tasks 2/3; existing live machinery (liveStates/statusPatch flows call `pendingActions.resolve`).
- Produces:
  - `RunDetailStore`: `public func approve(gate stepID: String, mode: String? = nil) async`, `reject(gate stepID: String, reason: String? = nil) async`, `cancel() async`, `pause() async`, `resume() async`, `archive() async`, `restore() async`. Each: `pendingActions.begin(key)` → POST → immediate verbs (reject/cancel/pause/archive/restore): if `RunControlResponse.confirmedStatus`/`archived` proves the effect, `confirm(key)` and refresh `detail`; marker verbs (approve/resume): stay pending; the store's existing `apply(_:)`/refresh paths call `pendingActions.resolve(runID:observedStatus:)` on every status change. POST error → `fail(key, message)` (501 → the launch-runtime message).
  - `ActivityStore`: `public func approve(runID:gate:host:)`, `reject(runID:gate:host:)` (same keys; on POST success trigger a debounced refresh so the row's status catches up; resolve() is already wired via statusPatch).
  - UI: awaiting banner → per-gate Approve (borderedProminent, brand) + Reject buttons with spinner-while-pending + inline failure text + stale note (`isStale`); header shows Cancel when status ∈ {running, awaiting, pending}, Pause when running, Resume when paused; overflow menu Archive (when not archived) / Restore. Activity awaiting rows: compact ✓/✕ buttons, same keys. All controls disabled while their key is pending.

- [ ] **Step 1: Failing store tests** (fake client closures + scripted streams): approve → pending → `stepAwaitingApproval` cleared via runResumed/running event → confirmed (condition-polled); cancel → response record status cancelled → confirmed immediately + detail refreshed; pause 409 → failed with message; resume 501 → failed with launch-runtime message; activity approve → row patched after refresh; keys shared: ActivityStore approve then RunDetailStore observes same key pending.
- [ ] **Step 2: RED → implement stores → GREEN.**
- [ ] **Step 3: UI wiring (no view unit tests); `make macos-build`; verify no-dead-verbs rendering logic via store-driven predicates (`public var availableVerbs: Set<ActionVerb>` on RunDetailStore, unit-tested per status).**
- [ ] **Step 4: Commit** — `feat(macos): run mutations — approve/reject/cancel/pause/resume/archive with pending-state UX`

### Task 6: Session send + archive/restore

**Files:**
- Modify: `RupuKit/Sources/RupuStore/SessionDetailStore.swift`, `RupuKit/Sources/RupuRunDetail/SessionDetailScreen.swift`
- Test: `SessionDetailStoreTests.swift` additions

**Interfaces:**
- Consumes: Tasks 2/3/5 (shared PendingActions via BackendController).
- Produces: `SessionDetailStore.send(prompt: String) async` — trims; empty → no-op (button disabled anyway); key `(sessionID, .send)`; POST → on `{run_id}` success: `confirm`, refresh `runs`, `focusRun` the new run (its transcript IS the reply surface); failure → `fail` (409 stopped-session message surfaced verbatim). `archive()/restore()` mirroring Task 5. UI: send box (TextField + Send) pinned under the transcript; disabled while `(sessionID, .send)` pending or session stopped/archived; MicroLabel "READ-ONLY…" removed; stopped sessions show "SESSION STOPPED" instead of the box.

- [ ] **Step 1: Failing tests** — send happy path (pending→confirmed, runs refreshed, focus moved to new run); empty prompt no-op; 409 stopped → failed with message; archive confirm.
- [ ] **Step 2: RED → implement → GREEN; UI; build. Commit** — `feat(macos): session send with pending-state, session archive/restore`

### Task 7: LauncherStore

**Files:**
- Create: `RupuKit/Sources/RupuStore/LauncherStore.swift`
- Test: `RupuKit/Tests/RupuStoreTests/LauncherStoreTests.swift`

**Interfaces:**
- Consumes: Task 2 definition models + write methods; `APIHostRow`; PendingActions.
- Produces:
  - `public enum LaunchKind: String, CaseIterable, Sendable { case agentRun, session, workflow }`
  - `@MainActor @Observable public final class LauncherStore { public var kind: LaunchKind; public private(set) var agents: BlockState<[AgentDefinition]>; public private(set) var workflows: BlockState<[WorkflowDefinition]>; public var selectedDefinition: String?; public private(set) var workflowInputs: [String: WorkflowInputDef]; public var inputValues: [String: String]; public var prompt: String; public var mode: String ("ask" default); public private(set) var hosts: [APIHostRow]; public var selectedHosts: Set<String> (default ["local"]); public var fanOutAllHealthy: Bool; public private(set) var validationError: String?; public private(set) var launchResults: [LaunchOutcome]; public init(client: CPClient); public func activate() async; public func selectDefinition(_ name: String) async; public var canLaunch: Bool; public func launch() async -> Route? }`
  - `public struct LaunchOutcome: Equatable, Sendable { public let host: String; public let result: Result<Route, String> }` (Route = .runDetail/.sessionDetail destination)
  - Semantics: `activate()` loads definitions (both lists in parallel) and hosts PROGRESSIVELY (hosts fetch never blocks definitions; sheet renders as soon as definitions arrive; hosts fill in). `selectDefinition` for workflows fetches `workflowDetail` and seeds `workflowInputs` + defaulted `inputValues`. `canLaunch`: definition chosen AND (workflow: all `required` inputs non-empty; agent/session: prompt non-empty) AND ≥1 target host (or fanOut). Workflow launch runs `validateWorkflow` on the CURRENT definition's yaml first only when the definition was edited locally — NOT applicable this phase (no editing): skip validate on launch; `validationError` is reserved for required-input gaps (client-side message). Launch: targets = fanOutAllHealthy ? all `status=="online"` host ids : selectedHosts; one POST per target with `host` in the body (`"local"` sent as nil); collect `LaunchOutcome` per host; single-target success returns the destination Route for auto-navigation; multi-target shows the outcome list (caller navigates per row). Key `("launcher", .launch)` pending during the batch.

- [ ] **Step 1: Failing tests** — activate parallel loads + progressive hosts (slow hosts fetch doesn't delay definitions — condition-polled); selectDefinition seeds inputs w/ defaults; canLaunch matrix (missing required input blocks; prompt empty blocks agent; no hosts blocks); single-host launch returns Route; fan-out over 2 online + 1 offline → 2 POSTs, offline skipped, outcomes recorded incl. one failure; 501 outcome message.
- [ ] **Step 2: RED → implement → GREEN. Commit** — `feat(macos): LauncherStore — definitions, declared inputs, progressive hosts, client-side fan-out`

### Task 8: Launcher UI + shell wiring

**Files:**
- Modify: `RupuKit/Package.swift` (target `RupuLauncher` + no test target — store already tested; deps RupuAPI/RupuStore/RupuDesign)
- Create: `RupuKit/Sources/RupuLauncher/LauncherSheet.swift`, `DefinitionPicker.swift`, `InputsForm.swift`, `HostChips.swift`
- Modify: `RupuKit/Sources/RupuShell/ShellToolbar.swift` ("+ New run" button), `RootView.swift` (sheet presentation + ⌘N via `.keyboardShortcut("n", modifiers: .command)` on a hidden/toolbar button), `App/RupuApp.swift` only if a Commands menu entry is needed (keep thin)
- Test: none new (visual)

**Interfaces:**
- Consumes: `LauncherStore`, RupuDesign chrome, `AppModel.navigate(to:)`.
- Produces: `public struct LauncherSheet: View { public init(model: AppModel, backend: BackendController) }` — layout per HANDOFF screen 9: kind segmented → DefinitionPicker (name + model/scope MicroLabels, agents for agentRun/session, workflows for workflow) → prompt TextEditor OR InputsForm (kv rows from workflowInputs: label + required marker + values popup when `values` present, TextField otherwise) → mode segmented (bypass segment tinted `Color.status(.fail)` — loud rule) → HostChips (chip per host: name + `active_run_count` in Font.numeral when known; disabled+dim when offline/stale; selected = brand fill 15%; "FAN OUT: ALL HEALTHY" toggle chip) → footer: Launch button (pending spinner), per-host outcome rows after a multi-launch (success rows are buttons navigating via `navigate(to:)`), inline error text. Sheet dismisses on single-target success after navigation. `.interactiveDismissDisabled(false)` (Esc allowed — it's a form, unlike onboarding).

- [ ] **Step 1: Build the four views; wire toolbar + ⌘N; `make macos-test && make macos-build`.**
- [ ] **Step 2: Live check (screen permitting): open Launcher, launch a workflow locally, auto-navigate to its run detail; screenshot. Commit** — `feat(macos): Launcher sheet — definition picker, inputs form, host chips, fan-out`

### Task 9: Docs, gates, live validation

**Files:**
- Modify: `CLAUDE.md` (module map + RupuLauncher; pending-state contract one-liner; firehose-scope note from spec §5), umbrella spec §8 (Phase 3 dispositions: `agents` run/session, `workflows` run+validate, run mutations, `sessions` send/archive/restore, `tools`, hosts-for-launcher → shipped; `host_fanout` → corrected to "internal helper — client-side fan-out"; note definition lists shipped early ahead of Phase 5 Library)
- Test: full gates

- [ ] **Step 1: Docs edits.**
- [ ] **Step 2: Gates** — `make macos-test && make macos-build && cargo test -p rupu-cp` + scoped clippy; no new failures beyond the two documented environmental ones.
- [ ] **Step 3: Live validation smoke (headless-capable parts)** — against local `cp serve` (7420, never 7878): via the app or curl-assisted: launch a gated workflow from the Launcher → approve its gate from Run detail → watch status leave awaiting → completes; cancel a second run → row flips cancelled; pause/resume; session send → reply run appears; archive → row leaves Activity. Record evidence per scenario; screenshot where the screen is unlocked; the rest lands on matt's pre-merge pass.
- [ ] **Step 4: Commit** — `feat(macos): Phase 3 docs, parity ledger, validation evidence`

---

## Self-review notes

- Spec coverage: §1 contract → Tasks 3/5 (confirmation table + stale note); §2 plumbing/fixtures both directions → Tasks 1/2; §3 in-place mutations → Tasks 5/6; §4 Launcher incl. client-side fan-out correction → Tasks 7/8; §5 carry-overs → Task 4 (+ firehose doc in Task 9); §6 testing/live validation → per-task TDD + Task 9; §7 out-of-scope respected (no delete-run, no menu bar).
- Sequencing: PendingAction (3) and carry-overs (4) land before mutation UI (5/6) and Launcher (7/8) so navigation + shared state exist first.
- Type consistency: `PendingActions` API used identically in Tasks 5/6/7; `LaunchResponse/RunControlResponse` named consistently; `navigate(to:)`/`navigateBack()` used by Tasks 5/6/8.
- Known judgment encoded: validate-on-launch skipped (no local editing this phase — validate stays a CPClient method for future use, exercised by its test only); `host: "local"` sent as nil (server treats absent as local).
