// Golden corpus for Task 2's `parsesEveryRepoWorkflowSample` test: the
// verbatim contents of every `.rupu/workflows/*.yaml` file in this repo
// (as of the commit that added this file), embedded as committed snapshots
// so the test stays hermetic (no filesystem reads at test time). `.bak`/
// `.lock` sibling files in that directory are working artifacts, not
// workflow definitions, and are excluded.
//
// The closing `"""` of each multiline literal below is deliberately at
// column 0 (not indented to match the surrounding array literal): Swift
// strips each content line's leading whitespace up to the closing
// delimiter's own indentation, and these YAML samples have real lines at
// zero indentation, so any non-zero closing indentation would either fail
// to compile ("insufficient indentation") or corrupt the sample's leading
// whitespace.
enum YAMLGolden {
    static let samples: [(String, String)] = [
        (
            "action-demo.yaml",
            """
name: action-demo
description: "Demo: a read-only action step (scm.prs.list) feeding an agent step."
steps:
  - id: list_prs
    action: scm.prs.list
    with:
      platform: github
      owner: "Section9Labs"
      repo: "rupu"
      state: "open"

  - id: summarize
    agent: code-reviewer
    prompt: "Open PRs on Section9Labs/rupu: {{ steps.list_prs.output }} — summarize in one line."

"""
        ),
        (
            "code-review-panel.yaml",
            """
name: code-review-panel
description: Run a specialist review panel over one diff or review subject.
inputs:
  diff:
    type: string
    required: true
steps:
  - id: review
    actions: []
    panel:
      panelists:
        - security-reviewer
        - performance-reviewer
        - maintainability-reviewer
      subject: "{{ inputs.diff }}"
      max_parallel: 3

  - id: summarize
    agent: writer
    actions: []
    prompt: |
      Summarize this review panel result.

      Max severity: {{ steps.review.max_severity }}
      Findings count: {{ steps.review.findings | length }}

      {% for f in steps.review.findings %}
      - [{{ f.severity }}] {{ f.title }} ({{ f.source }}): {{ f.body }}
      {% endfor %}

"""
        ),
        (
            "dispatch-demo.yaml",
            """
name: dispatch-demo
description: Demonstrates sub-agent dispatch — a `writer-with-reviewers` parent dispatches specialist reviewers mid-draft. Run with `--mode bypass` to skip per-tool prompts.
inputs:
  subject:
    type: string
    required: true
    description: A file path or short description of the code to review.
steps:
  - id: review
    agent: writer-with-reviewers
    actions: []
    prompt: |
      Review {{ inputs.subject }}.

      Steps:
      1. Read the relevant code into context.
      2. Dispatch the security-reviewer and (if relevant) the perf-reviewer.
      3. Aggregate their findings into a single short summary, ordered by severity.

"""
        ),
        (
            "dispatch-smoke.yaml",
            """
name: dispatch-smoke
description: >
  Smoke-test sub-agent dispatch — the `dispatch-smoke` parent fans out three
  `dispatch-echo` children in parallel and reports each reply. Dispatch is only
  wired under `rupu workflow run` (a bare `rupu run` disables it), so this
  workflow is the entry point. Run with `--mode bypass` to skip per-tool prompts.
steps:
  - id: smoke
    agent: dispatch-smoke
    actions: []
    prompt: |
      Run the sub-agent dispatch smoke test: fan out three `dispatch-echo`
      children in parallel (ids one/two/three) and report each child's reply,
      then the completion line.

"""
        ),
        (
            "gate-demo.yaml",
            """
name: gate-demo
description: "Demo: approval gate node with auto-approve and reject cleanup."
steps:
  - id: assess
    agent: code-reviewer
    prompt: "Assess the working tree; reply exactly 'clean' if nothing is risky."

  - id: ship_gate
    approval:
      prompt: "Assessment: {{ steps.assess.output }} — approve to continue."
      auto_approve: "{{ steps.assess.output == 'clean' }}"
      timeout_seconds: 86400
      on_timeout: reject
      notify:
        - action: scm.prs.list
          with: { owner: Section9Labs, repo: rupu, state: open }
      on_reject:
        - id: note_rejection
          agent: code-reviewer
          prompt: "Summarize why run {{ steps.assess.output }} was rejected, one line."

  - id: proceed
    agent: code-reviewer
    prompt: "Gate decision was {{ steps.ship_gate.decision }}. Say 'shipped'."

"""
        ),
        (
            "investigate-then-fix.yaml",
            """
name: investigate-then-fix
description: Two-step bug fix — investigate, then propose minimal edit.
steps:
  - id: investigate
    agent: fix-bug
    actions: []
    prompt: |
      Investigate the bug described by:
      {{ inputs.prompt }}

      Stop without making edits. Report the root cause as text.
  - id: propose
    agent: fix-bug
    actions: []
    prompt: |
      Based on this investigation:
      {{ steps.investigate.output }}
      Propose and apply the minimal fix.

"""
        ),
        (
            "issue-supervisor-dispatch.yaml",
            """
name: issue-supervisor-dispatch
description: Controller autoflow that decides what workflow should run next for an issue.
autoflow:
  enabled: true
  entity: issue
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["autoflow"]
    limit: 100
  wake_on:
    - github.issue.opened
    - github.issue.labeled
    - github.pull_request.closed
    - github.pull_request.reopened
  reconcile_every: "10m"
  claim:
    key: issue
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: issue-understander
    actions: []
    contract:
      emits: autoflow_outcome_v1
      format: json
    prompt: |
      You are the controller autoflow for issue #{{ issue.number }}.

      Available context:
      - issue title, body, and labels
      - repo-local spec path: `docs/specs/issue-{{ issue.number }}.md`
      - repo-local phase plan path: `docs/plans/issue-{{ issue.number }}.md`

      Decide what workflow should run next.

      Rules:
      - If the spec or phase plan does not exist yet, dispatch `issue-to-spec-and-plan`.
      - If a planned phase is ready to execute, dispatch `phase-delivery-cycle` and provide `phase` in `dispatch.inputs`.
      - If the issue is waiting on a human merge or another external repo change, return `await_external`.
      - If the issue is complete, return `complete`.

      Return only the raw JSON object for `autoflow_outcome_v1`.
      Do not wrap it in markdown fences.
      Do not wrap it in an `autoflow_outcome_v1` key.
      Do not add any prose before or after the JSON.

      Valid examples:

      If the spec or plan is missing:
      {"status":"continue","summary":"Create the spec and phase plan first.","dispatch":{"workflow":"issue-to-spec-and-plan","target":"{{ issue.ref }}","inputs":{}}}

      If phase 1 is ready:
      {"status":"continue","summary":"Phase 1 is ready to execute.","dispatch":{"workflow":"phase-delivery-cycle","target":"{{ issue.ref }}","inputs":{"phase":"phase-1"}}}

      If waiting on a human:
      {"status":"await_external","summary":"Waiting for a human merge decision."}

      If complete:
      {"status":"complete","summary":"The issue is complete."}

"""
        ),
        (
            "issue-to-spec-and-plan.yaml",
            """
name: issue-to-spec-and-plan
description: Turn an issue into a repo-local spec document and phased implementation plan.
notifyIssue: true
steps:
  - id: understand
    agent: issue-understander
    actions: []
    prompt: |
      Read issue #{{ issue.number }} on {{ issue.r.project }}.

      Title: {{ issue.title }}
      Labels: {{ issue.labels | join(", ") }}

      Body:
      {{ issue.body }}

      Produce an implementation-ready understanding of the work.

  - id: spec
    agent: spec-writer
    actions: []
    prompt: |
      Create `docs/specs/issue-{{ issue.number }}.md` from this issue analysis:

      {{ steps.understand.output }}

      Make the spec specific to this repo.
      Return the written file path and a brief summary.

  - id: plan
    agent: phase-planner
    actions: []
    prompt: |
      Read `docs/specs/issue-{{ issue.number }}.md` and create
      `docs/plans/issue-{{ issue.number }}.md`.

      Split the work into reviewable phases. Each phase should fit in
      one PR and should include validation and exit criteria.

      Return the numbered phase list.

  - id: comment_back
    agent: issue-commenter
    actions: []
    prompt: |
      Post a concise issue comment for issue #{{ issue.number }}.

      Mention:
      - the spec file path
      - the plan file path
      - the proposed phases

      Use this plan output:
      {{ steps.plan.output }}

"""
        ),
        (
            "issue-triage.yaml",
            """
name: issue-triage
description: >
  Read-mostly issue triage autoflow. On a newly opened issue from a known
  collaborator, produces a triage assessment and posts it as a single
  comment via the native `issues.comment` tool. No repo writes; see the
  `post_triage` step for why it cannot itself apply the "triaged" label.
autoflow:
  enabled: true
  entity: issue
  selector:
    states: [open]
    labels_none: [triaged]
    authors_from: collaborators
  wake_on:
    - github.issue.opened
    - github.issue.labeled
  reconcile_every: "15m"
  claim:
    key: issue
steps:
  - id: assess
    agent: issue-understander
    actions: []
    prompt: |
      Triage issue #{{ issue.number }} ({{ issue.ref }}).

      Title: {{ issue.title }}
      Author: {{ issue.author }}
      Current labels: {{ issue.labels | join(", ") }}

      Body:
      {{ issue.body }}

      Produce a triage assessment with these sections:
      - Summary: one paragraph on what's being asked or reported
      - Priority: low|medium|high|critical, with a one-line reason
      - Type: bug|feature|question|chore
      - Suggested labels: a short comma-separated list
      - Open questions / missing info: anything blocking action, or "none"

  - id: post_triage
    agent: issue-commenter
    actions: ["issues.comment"]
    prompt: |
      Post exactly one comment on issue #{{ issue.number }} ({{ issue.ref }})
      via the native `issues.comment` tool, using this triage assessment as
      the body:

      {{ steps.assess.output }}

      Append a closing line to the comment noting: this autoflow can only
      comment — the sole state-changing native tool available,
      `issues.update_state`, toggles open/closed and cannot apply a label —
      so a human (or a follow-up label-writing automation) should apply the
      "triaged" label from the "Suggested labels" list above once this
      triage is reviewed, so the issue drops out of this autoflow's
      `labels_none: [triaged]` selector on the next tick.

"""
        ),
        (
            "nightly-health.yaml",
            """
name: nightly-health
description: >
  Nightly build/test/clippy health check over the whole workspace, using
  the repo's pinned toolchain. Opens or updates a rolling "CI health"
  issue on failure (dedup by exact title, via native `issues.*` tools),
  and closes/comments it once green. No `gh`/bash for SCM (bash is used
  only to run the build/test/clippy commands, not for SCM effects).
autoflow:
  enabled: true
trigger:
  on: cron
  cron: "0 6 * * *"
steps:
  - id: health_check
    agent: repo-investigator
    actions: []
    prompt: |
      Run this repo's CI health checks using the repo's PINNED toolchain
      declared in `rust-toolchain.toml` at the repo root — do NOT override
      it with `+stable`, `+nightly`, or any other explicit toolchain
      selector. Running plain `cargo ...` from inside the repo already
      picks up the pinned toolchain via rustup's directory override, so
      just invoke `cargo` normally from the repo root.

      Run, from the repo root, in this exact order:
      1. `cargo build --workspace`
      2. `cargo test --workspace`
      3. `cargo clippy --workspace --no-deps`

      Do NOT run `cargo fmt --check` — this repo's main branch is known to
      be fmt-dirty under the pinned rustfmt version, so a fmt gate would be
      permanently red and is out of scope for this check.

      For each command, capture: exit status, and — if non-zero — enough of
      the failing output to diagnose it (command name, first error(s),
      offending file:line). Do not modify any files; this is a read-only
      health check.

      Final response must start with exactly one of these two lines:
      `HEALTH: pass`
      `HEALTH: fail`
      followed by a per-command PASS/FAIL line, and for any FAIL, the
      captured failing output.

  - id: report_health
    # NOTE: this step runs as `issue-reporter`, whose frontmatter `tools:`
    # list declares `issues.list` / `issues.get` / `issues.comment` /
    # `issues.create` / `issues.update_state`. The `actions:` list below is
    # a REAL enforcement point (crates/rupu-orchestrator/src/step_factory.rs
    # `narrow_agent_tools`, spec: docs/superpowers/specs/
    # 2026-07-26-rupu-step-actions-enforcement-design.md): it narrows this
    # step to exactly the connector tools listed here (builtin tools, if any
    # were granted, would pass through untouched — this agent has none).
    # Listed in full to match the agent's grant so this step keeps its
    # existing behavior rather than silently losing a tool.
    agent: issue-reporter
    actions: ["issues.list", "issues.get", "issues.comment", "issues.create", "issues.update_state"]
    prompt: |
      Use the native `issues.list`, `issues.comment`, `issues.create`, and
      `issues.update_state` tools — do not use `gh` or any shell command for
      this. Do not modify any repository files and do not open a pull
      request; this step only reads/writes GitHub issues based on the health
      check result below.

      Rolling issue title (must match exactly, character for character):
      CI health: nightly build/test/clippy

      Health check result from the previous step:
      {{ steps.health_check.output }}

      1. Call `issues.list` (state: open) and check whether an open issue
         with that *exact* title already exists.
      2. If the health check result starts with `HEALTH: fail`:
         - Compose a comment/issue body naming the failing command(s) and
           including the captured failing output.
         - If a matching open issue exists, call `issues.comment` with the
           body. Otherwise call `issues.create` with title "CI health:
           nightly build/test/clippy" and the body.
      3. If the health check result starts with `HEALTH: pass`:
         - If a matching open issue exists, call `issues.comment` with a
           short note that the workspace is green again, then call
           `issues.update_state` to set it to `closed`.
         - If no matching open issue exists, do nothing further — do not
           open a new issue on a passing run.

      Final response must state: pass or fail, whether a matching issue was
      found (and its number), and which native tool call(s) were actually
      made (or that nothing was needed).

"""
        ),
        (
            "nightly-maintainability-security.yaml",
            """
name: nightly-maintainability-security
description: >
  Nightly maintainability + security sweep across the rupu workspace,
  report-only (no auto-fix, no PR). Findings are posted to one rolling
  issue per category via native `issues.*` tools (dedup: `issues.list`
  finds an existing open issue by exact title, then `issues.comment`;
  otherwise `issues.create`). No `gh`/bash for SCM.
autoflow:
  enabled: true
trigger:
  on: cron
  cron: "0 7 * * *"
steps:
  - id: review
    actions: []
    panel:
      panelists:
        - maintainability-reviewer
        - security-reviewer
      subject: |
        Review the rupu workspace for maintainability and security concerns.

        Crates (each has its own `src/`):
        crates/rupu-agent, crates/rupu-app, crates/rupu-app-canvas,
        crates/rupu-auth, crates/rupu-cli, crates/rupu-config,
        crates/rupu-coverage, crates/rupu-cp, crates/rupu-keychain-acl,
        crates/rupu-mcp, crates/rupu-orchestrator, crates/rupu-providers,
        crates/rupu-runtime, crates/rupu-scm, crates/rupu-tools,
        crates/rupu-transcript, crates/rupu-webhook, crates/rupu-workspace.

        Use read_file/grep/glob to sample each crate's `src/` tree — you do
        not have time to read every file, so prioritize entry points, public
        APIs, and anything that looks unusually large or complex. This is a
        report-only sweep: do not propose diffs or make any code changes,
        just report findings per your system prompt's JSON contract.
      max_parallel: 2

  - id: report_maintainability
    # NOTE: this step runs as `issue-reporter`, whose
    # `.rupu/agents/issue-reporter.md` frontmatter `tools:` list declares
    # `issues.list` / `issues.get` / `issues.comment` / `issues.create` /
    # `issues.update_state`. The `actions:` list below is a REAL enforcement
    # point (crates/rupu-orchestrator/src/step_factory.rs
    # `narrow_agent_tools`, spec: docs/superpowers/specs/
    # 2026-07-26-rupu-step-actions-enforcement-design.md): it narrows this
    # step to exactly the connector tools listed here. Listed in full to
    # match the agent's grant so this step keeps its existing behavior
    # rather than silently losing a tool.
    agent: issue-reporter
    actions: ["issues.list", "issues.get", "issues.comment", "issues.create", "issues.update_state"]
    prompt: |
      File today's maintainability report using the native `issues.list`,
      `issues.comment`, and `issues.create` tools — do not use `gh` or any
      shell command for this; do not modify any repository files and do not
      open a pull request.

      Rolling issue title (must match exactly, character for character):
      Nightly maintainability findings

      Steps:
      1. Call `issues.list` (state: open) and look for an existing issue
         whose title is exactly "Nightly maintainability findings".
      2. Build the report body from the findings below. If there are no
         maintainability findings today, the body should say so plainly
         (e.g. "No maintainability findings today.").
      3. If a matching open issue exists, call `issues.comment` on it with
         the body. Otherwise call `issues.create` with title "Nightly
         maintainability findings" and the body.
      4. Do not close, re-label, or otherwise alter any other issue.

      Today's maintainability findings (from the review panel; empty means none):
      Max severity: {{ steps.review.max_severity }}
      {% for f in steps.review.findings %}{% if f.source == "maintainability-reviewer" %}
      - [{{ f.severity }}] {{ f.title }}: {{ f.body }}
      {% endif %}{% endfor %}

      Final response must state: whether an existing issue was found (and its
      number) or a new one was created, and which native tool call(s) were made.

  - id: report_security
    # NOTE: same as report_maintainability above — this step runs as
    # `issue-reporter`, which has native `issues.*` tools via its own
    # frontmatter; see that step's comment.
    agent: issue-reporter
    actions: ["issues.list", "issues.get", "issues.comment", "issues.create", "issues.update_state"]
    prompt: |
      File today's security report using the native `issues.list`,
      `issues.comment`, and `issues.create` tools — do not use `gh` or any
      shell command for this; do not modify any repository files and do not
      open a pull request.

      Rolling issue title (must match exactly, character for character):
      Nightly security findings

      Steps:
      1. Call `issues.list` (state: open) and look for an existing issue
         whose title is exactly "Nightly security findings".
      2. Build the report body from the findings below. If there are no
         security findings today, the body should say so plainly (e.g.
         "No security findings today.").
      3. If a matching open issue exists, call `issues.comment` on it with
         the body. Otherwise call `issues.create` with title "Nightly
         security findings" and the body.
      4. Do not close, re-label, or otherwise alter any other issue.

      Today's security findings (from the review panel; empty means none):
      Max severity: {{ steps.review.max_severity }}
      {% for f in steps.review.findings %}{% if f.source == "security-reviewer" %}
      - [{{ f.severity }}] {{ f.title }}: {{ f.body }}
      {% endif %}{% endfor %}

      Final response must state: whether an existing issue was found (and its
      number) or a new one was created, and which native tool call(s) were made.

"""
        ),
        (
            "phase-delivery-cycle.yaml",
            """
name: phase-delivery-cycle
description: Execute one planned phase, open a draft PR, run a review panel, and pause for human approval.
autoflow:
  enabled: true
  entity: issue
  priority: 50
  selector:
    states: ["open"]
    labels_all: ["autoflow", "phase:phase-1"]
    limit: 100
  wake_on:
    - github.issue.labeled
    - github.pull_request.closed
    - github.pull_request.reopened
  reconcile_every: "10m"
  claim:
    key: issue
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: handoff
      format: json
      schema: autoflow_outcome_v1
inputs:
  phase:
    type: string
    required: true
steps:
  - id: phase_scope
    agent: repo-investigator
    actions: []
    prompt: |
      Read `docs/plans/issue-{{ issue.number }}.md` and extract the exact
      scope for phase `{{ inputs.phase }}`.

      Return:
      - a short scope summary
      - the likely touched files or modules
      - focused validation to run
      - what counts as done for this phase

  - id: implement
    agent: repo-implementer
    actions: []
    prompt: |
      Implement phase `{{ inputs.phase }}` for issue #{{ issue.number }}.

      Use this scoped plan:
      {{ steps.phase_scope.output }}

      Constraints:
      - keep the change limited to this phase
      - create or switch to branch `issue-{{ issue.number }}-{{ inputs.phase }}`
      - do not open a PR yet
      - end with branch name, changed files, and validation summary

  - id: open_pr
    agent: pr-author
    actions: []
    prompt: |
      Open a draft PR for phase `{{ inputs.phase }}` of issue #{{ issue.number }}.

      Context:
      {{ steps.implement.output }}

      Requirements:
      - first response line must be `PR: <platform>:owner/repo#number`
      - title format: `[Issue #{{ issue.number }}][{{ inputs.phase }}] <short title>`
      - include scope, validation, and follow-up notes in the PR body

  - id: review_panel
    actions: []
    panel:
      panelists:
        - security-reviewer
        - performance-reviewer
        - maintainability-reviewer
      subject: |
        Review this draft PR for phase `{{ inputs.phase }}` of issue #{{ issue.number }}.

        {{ steps.open_pr.output }}

        Intended scope:
        {{ steps.phase_scope.output }}
      max_parallel: 3
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: finding-fixer
        max_iterations: 4

  - id: ready_for_merge
    agent: issue-commenter
    actions: []
    approval:
      required: true
      prompt: |
        Phase `{{ inputs.phase }}` for issue #{{ issue.number }} is ready for human review.

        PR:
        {{ steps.open_pr.output }}

        Final panel state:
        - max severity: {{ steps.review_panel.max_severity }}
        - remaining findings: {{ steps.review_panel.findings | length }}

        Approve to post the ready-for-merge issue comment.
    prompt: |
      Post a concise issue comment on issue #{{ issue.number }} stating:
      - phase `{{ inputs.phase }}` is implemented
      - the draft PR reference
      - the validation summary from implementation
      - the panel iterations and remaining findings count
      - that the PR is ready for human review and merge

      Use this implementation summary:
      {{ steps.implement.output }}

  - id: handoff
    agent: writer
    actions: []
    contract:
      emits: autoflow_outcome_v1
      format: json
    prompt: |
      Return only valid JSON for `autoflow_outcome_v1`.

      Context:
      - issue #{{ issue.number }}
      - phase `{{ inputs.phase }}`
      - PR summary: {{ steps.open_pr.output }}
      - panel max severity: {{ steps.review_panel.max_severity }}
      - remaining findings: {{ steps.review_panel.findings | length }}

      Emit:
      - `status: "continue"`
      - a short `summary`
      - a `dispatch` object that hands control back to `issue-supervisor-dispatch`
        for the same issue target with no extra inputs

"""
        ),
        (
            "pr-code-review.yaml",
            """
name: pr-code-review
description: >
  Comment-only PR code-review autoflow. Runs the security /
  maintainability / performance panel over the PR diff and posts one
  summary review comment via the native `scm.prs.comment` tool. No
  approve, no merge, no code changes.
autoflow:
  enabled: true
  entity: pull_request
  selector:
    states: [open]
    draft: exclude
    authors_from: collaborators
  wake_on:
    - github.pr.opened
    - github.pr.updated
    - github.pr.ready_for_review
  reconcile_every: "10m"
  claim:
    key: pr_head_sha
steps:
  - id: review
    actions: []
    panel:
      panelists:
        - security-reviewer
        - maintainability-reviewer
        - performance-reviewer
      subject: "{{ event.pull_request.diff }}"
      max_parallel: 3
      # No `gate:` here (unlike a fixer workflow): this autoflow is
      # comment-only per spec — a fix-loop would need a fixer agent with
      # write access to the repo and a re-push, which is out of scope for
      # a review-and-comment autoflow.

  - id: post_review
    # NOTE: this step runs as `scm-pr-review`, whose
    # `.rupu/agents/scm-pr-review.md` frontmatter `tools:` list declares
    # `scm.prs.get` / `scm.prs.diff` / `scm.prs.comment`. The `actions:`
    # list below is a REAL enforcement point
    # (crates/rupu-orchestrator/src/step_factory.rs `narrow_agent_tools`,
    # spec: docs/superpowers/specs/
    # 2026-07-26-rupu-step-actions-enforcement-design.md): it narrows this
    # step to exactly the connector tools listed here. Listed in full to
    # match the agent's grant so this step keeps its existing behavior
    # rather than silently losing a tool.
    agent: scm-pr-review
    actions: ["scm.prs.get", "scm.prs.diff", "scm.prs.comment"]
    prompt: |
      Post exactly one summary review comment on PR
      #{{ event.pull_request.number }} ({{ event.pull_request.url }},
      reviewed at {{ event.pull_request.head_sha }}) via the native
      `scm.prs.comment` tool.

      This is a comment-only review — do not approve, request changes, or
      merge; no such native tool is used even if one happens to be
      available to you.

      Compose the comment from this panel result:

      Max severity: {{ steps.review.max_severity }}
      Findings count: {{ steps.review.findings | length }}

      {% for f in steps.review.findings %}
      - [{{ f.severity }}] {{ f.title }} ({{ f.source }}): {{ f.body }}
      {% endfor %}

      If there are no findings, the comment should say so plainly (e.g.
      "No security, maintainability, or performance issues found in this
      diff.").

"""
        ),
        (
            "quick-bugfix.yaml",
            """
name: quick-bugfix
description: Investigate a bug, implement the minimal fix, and summarize verification.
inputs:
  prompt:
    type: string
    required: true
steps:
  - id: investigate
    agent: repo-investigator
    actions: []
    prompt: |
      Investigate this bug report or failing test:

      {{ inputs.prompt }}

      Stop at diagnosis. Identify the likely root cause, affected files,
      and the narrowest useful validation.

  - id: implement
    agent: repo-implementer
    actions: []
    prompt: |
      Implement the minimal fix for this investigation:

      {{ steps.investigate.output }}

      Keep the change tightly scoped.
      Verify with the smallest useful command first.

"""
        ),
        (
            "review-changed-files.yaml",
            """
name: review-changed-files
description: Fan out one reviewer across a list of files and aggregate the findings.
inputs:
  files:
    type: string
    required: true
steps:
  - id: review_each
    agent: code-reviewer
    actions: []
    for_each: "{{ inputs.files }}"
    max_parallel: 4
    prompt: |
      Review file {{ item }} ({{ loop.index }} of {{ loop.length }}).
      Return either `no issues` or a short bulleted list.

  - id: summarize
    agent: writer
    actions: []
    prompt: |
      Combine these per-file reviews into one concise engineering summary.

      {% for r in steps.review_each.results %}
      ---
      {{ r }}
      {% endfor %}

"""
        ),
        (
            "run-demo.yaml",
            """
name: run-demo
description: >
  Demonstrate the `run:` step kind — a deterministic, non-LLM step that
  executes a declared command. Fans one command across a list, then reads
  the parsed output of an earlier step. No model is dispatched: this
  workflow reports 0 tokens.

  `run:` is opt-in; set `[workflow] run_step_enabled = true` first.
  See docs/workflow-format.md.

steps:
  # `parse: json` makes stdout available at `steps.<id>.json`, which is
  # indexable. `steps.<id>.output` stays the raw string for every mode.
  - id: plan
    run:
      cmd: echo
      args: ['{"items": ["alpha", "beta", "gamma"]}']
      parse: json
      timeout_seconds: 30

  # `run:` composes with `for_each:` — this is how a benchmark scores N
  # items concurrently.
  - id: fan_out
    for_each: "{{ steps.plan.json.items }}"
    max_parallel: 3
    run:
      cmd: echo
      args: ["processing {{ item }}"]
      timeout_seconds: 30

  # Downstream steps see the earlier step's exit code and unit results.
  - id: summarize
    run:
      cmd: echo
      args:
        - "plan exited {{ steps.plan.exit_code }}; {{ steps.fan_out.results | length }} unit(s) processed"
      timeout_seconds: 30

"""
        ),
    ]
}
