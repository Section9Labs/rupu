# Non-linear Orchestration — Phase 1 (language + editor) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let authors design + validate non-linear workflows — explicit `next:` edges, `split`/`join` orchestration nodes, DAG validation, data-edge inference, in both the language and the editor — while every existing (linear) workflow keeps running unchanged and a non-linear workflow errors clearly at run time until the Phase 2 scheduler.

**Architecture:** The **language** (`rupu-orchestrator`) gains explicit edges + `split`/`join` + a dependency-graph validator + a runtime gate; the **runner is NOT changed to execute graphs** (Phase 2). The **editor** (`rupu-cp/web`) flips `deriveEdges` from "consecutive order" to "explicit edges," so drawing sets a `next` and dropping leaves a node unconnected.

**Tech Stack:** Rust 2021 (`thiserror`, `serde`), React 18 + TypeScript, Vitest, `cargo test`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-24-rupu-nonlinear-phase1-design.md`; parent proposal + decisions: `…-nonlinear-orchestration-proposal.md` (D1 `next:` now; D2 implicit all-join; D3 infer data edges; D4 loops→Phase 3; D5 language+editor first).
- **Branch:** `nonlinear-phase1`, off current `main` (v0.68.6+).
- **No scheduler in Phase 1.** The runner must NOT execute `split`/`join`/forks — it returns a clear error (Task 3). No silent linear mis-run.
- **Legacy compatibility is non-negotiable:** a workflow with no `next`/`split`/`join` parses, validates, serializes, and runs byte-for-byte as today. New fields are `#[serde(default, skip_serializing_if …)]`.
- Rust: `#![deny(clippy::all)]`; errors via `thiserror`. Run `cargo test -p rupu-orchestrator`. Editor: `next` path only; classic byte-identical; tokens only; `npx vitest run` + `npx tsc -b --noEmit` from `crates/rupu-cp/web`.
- **Edge direction is successor (`next`);** `join` derives its inbound from the edges pointing at it (predecessor `depends_on` is Phase 3).

## File Structure

| File | Change |
|---|---|
| `crates/rupu-orchestrator/src/workflow.rs` | `Step` gains `next`/`split`/`join`; `Join`/`JoinWait` types; dependency-graph model + DAG/cycle + edge/shape validation; `is_nonlinear`; new `WorkflowParseError` variants. |
| `crates/rupu-orchestrator/src/runner.rs` | `run_workflow` gate: `is_nonlinear` → `NonlinearNotYetSupported` error before the linear loop. |
| `crates/rupu-cp/web/src/lib/workflowGraph.ts` | `StepNodeData` gains `next`/`split`/`joinWait`; `yamlToGraph` parses them; `deriveEdges` flips to explicit edges; `nodeToStepObject` emits them; `validateGraph` cycle/edge checks; a `hasExplicitEdges`/migration helper. |
| `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.tsx` | `applyConnect` sets `next` (replace on regular / accumulate on split); `applyDelete`/`applyRemoveEdges` clear `next`; drop leaves no edges. |
| `crates/rupu-cp/web/src/components/workflow-editor/kindVisuals.ts` + `nodeShapes.ts` | `split`/`join` `KIND_SHAPE` + silhouettes (placeholder fan glyphs); work/orchestration grouping. |
| `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` + `NodePalette.tsx` | `split`/`join` bodies + palette entries under a work/orchestration split. |
| respective `*.test.*` | tests per task. |

---

### Task 1: Language — `next` / `split` / `join` schema

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` — `Step` (808-915), add `Join`/`JoinWait`, new error variants
- Test: same file's `#[cfg(test)]` module

**Interfaces:**
- Produces: `Step.next: Vec<String>`, `Step.split: Option<Vec<String>>`, `Step.join: Option<Join>`; `pub struct Join { pub wait: JoinWait }`; `pub enum JoinWait { All, Any, Count(u32) }`. Tasks 2-3 consume these. Editor Task 4 mirrors the wire shape.

- [ ] **Step 1: Write the failing test**

Add to the tests module:

