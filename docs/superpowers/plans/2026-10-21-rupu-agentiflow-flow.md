# Agentiflow flow (unfolding map) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the agentiflow page's Flow tab with the "unfolding map" (rounds down a spine, launched work branching right as run-graph cards, generated workflows drawn as their real run graph, message arcs, a board rail, replay), and give workflow run pages the same message, board and timeline layer.

**Architecture:** Part 1 adds the facts the CP lacks (a child's launching round, a generated-workflow flag and its definition, the lead's per-round decision, a claim journal and a board endpoint with host transports). Part 2 builds one pure timeline engine, a pure flow model and layout, and xyflow components that reuse the run graph's node components; the agentiflow map and the run graph both render "the run as of time t" from the same engine.

**Tech Stack:** Rust (rupu-launch, rupu-agentiflow, rupu-fleet, rupu-runtime, rupu-cp, rupu-cli), React + TypeScript + @xyflow/react + vitest in `crates/rupu-cp/web`.

**Spec:** `docs/superpowers/specs/2026-10-21-rupu-agentiflow-flow-design.md` (approved mock: `docs/superpowers/specs/assets/agentiflow-flow/a-unfolding-map/index.html`, fixture `…/fixture.js`, screenshots beside it).

## Global Constraints

- Every card, edge, badge and status glyph comes from the run graph's components (`components/graph/*`, `RunGraph.tsx`); no look-alike components.
- Colors are theme tokens only (`useThemeColors`, `--c-*`, crew tints from `lib/codename`); light and dark both.
- Motion uses `styles.css` "Chart motion" and `styles/graph-motion.css` vocabulary: `--ease-out` for arrivals, `--ease-in-out` for movement, transform/opacity/clip-path/stroke-dashoffset only, UI transitions under 300ms, 30–80ms stagger; `prefers-reduced-motion` = fades, no movement.
- Agentiflow routes stay local-only (`require_local`, 501 on `?host=`); only `GET /api/runs/:id/board` goes through the host transports.
- New serialized fields are `#[serde(default, skip_serializing_if = …)]` so older records read and older readers ignore them.
- A run that predates a field shows less, never a guess: inferred round placement is marked `inferred`, missing decisions show the round number, counts and transcript link only.
- Integration tests go in each crate's single `tests/it/` binary (one module per file, listed in `main.rs`); env-mutating tests `#[serial]`.
- Main is fmt-dirty: run `rustfmt` only on files you changed, never on `lib.rs`/`main.rs`/`mod.rs`, never `cargo fmt`.
- No assessment-derived data in fixtures or docs; the web fixture is the invented `acme-shop-auth` data.
- Never `git stash`. Commit unsigned (`git -c commit.gpgsign=false commit`), messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A run from before these changes** (no `round`, no `generated`, no `decision`, no claim journal): the map still draws every child, placed by start time and marked inferred, round nodes show number/counts/transcript only, the board shows live claims only. Tests: Task 5 `children_without_round_infer_nothing`, Task 9 `legacy_children_are_placed_by_time_and_marked_inferred`.
2. **A message whose address maps to no participant card** (a participant that left, a `role:` address, a legacy space): it is still drawn (to the round node, or as an off-canvas dead end) and listed, never dropped. Test: Task 9 `unmapped_addresses_still_produce_an_edge`.
3. **Real density** (a 200-round run, 23 children in one round, 500 messages): layout stays bounded, finished rounds collapse, a round over the card limit shows "+N more", arcs past the rest limit draw only on hover/selection. Tests: Task 9 `density_rules_apply_at_scale`, Task 10 `layout_is_bounded_and_grows_down_and_right`.
4. **A live run polling every 5 s**: new children and messages append without resetting the operator's pan, zoom, expanded frames or an in-progress replay. Test: Task 12 `a_poll_keeps_pan_expansion_and_replay`.
5. **A remote workflow run whose host lacks `run.board`** (or a tunnel/bucket host): the board rail says the host can't serve it, the message layer still draws. Tests: Task 5 `http_without_run_board_is_unsupported`, Task 13 `board_unavailable_is_said_in_place`.

---

## Part 1: backend facts

### Task 1: Record the launching round on agentiflow children

**Files:**
- Modify: `crates/rupu-launch/src/ledger.rs` (`Started`, ~:88-132), `crates/rupu-launch/src/launcher.rs` (`LaunchRecord` :142, `started_of` :151, `launch` :1952)
- Modify: `crates/rupu-agentiflow/src/run.rs` (`AcrossRounds` :335-~360, its construction ~:913)
- Test: `crates/rupu-launch/tests/it/w7.rs`, `crates/rupu-agentiflow/tests/it/` (new module `rounds.rs`, listed in `main.rs`)

