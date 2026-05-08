# rupu Sub-Agent Dispatch — Design

**Status:** Draft (design phase)
**Date:** 2026-05-08
**Companion docs:** [Slice A design](./2026-05-01-rupu-slice-a-design.md), [Slice B-1 design](./2026-05-02-rupu-slice-b1-multi-provider-design.md)
**Backlog entry:** [TODO.md → Sub-agent dispatch](../../../TODO.md)

---

## 1. What this is

A new builtin tool family that lets one agent invoke another agent as a tool call within its own run — synchronously or in parallel — and fold the child's output back into its own context. Today, agents only compose at the workflow layer (one step = one agent, or panelists in a `panel:`). This design adds composition at the agent layer.

The motivating shape: a "writer" agent dispatches a `security-reviewer` and a `perf-reviewer` mid-draft, reads their findings, revises the draft, and continues — without the workflow author having to fan out at the YAML level. Closer to the Claude Code agent-as-tool pattern and the Okesu meeting-step delegation model.

## 2. Why a separate spec

Sub-agent dispatch crosses every internal boundary that the existing six builtins respect cleanly:

- **rupu-tools** — adds two new builtins (`dispatch_agent`, `dispatch_agents_parallel`).
- **rupu-agent** — extends agent-frontmatter `tools:` to accept agent names alongside builtin names; recursion/depth limits live here.
- **rupu-orchestrator** — child runs need run-id assignment, transcript paths, and a parent→child link in the run record so workflow runs index sub-agent transcripts correctly.
- **rupu-cli/output** — line-stream printer renders dispatched runs as indent+1 child callout frames inside the parent's frame.
- **rupu-providers** — no changes; child runs use the same provider plumbing.

A single design captures the contract between layers so each can evolve independently and so we don't re-architect later when (e.g.) Slice E rupu.cloud needs to know what a sub-agent run is.

## 3. The shape of a dispatch

### 3.1 New tools

Two builtin tools, both gated through the existing per-tool allowlist + permission-mode model.

#### `dispatch_agent`

Runs one child agent synchronously. Returns the child's final assistant text plus optional structured findings.

```json
{
  "tool": "dispatch_agent",
  "input": {
    "agent": "security-reviewer",
    "prompt": "Review this diff for auth issues:\n<diff body>",
    "inputs": { "subject": "auth.rs:42" }
  }
}
```

**Output shape (returned to the parent agent):**

```json
{
  "ok": true,
  "agent": "security-reviewer",
  "output": "<child's final assistant text>",
  "findings": [
    { "severity": "high", "title": "...", "body": "...", "source": "security-reviewer" }
  ],
  "tokens_used": 1842,
  "duration_ms": 4123,
  "transcript_path": "/Users/matt/.rupu/runs/run_PARENT/sub/<sub_run_id>.jsonl"
}
```

`findings` is populated when the child's agent has `outputFormat: json` and emits a parseable findings array (same logic as panel panelists today). For non-JSON agents it's empty.

#### `dispatch_agents_parallel`

Fans out N child agents concurrently. Mirrors the workflow-layer `parallel:` shape but at the agent layer.

```json
{
  "tool": "dispatch_agents_parallel",
  "input": {
    "agents": [
      { "id": "sec", "agent": "security-reviewer", "prompt": "<diff>" },
      { "id": "perf", "agent": "perf-reviewer", "prompt": "<diff>" }
    ],
    "max_parallel": 2
  }
}
```

**Output shape:**

```json
{
  "ok": true,
  "results": {
    "sec": {
      "agent": "security-reviewer",
      "output": "...",
      "findings": [ ... ],
      "tokens_used": 921,
      "duration_ms": 2008,
      "transcript_path": "..."
    },
    "perf": {
      "agent": "perf-reviewer",
      "output": "...",
      "findings": [ ... ],
      "tokens_used": 1102,
      "duration_ms": 1953,
      "transcript_path": "..."
    }
  },
  "all_succeeded": true
}
```

`max_parallel` defaults to the number of agents (full parallelism).

### 3.2 Why two tools, not one

A single `dispatch_agent` tool that takes an optional `parallel: true` flag would be smaller surface area, but the input/output shapes diverge enough (`agent` vs `agents`, scalar return vs map return) that one tool would force every callsite into a sum-typed input. Two tools keeps each invocation's intent explicit at the call site, which is valuable when the parent agent is reasoning about whether to spawn a fan-out.

This also matches the workflow-layer split (`for_each:` vs `parallel:` vs single-step) — same conceptual primitive at a different layer.

## 4. Permissions

### 4.1 Per-parent allowlist