```rust
#[test]
fn parses_next_split_join() {
    let raw = r#"
name: w
steps:
  - id: a
    agent: x
    prompt: p
    next: [fan]
  - id: fan
    split: [b, c]
  - id: b
    agent: x
    prompt: p
    next: [gather]
  - id: c
    agent: x
    prompt: p
    next: [gather]
  - id: gather
    join: { wait: all }
"#;
    let wf = Workflow::parse(raw).expect("should parse");
    let a = wf.steps.iter().find(|s| s.id == "a").unwrap();
    assert_eq!(a.next, vec!["fan".to_string()]);
    let fan = wf.steps.iter().find(|s| s.id == "fan").unwrap();
    assert_eq!(fan.split.as_deref(), Some(&["b".to_string(), "c".to_string()][..]));
    let gather = wf.steps.iter().find(|s| s.id == "gather").unwrap();
    assert!(matches!(gather.join.as_ref().unwrap().wait, JoinWait::All));
}

#[test]
fn join_wait_count_and_any_parse() {
    let any = Workflow::parse("name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n  - id: j\n    join: { wait: any }\n").unwrap();
    assert!(matches!(any.steps[1].join.as_ref().unwrap().wait, JoinWait::Any));
    let cnt = Workflow::parse("name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n  - id: j\n    join: { wait: { count: 2 } }\n").unwrap();
    assert!(matches!(cnt.steps[1].join.as_ref().unwrap().wait, JoinWait::Count(2)));
}

#[test]
fn legacy_workflow_serializes_without_new_fields() {
    let raw = "name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n";
    let wf = Workflow::parse(raw).unwrap();
    let out = serde_yaml::to_string(&wf).unwrap();
    assert!(!out.contains("next"), "legacy step must not emit next:");
    assert!(!out.contains("split"));
    assert!(!out.contains("join"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator parses_next_split_join`
Expected: FAIL — `deny_unknown_fields` rejects `next`/`split`/`join`.

- [ ] **Step 3: Implement**

Add the types before `Step` (near `Branch`/`Panel`):

```rust
/// Join barrier policy (Phase 1 language; executed in Phase 2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JoinWait {
    All,
    Any,
    Count(u32),
}
impl Default for JoinWait {
    fn default() -> Self { JoinWait::All }
}

/// A join (barrier) orchestration node. Its inbound set is derived from the
/// edges that point at it; it declares only the wait policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Join {
    #[serde(default)]
    pub wait: JoinWait,
}
```

Add three fields to `Step` (before `action`), each with a doc comment:

```rust
    /// Explicit successor edge(s). Empty in a legacy (edge-free) workflow,
    /// where flow follows list order. Non-empty makes this an explicit graph.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// `split` orchestration node — fans the flow into N independent concurrent
    /// tracks. Carries no agent/action/etc. (validated). Executed in Phase 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<Vec<String>>,
    /// `join` (barrier) orchestration node — waits for its inbound paths per
    /// `wait`. Carries no agent/etc. Executed in Phase 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<Join>,
```

