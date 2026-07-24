# Non-linear orchestration for rupu — a proposal

**Date:** 2026-07-24
**Status:** Proposal for discussion. No implementation; this is the "think it through" document the operator asked for.
**Scope of the idea:** the workflow **language** (`crates/rupu-orchestrator/src/workflow.rs`), the **runner/engine** (`runner.rs`), and the **editor** (`crates/rupu-cp/web`). Grounded in two code investigations of the current engine (cited inline).

---

## 1. Where we are (verified, not assumed)

The engine is **linear all the way down**:

- **Order is list position.** A plain step has no successor field at all; `Workflow.steps: Vec<Step>` is walked by one flat loop — `for step in &opts.workflow.steps` (`runner.rs:991`). There is one execution cursor.
- **`branch` is not a jump — it's a skip-filter.** `then`/`else` are *forward-only sets of ids to exclude on the not-taken side* (`validate_branch_targets` forces `target_idx > idx`, `workflow.rs:1554`). A branch dispatches nothing and advances no cursor; the loop still goes to `steps[i+1]`, and each later step is skipped if it's in the branch's skip-set (`runner.rs:1025`). `when:` uses the identical skip mechanism.
- **`parallel`/`panel`/`for_each` fan out *inside one node* and join internally.** This is the one place with real concurrency — `tokio::spawn` + `Semaphore(max_parallel)` + a `for handle in handles { .await }` join (`runner.rs:3168`, `3216`). But the sub-units aren't addressable graph nodes; the container is a single entry in the top-level list, and the outer loop resumes at `steps[i+1]` after the internal join.
- **State is a single cursor.** Resume rebuilds one `already_done` set from a flat append-only `step_results.jsonl`, with at most one `paused_step` (`runner.rs:965-1018`, `ResumeState`). Nothing models multiple concurrently-active positions.
- **Cycles are impossible by construction** — every cross-step reference (`branch` targets, `steps.X` template refs) is forced monotonic against the one ordered list, so there's no cycle *detector*; there's simply no way to form one.

**The consequence you hit:** because flow == list order, dropping a node "connects" it to its neighbour implicitly, and there's no way to express two divergent paths, concurrent tracks, or a reconverge. The language has no edges to author.

## 2. The shift: connections become explicit data

The single change everything else hangs off: **a step declares its own outgoing connections; flow is the edge set, not the list order.**

```yaml
steps:
  - id: triage
    agent: triager
    next: [assess]              # explicit successor

  - id: assess
    branch:
      condition: "{{ steps.triage.output.severity == 'high' }}"
      then: [page_oncall]       # then/else are conditional edges
      else: [file_ticket]

  - id: page_oncall
    action: pagerduty.trigger
    next: [postmortem]

  - id: file_ticket
    action: issues.create
    next: [postmortem]

  - id: postmortem            # two inbound edges → runs once, after both paths that reach it
    agent: writer
```

- **`next: [ids]`** is the explicit edge. Drawing a line in the editor sets `next`; deleting it clears it; **a freshly-dropped node has empty `next` — connected to nothing** until you wire it, exactly as you asked.
- **Branch becomes real routing.** `then`/`else` are conditional edges; the *not-taken subgraph is pruned* (genuinely not run), instead of skip-marked on a linear pass.
- **Backward compatibility (the load-bearing requirement):** a workflow with **no** explicit edges anywhere is interpreted exactly as today — implicit list order — so every existing `.yaml` runs unchanged. The moment any `next`/`split`/`join` appears, the workflow is an explicit graph. The editor, on first graph-edit of a legacy workflow, **materialises the implicit chain into explicit `next`** (each step → the next in the list) and from then on authors edges explicitly. One-way, lossless, and it matches how the editor already re-emits YAML.

## 3. Two edge sources — and why you'll author fewer of them than you'd think

There are two ways one node can depend on another, and the scheduler should honour **both**:

1. **Control edges** — explicit `next` (and branch `then`/`else`). "Run B after A because I said so."
2. **Data edges — inferred.** If B's prompt/condition references `{{ steps.A.output }}`, B *cannot* run before A. The language already tracks these references (`validate_template_refs`, `workflow.rs:1596`) — today only as a forward-only lint. **Promote them to real dependency edges:** referencing another step's output automatically orders you after it.

