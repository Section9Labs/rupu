# Step `actions:` — real permission narrowing + audit trail (design)

**Date:** 2026-07-26
**Status:** Design — approved by matt (semantics + scope locked in §8). Ready to plan.
**Scope:** `crates/rupu-orchestrator` (validation + the enforcement point), `crates/rupu-agent` (legacy protocol removal), `crates/rupu-mcp` + the agent tool-call path (the audit event), `crates/rupu-cp` (`/api/agents` tools exposure + the `actions:` editor).
**Out of scope:** growing the MCP tool catalog (the T3 tier — its own arc if wanted).

## 1. The problem (verified against the code)

There is **one authoritative action namespace** — the MCP tool catalog (`rupu_mcp::tools::tool_catalog()`, dispatched in `crates/rupu-mcp/src/dispatcher.rs`), 17 tools:

```
scm.repos.list      scm.repos.get
scm.branches.list   scm.branches.create
scm.files.read
scm.prs.list        scm.prs.get      scm.prs.diff    scm.prs.comment   scm.prs.create
issues.list         issues.get       issues.comment  issues.create     issues.update_state
github.workflows_dispatch
gitlab.pipeline_trigger
```

Each name routes into `rupu-scm`'s `Registry`, which wires **GitHub, GitLab, Jira, and Linear** (Jira/Linear as *issue* connectors, credential-gated — so `issues.*` is the cross-platform surface; `github.*`/`gitlab.*` are platform extras). The catalog is already served to the web at `GET /api/tools` (`crates/rupu-cp/src/api/tools.rs`).

Three surfaces reference that namespace. Two are real; one is orphaned:

| Surface | Status today |
|---|---|
| agent frontmatter `tools:` | **Real + enforced** — the runtime grant (`step_factory.rs` → `agent_tools: spec.tools`) |
| `action:` step (action blocks) | **Real + executes** — `execute_action_step` dispatches `step.action` through the in-process `ToolDispatcher`; `validate_action_step` (`workflow.rs:1317`) already validates the name against the catalog (`ActionUnknownTool`) **and** schema-checks `with:` against the tool's `input_schema` |
| step `actions:` | **Orphaned** — `validate_actions` (`action_protocol.rs`) has **no production caller** (only `tests/action_allowlist.rs`); `ActionValidator` is a bare unit struct with a stale *"Real impl lands in Task 11"* comment |

**Two vocabularies drifted apart.** The legacy protocol's verbs are Okesu-heritage (`open_pr`, `comment`, `delete_branch`, `file_write`, `shell_exec` — see `crates/rupu-agent/src/action.rs` + the allowlist tests), but authoring drifted to MCP names (`nightly-health.yaml`: `actions: ["issues.list", "issues.comment", "issues.create", "issues.update_state"]`). Neither is enforced, and nothing validates the strings — which is how `actions: ["",""]` can sit in a workflow unnoticed.

There is also **no UI**: no `actions:` field exists in either editor. `workflowGraph.ts` parses + re-emits it when non-empty and `StepForm.tsx:143` carries it across a kind-switch (so existing values survive an edit), but there is no way to author it from the CP.

## 2. Semantics (operator-locked)

**`actions:` is a narrowing filter over the agent's grant:**

```
effective_tools = agent.tools ∩ step.actions      when step.actions is NON-empty
effective_tools = agent.tools                     when step.actions is empty or absent
```