Sub-agent dispatch respects the existing per-tool allowlist mechanism. Parent agents declare which children they may dispatch via the same `tools:` frontmatter field that already gates the six v0 builtins:

```yaml
---
name: writer
tools: [read_file, dispatch_agent, dispatch_agents_parallel]
dispatchable_agents: [security-reviewer, perf-reviewer, maintainability-reviewer]
---
```

`dispatchable_agents` is a new frontmatter field. When `dispatch_agent` is invoked, the runtime checks the requested `agent` name against this list. Not in list → tool call fails with `dispatch_agent: agent 'X' is not in this agent's dispatchable_agents allowlist`. Empty list = can't dispatch anyone (defensive default).

This is symmetric with how the bash tool's `bashAllow:` works today: tools have argument-level allowlists, applied per agent.

### 4.2 Permission mode propagation

The child agent's permission mode is **its own**, not inherited from the parent. A `bypass`-mode parent dispatching a `readonly`-mode child gets a child run that respects the child's `readonly` constraint. This matches how panel panelists work today and keeps the per-agent-file permission contract honest.

If the parent and child both have `ask` mode, the user gets prompted in the child's run for child tool calls — the prompts thread through the parent's terminal session via the existing approval flow. Already works for panel panelists; we reuse the path.

### 4.3 Recursion + depth limits

Children can dispatch grandchildren, which can dispatch further. To prevent runaway, two limits ship in the runtime:

- **Max depth:** 5 by default. Configurable per-agent via `dispatch_max_depth: <n>` in frontmatter, capped at a workspace-config ceiling (default 8). Exceeded → tool call fails.
- **Max breadth:** No global cap. `dispatch_agents_parallel` is bounded by `max_parallel` and by the parent's `dispatchable_agents` list. A pathological agent that dispatches 100 children one-by-one will hit the per-run token budget before becoming a real problem.

Cycle detection: not needed at depth 5. If a writer dispatches a writer dispatches a writer at depth 5, the depth limit catches it.

## 5. Run state — parent ↔ child linkage

### 5.1 RunRecord changes

Every child run produces its own transcript at a deterministic path:

```
<global>/runs/<parent_run_id>/sub/<sub_run_id>.jsonl
```

`<sub_run_id>` is `sub_<ULID>`. The path lives under the parent's run directory (not at the top level) so cleanup follows parent lifecycle.

`RunRecord` gains an optional `parent_run_id: Option<String>` field. When `Some`, this is a sub-agent run; when `None`, top-level. Sub-runs don't appear in `rupu workflow runs` output by default (filtered out — they're internals); a `--include-sub` flag opts in.

### 5.2 step_results.jsonl

The parent's `step_results.jsonl` continues to track workflow-level steps. Sub-agent dispatches don't appear there — they're agent-level events, not workflow-level. The parent's transcript carries `tool_call` events for the dispatch and `tool_result` events for the return, just like any other tool.

The child's transcript is a normal JSONL stream identical in shape to a top-level run transcript. The line-stream printer can replay it as-is into the indent+1 child frame.

### 5.3 New `RunStore` operations

```rust
impl RunStore {
    pub fn create_sub_run(&self, parent_run_id: &str, agent: &str) -> Result<String, ...>;
    pub fn list_sub_runs(&self, parent_run_id: &str) -> Result<Vec<RunRecord>, ...>;
}
```

`create_sub_run` allocates a `sub_<ULID>` id, creates the sub directory, and writes a `RunRecord` with `parent_run_id` set. The runner uses the returned id for the child agent's run.

## 6. Tree-flow rendering

The line-stream printer already supports indent+1 child frames (PR #112 for `for_each` / `parallel` / `panel`). Sub-agent dispatch reuses this surface:

```
│
├─╭─ ◐ writer ──── (anthropic · sonnet)
│ ┃  Reviewing the diff... let me get a security check first.
│ ┃
│ ├─╭─ ◐ security-reviewer ──── (anthropic · sonnet)
│ │ ┃  Looking for auth issues...
│ │ ┃  Found one HIGH-severity finding: missing CSRF on /admin endpoint.
│ ├─╰─ ✓ done · 2.1s · 1.3k tokens · 1 finding
│ ┃
│ ┃  Got it — folding the CSRF finding into the draft.
│ ┃  ...
├─╰─ ✓ done · 12.4s · 4.7k tokens
│
```

The mechanism: the parent's transcript emits a `tool_call { tool: "dispatch_agent", ... }` event, the workflow_printer recognizes that tool, switches to "child frame mode" (push_indent + open child frame), tails the child's transcript inline, then closes the child frame on `tool_result`.

For `dispatch_agents_parallel`, the printer renders one child frame per concurrent run. Children print serially in declared order (by `id`) once they've all completed — same UX as the panel rendering. Live interleaved streaming during dispatch is a follow-up (same caveat as PR #112's fan-out rendering).