Note: `JoinWait::Count` needs a custom (or externally-tagged) serde form so `{ count: 2 }` parses. The simplest robust encoding — represent `JoinWait` as:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum JoinWait {
    Keyword(JoinWaitKeyword),      // "all" | "any"
    Count { count: u32 },          // { count: k }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JoinWaitKeyword { All, Any }
```

Adjust the test matchers accordingly (`JoinWait::Keyword(JoinWaitKeyword::All)`, `JoinWait::Count { count: 2 }`). Pick whichever encoding parses `wait: all` / `wait: any` / `wait: { count: 2 }` cleanly and round-trips; document the choice in your report.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p rupu-orchestrator` — the three new tests pass; the whole crate's existing tests still pass (legacy round-trip unaffected).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/workflow.rs
git commit -m "feat(orch): next/split/join schema (Phase 1 language)"
```

---

### Task 2: Language — dependency graph + DAG/cycle/edge/shape validation

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` — new validation fns + error variants; wire into `Workflow::parse` (~977-1018)
- Test: same file

**Interfaces:**
- Consumes: Task 1 fields.
- Produces: `fn workflow_edges(&Workflow) -> Vec<(String, String)>` (control ∪ inferred data edges) and validation that runs in `Workflow::parse`. New errors: `EdgeTargetUnknown`, `EdgeSelfLoop`, `WorkflowCycle`, `OrchestrationNodeHasWork`, `JoinCountInvalid`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn rejects_a_cycle() {
    // a -> b -> a via explicit next
    let raw = "name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n    next: [b]\n  - id: b\n    agent: x\n    prompt: p\n    next: [a]\n";
    let err = Workflow::parse(raw).unwrap_err();
    assert!(matches!(err, WorkflowParseError::WorkflowCycle { .. }), "got {err:?}");
}

#[test]
fn rejects_unknown_edge_target() {
    let raw = "name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n    next: [nope]\n";
    assert!(matches!(Workflow::parse(raw).unwrap_err(), WorkflowParseError::EdgeTargetUnknown { .. }));
}

#[test]
fn split_node_may_not_carry_an_agent() {
    let raw = "name: w\nsteps:\n  - id: s\n    agent: x\n    prompt: p\n    split: [a]\n  - id: a\n    agent: x\n    prompt: p\n";
    assert!(matches!(Workflow::parse(raw).unwrap_err(), WorkflowParseError::OrchestrationNodeHasWork { .. }));
}

#[test]
fn data_reference_creates_an_inferred_edge_but_forward_ok() {
    // a is referenced by b's prompt; b declares no next; still valid (inferred a->b)
    let raw = "name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n  - id: b\n    agent: x\n    prompt: \"use {{ steps.a.output }}\"\n";
    let wf = Workflow::parse(raw).unwrap();
    let edges = workflow_edges(&wf);
    assert!(edges.contains(&("a".to_string(), "b".to_string())));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator rejects_a_cycle`
Expected: FAIL — no cycle detection / edges yet.

- [ ] **Step 3: Implement**

Add error variants to `WorkflowParseError`:

```rust
    #[error("step `{step}`: edge target `{target}` is not a known step")]
    EdgeTargetUnknown { step: String, target: String },
    #[error("step `{step}`: an edge cannot target its own step")]
    EdgeSelfLoop { step: String },
    #[error("workflow has a cycle through: {path}")]
    WorkflowCycle { path: String },
    #[error("step `{step}`: a `{kind}` orchestration node cannot also carry agent/action/for_each/parallel/panel/branch/approval")]
    OrchestrationNodeHasWork { step: String, kind: String },
    #[error("step `{step}`: `join.wait.count` must be at least 1")]
    JoinCountInvalid { step: String },
```

Add `workflow_edges` (control ∪ inferred data). Reuse the existing `collect_templates_for_step` / `scan_step_refs` machinery for the data edges:

```rust
/// All dependency edges (source_id -> target_id): explicit control edges
/// (`next`, `split`, branch then/else) UNION inferred data edges (a step that
/// references `steps.X.*` depends on X). Deduped.
pub fn workflow_edges(wf: &Workflow) -> Vec<(String, String)> {
    let ids: std::collections::BTreeSet<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();
    let mut edges: std::collections::BTreeSet<(String, String)> = Default::default();
    for step in &wf.steps {
        for t in &step.next { edges.insert((step.id.clone(), t.clone())); }
        if let Some(sp) = &step.split { for t in sp { edges.insert((step.id.clone(), t.clone())); } }
        if let Some(br) = &step.branch {
            for t in br.then.iter().chain(br.r#else.iter()) { edges.insert((step.id.clone(), t.clone())); }
        }
        // inferred data edges: X -> step for every steps.X reference in this step
        for tmpl in collect_templates_for_step(step) {
            for referenced in scan_step_refs(&tmpl) {
                if ids.contains(referenced.as_str()) && referenced != step.id {
                    edges.insert((referenced, step.id.clone()));
                }
            }
        }
    }
    edges.into_iter().collect()
}
```

Add `validate_graph` (targets-exist, no-self, orchestration-shape, join-count, cycle via Kahn's/DFS) and call it from `Workflow::parse` **only when the workflow has explicit edges** (so legacy forward-only checks stay for edge-free workflows):

```rust
fn workflow_has_explicit_edges(wf: &Workflow) -> bool {
    wf.steps.iter().any(|s| !s.next.is_empty() || s.split.is_some() || s.join.is_some())
}
fn validate_graph(wf: &Workflow) -> Result<(), WorkflowParseError> {
    let ids: std::collections::BTreeSet<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();
    for step in &wf.steps {
        // orchestration nodes carry no work
        let is_orch = step.split.is_some() || step.join.is_some();
        let has_work = step.agent.is_some() || step.action.is_some() || step.for_each.is_some()
            || step.parallel.is_some() || step.panel.is_some() || step.branch.is_some() || step.approval.is_some();
        if is_orch && has_work {
            return Err(WorkflowParseError::OrchestrationNodeHasWork {
                step: step.id.clone(),
                kind: if step.split.is_some() { "split".into() } else { "join".into() },
            });
        }
        if let Some(Join { wait: JoinWait::Count { count } }) = &step.join { if *count < 1 {
            return Err(WorkflowParseError::JoinCountInvalid { step: step.id.clone() });
        }}
        for t in step.next.iter().chain(step.split.iter().flatten()) {
            if t == &step.id { return Err(WorkflowParseError::EdgeSelfLoop { step: step.id.clone() }); }
            if !ids.contains(t.as_str()) {
                return Err(WorkflowParseError::EdgeTargetUnknown { step: step.id.clone(), target: t.clone() });
            }
        }
    }
    // cycle detection over the full dependency graph (Kahn's algorithm)
    let edges = workflow_edges(wf);
    let mut indeg: std::collections::BTreeMap<&str, usize> = wf.steps.iter().map(|s| (s.id.as_str(), 0)).collect();
    let mut adj: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (a, b) in &edges { *indeg.entry(b.as_str()).or_insert(0) += 1; adj.entry(a.as_str()).or_default().push(b.as_str()); }
    let mut ready: Vec<&str> = indeg.iter().filter(|(_, &d)| d == 0).map(|(&k, _)| k).collect();
    let mut seen = 0usize;
    while let Some(n) = ready.pop() {
        seen += 1;
        for &m in adj.get(n).map(|v| v.as_slice()).unwrap_or(&[]) {
            let e = indeg.get_mut(m).unwrap(); *e -= 1; if *e == 0 { ready.push(m); }
        }
    }
    if seen != wf.steps.len() {
        let stuck: Vec<&str> = indeg.iter().filter(|(_, &d)| d > 0).map(|(&k, _)| k).collect();
        return Err(WorkflowParseError::WorkflowCycle { path: stuck.join(" -> ") });
    }
    Ok(())
}
```

In `Workflow::parse`, after the existing per-step checks and before returning `Ok`, add:

```rust
    if workflow_has_explicit_edges(&wf) {
        validate_graph(&wf)?;
    }
```

(Leave the existing `validate_branch_targets` / `validate_template_refs` for edge-free workflows; a graph workflow additionally runs `validate_graph`. Confirm the sample workflows — all edge-free — are unaffected.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p rupu-orchestrator` — new tests pass; full crate green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/workflow.rs
git commit -m "feat(orch): dependency graph + DAG/cycle/edge/shape validation"
```

---

### Task 3: Language — the runtime gate (`is_nonlinear` → clear error)

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` — `pub fn is_nonlinear`
- Modify: `crates/rupu-orchestrator/src/runner.rs` — gate in `run_workflow` (~537-680, before `run_steps_inner`)
- Test: `runner.rs` tests + `workflow.rs` tests

**Interfaces:**
- Consumes: `workflow_edges` (Task 2).
- Produces: `pub fn is_nonlinear(&Workflow) -> bool`; a `RunWorkflowError::NonlinearNotYetSupported` variant returned before execution.

- [ ] **Step 1: Write the failing tests**

```rust
// workflow.rs
#[test]
fn is_nonlinear_is_false_for_every_sample_workflow() {
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.rupu/workflows")).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) != Some("yaml") { continue; }
        let wf = Workflow::parse(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(!is_nonlinear(&wf), "{:?} should be linear-runnable", p.file_name().unwrap());
    }
}
#[test]
fn is_nonlinear_true_for_split_and_for_a_fork() {
    let sp = Workflow::parse("name: w\nsteps:\n  - id: s\n    split: [a, b]\n  - id: a\n    agent: x\n    prompt: p\n  - id: b\n    agent: x\n    prompt: p\n").unwrap();
    assert!(is_nonlinear(&sp));
    let fork = Workflow::parse("name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n    next: [b, c]\n  - id: b\n    agent: x\n    prompt: p\n  - id: c\n    agent: x\n    prompt: p\n").unwrap();
    assert!(is_nonlinear(&fork));
}
```
Plus a `runner.rs` test that `run_workflow` on a `split` workflow returns `NonlinearNotYetSupported` (adapt the crate's existing runner-test harness / MockProvider).

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-orchestrator is_nonlinear` → FAIL (undefined).

- [ ] **Step 3: Implement**

```rust
/// True iff this workflow needs the Phase 2 DAG scheduler: it uses split/join,
/// OR its edge graph has a fork (a node with >1 non-branch successor) or a
/// reconverge (a node with >1 inbound edge). A plain linear chain — even with
/// explicit `next` that just restates list order, and today's branches — is
/// false (the existing linear runner executes it faithfully).
pub fn is_nonlinear(wf: &Workflow) -> bool {
    if wf.steps.iter().any(|s| s.split.is_some() || s.join.is_some()) { return true; }
    // fork: a non-branch step whose `next` has >1 target
    if wf.steps.iter().any(|s| s.branch.is_none() && s.next.len() > 1) { return true; }
    // reconverge: any node with >1 inbound control edge (next/split/branch arms)
    let mut indeg: std::collections::BTreeMap<&str, usize> = Default::default();
    for s in &wf.steps {
        for t in s.next.iter().chain(s.split.iter().flatten())
            .chain(s.branch.iter().flat_map(|b| b.then.iter().chain(b.r#else.iter()))) {
            *indeg.entry(t.as_str()).or_insert(0) += 1;
        }
    }
    indeg.values().any(|&d| d > 1)
}
```

In `runner.rs`, add the `RunWorkflowError` variant and gate at the top of `run_workflow` (before any run-record work that assumes linear execution — put it right after the workflow is available):

```rust
    if rupu_orchestrator::workflow::is_nonlinear(&opts.workflow) {
        return Err(RunWorkflowError::NonlinearNotYetSupported {
            name: opts.workflow.name.clone(),
        });
    }
```

Add to `RunWorkflowError`:

```rust
    #[error("workflow `{name}` uses non-linear orchestration (split/join/fork), which requires the DAG scheduler — not yet available (Phase 2)")]
    NonlinearNotYetSupported { name: String },
```

(Use the crate's real error type/path for the runner; if `is_nonlinear` is in `workflow`, reference it correctly. Legacy/linear workflows — including branches and a linear chain with explicit `next` — pass the gate and run exactly as before.)

- [ ] **Step 4: Run to verify pass** — `cargo test -p rupu-orchestrator` full green (esp. the all-samples-linear test).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/workflow.rs crates/rupu-orchestrator/src/runner.rs
git commit -m "feat(orch): runtime gate — non-linear workflows error until the Phase 2 scheduler"
```

---

### Task 4: Editor — parse/serialize/derive edges from explicit connections

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` — `StepNodeData`, `yamlToGraph`, `deriveEdges`, `nodeToStepObject`
- Test: `crates/rupu-cp/web/src/lib/workflowGraph.test.ts`

**Interfaces:**
- Produces: `StepNodeData` gains `next?: string[]`, `split?: string[]`, `joinWait?: 'all' | 'any' | { count: number }`; `deriveEdges` sources edges from explicit connections + inferred data refs; `nodeToStepObject` emits `next`/`split`/`join`; a `hasExplicitEdges(nodes)` helper for the legacy/graph distinction. Tasks 5-7 consume these.

- [ ] **Step 1: Write the failing tests**

```ts
describe('explicit-edge model', () => {
  it('derives edges from next: not from list order', () => {
    // two steps, NO next → no chain edge (graph mode is off, but the key change:
    // when next IS present, order does not add edges). Use an explicit-next case:
    const g = yamlToGraph({ name: 'w', steps: [
      { id: 'a', agent: 'x', prompt: 'p', next: ['c'] },
      { id: 'b', agent: 'x', prompt: 'p' },     // no next → terminal in graph mode
      { id: 'c', agent: 'x', prompt: 'p' },
    ]});
    const e = deriveEdges(g.nodes);
    expect(e).toContainEqual(expect.objectContaining({ source: 'a', target: 'c' }));
    // a→b is NOT an edge just because b follows a in the list
    expect(e.some((x) => x.source === 'a' && x.target === 'b')).toBe(false);
  });

  it('a legacy workflow (no explicit edges) still shows the linear chain', () => {
    const g = yamlToGraph({ name: 'w', steps: [
      { id: 'a', agent: 'x', prompt: 'p' }, { id: 'b', agent: 'x', prompt: 'p' },
    ]});
    expect(deriveEdges(g.nodes)).toContainEqual(expect.objectContaining({ source: 'a', target: 'b' }));
  });

  it('a data reference infers an edge', () => {
    const g = yamlToGraph({ name: 'w', steps: [
      { id: 'a', agent: 'x', prompt: 'p', next: ['z'] },
      { id: 'b', agent: 'x', prompt: 'use {{ steps.a.output }}' },
      { id: 'z', agent: 'x', prompt: 'p' },
    ]});
    expect(deriveEdges(g.nodes)).toContainEqual(expect.objectContaining({ source: 'a', target: 'b' }));
  });

  it('round-trips next/split/join', () => {
    const input = { name: 'w', steps: [
      { id: 's', split: ['a', 'b'] },
      { id: 'a', agent: 'x', prompt: 'p', next: ['j'] },
      { id: 'b', agent: 'x', prompt: 'p', next: ['j'] },
      { id: 'j', join: { wait: 'all' } },
    ]};
    const out = graphToWorkflowObject(yamlToGraph(input)) as { obj: any };
    expect(out.obj.steps.find((s: any) => s.id === 's').split).toEqual(['a', 'b']);
    expect(out.obj.steps.find((s: any) => s.id === 'j').join).toEqual({ wait: 'all' });
  });
});
```

- [ ] **Step 2: Run to verify failure** — the derive/round-trip tests fail (order still drives edges; new fields not modeled).

- [ ] **Step 3: Implement**

- `StepNodeData` (workflowGraph.ts) gains `next?: string[]`, `split?: string[]`, `joinWait?: 'all' | 'any' | { count: number }`, and a `kind` value each for `'split'`/`'join'` (extend `StepKind`).
- `yamlToGraph`/`parseStepData`: parse `next`/`split`/`join` into those fields; classify `kind` as `split` when `split` present, `join` when `join` present.
- **`deriveEdges` flip** — this is the core change. Compute `hasExplicitEdges(nodes)` = any node has `next`/`split`/`join`. Then:
  - **Graph mode** (`hasExplicitEdges`): edges = per-node `next` + `split` targets + branch `then`/`else` + inferred data-ref edges (keep the existing `extractStepRefs` logic). **Do NOT add the consecutive-order chain.**
  - **Legacy mode** (no explicit edges): keep today's behavior exactly — consecutive chain + data refs + branch arms (so old workflows display unchanged).
- `nodeToStepObject`: emit `next` (if non-empty), `split` (if the node is a split), `join: { wait }` (if the node is a join). Omit when empty (legacy round-trips clean).
- Export `hasExplicitEdges(nodes)`.

- [ ] **Step 4: Run to verify pass** — `npx vitest run src/lib/workflowGraph.test.ts` green; the pre-existing round-trip suite still passes (legacy path unchanged).

- [ ] **Step 5: Commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/lib/workflowGraph.ts src/lib/workflowGraph.test.ts
git commit -m "feat(cp): editor edges come from explicit next/split/join + inferred data refs"
```

---

### Task 5: Editor — draw-to-connect, drop-disconnected, replace-on-regular

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.tsx` — `applyConnect`, `applyDelete`, `applyRemoveEdges`, `newNodeData`
- Test: `WorkflowEditorGraph.test.tsx`

**Interfaces:**
- Consumes: `deriveEdges`/`withDerivedEdges` (Task 4 + earlier work).
- Produces: connecting sets `next`; a fresh node has empty `next` (disconnected); branch arms unchanged.

- [ ] **Step 1: Write the failing tests**

```ts
describe('explicit connect/drop', () => {
  it('drawing a plain edge sets next; a second draw from a regular node REPLACES it', () => {
    let g = yamlToGraph({ name: 'w', steps: [
      { id: 'a', agent: 'x', prompt: 'p', next: [] }, { id: 'b', agent: 'x', prompt: 'p' }, { id: 'c', agent: 'x', prompt: 'p' },
    ]});
    applyConnect(g, { source: 'a', target: 'b', sourceHandle: null }, (ng) => (g = ng), () => {});
    expect(g.nodes.find((n) => n.id === 'a')!.data.next).toEqual(['b']);
    applyConnect(g, { source: 'a', target: 'c', sourceHandle: null }, (ng) => (g = ng), () => {});
    expect(g.nodes.find((n) => n.id === 'a')!.data.next).toEqual(['c']); // replaced, not [b, c]
  });

  it('a split node accumulates edges instead of replacing', () => {
    let g = yamlToGraph({ name: 'w', steps: [{ id: 's', split: [] }, { id: 'a', agent:'x', prompt:'p' }, { id: 'b', agent:'x', prompt:'p' }] });
    applyConnect(g, { source: 's', target: 'a', sourceHandle: null }, (ng) => (g = ng), () => {});
    applyConnect(g, { source: 's', target: 'b', sourceHandle: null }, (ng) => (g = ng), () => {});
    expect(g.nodes.find((n) => n.id === 's')!.data.split).toEqual(['a', 'b']);
  });

  it('a freshly added node is disconnected (no next)', () => {
    const r = applyAddNode(yamlToGraph({ name: 'w', steps: [{ id: 'a', agent:'x', prompt:'p', next: [] }] }), 'step');
    expect(r.graph.nodes.find((n) => n.id === r.id)!.data.next ?? []).toEqual([]);
  });
});
```

- [ ] **Step 2: Run to verify failure** — connect still creates a stored plain edge / reorder (earlier model), not a `next`.

- [ ] **Step 3: Implement**

- `applyConnect`: for a **branch** arm handle → unchanged (then/else). For a **split** source → append target to `split` (accumulate). For a **regular** source → set `next = [target]` (replace any existing single successor). Then return `withDerivedEdges`.
- `applyDelete`: also strip the deleted id from every node's `next`/`split`/branch arms (scrub), then `withDerivedEdges`.
- `applyRemoveEdges`: removing a plain edge clears that `next`/`split` entry (not a reorder); branch arm removal unchanged.
- `newNodeData`: a fresh node has `next: []` (or leave undefined → treated as empty); ensure it is NOT auto-linked. (This changes the earlier "plain connect = reorder" behavior from the single-source work — it's superseded by explicit edges.)

- [ ] **Step 4: Run to verify pass** — `npx vitest run src/components/workflow-editor/` green. Update any earlier test that asserted plain-connect-reorders to the new set-`next` behavior (do not delete; rewrite to the new model, note in report).

- [ ] **Step 5: Commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/WorkflowEditorGraph.tsx src/components/workflow-editor/WorkflowEditorGraph.test.tsx
git commit -m "feat(cp): draw sets an explicit edge; drop leaves a node unconnected"
```

---

### Task 6: Editor — `split`/`join` nodes, shapes, work/orchestration palette

**Files:**
- Modify: `kindVisuals.ts`, `nodeShapes.ts`, `nodeShapes.test.ts`, `StepForm.tsx`, `NodePalette.tsx` + tests
- Test: as above

**Interfaces:**
- Consumes: `StepKind` gains `'split'`/`'join'` (Task 4).
- Produces: `KIND_ACCENT`/`KIND_ICON`/`KIND_SHAPE` entries for `split`/`join`; placeholder silhouettes; palette entries under a work/orchestration grouping; `StepForm` bodies.

- [ ] **Step 1: Write the failing tests**

`kindVisuals.test.ts`: `KIND_SHAPE.split` and `.join` are defined (placeholder shape names). `nodeShapes.test.ts`: the two new shapes pass the point-in-polygon + simplicity + on-outline anchor `it.each(ALL)` checks. `NodePalette.test.tsx`: `split` and `join` chips render under an "orchestration" group when `workflowEditorUi==='next'`. `StepForm.test.tsx`: a `join` node renders a wait-policy control (all/any/count); a `split` node renders its target list.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement**

- `kindVisuals.ts`: `KIND_ACCENT`/`KIND_ICON`/`KIND_SHAPE` gain `split`/`join` (use a distinct orchestration accent; lucide `Split`/`Merge` icons; shape names `'fanout'`/`'fanin'`). Add a `KIND_FAMILY: Record<StepKind, 'work' | 'orchestration'>` map (work: step/action/for_each/parallel/panel; orchestration: branch/split/join/approval_gate).
- `nodeShapes.ts`: add `'fanout'` (a right-fanning pentagon — points fan to the right) and `'fanin'` (left-fanning) placeholder silhouettes, following the existing shape conventions (path/safe/align/anchors, satisfy the geometry tests). These are placeholders; a later pass refines them.
- `NodePalette.tsx`: add `split`/`join` block items; group the rail chips by `KIND_FAMILY` with small "Work" / "Orchestration" subheadings.
- `StepForm.tsx`: `SplitFields` (edit the target id list, or hint "draw from this node") and `JoinFields` (a `<select>` all/any/count + a count number input when count).

- [ ] **Step 4: Run to verify pass** — `npx vitest run src/components/workflow-editor/` green.

- [ ] **Step 5: Commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add -A src/components/workflow-editor/
git commit -m "feat(cp): split/join orchestration nodes + work/orchestration palette"
```

---

### Task 7: Editor — graph validation surfaced inline

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` — `validateGraph`
- Test: `workflowGraph.test.ts`

**Interfaces:**
- Consumes: `deriveEdges`, `hasExplicitEdges` (Task 4).
- Produces: `validateGraph` flags a cycle, an unknown edge target, and a `split`/`join` with degree < 2, mirroring the backend `validate_graph` so the editor warns before the server does.

- [ ] **Step 1: Write the failing tests**

```ts
it('validateGraph flags a cycle', () => {
  const g = yamlToGraph({ name: 'w', steps: [
    { id: 'a', agent: 'x', prompt: 'p', next: ['b'] }, { id: 'b', agent: 'x', prompt: 'p', next: ['a'] },
  ]});
  expect(Object.values(validateGraph(g)).flat().some((m) => /cycle/i.test(m))).toBe(true);
});
it('validateGraph flags an unknown edge target', () => {
  const g = yamlToGraph({ name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p', next: ['ghost'] }] });
  expect(Object.values(validateGraph(g)).flat().some((m) => /ghost|unknown|not a known/i.test(m))).toBe(true);
});
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** — in `validateGraph`, when `hasExplicitEdges`, run a client-side cycle detection (Kahn's over `deriveEdges`) → add a "cycle through …" problem to the involved nodes; flag `next`/`split` targets that aren't known ids; flag a `split`/`join` with < 2 edges. Match the message style the editor already uses.

- [ ] **Step 4: Run to verify pass** — `npx vitest run src/components/workflow-editor/ src/lib/ && npx vitest run` (full), `npx tsc -b --noEmit`, `npm run build`.

- [ ] **Step 5: Commit**

```bash
git add src/lib/workflowGraph.ts src/lib/workflowGraph.test.ts
git commit -m "feat(cp): inline cycle + edge validation in the editor"
```

---

## Operator gate (before merge)
matt validates in the running app (light + dark): drop a node → it connects to nothing; draw a line → it connects, and a second line from a regular node replaces the first; author a `split` → two lines fan out; author a `join` with a wait policy; a cycle shows an inline error; **running a `split`/`join` workflow gives the clear "requires the Phase 2 scheduler" error, not a mis-run**; every existing workflow still loads and runs unchanged.

## Self-review notes
- Spec §1-§6 map to tasks: §1 language → T1/T2; §2 validation → T2; §6 runtime gate → T3; §3 editor edges → T4/T5; §3c split/join+shapes → T6; §2 validation surfaced → T7. Legacy compat (§4) is tested in T1/T3/T4.
- `next`/`split`/`join`/`joinWait` names are consistent across the Rust wire shape (T1) and the TS model (T4).
- The `deriveEdges` flip (T4) supersedes the earlier "plain-connect = reorder" behavior (T5) — noted so the reviewer expects the changed test.
