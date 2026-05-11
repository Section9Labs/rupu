# rupu Native Tracker State Events — Plan 1

**Status:** Historical — implemented, with follow-on Jira webhook ingress landed afterward
**Date:** 2026-05-10  
**Companion spec:** [`../specs/2026-05-10-rupu-native-tracker-state-events-design.md`](../specs/2026-05-10-rupu-native-tracker-state-events-design.md)

---

## Goal

Land the normalized native tracker state-event foundation plus the first real tracker transport: **Linear via webhook ingress**.

This plan intentionally does **not** solve generalized polling for non-repo trackers yet. That is the next plan.

---

## Why this cut

The current event runtime has two very different shapes:

- webhook dispatch can consume any event payload shape
- polled events are repo-scoped and require `RepoRef`

Linear and Jira native workflow-state events fit the webhook path first. Trying to solve polling and connector transport at the same time would mix two architectural changes into one PR stack.

---

## Deliverables

### PR 1 — normalized alias foundation

- extend `rupu-orchestrator::event_vocab` to derive native state aliases from normalized payloads
- add tests for:
  - workflow-state transition aliases
  - named `entered_*` / `left_*` aliases
  - exact `state_changed.<from>.to.<to>` matching
- add the design + plan docs

### PR 2 — Linear auth + webhook source

- add `linear` to auth provider IDs and resolver parsing
- add webhook secret config/env surface for Linear
- extend `rupu-webhook` source enum and server routing for Linear
- verify signature handling against Linear’s HMAC header

### PR 3 — Linear event mapping

- map Linear `Issue` webhook payloads into canonical ids:
  - `linear.issue.state_changed`
  - `linear.issue.project_changed`
  - `linear.issue.cycle_changed`
  - `linear.issue.blocked`
  - `linear.issue.unblocked`
- populate normalized transition payloads from `updatedFrom`
- dispatch through the existing workflow engine and alias layer

### PR 4 — docs and examples

- add user-facing docs for native tracker state events
- add workflow examples:
  - issue enters in-progress
  - issue enters ready-for-review
  - issue becomes blocked

---

## Technical notes

### 1. Normalized payload first

The alias layer should not know about Linear’s raw field names beyond the connector mapping layer.

`rupu-orchestrator` should only understand normalized shapes like:

- `state.before` / `state.after`
- `project.before` / `project.after`
- `cycle.before` / `cycle.after`

### 2. Webhook-first for Linear

Linear webhook payloads already carry `updatedFrom`, which makes before/after transitions explicit. That gives us a clean first implementation without redesigning poll sources.

### 3. Polling follows later

The current poll-source contract is repo-only. Non-repo polling needs a new source abstraction such as:

- repo source
- tracker/project source
- organization/team source

That work belongs in the next plan, not this one.

---

## Acceptance criteria

- A workflow can match `issue.entered_workflow_state.ready_for_review`
- A workflow can match `linear.issue.state_changed.todo.to.in_progress`
- The alias layer is tracker-agnostic once the payload is normalized
- Linear webhook payloads dispatch through the same `rupu webhook serve` path as GitHub/GitLab

---

## Validation

- `cargo test -p rupu-orchestrator --lib`
- `cargo test -p rupu-webhook --lib`
- targeted CLI webhook tests once the Linear route exists