## 7. What does NOT change

- **Workflow YAML schema** — sub-agent dispatch is an agent-level mechanism, not a step shape. `for_each`, `parallel`, and `panel` continue to work as they do.
- **Provider plumbing** — child runs go through the same `rupu-providers` factory. Each child can use a different provider/model than its parent.
- **Tool registry** — the existing six builtins keep their identity. The two new tools join the registry; agents that don't list them in `tools:` can't invoke them (preserves least-privilege).
- **Transcript event vocabulary** — no new event kinds. `tool_call` + `tool_result` carry the dispatch payloads.

## 8. Implementation phases

This design suggests three plans, each landable as an independent PR:

**Plan 1 — `dispatch_agent` (single-child synchronous)**
- New tool in `rupu-tools` builtin registry.
- Frontmatter `dispatchable_agents:` field on `Agent`.
- Recursion-depth tracking through `AgentRunOpts`.
- `RunStore::create_sub_run` + `parent_run_id` on `RunRecord`.
- Line-stream printer recognizes the new tool's `tool_call` event and renders indent+1 child frame.
- Tests: unit (tool registration + permission gate) + integration (a parent runs, dispatches a mock child, parent sees child output).

**Plan 2 — `dispatch_agents_parallel` (fan-out)**
- Builds on Plan 1's infra.
- New tool with `agents: [{id, agent, prompt}]` input shape.
- Parallel dispatch via `tokio::join_all` with `max_parallel` semaphore (mirror `runner::run_parallel_step` shape).
- Printer renders N child frames in declared order.
- Tests: parallel dispatch with mixed-success children + recursion depth at the parallel layer.

**Plan 3 — Live per-child streaming (optional polish)**
- Today (post Plan 1+2): child frames render after the dispatch returns.
- Plan 3 enables live tailing: the printer attaches to each child's transcript file as it's written, streams events into the open child frame in real time.
- Mirror to PR #112's "live per-iteration streaming" follow-up — same problem class, same solution.

## 9. Open questions

These are flagged for design review before implementation begins.

### Q1. Should child agents see the parent's context?

**Today** (panel panelists, for_each iterations): each child runs with a fresh context. They get only their rendered prompt — no inherited memory.

**Sub-agent dispatch:** same model, or should the parent's conversation history flow into the child's system prompt? The Claude Code pattern is "fresh context per child." Going with that unless we have a reason not to.

**Recommendation:** fresh context. Parents can pass needed state via the prompt explicitly.

### Q2. Should `dispatch_agent` be a separate tool from regular bash?

I.e. is `dispatch_agent` discoverable from the agent's tool list, or do we hide it behind a flag?

**Recommendation:** discoverable. Listing it in `tools:` is the consent gesture; that's enough.

### Q3. What happens if a child's permission mode is `ask` and the parent is non-interactive?

`ask` requires a TTY. If the parent run is a cron-triggered or webhook-triggered workflow (no TTY), an `ask`-mode child has nowhere to ask.

**Recommendation:** error at dispatch time with a clear message: `child agent 'X' has permissionMode: ask but this run is non-interactive`. Parent can choose to gate dispatches with `when:` or use a non-`ask` child.

This matches the existing top-level rule for `ask` workflows — they error out cleanly when run non-interactively.

### Q4. Should `dispatch_agents_parallel` allow passing the parent's tool budget down?

If the parent has 50 turns left and dispatches 4 children in parallel, do they each get 50, or do they share?

**Recommendation:** each child has its own `maxTurns` (declared in its agent file). Parent's budget is unaffected by child runs except for the dispatch tool calls themselves (which count as one turn each, like any other tool).

This keeps the "each agent file is the contract for its own budget" invariant.

## 10. Glossary

- **Parent agent** — an agent whose run includes `dispatch_agent` / `dispatch_agents_parallel` tool calls.
- **Child agent** — an agent dispatched as a tool call from a parent. Its run is a sub-run.
- **Sub-run** — a `RunRecord` with `parent_run_id: Some(_)`. Lives under the parent's run directory.
- **Depth** — number of dispatch hops from a top-level workflow agent. Top-level run = depth 0; first child = depth 1; etc.
- **Dispatchable agents** — the per-parent allowlist of children, declared in the parent's `dispatchable_agents:` frontmatter field.