**Interfaces:**
- Produces: `Started.round: Option<u32>` (serde default/skip none); `LaunchRecord.round: Option<u32>`; `RunLauncher::launch_in_round(&self, req: ChildRequest, ctx: &ToolContext, round: Option<u32>) -> Result<ChildHandle, LaunchError>`; the `Launcher::launch` impl calls `launch_in_round(req, ctx, None)`. `AcrossRounds { launcher: Arc<RunLauncher>, round: Arc<AtomicU32> }` whose `launch` passes `Some(round.load(Relaxed))`.

- [ ] **Step 1: Write the failing tests.** In `w7.rs`: `started_line_carries_its_round` — launch a scripted child through `launch_in_round(.., Some(3))`, read the ledger with `ledger::read(dir)`, assert `entries[0].started.round == Some(3)`; `launch_without_round_writes_none` asserts `round == None` and that the raw `children.jsonl` line has no `"round"` key. In `rounds.rs`: `lead_launches_are_stamped_with_the_current_round` — run a mock-provider agentiflow whose lead dispatches one unit in round 0 and one in round 1, assert the two `started` lines carry `round` 0 and 1.
- [ ] **Step 2: Run them and see them fail.** `cargo test -p rupu-launch --test it w7::started_line_carries_its_round` and `cargo test -p rupu-agentiflow --test it rounds::` → FAIL (no field / no method).
- [ ] **Step 3: Implement.** Add the field and `LaunchRecord.round`, copy it in `started_of`, add `launch_in_round` holding today's `launch` body with the round threaded into the `LaunchRecord`. Give `AcrossRounds` the round `Arc<AtomicU32>` that `run.rs` creates at ~:852 (pass the clone already handed to `RecordingLead`).
- [ ] **Step 4: Run the tests and see them pass**, plus the crates' existing suites: `cargo test -p rupu-launch --test it` and `cargo test -p rupu-agentiflow --test it`.
- [ ] **Step 5: Commit.** `rupu-launch, rupu-agentiflow: record the round that launched each agentiflow child`

### Task 2: Flag generated workflows

**Files:**
- Modify: `crates/rupu-launch/src/ledger.rs` (`Request` :169-184), `crates/rupu-launch/src/launcher.rs` (`launch` building `Request` :2027-2045; the generated-dir check :800-814)
- Test: `crates/rupu-launch/tests/it/w7.rs`

**Interfaces:**
- Produces: `Request.generated: bool` (`#[serde(default, skip_serializing_if = "std::ops::Not::not")]`), true exactly when the dispatched `WorkflowRef::File` lies inside the caller's `<children_dir>/generated/` (the same test :800-814 applies; extract it as `fn is_generated_file(children_dir: &Path, file: &Path) -> bool`).

- [ ] **Step 1: Write the failing tests.** `generate_then_dispatch_marks_the_child_generated` (generate with the scripted generator, dispatch the returned file, assert `started.request.unwrap().generated` and `workflow` is that file); `named_workflow_child_is_not_generated` (dispatch a pool workflow by name, assert `!generated` and the raw line has no `"generated"` key).
- [ ] **Step 2: Run them and see them fail** (`cargo test -p rupu-launch --test it w7::generate_then_dispatch_marks_the_child_generated`).
- [ ] **Step 3: Implement** the field, `is_generated_file`, and set it where `Request` is built.
- [ ] **Step 4: Run the tests and the crate suite; they pass.**
- [ ] **Step 5: Commit.** `rupu-launch: mark a workflow child dispatched from workflows.generate as generated`

### Task 3: Put the lead's decision and timing on the round event

**Files:**
- Modify: `crates/rupu-agentiflow/src/envelope.rs` (the trait that declares `run_round`), `crates/rupu-agentiflow/src/lead.rs` (`RunAgentLeadDriver`, ~:736-808), `crates/rupu-agentiflow/src/run.rs` (`RecordingLead::run_round` :533-557)
- Test: `crates/rupu-agentiflow/tests/it/rounds.rs`

**Interfaces:**
- Produces: on that trait, `fn last_decision(&self) -> Option<String> { None }` (default); `RunAgentLeadDriver` returns the text of the last assistant message of its latest round's `final_messages`, trimmed, capped by `pub fn decision_excerpt(text: &str) -> String` (in `run.rs`: at most 600 chars, cut on a char boundary at the last whitespace before the cap, `…` appended when cut). The `round` event gains `started_at` (RFC 3339, `(self.now)()` taken before the inner `run_round`), `decision` (the excerpt, omitted when none), `goals` (`[{id, met, current}]` from `ctx.digest.goals`, the assessment the round started from) and `warnings` (`ctx.digest.warnings`, omitted when empty).
- Ruling recorded here: "goals after round N" is read by the web from round N+1's `goals`, and for the last round from `record.goals`; no extra evaluation runs.