- A step can only ever **narrow**, never grant — it cannot give a step a tool the agent's frontmatter doesn't declare. Fail-closed by construction.
- **Empty means unrestricted** (the agent's grant stands). This is non-negotiable for compatibility: every existing workflow carries `actions: []` while relying on the agent's `tools:`; a deny-all reading would break all of them.

```yaml
# agent issue-reporter frontmatter: tools: [issues.list, issues.comment, issues.create, issues.update_state]
steps:
  - id: triage
    agent: issue-reporter
    actions: [issues.list, issues.get]   # narrowed to read-only for THIS step
  - id: report
    agent: issue-reporter
    actions: []                          # unrestricted — full agent grant
```

## 3. T1 — make the field real

### 3a. Validate entries against the catalog
In `workflow.rs`, each `actions:` entry must name a catalog tool → new `WorkflowParseError::ActionsUnknownTool { step, tool }`. This mirrors the catalog lookup `validate_action_step` (`workflow.rs:1317`) already performs — same crate, same dependency (`rupu-orchestrator` already depends on `rupu-mcp`), so it is a short, proven path. Rejects `""`, typos, and the legacy `open_pr`-style verbs.

### 3b. Shape rule vs `action:` steps
A **non-empty** `actions:` on an `action:` step is a validation error (`ActionsOnActionStep { step }`) — an action node's tool is already explicit and permission-checked, so an allowlist there is redundant and confusing. An **empty** `actions:` on an action step stays legal (it is already present across the repo's workflows) — compat-safe.

### 3c. Enforce at exactly one point
`crates/rupu-orchestrator/src/step_factory.rs` computes `agent_tools: spec.tools`. That becomes the intersection per §2. This is the single wiring point; no other call site changes.

**When `actions:` names a tool the agent does not grant:** the intersection narrows to the agent's set (already safe — no escalation). This is **not** a hard error, because agent specs resolve at runtime (parse-time cannot see them) and failing a run over an already-safe narrowing is worse than surfacing it. It **is** made visible: a `tracing` warning **plus** a `tool_audit` event (§4) with `declared: true, granted: false`, so it appears in the CP run view rather than only in a log. (Decision: visible, not fatal.)

### 3d. The editor
`StepForm` gains an `actions:` multi-select on **agent** steps (not action steps, per §3b), fed by `GET /api/tools`, cross-referenced against the selected agent's granted tools:
- **`/api/agents` must expose the agent's frontmatter `tools:`** — it does not today (verified). Additive field on the existing agent DTO.
- Tools the selected agent does not grant are shown but flagged ("not granted by `<agent>`") rather than hidden, so the mistake in §3c is visible at authoring time.
- The empty state must read **"unrestricted — inherits `<agent>`'s tools"**. Today `actions: []` reads like "nothing allowed" and means the opposite; the UI must say so plainly.
- Round-trip: authoring writes `actions: [ ... ]`; clearing all entries writes `actions: []` (or omits it — either is "unrestricted"; prefer omitting on new steps, preserving `[]` where it already exists to avoid churn).

## 4. T2 — the audit trail

### 4a. A new event name (`tool_audit`) — NOT `action_emitted`
`action_emitted` is **already treated as a dead/legacy shape by the web** and explicitly ignored (`crates/rupu-cp/web/src/components/transcript/transcriptView.ts:353` — *"user_message and action_emitted are dead/legacy shapes"*). Reusing that name would make the audit trail silently invisible in the CP. The new event is therefore **`tool_audit`**:

```json
{ "type": "tool_audit", "tool": "issues.create", "declared": false, "granted": true, "blocked": false }
```

- `declared` — was the tool in the step's `actions:` list (false when `actions:` is empty ⇒ unrestricted, which is not a violation).
- `granted` — is the tool in the agent's frontmatter `tools:`.
- `blocked` — was the call denied (by the narrowing, or by the existing per-tool/per-mode permission check).

### 4b. Emit at the two choke points
1. **`action:` nodes** — the `ToolDispatcher`/permission layer (`crates/rupu-mcp`), which every action-node call already funnels through (`permission.check(name, kind)`).
2. **Agent tool calls** — the agent tool-call path, which is where a narrowing denial happens.

A denied call returns a tool error to the agent (so the model sees it and can adapt) **and** emits `blocked: true`. Events land in the transcript JSONL and are surfaced in the CP run view alongside existing step events.

