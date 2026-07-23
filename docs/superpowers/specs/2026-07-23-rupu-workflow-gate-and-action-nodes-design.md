# rupu — Approval Gate Nodes & SCM Action Steps (design)

**Date:** 2026-07-23
**Status:** approved (matt, 2026-07-23)
**Depends on:** run-state closure (PR #501 — store-side terminal events), Slice B-2 SCM connectors, workflow triggers Plan 1, CP visual workflow editor.

## 1. Problem

Two gaps in the workflow orchestrator:

1. **Approval is invisible structure.** `approval:` is a per-step *option* (`Step.approval`, `workflow.rs`) that pauses the run before the step body dispatches. The graph shows the same node and the same connecting lines whether or not a human gate exists — there is no node that *means* "a human decides here", and no way to route on rejection (reject is always terminal, with no cleanup).
2. **SCM interactions have no step form.** rupu-scm ships typed `RepoConnector`/`IssueConnector` implementations (GitHub, GitLab) and the embedded MCP server exposes them as tools (`scm.prs.create`, `issues.comment`, …), but the only way a workflow uses them is *through an agent prompt* — an LLM call to make one deterministic API call. Triggers can react to issues/PRs; steps cannot act on them directly.

## 2. Decision summary

Add **two new first-class step shapes** to the existing shape-inference model (linear / `for_each` / `parallel` / `panel`):

- **`approval:`-standalone → gate node.** A step with an `approval:` block and no agent/prompt/fan-out is an approval gate with its own id, rendered as a distinct node in every renderer. Richer semantics: `auto_approve` expression, `on_timeout` routing, `notify` hooks, `on_reject` inline cleanup steps. The legacy inline per-step option stays supported; renderers synthesize an implicit gate node from it.
- **`action:` → connector step.** `action: <tool-name>` + `with: <params>` executes an SCM/issue/CI tool from the existing MCP tool catalog directly — no agent, no tokens, deterministic. Output binds to `{{ steps.<id>.output }}`.

Rejected alternatives: visual-only synthesis with no schema change (gates stay semantically buried, SCM cards would hide an LLM behind a deterministic-looking card); full DAG rework with `depends_on`/branching (rewrites runner, template validation, resume, and all renderers — inline `on_reject` covers the real cases).

Named approvers are **out of scope** (rupu has no identity/auth layer; they would be unenforceable metadata).

## 3. YAML schema

### 3.1 Gate node

```yaml
steps:
  - id: review
    agent: security-reviewer
    prompt: "Review PR {{ event.payload.pull_request.number }} …"

  - id: merge_gate
    approval:
      prompt: |
        {{ steps.review.findings | length }} findings, max {{ steps.review.max_severity }}.
        Approve to open the PR.
      auto_approve: "{{ steps.review.max_severity in ['low', 'none'] }}"
      timeout_seconds: 86400
      on_timeout: reject            # approve | reject | fail   (default: fail — today's behavior)
      notify:
        - action: scm.prs.comment
          with: { pr: "{{ event.payload.pull_request.number }}", body: "⏸ Approval needed: …" }
      on_reject:
        - id: note_rejection
          action: issues.comment
          with: { issue: "{{ issue.number }}", body: "Rejected at merge gate." }
```

- Shape rule: `approval:` present **and** no `agent`/`prompt`/`for_each`/`parallel`/`panel`/`action` → gate node. `approval:` alongside `agent`+`prompt` remains the legacy inline option (unchanged semantics).
- `auto_approve`: minijinja expression rendered with the same context as prompts; truthy → gate resolves without pausing.
- `notify`: list of action-step entries (§3.2 shape), fired best-effort on entering AwaitingApproval. Failures are logged in the transcript, never block the pause.
- `on_reject`: inline steps (linear agent steps and action steps only — **no nested gates, no fan-out**) that run after a reject decision; each failure is logged and the chain continues; the run then ends `Rejected`. Their results are recorded in `step_results` under their own ids.
- Gate output: `steps.<id>.output = { decision: "approved"|"rejected", via: "human"|"auto"|"timeout", reason: string|null, decided_at: <ts> }` — `decision` is the final outcome, `via` records how it was reached (a timed-out gate with `on_timeout: approve` yields `decision: approved, via: timeout`). The valid template-field list (`workflow.rs` `validate_template_refs`) gains `decision`.
- Validation: `on_timeout` requires `timeout_seconds`; `on_reject`/`notify` entries are validated with the same step/action rules at parse time.

### 3.2 Action step

```yaml
  - id: open_pr
    action: scm.prs.create
    with:
      repo: "{{ event.repo.full_name }}"
      title: "fix: {{ inputs.title }}"
      head: "{{ steps.implement.output.branch }}"
      base: main
```

- `action` names a tool from the MCP catalog. v1 set: `issues.create|comment|update_state`, `scm.prs.create|comment|add_labels`, read/query (`issues.get|list`, `scm.prs.get|list|diff`, `scm.files.read`), CI dispatch (`github.workflows_dispatch`, `gitlab.pipeline_trigger`).
- `with` values are minijinja-rendered strings/JSON at execution time.
- Optional `platform:` / `tracker:` keys inside `with:` fall back to `[scm.default]` / `[issues.default]`, identically to the MCP tools.
- Output: the connector's JSON response, bound to `steps.<id>.output`. `when:`, `continue_on_error:` behave as on any step.

## 4. Runner semantics

### 4.1 Gate lifecycle

1. `when:` gate evaluates first (unchanged ordering: skip beats approval).
2. Render `approval.prompt` (fallback text unchanged).
3. Evaluate `auto_approve` if present. Truthy → record `decision: auto_approved, via: auto`, emit `step_started` + `step_completed` (duration ~0), continue.
4. Otherwise: existing pause path — emit `step_awaiting_approval`, persist `AwaitingApproval` keyed by the gate's step id, store prompt + `expires_at`. Fire `notify` entries (best-effort) before parking.
5. **Approve** (CLI or CP): existing resume-with-suppressed-gate mechanism, untouched. Decision recorded; run continues at the step after the gate.
6. **Reject**: run the `on_reject` chain (sequential, failures logged, chain continues), then terminal `Rejected`. The store appends the terminal event (PR #501 machinery).
7. **Timeout**: outcome per `on_timeout` — `fail` (today's behavior, default), `reject` (runs the `on_reject` chain), `approve` (resumes). Enforcement stays lazy **plus** a gate sweep added to `rupu cp serve`'s existing background loop, so timeout routing fires without operator interaction. The sweep resolves through the same `RunStore` methods (which append terminal events); `on_timeout: approve` routes through the existing resume worker.

### 4.2 Action execution

- **Parse time:** the tool name and `with:` keys validate against the *static* `tool_catalog()` metadata (no credentials needed to lint a workflow). Unknown tool or schema-invalid params → parse error.
- **Run time:** templates render, params re-validate, then the call executes through the same in-process MCP tool layer agents use — one catalog, one gating model, credentials resolved via the shared `Registry`.
- **Permission mode:** tools are Read/Write-classified in the catalog. In `readonly` mode, Write-class action steps fail with an explicit message (mirroring MCP gating). `ask` mode treats a Write action step like any approval-worthy operation — v1 keeps it simple: action steps run without per-call prompting (the workflow was authored deliberately); revisit if dogfooding disagrees.
- **Audit:** the executed action is recorded in the transcript/audit trail as an action envelope with `applied: true`, unifying with the agent action protocol (the `actions:` allowlist names already match tool names).
- **Failures:** network/API errors → `step_failed` with the connector error; `continue_on_error` honored. No built-in retry in v1.

### 4.3 Events — no new variants

Gates and action steps emit only existing events (`step_started`, `step_awaiting_approval`, `step_completed`, `step_failed`). The Rust `Event` enum rejects unknown variants on deserialize, so new variants would break old binaries tailing new `events.jsonl`. Decision detail (`via: auto`, etc.) travels in `step_results` and the run DTOs, not the event wire.

## 5. Renderers & editor

- **DTO:** `StepNodeDto.kind` union += `'approval' | 'action'`; kind precedence updated in `lib/workflowGraph.ts` (panel > parallel > for_each > approval > action > step; a well-formed step can only be one).
- **RunGraph (CP viewer):** new `GateNode` (diamond silhouette; ⏸ awaiting / ✓ approved (`via` shown when auto) / ✕ rejected) and `ActionNode` (compact card: tool name + Write badge) in `components/graph/`. When `on_reject` exists, a short labeled reject edge/branch renders off the gate.
- **rupu-app-canvas:** new `emit_gate` / `emit_action` branches in `render_rows` (◇ gate glyph, distinct action glyph, meta labels); `on_reject` uses the existing branch-cell machinery. Insta snapshots.
- **Editor (`components/workflow-editor/`):** palette reorganizes into two groups — *Flow* (step, for_each, parallel, panel, **approval**) and *Connectors* (cards **generated from the static tool catalog**, grouped issues / PRs / query / CI; each card's form derives from the tool's JSON schema, so future tools appear with zero editor code). `StepForm` gains per-kind forms; `graphToWorkflowObject` serializer branches. Legacy inline-approval steps render a dashed implicit gate node plus a one-click **"extract to gate node"** migration that rewrites the YAML.

## 6. Compatibility

- Existing workflows parse and run identically; they gain gate visuals (synthesized node) for free.
- Workflows using the new shapes on older binaries fail with a clean parse error (`deny_unknown_fields`) — version-gated, acceptable.
- The inline `approval:` option is documented as legacy-supported (not removed); new docs and the editor produce gate nodes.

## 7. Testing

- **Schema:** parse/validate per shape; negatives: gate mixed with `agent`, unknown `action` tool, schema-invalid `with`, nested gate inside `on_reject`, `on_timeout` without `timeout_seconds`.
- **Runner** (mock connector registry): action happy-path binds output; connector error → `step_failed` (+ `continue_on_error`); readonly refuses Write; gate auto-approve truthy/falsy; reject runs cleanup chain then `Rejected`; timeout sweep × {approve, reject, fail}; notify failure doesn't block the pause.
- **Web:** `workflowGraph` yaml↔graph round-trips for both kinds; palette/StepForm tests; GateNode/ActionNode render states; implicit-gate synthesis from legacy option.
- **Canvas:** insta snapshots for gate/action rows incl. reject branch.
- **E2E:** extend `pause_resume_e2e` with a gate-node approve and a reject-with-cleanup path.

## 8. Rollout — 4-PR arc

1. **Schema + gate runner** — new shapes parse/validate; gate lifecycle (auto-approve, reject chain, timeout outcomes, lazy enforcement); output binding.
2. **Action steps** — catalog validation, in-process MCP execution, permission gating, audit unification.
3. **Renderers + editor** — DTO kinds, GateNode/ActionNode, canvas emitters, palette groups, generated connector forms, legacy extraction.
4. **Notify hooks + cp-serve gate sweep** — best-effort notify entries; background timeout resolution.

Each PR lands with its test slice; nothing in an earlier PR depends on a later one.
