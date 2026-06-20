# CP Live Run View depth (Phase 1.5) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deepen the Control Plane's run view into a professional workflow run-graph (all steps, all states, distinct shapes for `for_each`/`parallel`/`panel+gate`/gate, active-frontier animation) and split the activity surface into Build (definitions) vs Runs (executions: Agents/Workflows/Autoflows).

**Architecture:** Additive, read-only on top of the Phase-1 `rupu-cp` crate + `crates/rupu-cp/web` app. New read endpoints assemble graph inputs from each run's own persisted `workflow.yaml` snapshot + `step_results.jsonl` + `unit_checkpoints.jsonl`; the frontend merges those with the existing live SSE stream into a `RunGraphModel`, lays it out with dagre (LR), and paints it with React Flow. One small non-read change: an optional `Event::PanelRound` for the live loop counter (isolated, cuttable task).

**Tech Stack:** Rust (axum, serde, rupu-orchestrator/runtime), React 18 + TypeScript + `@xyflow/react` + `@dagrejs/dagre`.

**Spec:** `docs/superpowers/specs/2026-06-18-rupu-cp-live-run-view-depth-design.md`

**Branch:** `feat-cp-live-run-view-depth` (stacks on the Phase-1 PR #319; rebase onto `main` once #319 merges).

---

## File structure

```
crates/rupu-cp/src/api/
  graph.rs            # GET /api/runs/:id/graph  (skeleton + units)
  runs.rs    (modify) # add trigger field to list DTO; GET /api/runs/workflows
  run_streams.rs      # GET /api/runs/autoflows, /api/runs/agents
  autoflows.rs        # GET /api/autoflows (definitions)
crates/rupu-cp/src/dto.rs (new, optional) # shared slim DTOs

crates/rupu-orchestrator/src/executor/event.rs (modify) # Event::PanelRound (Task 9, cuttable)
crates/rupu-orchestrator/src/...runner...      (modify) # emit PanelRound

crates/rupu-cp/web/src/
  lib/api.ts          (modify) # getRunGraph + run-stream getters + types
  lib/runGraphModel.ts (new)   # pure builder: graph+events -> RunGraphModel
  lib/graphLayout.ts   (new)   # dagre LR layout + structure-hash cache
  lib/sidebarNav.ts    (modify)# Runs group, Build>Autoflows, Run->Fleet
  components/RunGraph.tsx (rewrite)
  components/graph/{StepNode,ParallelNode,FanoutNode,PanelLoopNode}.tsx (new)
  components/FanoutDrill.tsx (new)
  pages/runs/{WorkflowRuns,AutoflowRuns,AgentRuns}.tsx (new; Runs.tsx folds into WorkflowRuns)
  pages/AutoflowsDefs.tsx (new)
```

---

## PART A — Backend (Rust, TDD)

### Task 1: Workflow → graph-skeleton DTO mapping

**Files:** Create `crates/rupu-cp/src/api/graph.rs`; modify `crates/rupu-cp/src/api/mod.rs`, `server.rs`.

Goal: a pure function that maps a parsed `rupu_orchestrator::Workflow` into a slim step-DAG the UI can render, deriving each step's `kind`.

- [ ] **Step 1: Inspect the real Step schema.** Read `crates/rupu-orchestrator/src/workflow.rs` for the exact `Step` fields. Confirm presence of: `id: String`, `agent: Option<String>` (or similar), `for_each: Option<String>`, `parallel: Option<Vec<SubStep>>`, `max_parallel: Option<u32>`, `panel: Option<Panel>`; `Panel` has `gate: Option<PanelGate>`; `PanelGate { max_iterations: u32, until_no_findings_at_severity_or_above: Severity }`; `SubStep { id, agent, .. }`. Note the exact field names — the code below uses them and must match.

- [ ] **Step 2: Write the failing test** `crates/rupu-cp/tests/graph.rs`:
```rust
use rupu_orchestrator::Workflow;

#[test]
fn maps_step_kinds() {
    let yaml = r#"
name: demo
steps:
  - id: ingest
    agent: a
    prompt: x
  - id: review
    parallel:
      - { id: sec, agent: s, prompt: x }
      - { id: perf, agent: p, prompt: x }
  - id: fix
    agent: f
    for_each: "{{ steps.review.results }}"
    prompt: x
  - id: panelcheck
    panel:
      reviewers: [{ id: r1, agent: r, prompt: x }]
      gate: { max_iterations: 5, until_no_findings_at_severity_or_above: high }
    prompt: x
"#;
    let wf = Workflow::parse(yaml).expect("parse");
    let dag = rupu_cp::api::graph::to_step_dag(&wf);
    let kinds: Vec<&str> = dag.steps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, vec!["step", "parallel", "for_each", "panel"]);
    // parallel sub-steps captured
    assert_eq!(dag.steps[1].parallel.as_ref().unwrap().len(), 2);
    // panel gate captured
    let g = dag.steps[3].gate.as_ref().unwrap();
    assert_eq!(g.max_iterations, 5);
}
```
(Adjust the YAML to the minimal valid shape `Workflow::parse` accepts — verify against `crates/rupu-orchestrator/tests` fixtures; the panel/parallel sub-keys must match the real schema field names.)

- [ ] **Step 3: Run it — fails to compile** (`rupu_cp::api::graph` absent). `cargo test -p rupu-cp --test graph`.

- [ ] **Step 4: Implement `graph.rs` DTOs + mapper:**
```rust
use serde::Serialize;
use rupu_orchestrator::Workflow;

#[derive(Serialize)]
pub struct StepDag { pub steps: Vec<StepNodeDto> }

#[derive(Serialize)]
pub struct StepNodeDto {
    pub id: String,
    pub kind: String,                 // "step" | "for_each" | "parallel" | "panel"
    pub agent: Option<String>,
    pub for_each: Option<String>,     // the templated expression, for display
    pub parallel: Option<Vec<SubStepDto>>,
    pub gate: Option<GateDto>,
}
#[derive(Serialize)]
pub struct SubStepDto { pub id: String, pub agent: Option<String> }
#[derive(Serialize)]
pub struct GateDto { pub max_iterations: u32, pub until_severity: String }

impl StepNodeDto {
    fn kind_of(s: &rupu_orchestrator::workflow::Step) -> &'static str {
        if s.parallel.is_some() { "parallel" }
        else if s.panel.is_some() { "panel" }
        else if s.for_each.is_some() { "for_each" }
        else { "step" }
    }
}

pub fn to_step_dag(wf: &Workflow) -> StepDag {
    let steps = wf.steps.iter().map(|s| StepNodeDto {
        id: s.id.clone(),
        kind: StepNodeDto::kind_of(s).to_string(),
        agent: s.agent.clone(),
        for_each: s.for_each.clone(),
        parallel: s.parallel.as_ref().map(|subs| subs.iter()
            .map(|ss| SubStepDto { id: ss.id.clone(), agent: ss.agent.clone() }).collect()),
        gate: s.panel.as_ref().and_then(|p| p.gate.as_ref()).map(|g| GateDto {
            max_iterations: g.max_iterations,
            until_severity: format!("{:?}", g.until_no_findings_at_severity_or_above).to_lowercase(),
        }),
    }).collect();
    StepDag { steps }
}
```
(Fix field paths to the real schema; `rupu_orchestrator::workflow::Step` may be re-exported at the crate root — use whatever path compiles. Add `pub mod graph;` to `api/mod.rs`.)

- [ ] **Step 5: Run test → PASS.** `cargo test -p rupu-cp --test graph`.

- [ ] **Step 6: Commit.** `feat(cp): workflow → step-DAG DTO mapper`.

---

### Task 2: `GET /api/runs/{id}/graph`

**Files:** Modify `crates/rupu-cp/src/api/graph.rs`, `server.rs`. Test `crates/rupu-cp/tests/graph.rs`.

- [ ] **Step 1: Confirm run-dir readers.** `RunStore` exposes `workflow_snapshot(run_id)`/the `run_dir/workflow.yaml` path, `read_step_results`, and a unit-checkpoints reader for `unit_checkpoints.jsonl` (`UnitCheckpoint`). Check `crates/rupu-orchestrator/src/runs.rs` for the public accessor for the workflow snapshot path and a `read_unit_checkpoints`-style method. If the snapshot path getter or checkpoint reader is private, add a minimal `pub fn read_workflow_snapshot(&self, run_id) -> io::Result<String>` and `pub fn read_unit_checkpoints(&self, run_id) -> Result<Vec<UnitCheckpoint>, RunStoreError>` to `runs.rs` (with their own unit tests) rather than reaching into private paths from the CP.

- [ ] **Step 2: Write the failing test:**
```rust
#[tokio::test]
async fn run_graph_returns_skeleton_and_units() {
    let dir = tempfile::tempdir().unwrap();
    let store = rupu_orchestrator::RunStore::new(dir.path().join("runs"));
    // seed a run with a 2-step workflow incl. a for_each, one step result, one unit checkpoint
    // (reuse the seeding helpers from tests/runs.rs + RunStore::create(record, yaml))
    // ... build record, call store.create(&record, WF_YAML), append a StepResultRecord + a UnitCheckpoint ...
    let state = rupu_cp::state::AppState::new(dir.path().into(), Default::default());
    // serve + GET /api/runs/{id}/graph
    // assert body.workflow.steps has all steps (incl. the pending one with no result),
    // body.step_results present, body.units has the seeded unit.
}
```

- [ ] **Step 3: Run → fails (404/route missing).**

- [ ] **Step 4: Implement the handler** in `graph.rs`:
```rust
use axum::{extract::{State, Path}, Json, routing::get, Router};
use crate::{state::AppState, error::{ApiError, ApiResult}};
use rupu_orchestrator::RunStoreError;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/graph", get(run_graph))
}

async fn run_graph(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let run = s.run_store.load(&id).map_err(|e| match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;
    let yaml = s.run_store.read_workflow_snapshot(&id).map_err(|e| ApiError::internal(e.to_string()))?;
    let wf = rupu_orchestrator::Workflow::parse(&yaml).map_err(|e| ApiError::internal(e.to_string()))?;
    let dag = super::graph::to_step_dag(&wf);
    let steps = s.run_store.read_step_results(&id).unwrap_or_default();
    let units = s.run_store.read_unit_checkpoints(&id).unwrap_or_default();
    Ok(Json(serde_json::json!({ "run": run, "workflow": dag, "step_results": steps, "units": units })))
}
```
Merge `graph::routes()` in `server.rs`.

- [ ] **Step 5: Run → PASS.** `cargo test -p rupu-cp`. `cargo clippy -p rupu-cp --all-targets` → exit 0.

- [ ] **Step 6: Commit.** `feat(cp): GET /api/runs/{id}/graph (skeleton + units)`.

---

### Task 3: Runs list trigger-type + `GET /api/runs/workflows`

**Files:** Modify `crates/rupu-cp/src/api/runs.rs`. Test `crates/rupu-cp/tests/runs.rs`.

- [ ] **Step 1: Write the failing test:** seed two runs — one with `event: None` (direct) and one with `event: Some(json!({...}))` (triggered). Assert `GET /api/runs/workflows` returns only the direct one, and that each row in `GET /api/runs` carries `"trigger": "manual"` vs `"event"`. (Check `RunRecord`'s trigger field shape in `runs.rs` — derive `trigger` from `record.event` presence and/or a `trigger`/`trigger_source` field if one exists; verify before coding.)

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement.** Add a `trigger_of(record) -> &'static str` helper (`"manual"` when no trigger payload; `"cron"`/`"event"` per the record's trigger info — inspect the record for how cron vs event is distinguished; if only "triggered vs not" is knowable, use `"manual"` | `"triggered"` and note it). Add the `trigger` field to the runs list DTO (introduce a slim `RunListRow` DTO if the handler currently returns `Vec<RunRecord>` directly — keep `id, workflow_name, status, started_at, finished_at, trigger`). Add `route("/api/runs/workflows", get(list_workflow_runs))` filtering to non-triggered runs.

- [ ] **Step 4: Run → PASS; clippy clean.**

- [ ] **Step 5: Commit.** `feat(cp): runs trigger-type + /api/runs/workflows`.

---

### Task 4: `GET /api/runs/autoflows` (executions)

**Files:** Create `crates/rupu-cp/src/api/run_streams.rs`; modify `api/mod.rs`, `server.rs`, `state.rs`, `Cargo.toml` (+`rupu-runtime` path dep if not present).

- [ ] **Step 1: Inspect `AutoflowHistoryStore`.** Read `crates/rupu-runtime/src/autoflow_history.rs`: confirm constructor (root path under `~/.rupu/autoflows/...`), `list_recent(...)` signature + `AutoflowCycleRecord` fields (autoflow id, cycle id, status, `run_id: Option<String>`, timestamps, outcome). Add `rupu-runtime` to `crates/rupu-cp/Cargo.toml` (`.workspace`-style path dep) if missing.

- [ ] **Step 2: Failing test:** seed an `AutoflowHistoryStore` under a tempdir with one cycle (use its public write API or the on-disk format), point `AppState` at that global dir, assert `GET /api/runs/autoflows` returns the cycle with its `run_id`. Missing store dir → `[]`.

- [ ] **Step 3: Run → fails.**

- [ ] **Step 4: Implement** `list_autoflow_runs` in `run_streams.rs`: construct the `AutoflowHistoryStore` at the global-dir-relative path, `list_recent`, map to a slim DTO `[{ autoflow, cycle_id, status, run_id, started_at, finished_at, outcome }]`; tolerate missing dir → `[]` (map the not-found error to empty). Add `pub mod run_streams;` + merge routes.

- [ ] **Step 5: PASS; clippy clean.**

- [ ] **Step 6: Commit.** `feat(cp): GET /api/runs/autoflows (cycle history)`.

---

### Task 5: `GET /api/runs/agents`

**Files:** Modify `crates/rupu-cp/src/api/run_streams.rs`. Test `crates/rupu-cp/tests/`.

- [ ] **Step 1: Confirm on-disk contracts.** Standalone: `<transcripts>/<run_id>.meta.json` = `StandaloneRunMetadata` (`crates/rupu-cli/src/standalone_run_metadata.rs` — `run_id, session_id?, trigger_source, workspace_path, repo_ref?, issue_ref?, ...`). Session runs: each session's `session.json` has `runs: Vec<SessionRunRecord>` (`run_id, prompt, transcript_path, started_at, completed_at?, status?, tokens`). The CP reads BOTH as the on-disk contract with its own minimal `#[derive(Deserialize)]` DTOs (do NOT depend on `rupu-cli`). Transcripts dir = `<global>/transcripts` (confirm via `crates/rupu-cli/src/paths.rs::transcripts_dir`); sessions dir as in Phase-1 `api/sessions.rs`.

- [ ] **Step 2: Failing test:** write a `<global>/transcripts/run_X.meta.json` (minimal standalone metadata) and a session with one `SessionRunRecord`; assert `GET /api/runs/agents` lists both, each with `run_id` + a `source` tag (`"standalone"` / `"session"`) + `transcript_path`. Skip unparseable files with a warn.

- [ ] **Step 3: Run → fails.**

- [ ] **Step 4: Implement** `list_agent_runs`: scan `<global>/transcripts/*.meta.json` (parse minimal `StandaloneMetaDto { run_id, session_id, trigger_source, .. }`), scan session jsons for `runs[]` (minimal `SessionRunDto { run_id, prompt, transcript_path, started_at, completed_at, status }`), merge into `[{ run_id, source, agent?, session_id?, started_at, status?, transcript_path }]`. Tolerate missing dirs → partial/empty; warn-and-skip on parse errors (mirror Phase-1 sessions pattern).

- [ ] **Step 5: PASS; clippy clean.**

- [ ] **Step 6: Commit.** `feat(cp): GET /api/runs/agents (standalone + session runs)`.

---

### Task 6: `GET /api/autoflows` (definitions)

**Files:** Create `crates/rupu-cp/src/api/autoflows.rs`; modify `api/mod.rs`, `server.rs`.

- [ ] **Step 1: Find the autoflow definitions source.** Grep how `rupu` lists autoflow definitions (e.g. `crates/rupu-cli/src/cmd/` autoflow listing, or a `rupu_runtime`/`rupu_config` reader). Identify the on-disk location (likely `~/.rupu/autoflows/*.yaml|.toml|.md`) + the parsed shape. If there's a library reader, use it; else scan the dir as the contract.

- [ ] **Step 2: Failing test:** seed one autoflow definition file under the global dir; assert `GET /api/autoflows` lists it (name + key fields). Missing dir → `[]`.

- [ ] **Step 3: Run → fails.**

- [ ] **Step 4: Implement** `list_autoflow_defs` → slim DTO `[{ name, workflow?, trigger?, enabled?, .. }]` from the identified source. Tolerate missing → `[]`.

- [ ] **Step 5: PASS; clippy clean.**

- [ ] **Step 6: Commit.** `feat(cp): GET /api/autoflows (definitions)`.

---

## PART B — Frontend (logic TDD'd, visuals by contract)

### Task 7: API client additions

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`. Test `src/lib/api.test.ts`.

- [ ] **Step 1: Add types + methods.** Types: `RunGraphResponse { run: RunRecord; workflow: { steps: StepNodeDto[] }; step_results: StepResultRecord[]; units: UnitCheckpoint[] }`; `StepNodeDto { id; kind: 'step'|'for_each'|'parallel'|'panel'; agent?: string|null; for_each?: string|null; parallel?: {id:string;agent?:string|null}[]|null; gate?: {max_iterations:number;until_severity:string}|null }`; `UnitCheckpoint { step_id:string; index:number; unit_key:string; status:string; transcript_path?:string }` (match Task 2's JSON); `AutoflowRunRow`, `AgentRunRow`, `AutoflowDefRow`, and a `trigger` field on the runs list type. Methods: `getRunGraph(id)`, `getWorkflowRuns()`, `getAutoflowRuns()`, `getAgentRuns()`, `getAutoflowDefs()` — all via the existing `request<T>` wrapper. No `any`.

- [ ] **Step 2: Extend the vitest** to cover one new getter (200 → typed; 404 → ApiError) using the existing fetch-mock pattern.

- [ ] **Step 3: Run** `npm run build` (tsc strict, exit 0) + `npm test -- --run`. Paste lines.

- [ ] **Step 4: Commit.** `feat(cp/web): api client for run-graph + run streams`.

---

### Task 8: `runGraphModel` — pure merge builder (TDD)

**Files:** Create `crates/rupu-cp/web/src/lib/runGraphModel.ts`, `src/lib/runGraphModel.test.ts`.

This is the heart of "all steps, all states." Pure + fully unit-tested.

- [ ] **Step 1: Write the failing tests** `runGraphModel.test.ts`:
```ts
import { describe, it, expect } from 'vitest';
import { buildRunGraphModel, type StepState } from './runGraphModel';
import type { RunGraphResponse, RunEvent } from './api';

const base: RunGraphResponse = {
  run: { id: 'r1', workflow_name: 'wf', status: 'running', started_at: 't' } as any,
  workflow: { steps: [
    { id: 'a', kind: 'step', agent: 'x' },
    { id: 'b', kind: 'for_each', for_each: '{{ }}' },
    { id: 'c', kind: 'step', agent: 'y' },
  ] },
  step_results: [], units: [],
};

it('skeleton-only => every step pending, edges chain in order', () => {
  const m = buildRunGraphModel(base, []);
  expect(m.nodes.map(n => n.state)).toEqual<StepState[]>(['pending','pending','pending']);
  expect(m.edges).toEqual([{ from:'a', to:'b' }, { from:'b', to:'c' }]);
});

it('step_results overlay terminal states', () => {
  const g = { ...base, step_results: [
    { run_id:'r1', step_id:'a', success:true } as any,
    { run_id:'r1', step_id:'c', success:false } as any,
  ]};
  const m = buildRunGraphModel(g, []);
  expect(m.nodeById('a').state).toBe('done');
  expect(m.nodeById('c').state).toBe('failed');
});

it('live events win over results (running, awaiting)', () => {
  const ev: RunEvent[] = [
    { type:'step_started', step_id:'a' } as any,
    { type:'step_awaiting_approval', step_id:'c', reason:'gate' } as any,
  ];
  const m = buildRunGraphModel(base, ev);
  expect(m.nodeById('a').state).toBe('running');
  expect(m.nodeById('c').state).toBe('awaiting_approval');
});

it('for_each unit states aggregate to X/N + counts', () => {
  const g = { ...base, units: [
    { step_id:'b', index:0, unit_key:'u0', status:'done' },
    { step_id:'b', index:1, unit_key:'u1', status:'failed' },
  ]};
  const ev: RunEvent[] = [{ type:'unit_started', step_id:'b', index:2, unit_key:'u2' } as any];
  const m = buildRunGraphModel(g, ev);
  const b = m.nodeById('b');
  expect(b.fanout!.total).toBeGreaterThanOrEqual(3);
  expect(b.fanout!.byState.done).toBe(1);
  expect(b.fanout!.byState.failed).toBe(1);
  expect(b.fanout!.byState.running).toBe(1);
});
```

- [ ] **Step 2: Run → fail (module missing).** `npm test -- --run runGraphModel`.

- [ ] **Step 3: Implement `runGraphModel.ts`:**
```ts
import type { RunGraphResponse, RunEvent, StepNodeDto, UnitCheckpoint } from './api';

export type StepState = 'pending'|'running'|'awaiting_approval'|'done'|'failed'|'skipped';

export interface FanoutState { total: number; byState: Record<StepState, number>; units: UnitView[] }
export interface UnitView { index: number; key: string; state: StepState; transcriptPath?: string }
export interface GraphNode {
  id: string; kind: StepNodeDto['kind']; agent?: string|null; state: StepState;
  fanout?: FanoutState; parallel?: {id:string; state:StepState}[];
  round?: { current:number; max:number }; gate?: StepNodeDto['gate'];
}
export interface GraphEdge { from: string; to: string }
export interface RunGraphModel {
  nodes: GraphNode[]; edges: GraphEdge[];
  nodeById(id: string): GraphNode;
}

const ZERO = (): Record<StepState,number> =>
  ({ pending:0, running:0, awaiting_approval:0, done:0, failed:0, skipped:0 });

// precedence: live SSE > checkpoints/results > skeleton(pending)
export function buildRunGraphModel(g: RunGraphResponse, events: RunEvent[]): RunGraphModel {
  const nodes: GraphNode[] = g.workflow.steps.map(s => ({
    id: s.id, kind: s.kind, agent: s.agent ?? undefined, state: 'pending',
    gate: s.gate ?? undefined,
    parallel: s.parallel?.map(p => ({ id: p.id, state: 'pending' as StepState })),
  }));
  const byId = new Map(nodes.map(n => [n.id, n]));

  // layer 2: step_results
  for (const r of g.step_results) {
    const n = byId.get((r as any).step_id); if (!n) continue;
    n.state = (r as any).skipped ? 'skipped' : ((r as any).success ? 'done' : 'failed');
  }
  // layer 2: for_each units from checkpoints
  const unitMap = new Map<string, UnitView[]>();
  for (const u of g.units) {
    const arr = unitMap.get(u.step_id) ?? []; 
    arr.push({ index: u.index, key: u.unit_key, state: normUnit(u.status), transcriptPath: u.transcript_path });
    unitMap.set(u.step_id, arr);
  }
  // layer 3: live events (win)
  for (const e of events) {
    const stepId = (e as any).step_id; const n = stepId ? byId.get(stepId) : undefined; if (!n) continue;
    switch (e.type) {
      case 'step_started': case 'step_working': n.state = 'running'; break;
      case 'step_awaiting_approval': n.state = 'awaiting_approval'; break;
      case 'step_completed': n.state = (e as any).success ? 'done' : 'failed'; break;
      case 'step_failed': n.state = 'failed'; break;
      case 'step_skipped': n.state = 'skipped'; break;
      case 'unit_started': {
        const arr = unitMap.get(stepId) ?? []; 
        if (!arr.some(u => u.index === (e as any).index))
          arr.push({ index:(e as any).index, key:(e as any).unit_key, state:'running' });
        else arr.forEach(u => { if (u.index===(e as any).index && u.state==='pending') u.state='running'; });
        unitMap.set(stepId, arr); break;
      }
      case 'unit_completed': {
        const arr = unitMap.get(stepId) ?? [];
        arr.forEach(u => { if (u.index===(e as any).index) u.state = (e as any).success ? 'done':'failed'; });
        unitMap.set(stepId, arr); break;
      }
      // case 'panel_round': n.round = { current:(e as any).round, max:(e as any).max_iterations }; break;  // Task 9
    }
  }
  // fold fanout aggregates
  for (const [stepId, units] of unitMap) {
    const n = byId.get(stepId); if (!n) continue;
    const byState = ZERO(); units.forEach(u => { byState[u.state]++; });
    n.fanout = { total: units.length, byState, units: units.sort((a,b)=>a.index-b.index) };
    if (n.state !== 'running' && byState.running > 0) n.state = 'running';
    if (n.state === 'pending' && byState.done + byState.failed > 0) n.state = 'running';
  }

  const edges: GraphEdge[] = [];
  for (let i = 0; i < nodes.length - 1; i++) edges.push({ from: nodes[i].id, to: nodes[i+1].id });
  return { nodes, edges, nodeById: (id) => byId.get(id)! };
}

function normUnit(s: string): StepState {
  const x = s.toLowerCase();
  if (x.includes('fail')) return 'failed';
  if (x.includes('done') || x.includes('complete') || x.includes('success')) return 'done';
  if (x.includes('skip')) return 'skipped';
  if (x.includes('run')) return 'running';
  return 'pending';
}
```
(The `panel_round` case stays commented until Task 9 lands. Edge model here is the linear chain; nesting for parallel/panel is rendered by the node components, not extra top-level edges — keep edges step-to-step.)

- [ ] **Step 4: Run tests → PASS.** `npm test -- --run runGraphModel`.

- [ ] **Step 5: Commit.** `feat(cp/web): runGraphModel merge builder + tests`.

---

### Task 9: `Event::PanelRound` (BACKEND — isolated, cuttable)

**Files:** Modify `crates/rupu-orchestrator/src/executor/event.rs` + the panel-gate runner that loops. Tests in the orchestrator crate.

> Cuttable: if it adds friction, skip this task — the graph still renders the panel/gate construct; only the *live* round counter is lost (post-hoc round info can come from step_results). Do NOT block Part B on it.

- [ ] **Step 1: Add the variant** to `Event` (it derives `Serialize/Deserialize`, `#[serde(tag="type", rename_all="snake_case")]`):
```rust
PanelRound {
    run_id: String,
    step_id: String,
    round: u32,
    max_iterations: u32,
    max_severity_remaining: Option<String>,
},
```

- [ ] **Step 2: Round-trip test** (mirror the existing `run_started_round_trips_through_json` test in `event.rs`): serialize a `PanelRound` → assert `"type":"panel_round"` + fields; deserialize back.

- [ ] **Step 3: Emit it.** Find the panel/gate fix-loop in the runner (grep `max_iterations` / `until_no_findings_at_severity_or_above` usage). At the top of each gate iteration, emit `Event::PanelRound { round, max_iterations, .. }` through the same `EventSink` the step events use. Add/extend a runner test asserting the event is emitted per round (use the mock sink pattern from `crates/rupu-agent` runner tests / the executor's `InMemorySink`).

- [ ] **Step 4: Run** `cargo test -p rupu-orchestrator` → green; `cargo clippy -p rupu-orchestrator --all-targets`.

- [ ] **Step 5: Wire the client.** Uncomment the `panel_round` case in `runGraphModel.ts` (Task 8) + add `PanelRoundEvent` to the `RunEvent` union in `api.ts`; add a model test asserting `n.round` is set. `npm test -- --run`.

- [ ] **Step 6: Commit.** `feat(orchestrator,cp): Event::PanelRound for live loop counter`.

---

### Task 10: dagre layout

**Files:** Create `crates/rupu-cp/web/src/lib/graphLayout.ts`, `src/lib/graphLayout.test.ts`. Add `@dagrejs/dagre` to `web/package.json`.

- [ ] **Step 1: Add dep.** `npm i @dagrejs/dagre` (and `@types/dagre` if needed). Commit the lockfile.

- [ ] **Step 2: Failing test:**
```ts
import { layoutGraph } from './graphLayout';
it('positions a linear chain left to right, deterministic', () => {
  const m = { nodes:[{id:'a',kind:'step',state:'done'},{id:'b',kind:'step',state:'pending'}], edges:[{from:'a',to:'b'}] } as any;
  const p1 = layoutGraph(m); const p2 = layoutGraph(m);
  expect(p1.get('b')!.x).toBeGreaterThan(p1.get('a')!.x);   // LR
  expect(p1.get('a')!.y).toBeCloseTo(p1.get('b')!.y, 0);     // same rank row
  expect(p1).toEqual(p2);                                     // deterministic
});
```

- [ ] **Step 3: Implement** `graphLayout.ts`:
```ts
import dagre from '@dagrejs/dagre';
import type { RunGraphModel } from './runGraphModel';

export interface Pos { x: number; y: number; width: number; height: number }
const NODE_W = 150, NODE_H = 64;

export function layoutGraph(m: RunGraphModel): Map<string, Pos> {
  const g = new dagre.graphlib.Graph();
  g.setGraph({ rankdir: 'LR', nodesep: 28, ranksep: 60 });
  g.setDefaultEdgeLabel(() => ({}));
  for (const n of m.nodes) g.setNode(n.id, { width: NODE_W, height: NODE_H });
  for (const e of m.edges) g.setEdge(e.from, e.to);
  dagre.layout(g);
  const out = new Map<string, Pos>();
  for (const n of m.nodes) {
    const d = g.node(n.id);
    out.set(n.id, { x: d.x - NODE_W/2, y: d.y - NODE_H/2, width: NODE_W, height: NODE_H }); // dagre centers; RF wants top-left
  }
  return out;
}
```
(Add a structure-hash cache only if profiling shows relayout cost — YAGNI for the first cut; note it as a follow-up.)

- [ ] **Step 4: Run → PASS.** `npm test -- --run graphLayout`. `npm run build`.

- [ ] **Step 5: Commit.** `feat(cp/web): dagre LR graph layout`.

---

### Task 11: `RunGraph` rewrite + node components (visual — matt-validated)

**Files:** Rewrite `crates/rupu-cp/web/src/components/RunGraph.tsx`; create `components/graph/{StepNode,ParallelNode,FanoutNode,PanelLoopNode}.tsx` and `components/FanoutDrill.tsx`. Modify `pages/RunDetail.tsx`.

Contract-driven (rendering validated by matt; the bar here is a clean strict-TS `npm run build` + faithful adherence to the approved mockup `‎.superpowers/brainstorm/.../graph-pro.html` + `fanout-loop.html`).

- [ ] **Step 1: `RunGraph.tsx`** — takes `{ model: RunGraphModel; positions: Map<string,Pos>; onOpenUnit?(stepId,index): void }`. Builds React Flow `nodes` (type per `node.kind`: `'step'|'parallel'|'fanout'|'panel'`) with `position` from `positions` and `data: node`; builds React Flow `edges` from `model.edges`, setting `animated: true` + a blue class on the edge whose target is the running frontier node, an amber dashed class on the edge into an `awaiting_approval` node, static otherwise. Wrap in `ReactFlowProvider`; import `@xyflow/react/dist/style.css`; read-only (`nodesDraggable={false} nodesConnectable={false}`); `fitView`; `<MiniMap/>` + `<Controls/>`. Register the four custom `nodeTypes`.

- [ ] **Step 2: Node components** (`components/graph/`), each a memoized custom node reading `data: GraphNode`, styled per the approved mockup using the existing palette + the §2.2 state→color/glyph map (reuse a shared `STATE_STYLE` map; mirror `StatusPill`/`RunGraph` v1 colors so they stay consistent):
  - `StepNode` — card: glyph · name · agent chip · running-pulse ring when `state==='running'`.
  - `ParallelNode` — bordered container; header `parallel · {k}/{n}`; the `data.parallel[]` sub-cards stacked.
  - `FanoutNode` — if `data.fanout.total <= 12`: inline grid of unit squares (color per unit state), each clickable → `onOpenUnit`. Else: collapsed card leading with `{done}/{total}` + a single % bar (`done/total`), `{failed} failed` in red when >0, a density preview grid, and an "expand" affordance opening `<FanoutDrill>`.
  - `PanelLoopNode` — panel container + gate sub-node; `round` counter (`{current}/{max}`) in the header when present; the loop back-edge is drawn as a self-referencing React Flow edge (or a styled marker) labeled `findings remain`, animated while the panel is running.
- [ ] **Step 3: `FanoutDrill.tsx`** — a virtualized/scrollable, state-filterable list of `data.fanout.units` (each row: glyph · `unit_key` · state · link to its transcript via `transcriptPath`). Cap rendered rows with an explicit "+N more" (no silent truncation).
- [ ] **Step 4: Wire into `RunDetail`** — replace the v1 graph: build `model = buildRunGraphModel(graphResp, liveEvents)` (graphResp from `getRunGraph(id)`, liveEvents from the existing single SSE subscription), `positions = layoutGraph(model)`, render `<RunGraph .../>`. Keep the existing event-feed tab + single SSE subscription (now also feeding the model). Recompute the model on each event; recompute layout only when the node *set* changes (compare ids) to avoid relayout thrash.
- [ ] **Step 5: Build.** `npm run build` (strict, exit 0) + `npm test -- --run`. Paste lines. Note for matt: visual validation pending.
- [ ] **Step 6: Commit.** `feat(cp/web): run graph rewrite (all states, fan-out, panel loop, animation)`.

---

### Task 12: Nav restructure + Runs/Build pages

**Files:** Modify `src/lib/sidebarNav.ts`, `src/App.tsx`; create `pages/runs/{WorkflowRuns,AutoflowRuns,AgentRuns}.tsx`, `pages/AutoflowsDefs.tsx`; fold `pages/Runs.tsx` → `WorkflowRuns.tsx`.

- [ ] **Step 1: `sidebarNav.ts`** — restructure to the approved IA: a **Runs** group (`/runs/agents`, `/runs/workflows`, `/runs/autoflows`), **Observe** (Live Events, Coverage), **Build** (Workflows, Agents, **Autoflows** → `/autoflows`), **Fleet** (Sessions, Workers, renamed from "Run"), Settings. Use existing lucide icons. Update `GroupID` union.
- [ ] **Step 2: Run pages** — `WorkflowRuns.tsx` (today's `Runs.tsx` content + the new `trigger` filter chips; rows → `/runs/:id` graph), `AutoflowRuns.tsx` (`getAutoflowRuns()`; rows cross-link to `/runs/:runId` when present), `AgentRuns.tsx` (`getAgentRuns()`; rows → transcript view; NO graph). Reuse `ListCard`/`SectionHeader`/`StatusPill`/`lib/time`.
- [ ] **Step 3: `AutoflowsDefs.tsx`** — `getAutoflowDefs()` list (Build › Autoflows).
- [ ] **Step 4: `App.tsx`** — wire `/runs/agents`, `/runs/workflows`, `/runs/autoflows`, `/autoflows`, keep `/runs/:id` → `RunDetail`. Ensure `/runs/:id` doesn't shadow `/runs/agents` (order/segment-specific routes first).
- [ ] **Step 5: Build.** `npm run build` (strict) + `npm test -- --run`. Paste lines.
- [ ] **Step 6: Commit.** `feat(cp/web): Runs/Build/Fleet nav + run-stream pages`.

---

## Self-review

**Spec coverage:** IA Build/Runs/Fleet split ✓ (T12); Runs›Workflows/Autoflows/Agents data ✓ (T3/T4/T5); Build›Autoflows ✓ (T6/T12); run-graph skeleton-from-snapshot+units ✓ (T1/T2); all-states merge precedence ✓ (T8); state model colors/shapes ✓ (T11 STATE_STYLE); node types step/parallel/fanout/panel+gate/gate ✓ (T11); fan-out X/N+%+failed + ≤12 inline + drill-in ✓ (T8 fanout + T11 FanoutNode/FanoutDrill); dagre LR ✓ (T10); active-frontier animation ✓ (T11 edges); panel-round live counter ✓ cuttable (T9); single SSE preserved ✓ (T11 S4); testing backend+model+layout ✓.

**Placeholder scan:** the two backend "inspect/confirm the real schema" steps (T1S1, T2S1, T4S1, T5S1, T6S1) are deliberate verification steps with the exact symbols to check, not hand-waves; the code that follows is concrete. T6 (autoflow defs source) is the least-pinned — its Step 1 names where to look and the implementer pins the reader. Flagged, not hidden.

**Type consistency:** `StepState` (6 values) shared across runGraphModel/layout/RunGraph; `GraphNode`/`RunGraphModel.nodeById` used consistently; `StepNodeDto`/`UnitCheckpoint`/`RunGraphResponse` align between api.ts (T7) and the backend JSON (T2); `buildRunGraphModel(g, events)` and `layoutGraph(m)` signatures match their call sites in T11.

**Notes for the executor:** rupu rules — workspace deps pinned in root `Cargo.toml`; `rupu-cli` stays thin (no CP logic there); `#![deny(clippy::all)]` incl. `--all-targets`; never package-wide `cargo fmt`. The web UI can't be CI-asserted — matt validates rendering before merge (run `make cp` then `rupu cp serve`). Branch `feat-cp-live-run-view-depth` stacks on PR #319; rebase onto `main` after it merges. PART A and B Tasks 7–8/10 are independent and can interleave; T11 depends on T7/T8/T10; T9 is optional and isolated.