The union of the two is the dependency graph. In practice this means: for the common "pipeline" case you often write *no* `next` at all — the data flow defines the order — and you reach for explicit `next` only for pure control ordering (a step that must follow another without using its data) or for fork/branch. This is the Bazel/Nix idea (dependencies are discovered from use), and it keeps hand-authored YAML terse while staying fully explicit and inspectable.

## 4. The node taxonomy: work vs orchestration

Your framing — "these might not be agentic nodes, they're orchestration nodes" — is the right axis. Formalise it:

| family | node | does |
|---|---|---|
| **Work** | `step` | one agent |
| | `action` | one connector call (SCM/issues/CI) |
| | `for_each` | map one agent over a list (fan-out + aggregate, internal) |
| | `parallel` | run N agents on the same subject, aggregate (fan-out + join, internal) |
| | `panel` | N reviewers + a fix/gate loop (internal) |
| **Orchestration** | `branch` | conditional route (diamond) |
| | **`split`** | **fan the flow into N concurrent paths** ← new |
| | **`join`** | **barrier: wait for N inbound paths** ← new |
| | `approval_gate` | human hold |

**`split` vs `parallel` — the distinction you're drawing.** `parallel` is a *container*: one node runs N sub-agents on one thing and collapses them into one result, then flow continues from that single node. `split` is *structural*: it forks the graph into independent tracks that each continue their own way and may never rejoin (or rejoin at different points). You wanted the latter — "split the node into multiple paths to continue working on multiple paths." So:

```yaml
  - id: fan
    split: [build_ios, build_android, build_web]   # three independent tracks start here

  - id: build_ios      { agent: ios,     next: [ship] }
  - id: build_android  { agent: android, next: [ship] }
  - id: build_web      { agent: web,     next: [ship] }

  - id: ship
    join: { of: [build_ios, build_android, build_web], wait: all }   # barrier
    action: release.publish
```

**`join` policy** — `wait: all` (default; every inbound path must finish), `wait: any` (first one wins, cancel the rest), `wait: { count: 2 }` (k-of-n). This is your "wait for multiple agents to be done," made a first-class, visible node.

**Do you always need an explicit `join`?** No — a *regular* node with multiple inbound edges implicitly waits for all of them (standard DAG semantics), so the `postmortem` example in §2 needs no join node. The explicit `join` node earns its place when you want a non-default policy (`any`/`count`) or a barrier you can *see* in the graph. (Open decision D2 below: whether implicit all-join is the default, or we require an explicit join for every reconverge. I lean implicit-default + explicit-when-you-want-it.)

## 5. The engine: from a loop to a scheduler

Replace the `for step in steps` loop in `run_steps_inner` with a **ready-set scheduler** — and reuse the machinery that already exists:

- Build the dependency graph (control edges ∪ data edges). Validate it's a DAG (topological sort; reject cycles — a *real* detector now, since edges are no longer monotonic-by-construction).
- A node is **ready** when its inbound dependencies are satisfied: a regular node when all predecessors are done; a `join` when its policy is met; a `branch` selects which successors become eligible and **prunes** the rest.
- Run all ready nodes **concurrently**, dispatched through the *same* `dispatch_one` / `StepFactory` boundary that exists today (`runner.rs:3336`) — the agent-running code doesn't change. Bound concurrency with a workflow-scope semaphore, exactly the `parallel`-step pattern (`Semaphore` + spawn + join) **promoted from inside one step to the whole graph**. The investigation's own conclusion: this is the natural slot-in point, and the concurrency primitive is already written.
- The **executor boundary is unchanged** — it still spawns one task per run and fans events; only the sequencing inside `run_workflow` changes.

**Resume is the real work.** Today's single-cursor `already_done` + one `paused_step` becomes **per-node state**: done / running / paused / pruned, with multiple nodes possibly in-flight or awaiting approval at once. `step_results.jsonl` already keys by `step_id`, so the ready-set rebuilds cleanly from completed nodes; the `completed_units` per-unit checkpoint pattern generalises to per-node. Pause/cancel must handle several live nodes rather than one. This is the part to design carefully and phase.