- [ ] **Step 1: Write the failing tests.** `decision_excerpt_caps_at_600_on_a_word_boundary` (a 900-char input → ≤ 601 chars, ends with `…`, no split word; a short input → unchanged); `round_event_records_decision_start_goals` — a mock-provider lead that answers "Map the surface first." in round 0: the `round` event has `decision == "Map the surface first."`, a `started_at` ≤ its `ts`, and `goals` equal to the run's goal ids with `met`/`current`.
- [ ] **Step 2: Run and see them fail** (`cargo test -p rupu-agentiflow --test it rounds::`).
- [ ] **Step 3: Implement** the trait method, the driver override, `decision_excerpt`, and the four fields in `run_round`.
- [ ] **Step 4: Run the tests and the crate suite; they pass.**
- [ ] **Step 5: Commit.** `rupu-agentiflow: record what the lead decided each round, when the round started and its goals`

### Task 4: A claim journal and a board view

**Files:**
- Modify: `crates/rupu-fleet/src/board.rs`, `crates/rupu-fleet/src/types.rs` (`ClaimGuard` drop :17, `ClaimRecord` :54, `DirectiveEvent`), `crates/rupu-fleet/src/lib.rs` (exports)
- Create: `crates/rupu-runtime/src/services/board.rs` (+ `pub mod board;` in `services/mod.rs`)
- Test: `crates/rupu-fleet/src/tests.rs` (the crate's existing test module), `crates/rupu-runtime/tests/it/` (new module `board.rs`)

**Interfaces:**
- Produces in rupu-fleet: `<root>/board/claims.jsonl`, append-only, one JSON line per event: `{"ev":"claimed","key","owner","at","lease_expires_at"}`, `{"ev":"released","key","owner","at"}`, `{"ev":"expired","key","owner","at"}` — written on grant in `claim`, in `ClaimGuard::drop` (only when its compare-and-delete removed the file; the guard carries the board root and raw key), and in `reap_expired`. Appends take `claims.jsonl.lock` (same locking helper `post` uses). `retract_directive` writes `{"retract": id, "at": <now>}` (`at` optional on read). New pub types: `ClaimState { Held, Released, Expired }`, `ClaimView { key, owner, acquired_at, lease_expires_at: Option<DateTime>, ended_at: Option<DateTime>, state: ClaimState }`, `Board::claims_view(&self, now: DateTime<Utc>) -> Result<Vec<ClaimView>, FleetError>` — the journal folded per key and acquisition, plus any live claim file the journal lacks (a board written before the journal) as `Held`, a `Held` whose lease passed `now` reported `Expired`; ordered by `acquired_at`.
- Produces in rupu-runtime: `BoardView { claims: Vec<ClaimView>, directives: Vec<DirectiveView>, journal: bool }` with `DirectiveView { id, author, ts, body, addressed_to: Option<String>, retracted_at: Option<DateTime<Utc>>, retracted: bool }` (`Serialize, Deserialize`, the CP / `__board` / MCP wire shape), and `pub fn board_view(space: &Path, now: DateTime<Utc>) -> Result<BoardView, FleetError>`; `journal` is false when `claims.jsonl` doesn't exist (an older space).

- [ ] **Step 1: Write the failing tests.** rupu-fleet: `claim_release_writes_journal_lines` (claim, drop guard → two lines, `claims_view` shows one `Released` with `ended_at`); `expired_claim_is_reported_expired` (ttl 0, reap via a second claim → `expired` then `claimed` by the new owner); `pre_journal_live_claim_reads_as_held` (write a claim file by hand, no journal → one `Held`); `retraction_records_its_time`. rupu-runtime: `board_view_folds_claims_and_directives` and `board_view_of_an_empty_space_is_empty_not_an_error`.
- [ ] **Step 2: Run and see them fail** (`cargo test -p rupu-fleet` and `cargo test -p rupu-runtime --test it board::`).
- [ ] **Step 3: Implement** the journal writes, `claims_view`, the retract `at`, and `board_view`.
- [ ] **Step 4: Run the tests and both crate suites; they pass** (rupu-fleet's `log_is_superset` and the claim concurrency tests must stay green).
- [ ] **Step 5: Commit.** `rupu-fleet: journal claims; rupu-runtime: board_view`

### Task 5: CP endpoints, DTO fields, host transport and MCP tools

**Files:**
- Modify: `crates/rupu-cp/src/api/children.rs` (`ChildDto` :32, `from_entry` :82, the breadth-first walk :127-190), `crates/rupu-cp/src/api/agentiflows.rs` (`routes` :58-78, `UnitDto` :224)
- Create: `crates/rupu-cp/src/api/board.rs` (+ route registration in the API router)
- Modify: `crates/rupu-cp/src/host/connector.rs` (trait), `host/local.rs`, `host/http.rs`, `host/ssh.rs`, `host/tunnel.rs`, `host/bucket/connector.rs`, `crates/rupu-cp/src/node/protocol.rs` (`CAP_RUN_BOARD`, `host_features()` :438)
- Modify: `crates/rupu-cli/src/lib.rs` (hidden `__board`, next to `MessageHelper` :324 and its output-format guard :706), create `crates/rupu-cli/src/cmd/board_helper.rs`
- Modify: `crates/rupu-cp/src/mcp/ops/agentiflows.rs`, `crates/rupu-cp/src/mcp/ops/runs.rs`, `crates/rupu-cp/tests/fixtures/mcp/tool_names.json` (`BLESS=1`), `crates/rupu-cp/tests/it/mcp_routes.rs` (count 185 → 188)
- Test: `crates/rupu-cp/tests/it/children.rs` (or the module that tests `/children`), new `crates/rupu-cp/tests/it/board.rs`, `crates/rupu-cli/tests/it/` (the module that tests `__message`)

**Interfaces:**
- Consumes: `Started.round`, `Request.generated` (Tasks 1–2), `Finished.output`, `board_view` / `BoardView` (Task 4), `MessageRoot::resolve(global, id).space`.
- Produces:
  - `ChildDto` gains `round: Option<u32>` (a direct child's own; a deeper child's is its root ancestor's), `generated: bool`, `output_excerpt: Option<String>` (`finished.output` trimmed to 240 chars on a char boundary, `…` when cut). `UnitDto` gains `round` and `generated`.
  - `GET /api/agentiflows/:id/board` → `BoardView` (local only).
  - `GET /api/runs/:id/board[?host=]` → `BoardView`; remote through `HostConnector::run_board(&self, id: &str) -> Result<BoardView, HostConnectorError>` (required, no default).
  - `GET /api/agentiflows/:id/generated/:handle` → `{ handle, name, file_name, yaml }` for a direct child whose request is `generated`; 404 otherwise; 403 when the canonical file isn't under the canonical `<flow dir>/generated/`.
  - `CAP_RUN_BOARD = "run.board"`, in `host_features()`. HTTP proxies only to a peer advertising it (`require_known_feature`); SSH runs one remote command that checks `rupu __features` for `run.board` then runs `rupu __board <id>` (marker `rupu-board: unsupported` → `Unsupported`), mirroring the `__message` gate; tunnel and bucket return `Unsupported` → 501.
  - CLI `rupu __board <id>` (hidden): exact id, prints one `BoardView` JSON object; failure prints `{"error":{code,message}}` and exits 3 (codes `not_found` / `internal`), as `__message` does.
  - MCP operator tools `agentiflows.board`, `agentiflows.generated`, `runs.board`.

- [ ] **Step 1: Write the failing tests.** `children_carry_round_generated_and_excerpt` (a ledger with a round-2 generated workflow child and its depth-2 sub-agent → both `round: 2`, the workflow `generated: true`, `output_excerpt` capped); `children_without_round_infer_nothing` (old ledger → `round` absent, `generated` false); `agentiflow_board_serves_claims_and_directives`; `run_board_local`; `run_board_remote_is_501_on_tunnel`; `http_without_run_board_is_unsupported` (httpmock peer whose `/api/host/info` lacks `run.board` → 501, no request to `/board`); `generated_definition_is_served_and_path_checked` (a symlink or `../` escaping `generated/` → 403; non-generated child → 404); CLI `board_helper_prints_the_view_and_errors_with_exit_3`.
- [ ] **Step 2: Run and see them fail** (`cargo test -p rupu-cp --test it board::` and `… children::`).
- [ ] **Step 3: Implement** the DTO fields, the three routes (agentiflow routes with `require_local`), the connector method on every transport, the CLI helper, and the MCP ops; bless `tool_names.json` and bump the count.
- [ ] **Step 4: Run** the new tests, `cargo test -p rupu-cp --test it mcp_routes::`, and the CLI module; they pass.
- [ ] **Step 5: Commit.** `rupu-cp: child round/generated/excerpt, board and generated-definition endpoints, run.board over host transports`

### Task 6: Backend docs

**Files:**
- Modify: `docs/cp-api.md` (the three routes and `BoardView`), `docs/messaging.md` (the claim journal and what older spaces show), `CLAUDE.md` (one line each in the `rupu-fleet`, `rupu-launch`, `rupu-agentiflow` and `rupu-cp` entries; add the spec and this plan to "Read first")

- [ ] **Step 1: Write the doc changes** stating exactly the fields and behaviour Tasks 1–5 shipped, including "a run before this shows inferred placement / live claims only".
- [ ] **Step 2: Commit.** `docs: agentiflow flow data (round, generated, decision, board)`

---

## Part 2: web

All web paths below are under `crates/rupu-cp/web/`. Run tests with `npx vitest run <path>` and types with `npx tsc --noEmit -p .`.

### Task 7: API client types and calls

**Files:**
- Modify: `src/lib/api.ts` (`RunChild` :1877, `AgentiflowEvent` :~954, the `api` object near `getAgentiflowChildren` :3200)
- Test: `src/lib/api.flow.test.ts`

**Interfaces:**
- Produces: `RunChild` gains `round?: number; generated?: boolean; output_excerpt?: string`. `AgentiflowRoundEvent` (typed view of a `kind: 'round'` event: `round`, `ts`, `started_at?`, `decision?`, `goals?: {id; met; current}[]`, `warnings?`, `goals_met`, `goals_total`, `spent_usd`, `spent_tokens`, `outcome`, `error?`) and `roundEvents(events: AgentiflowEvent[]): AgentiflowRoundEvent[]`. `BoardView`, `ClaimView`, `DirectiveView` mirroring Task 4. `api.getAgentiflowBoard(id, opts?)`, `api.getRunBoard(id, { host?, signal? })` (a 501 resolves to `{ unavailable: reason }` the way `getMessages` does), `api.getAgentiflowGenerated(id, handle, opts?)`.

- [ ] **Step 1: Write the failing tests.** `roundEvents_types_old_and_new_events` (an old event without `decision`/`started_at` and a new one → both parsed, absent fields `undefined`); `getRunBoard_turns_501_into_unavailable` and `getRunBoard_passes_host` (mock fetch).
- [ ] **Step 2: Run and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run; pass.**
- [ ] **Step 5: Commit.** `web: api types and calls for the agentiflow flow`

### Task 8: The shared timeline engine

**Files:**
- Create: `src/lib/timeline/timeline.ts`, `src/lib/timeline/useTimeline.ts`, `src/components/timeline/TimeAxis.tsx`
- Modify: `src/styles/graph-motion.css` (the axis, scrubber and reveal classes, with reduced-motion rules)
- Test: `src/lib/timeline/timeline.test.ts`, `src/components/timeline/TimeAxis.test.tsx`

**Interfaces:**
- Produces (pure): `type TimelineKind = 'round' | 'child' | 'step' | 'unit' | 'message' | 'board'`; `interface TimelineItem { key: string; at: number /* epoch ms */; kind: TimelineKind }`; `interface TimelineBand { key: string; label: string; from: number; to: number | null }`; `timeBounds(items, bands, nowMs: number | null): { start: number; end: number }`; `revealed(items, t: number | null): Set<string>` (`null` = everything); `replayDuration(start, end): number` = clamp((end-start)/1000 × 40ms, 6000, 16000) ms.
- Produces (hook): `useTimeline({ items, bands, live, nowMs }): { t: number | null; playing: boolean; replay(): void; scrub(t: number): void; release(): void /* back to live/whole */ }`. `t` advances on rAF during replay from `start` to `end` over `replayDuration`, then returns to `null`; under reduced motion `replay()` sets `t = null` at once; a new item arriving keeps `t` (a poll never restarts or ends a replay).
- Produces (component): `TimeAxis({ bands, items, bounds, t, live, onScrub, onReplay, onRelease })` — round/step bands, message ticks coloured by kind, a draggable scrubber, a "live" pill (current time) when `live && t === null`, a Replay button.

- [ ] **Step 1: Write the failing tests.** `revealed_is_everything_at_null_and_by_time_otherwise`; `bounds_cover_items_bands_and_now`; `replay_duration_is_clamped` (1 min → 6000, 5 h → 16000); `replay_runs_to_the_end_and_releases` and `a_new_item_keeps_t` (fake timers + rAF); `reduced_motion_replay_jumps_to_whole`; TimeAxis `scrub_drag_reports_times_and_live_pill_shows_when_released`.
- [ ] **Step 2: Run and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run; pass.**
- [ ] **Step 5: Commit.** `web: shared timeline engine (reveal by time, replay, scrubber, live)`

### Task 9: The flow model (pure)

**Files:**
- Create: `src/lib/flow/flowModel.ts`, `src/lib/flow/fixtures.ts` (the invented `acme-shop-auth` data from `docs/superpowers/specs/assets/agentiflow-flow/fixture.js`, typed as the API shapes)
- Test: `src/lib/flow/flowModel.test.ts`

**Interfaces:**
- Consumes: Task 7 types, `RunGraphModel` (`lib/runGraphModel.ts`), `SpaceMessage` / `SpaceParticipant` (`lib/api.ts`), `TimelineItem` / `TimelineBand` (Task 8).
- Produces: `buildFlowModel(input: FlowInput, opts?: { expanded?: ReadonlySet<string> }): FlowModel` with
  - `FlowInput { detail: AgentiflowDetail; children: RunChild[]; spaces: FlowSpace[]; boards: Map<string, BoardView | { unavailable: string }>; childGraphs: Map<string, RunGraphModel> }`, `FlowSpace { id: string /* 'flow' or a child run id */; participants: SpaceParticipant[]; messages: SpaceMessage[] }`;
  - `FlowModel { rounds: FlowRound[]; nodes: FlowNodeRef[]; messages: FlowMessage[]; board: FlowBoard; timeline: { items: TimelineItem[]; bands: TimelineBand[] } }`;
  - `FlowRound { round: number; startedAt: number | null; endedAt: number | null; decision: string | null; goalsAfter: {id; met; current}[] | null; spentUsd: number | null; transcriptPath: string | null; collapsed: boolean; childIds: string[]; hidden: number /* "+N more" */ }`;
  - `FlowNodeRef { id: string /* child run id, 'round:N', 'lead', 'operator' */; kind: 'lead' | 'round' | 'child' | 'more' | 'frame' ; child?: RunChild; parentId: string; depth: number; round: number; inferred: boolean }`;
  - `FlowMessage { id; kind: RowKind; from: string /* node id */; to: string[] /* node ids; 'offcanvas' for an unknown address */; undelivered: {to; reason}[]; atRest: boolean; space: string; at: number; body: string }`.
- Rules (from the spec's Density section; constants exported): `KEEP_EXPANDED_ROUNDS = 2` (the latest rounds and the running one stay expanded; a finished round not in `expanded` collapses), `MAX_CARDS_PER_ROUND = 8` (failed and running children always shown; the rest past the limit fold into one `more` node unless the round is in `expanded`), `ARC_REST_LIMIT = 40` (above it, `atRest` is false for every message except operator ones). Round attribution: `child.round`, else a depth ≥ 2 child takes its root ancestor's, else the round whose `[startedAt or previous round end, ts]` contains `started_at`, with `inferred: true`; a child whose parent is missing hangs off its round, `inferred: true`. Message endpoints: `from`/`to` addresses resolved through the space's participants to a child run id (`run_id`), `lead` to `'lead'`, `operator` to `'operator'`, a board post's destination to `'board'` (the rail's anchor in the flow, or the frame's board strip for a workflow child's space), a workflow child's internal addresses to that child's frame-inner node ids (`<child run id>/<step_id>`), anything else to `'offcanvas'`.

- [ ] **Step 1: Write the failing tests** with the fixture: `fixture_rounds_children_and_frames` (4 rounds; `gannet#1` is a frame; the sub-agents hang off `lynx#1` / `kite#3`); `round_decision_and_goals_after_come_from_the_next_event`; `legacy_children_are_placed_by_time_and_marked_inferred` (strip `round` → same rounds, `inferred: true`); `unmapped_addresses_still_produce_an_edge` (a `role:` address with no resolvable participant → `to: ['offcanvas']`, still in `messages`); `density_rules_apply_at_scale` (a generated 200-round, 23-children-in-one-round, 500-message input → only `KEEP_EXPANDED_ROUNDS` + running expanded, one `more` node with `hidden: 15` minus failed/running shown, `atRest` false above the limit); `timeline_items_cover_rounds_children_messages_board`.
- [ ] **Step 2: Run and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run; pass.**
- [ ] **Step 5: Commit.** `web: agentiflow flow model (rounds, children tree, frames, messages, density)`

### Task 10: The flow layout (pure)

**Files:**
- Create: `src/lib/flow/flowLayout.ts`
- Test: `src/lib/flow/flowLayout.test.ts`

**Interfaces:**
- Consumes: `FlowModel` (Task 9), `layoutGraph(m: RunGraphModel): Map<string, Pos>` (`lib/graphLayout.ts`) for a frame's inner graph.
- Produces: `layoutFlow(model: FlowModel, sizes?: Map<string, Size>): FlowLayout`, `FlowLayout { positions: Map<string, Pos>; frames: Map<string, Pos /* the frame box */>; railX: number; width: number; height: number }`. Geometry: the spine at x 0; round cards in column 0; a child at depth d in column d (column width = card width + gap); a frame spans from its column rightwards with its inner graph laid out by `layoutGraph` and offset into it; rounds stack downward in order, each round's block as tall as its tallest branch; the rail sits right of the widest block. Card sizes come from `sizes` (measured, as `RunGraph` does with `mergeMeasured`) with defaults.

- [ ] **Step 1: Write the failing tests.** `rounds_stack_down_in_order`; `depth_moves_right`; `frame_contains_its_inner_graph`; `no_two_cards_overlap` (fixture and the 200-round input); `layout_is_bounded_and_grows_down_and_right` (collapsed rounds keep `height` linear in rounds, `width` bounded by the deepest column + widest frame + rail).
- [ ] **Step 2: Run and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run; pass.**
- [ ] **Step 5: Commit.** `web: unfolding-map layout`

### Task 11: Message arcs, board rail and message list

**Files:**
- Create: `src/components/flowmap/MessageEdge.tsx`, `src/components/flowmap/BoardRail.tsx`, `src/components/flowmap/MessageListPanel.tsx`, `src/components/flowmap/FlowToolbar.tsx`, `src/lib/flow/messagePaint.ts`
- Modify: `src/styles/graph-motion.css` (packet travel, arc draw-in, dead end, rail item arrival; reduced-motion fades)
- Test: `src/components/flowmap/MessageEdge.test.tsx`, `BoardRail.test.tsx`, `MessageListPanel.test.tsx`, `src/lib/flow/messagePaint.test.ts`

**Interfaces:**
- Consumes: `FlowMessage` (Task 9), `BoardView` (Task 7), `rowKind` / `identityOf` / `undeliveredText` (`lib/messages.ts`).
- Produces:
  - `messagePaint(kind: RowKind, colors: ThemeColors): { stroke: string; dash?: string }` — one colour per kind: direct and mention `chart.5`, broadcast `chart.1`, post `chart.4`, directive `chart.3`, operator `status.awaiting`, retract `inkMute`; undelivered dashed `status.failed`.
  - `MessageEdge` — an xyflow edge type `message` (`data: { message: FlowMessage; highlighted: boolean; arriving: boolean }`): a curved arc between the two nodes' sides, a packet that travels it once when `arriving`, a dead end with the reason for undelivered, hover shows the body.
  - `BoardRail({ board: BoardView | { unavailable: string } | null, posts: FlowMessage[], onSelect })` — claims (held / released / expired with owner and times), board posts where they landed (each the end of its writer's arc), standing directives (with their retraction), and a note "this host can't show the board" for `unavailable`, "claims before this version show only while held" when `journal` is false.
  - `MessageListPanel({ messages: FlowMessage[]; spaces: Map<string, string /* label */>; selected; onSelect })` — every message, tagged by space, kind and participants; selecting a row selects its arc.
  - `FlowToolbar({ kinds, participant, participants, onKinds, onParticipant })`.

- [ ] **Step 1: Write the failing tests.** `each_kind_has_its_paint`; `arc_shows_body_on_hover_and_dead_end_for_undelivered`; `board_unavailable_is_said_in_place`; `pre_journal_board_says_so`; `list_tags_rows_by_space_and_selects`.
- [ ] **Step 2: Run and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run; pass.**
- [ ] **Step 5: Commit.** `web: message arcs, board rail, message list`

### Task 12: The agentiflow map

**Files:**
- Create: `src/components/flowmap/FlowMap.tsx`, `src/components/flowmap/RoundNode.tsx`, `src/components/flowmap/ChildNode.tsx`, `src/components/flowmap/WorkflowFrameNode.tsx`, `src/components/flowmap/MoreNode.tsx`, `src/lib/flow/useFlowData.ts`
- Modify: `src/components/RunGraph.tsx` (export its `NODE_TYPES` and `flowKind` so frames render the same step components), `src/pages/AgentiflowDetail.tsx` (Flow tab renders `FlowMap` in place of `AgentiflowGraph`, :344-357)
- Delete: `src/components/agentiflow/AgentiflowGraph.tsx` and its test, and `TimeRail.tsx` if nothing else imports it (check with grep)
- Test: `src/components/flowmap/FlowMap.test.tsx`, `src/lib/flow/useFlowData.test.ts`

**Interfaces:**
- Consumes: Tasks 7–11; `useMessageSpace` for the flow and for each loaded workflow child (`{ route: 'runs', id: childRunId }`).
- Produces:
  - `useFlowData(detail: AgentiflowDetail, opts: { running: boolean }): FlowInput & { loadFrame(childRunId: string): void }` — children re-read on each `detail` change; the flow board re-read with it while running; a workflow child's graph (`getRunGraph` + its events → `buildRunGraphModel`), messages and board fetched once on `loadFrame`, then followed only while that child is running.
  - `ChildNode` built on `NodeFrame` (the run graph's frame) with codename, agent, kind badge (AGENT / SUB-AGENT / WORKFLOW), status glyph, tokens, duration, `output_excerpt`, transcript link, a message-count badge, an `inferred` marker. `RoundNode` shows round, decision (or number/counts only), goals after, spend, lead-transcript link; a collapsed round shows its summary line. `WorkflowFrameNode` is a group whose inner nodes use `NODE_TYPES`, with the child space's arcs drawn between them and a small board strip (a compact `BoardRail`) along its bottom edge; a generated frame is labelled "written by the lead in round N" with a "definition" button that opens the YAML from `getAgentiflowGenerated`. `MoreNode` expands its round.
  - `FlowMap({ detail, running })`: header strip (goals met, spend, tokens), `TimeAxis`, `FlowToolbar`, the xyflow canvas (pan/zoom/fit controls as `RunGraph`), the rail, then `MessageListPanel` and the goals panel below. Live: running rounds and children breathe with the run graph's running glow and the end of the spine pulses; the canvas follows the newest round unless the operator has panned; nodes, arcs and rail items not in `revealed(t)` are hidden, newly revealed ones arrive with the motion classes.

- [ ] **Step 1: Write the failing tests** (jsdom, fixture through mocked api): `renders_every_round_child_and_frame`; `frame_loads_its_graph_on_first_view` (`loadFrame` called once, child graph nodes render with the run graph's node components); `generated_frame_opens_its_definition`; `a_poll_keeps_pan_expansion_and_replay` (re-render with a new child appended while a frame is expanded and `t` is mid-replay → expansion kept, `t` unchanged, the new child present); `replay_hides_then_reveals_in_time_order`.
- [ ] **Step 2: Run and see them fail.**
- [ ] **Step 3: Implement**, wire the Flow tab, delete the old graph.
- [ ] **Step 4: Run** the new tests, `npx vitest run src/pages/AgentiflowDetail src/components/agentiflow` and `npx tsc --noEmit -p .`; pass.
- [ ] **Step 5: Commit.** `web: agentiflow Flow tab is the unfolding map`

### Task 13: The workflow run page layer

**Files:**
- Modify: `src/components/RunGraph.tsx` (optional `overlay?: { messages: FlowMessage[]; board: BoardView | { unavailable: string } | null; revealed: Set<string> | null; selected?: string; onSelect?(id: string): void }`), `src/pages/RunDetail.tsx` (around the graph section :1395-1410 and `useMessageSpace` :596)
- Create: `src/lib/flow/runLayer.ts`
- Test: `src/lib/flow/runLayer.test.ts`, `src/components/RunGraph.overlay.test.tsx`, `src/pages/RunDetail.layer.test.tsx`

**Interfaces:**
- Consumes: Tasks 7–11.
- Produces: `buildRunLayer(model: RunGraphModel, events: RunEvent[], space: { participants; messages }, board): { messages: FlowMessage[]; timeline: { items: TimelineItem[]; bands: TimelineBand[] } }` — message endpoints mapped through participants' `step_id` (and `unit`) to step node ids, `operator` to an off-canvas entry, unmapped to `'offcanvas'`; timeline items from `step_started` / unit start events and message/board times, bands per step. `RunDetail` shows the layer only when `messages.space === 'present'`: `RunGraph` gets `overlay`, the `TimeAxis` sits above the graph, `MessageListPanel` and a board panel below it; when Replay is idle a finished run renders whole and a running one live, as today. The Messages tab is unchanged.

- [ ] **Step 1: Write the failing tests.** `run_messages_map_to_step_nodes`; `operator_broadcast_enters_offcanvas`; `overlay_draws_arcs_and_rail`; `no_space_no_layer` (a run with `space: 'absent'` renders exactly today's graph); `board_unavailable_is_said_in_place` (remote 501).
- [ ] **Step 2: Run and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the new tests plus `npx vitest run src/pages/RunDetail src/components/RunGraph` and `npx tsc --noEmit -p .`; pass.
- [ ] **Step 5: Commit.** `web: workflow runs show their messages, board and timeline on the graph`

### Task 14: Verify on real runs and finish

**Files:**
- Modify: `docs/messaging.md` or `docs/cp-api.md` web notes if behaviour differs from Task 6's text; `CLAUDE.md` `rupu-cp` web paragraph (one line naming `components/flowmap/`, `lib/flow/`, `lib/timeline/`)

- [ ] **Step 1: Full checks.** `npx vitest run` (all pass), `npx tsc --noEmit -p .`, `npx vite build --outDir <scratch>`; the touched crates' `cargo test -p <crate> --test it` and `cargo clippy -p <crate> --tests -- -D warnings` for rupu-launch, rupu-agentiflow, rupu-fleet, rupu-runtime, rupu-cp, rupu-cli.
- [ ] **Step 2: Browser check** (dev server `cp-web-dev`, proxying the live CP read-only — GETs only, never mutate or restart it): a real agentiflow with messages (`af_01M4W605KQY5DRHZD3TKB8KZ2K`, 23 units, 26 messages, pre-change data: everything inferred, live claims only), a long one (`af_01M4C71JHT6G7HTJM4NAF3WCZ7`, 200 rounds), a workflow run with a message space and one without; light and dark; desktop and tablet; reduced motion; Replay and scrubber on both pages. Screenshot each and fix what's wrong before the PR.
- [ ] **Step 3: Commit** any doc changes. `docs: agentiflow flow web notes`