### 4c. Retire the legacy protocol
Delete, in one pass:
- `crates/rupu-agent/src/action.rs` — `ActionEnvelope`, `ActionValidator`, the `open_pr`/`file_write`/`shell_exec` vocabulary, and the stale *"Real impl lands in Task 11"* comment; drop the `lib.rs` re-export.
- `crates/rupu-orchestrator/src/action_protocol.rs` — `validate_actions`, `ActionValidationResult`; drop the `lib.rs` re-export.
- `crates/rupu-orchestrator/tests/action_allowlist.rs`.

The web's dead-shape handling for `action_emitted` (`transcriptView.ts`) stays as-is — it is defensive handling of historical transcripts, not live code.

## 5. Compatibility & risks

- **Every existing workflow keeps working** — empty `actions:` = unrestricted (§2), and no sample carries a non-catalog verb.
- **Parse-time rejection risk:** §3a will reject a workflow whose `actions:` holds a non-catalog verb. The in-repo samples are clean (`[]`, plus nightly-health's real MCP names), but live workflows under `~/.rupu` may carry legacy verbs and will start erroring with a message naming the offending verb. This is the honest outcome (those entries never did anything) and the fix is to remove or correct them.
- **`actions: []` in the samples is left in place** (decision): it is harmless, it now documents "unrestricted" explicitly, and stripping ~10 files is pure churn.
- The MCP catalog is unchanged; no new connector work.

## 6. Testing

- **Rust (validation):** an `actions:` entry not in the catalog → `ActionsUnknownTool` naming the verb; `actions: [""]` rejected; a valid catalog list accepted; a non-empty `actions:` on an `action:` step → `ActionsOnActionStep`; an empty `actions:` on an action step still accepted; every `.rupu/workflows/*.yaml` still parses.
- **Rust (enforcement):** effective tools = intersection when `actions:` non-empty; = agent grant when empty; a step listing a tool the agent lacks narrows (no escalation) and emits `granted: false`; a call outside the narrowed set is blocked and returns a tool error to the agent.
- **Rust (audit):** a `tool_audit` event per catalog call from both choke points, with correct `declared`/`granted`/`blocked`; a blocked call emits `blocked: true`; no `action_emitted` is emitted anywhere.
- **Rust (cleanup):** the workspace builds with the legacy protocol deleted; no dangling re-exports.
- **CP/web:** `/api/agents` returns the agent's `tools:`; the `actions:` multi-select lists catalog tools, flags non-granted ones, and reads "unrestricted" when empty; authoring round-trips to `actions:`; the field appears on agent steps and not on action steps; `tool_audit` renders in the run view.
- **Operator gate:** matt authors a step narrowed to read-only `issues.*` on an agent that also has write tools, runs it, and confirms (a) a write attempt is blocked with a visible `tool_audit`, (b) the same agent on an `actions: []` step can still write, (c) a bogus verb is rejected at save with a clear message.

## 7. Build order (for the plan)

**T1** — 1. validation (§3a/§3b) → 2. enforcement at `step_factory` (§3c) → 3. `/api/agents` tools + the `StepForm` picker (§3d).
**T2** — 4. the `tool_audit` event + both choke points + CP run-view rendering (§4a/§4b) → 5. delete the legacy protocol (§4c).

Each task ends green with the full suite; the `.rupu/workflows/*.yaml` parse test guards every step.

## 8. Decisions (locked, 2026-07-26)

- **`actions:` = narrowing filter** over the agent grant; **empty = unrestricted** (compat-critical).
- **Scope = T1 + T2** (enforce + validate + UI, then audit + legacy removal). Catalog growth (T3) is out of scope.
- **Entries are validated against the MCP catalog** (`ActionsUnknownTool`), mirroring `validate_action_step`.
- **Audit event is `tool_audit`, not `action_emitted`** — the latter is already dead-shaped in the web and would be silently dropped.
- **A non-granted `actions:` entry warns + audits, it does not fail the run** (agent specs resolve at runtime; the narrowing is already safe).
- **The legacy action protocol is deleted**, not resurrected.
- **`actions: []` stays in the existing samples.**