## 6. Other improvements worth folding in (you asked me to think)

- **Guarded edges, generalised.** Let *any* edge carry a `when:` guard, not just `branch`. `branch` then becomes readable sugar for "a node with two guarded edge-groups." Enables conditional routing without a full branch node.
- **First-class error paths.** Today failure handling is a bool (`continue_on_error`) plus the gate's `on_reject`. Add an `on_error: [node]` edge so a failure routes to a handler/cleanup/notify path — a proper exception edge in the graph.
- **Explicit entry & terminal.** Nodes with no inbound are entry points (a forked graph can have several); no outbound are terminals. Surface them (optionally explicit `start`/`end`) so "where does this begin" is never ambiguous.
- **Workflow-scope concurrency budget.** A top-level `max_concurrency` cap across all live paths (the per-step semaphore, promoted).
- **Sub-workflows (later).** Call another workflow as a node — composition once the graph model exists.
- **Bounded loops (later, deliberately deferred).** Real edges make back-edges *possible*, which means loops (retry-until, iterate). But unbounded loops in an agentic system are dangerous, so I'd keep the core graph **acyclic** with cycle detection, and add loops only as an explicit, bounded construct (`loop: { until, max_iterations }`) — mirroring the panel gate's existing bounded-retry discipline — rather than by allowing raw back-edges.
- **Map over a subgraph (later).** Generalise `for_each` from "map one agent" to "run this subgraph per item, then join" — fan-out of whole pipelines.

## 7. Migration & compatibility (non-negotiable)

- Legacy workflows (no edges) run **byte-for-byte identically** — the scheduler with an all-linear dependency graph *is* the old loop.
- `branch`'s current forward-only skip semantics map onto pruning; existing branch workflows keep working, and the editor migrates them to real edges on first edit.
- The editor's `deriveEdges` (Phase 1) flips its source: from "consecutive list order" to "explicit `next` + branch + inferred data edges." Drawing sets `next`; dropping leaves a node unconnected — the behaviours you asked for fall straight out.
- Behind the existing `[cp].workflow_editor_ui = 'next'` flag for the editor; the engine change ships when the language does, with the legacy path preserved.

## 8. Phasing (each independently shippable)

1. **Language + validation.** Add `next`, `split`, `join`, `on_error`; DAG/cycle validation; data-edge inference; legacy-linear compatibility. No runtime change yet (parser + validator + a graph model).
2. **Editor.** Author explicit edges (draw/clear/replace), the `split`/`join` nodes, drop-is-disconnected; `deriveEdges` from explicit edges. This is where you *feel* the change, and it's independent of the engine.
3. **Scheduler.** Replace the linear loop with the ready-set scheduler; workflow-scope concurrency; branch pruning; **per-node resume/pause/cancel** (the hard part).
4. **The extras** (error edges, guarded edges, concurrency budget), then the deferred set (loops, sub-workflows, subgraph-map).

## 9. Open decisions I want your call on

- **D1 — Edge direction.** `next:` (successor, matches how you draw and how `branch` already works) vs `depends_on:` (predecessor, makes joins self-describing). I lean **`next:`** for authoring consistency, with joins reading their inbound set from the graph. *(This proposal is written in `next:`.)*
- **D2 — Implicit join.** A regular node with several inbound edges: **implicitly wait for all** (my lean — least surprising, terse) vs **require an explicit `join`** for every reconverge (more ceremony, more visible). Explicit `join` still exists either way for `any`/`count`.
- **D3 — Data-edge inference.** Auto-order from `steps.X` references (my lean — terse, powerful) vs edges must be 100% explicit (more verbose, zero magic). Inference can always be *overridden* by an explicit edge.
- **D4 — Loops.** Deferred bounded `loop` construct (my lean) vs allow raw back-edges now (riskier).
- **D5 — Scope now.** Do we do the full arc, or land Phase 1+2 (language + editor, so you can *author* non-linear flows) before committing to the Phase 3 scheduler rewrite?
