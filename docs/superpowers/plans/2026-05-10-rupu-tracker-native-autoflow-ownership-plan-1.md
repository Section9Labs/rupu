# rupu Tracker-Native Autoflow Ownership — Plan 1

**Date:** 2026-05-10
**Status:** Proposed
**Companion docs:** [Tracker-native ownership design](../specs/2026-05-10-rupu-tracker-native-autoflow-ownership-design.md), [Native tracker state events design](../specs/2026-05-10-rupu-native-tracker-state-events-design.md), [Autoflow v1 design](../specs/2026-05-08-rupu-autoflow-design.md)

---

## Goal

Let repo-local autoflows persistently own **Linear** and **Jira** issues while still executing code work inside managed repo worktrees.

---

## Scope

This plan covers:

- `autoflow.source` in workflow YAML
- claim metadata extensions for tracker-native subjects
- full `IssueConnector` support for Linear and Jira
- tracker-native discovery in `autoflow tick` / `serve`
- tracker-aware CLI inspection output

This plan does not cover:

- GitHub Projects mapping
- cloud control plane
- multi-repo tracker ownership
- a new visual UI

---

## PR 1 — Schema and stale-doc cleanup

- add `autoflow.source` to workflow parsing
- keep `entity: issue`; do not add a second entity kind
- extend docs to show shipped Linear/Jira trigger support accurately
- add tracker-native ownership design + plan docs

**Acceptance**
- workflow YAML can parse `autoflow.source`
- docs no longer claim Jira polling is future work
- docs clearly state current boundary: tracker-native triggers ship, ownership does not

---

## PR 2 — Claim metadata foundation

- extend `AutoflowClaimRecord` with tracker-native metadata:
  - `source_ref`
  - `issue_display_ref`
  - `issue_title`
  - `issue_url`
  - `issue_state_name`
  - `issue_tracker`
- stop assuming `issue_ref -> repo_ref` is always derivable
- keep backward compatibility for old claim files

**Acceptance**
- old claim files still deserialize
- repo-backed autoflows still run unchanged
- claim JSON can preserve tracker-native subject details

---

## PR 3 — Linear `IssueConnector`

- implement `IssueConnector` for Linear
- support `list_issues`, `get_issue`, `comment_issue`, `create_issue`, `update_issue_state`
- normalize labels/tags and issue URLs into the generic `Issue` model
- add focused tests against mocked Linear responses

**Acceptance**
- `Registry::issues(IssueTracker::Linear)` resolves when credentials exist
- `issues.list` and `issues.get` work for Linear
- autoflow discovery can list Linear issues through the generic issue path

---

## PR 4 — Jira `IssueConnector`

- implement `IssueConnector` for Jira Cloud
- support the same trait methods as Linear
- normalize project/state/labels/URL into generic `Issue`
- add mocked Jira tests

**Acceptance**
- `Registry::issues(IssueTracker::Jira)` resolves when credentials exist
- `issues.list` and `issues.get` work for Jira
- autoflow discovery can list Jira issues through the generic issue path

---

## PR 5 — Tracker-native discovery and ownership runtime

- teach autoflow discovery to resolve `autoflow.source`
- use tracker issue connectors for candidate selection
- claim tracker issues persistently while binding them to the repo-local workflow scope
- reconcile tracker-native wakes and selector scans in `tick` and `serve`
- keep worktree execution unchanged

**Acceptance**
- `rupu autoflow tick` can claim a Linear or Jira issue without a repo-derived issue ref
- branch/worktree execution still works from the bound repo
- repo-backed GitHub/GitLab autoflows do not regress

---

## PR 6 — CLI visibility and operator polish

- extend `autoflow claims` output with subject/source/state fields
- extend `autoflow explain` with tracker-native details
- extend `autoflow status` summaries
- document operator flows for Linear/Jira ownership
- add integration tests covering:
  - tracker-native claim lifecycle
  - wake-triggered reruns
  - repo binding visibility

**Acceptance**
- operator can understand a tracker-native claim without opening raw JSON
- docs show end-to-end setup commands and YAML examples

---

## Validation strategy

For each PR:

1. keep tests focused on the new slice first
2. rerun affected autoflow tests after runtime changes
3. rerun trigger and connector tests after issue-connector work
4. only broaden validation after focused tests pass

Minimum final validation for the full plan:

- `cargo test -p rupu-scm`
- `cargo test -p rupu-cli --lib autoflow::tests`
- `cargo test -p rupu-cli --lib cron::tests`
- `cargo test -p rupu-cli --lib webhook::tests`
- `cargo test -p rupu-cli --test cli_autoflow`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`

---

## Main risks

### 1. Overloading the existing claim model

Mitigation:
- keep `issue_ref`
- add tracker metadata incrementally
- avoid a subject-kind rewrite unless runtime pressure forces it

### 2. Connector/API drift

Mitigation:
- mock tests for Linear and Jira connectors
- use normalized `Issue` model so autoflow logic stays generic

### 3. Repo binding ambiguity

Mitigation:
- require a resolved repo binding in this phase
- keep tracker-native ownership repo-attached
- defer multi-repo ownership

### 4. Regressing repo-backed autoflows

Mitigation:
- keep repo-backed path as the default when `autoflow.source` is absent
- add regression coverage for GitHub/GitLab claim flows

---

## Exit criteria

Plan 1 is complete when:

- Linear and Jira tracker issues can be owned persistently by `autoflow tick` / `serve`
- tracker-native claims show meaningful progress in CLI inspection commands
- repo-backed flows still work unchanged
- docs clearly explain both repo-backed and tracker-native ownership modes
