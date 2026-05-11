# rupu Native Tracker State Events — Design

**Status:** In implementation — foundation, Linear webhook/polling, and Jira webhook shipped
**Date:** 2026-05-10  
**Companion specs:** [`2026-05-07-rupu-workflow-triggers-design.md`](./2026-05-07-rupu-workflow-triggers-design.md), [`2026-05-09-rupu-autoflow-plan-2-portable-runtime-design.md`](./2026-05-09-rupu-autoflow-plan-2-portable-runtime-design.md)

---

## 1. What this is

`rupu` already supports:

- raw SCM trigger ids like `github.issue.labeled`
- semantic aliases derived from SCM activity like `issue.queue_entered`

That works for repo-driven automation, but it is still inference. It is not the same as a tracker that models workflow state directly.

This design adds a second, more precise layer:

- **native tracker state events** for systems like Linear and Jira
- a **normalized payload contract** so workflows can react to actual state transitions
- a **portable vocabulary** so the same workflow pattern can later run against Linear, Jira, or GitHub Projects mappings

---

## 2. Why this is needed

Derived SCM semantics answer questions like:

- “a label was added”
- “a reviewer was requested”
- “a milestone changed”

Native tracker events answer different questions:

- “this issue moved from `Todo` to `In Progress`”
- “this issue entered `Ready for Review`”
- “this issue left sprint `Sprint 42`”
- “this issue became blocked”

That distinction matters for agentic workflows because state transitions are usually the true automation boundary.

---

## 3. Scope

This initiative has two parts:

1. **Normalized event model**
   - fixed event names
   - fixed payload shape
   - fixed derived alias rules

2. **Tracker mappings**
   - Linear native state events
   - Jira native state events
   - later: GitHub Projects state/field events

The normalized model lands first. Tracker-specific adapters come after that.

---

## 4. Vocabulary

### 4.1 Canonical tracker-specific ids

Canonical ids are source-specific and explicit:

- `linear.issue.state_changed`
- `linear.issue.project_changed`
- `linear.issue.cycle_changed`
- `linear.issue.blocked`
- `linear.issue.unblocked`
- `jira.issue.state_changed`
- `jira.issue.sprint_changed`
- `jira.issue.priority_changed`

These are what connectors emit.

### 4.2 Generic derived ids

The alias layer derives portable ids from those canonical events:

- `issue.state_changed`
- `issue.entered_state`
- `issue.left_state`
- `issue.workflow_state_changed`
- `issue.entered_workflow_state`
- `issue.left_workflow_state`
- `issue.project_changed`
- `issue.entered_project`
- `issue.left_project`
- `issue.cycle_changed`
- `issue.entered_cycle`
- `issue.left_cycle`
- `issue.sprint_changed`
- `issue.entered_sprint`
- `issue.left_sprint`
- `issue.priority_changed`
- `issue.blocked`
- `issue.unblocked`

### 4.3 Named state aliases

When a normalized payload includes before/after names, rupu also derives specific aliases:

- `issue.entered_state.in_progress`
- `issue.left_state.todo`
- `issue.state_changed.todo.to.in_progress`
- `issue.entered_workflow_state.ready_for_review`
- `issue.entered_project.core_platform`
- `issue.left_cycle.sprint_42`

Tracker-scoped forms also exist:

- `linear.issue.entered_state.in_progress`
- `jira.issue.state_changed.todo.to.in_progress`

These make simple workflows much cleaner because exact state triggers no longer need `filter:`.

---

## 5. Normalized payload contract

Native tracker state events must populate a normalized payload at `{{ event.* }}`.

### 5.1 Base shape

```yaml
event:
  id: issue.entered_workflow_state.ready_for_review
  canonical_id: linear.issue.state_changed
  matched_as: issue.entered_workflow_state.ready_for_review
  vendor: linear
  delivery: 5ab0...
  subject:
    kind: issue
    ref: ENG-123
    url: https://linear.app/acme/issue/ENG-123
  repo: {}            # optional; absent for non-repo trackers
  payload: {...}      # raw native payload
```

### 5.2 Transition fields

Connectors add normalized transition objects when relevant:

```yaml
state:
  category: workflow_state
  before:
    id: todo
    name: Todo
    type: unstarted
  after:
    id: in_progress
    name: In Progress
    type: started
```

Other transition families follow the same shape:

```yaml
project:
  before: { id: backlog, name: Backlog }
  after:  { id: core-platform, name: Core Platform }

cycle:
  before: { id: sprint-41, name: Sprint 41 }
  after:  { id: sprint-42, name: Sprint 42 }

sprint:
  before: { id: sprint-41, name: Sprint 41 }
  after:  { id: sprint-42, name: Sprint 42 }

priority:
  before: { id: p3, name: Medium }
  after:  { id: p1, name: Urgent }
```

Optional cross-cutting metadata:

```yaml
actor:
  id: user_123
  name: Matt

organization:
  id: org_123
  name: Acme

team:
  id: team_123
  key: ENG
  name: Engineering
```

---

## 6. Workflow authoring

### 6.1 Portable workflow

```yaml
name: start-dev-automation
trigger:
  on: event
  event: issue.entered_workflow_state.in_progress

steps:
  - id: implement
    agent: implementer
    prompt: |
      {{ event.subject.ref }} entered {{ event.state.after.name }}.
      Start the implementation workflow.
```

### 6.2 Tracker-specific workflow

```yaml
name: linear-ready-for-review
trigger:
  on: event
  event: linear.issue.entered_workflow_state.ready_for_review

steps:
  - id: review
    agent: reviewer
    prompt: |
      Linear issue {{ event.subject.ref }} is ready for review.
```

### 6.3 Fallback filter style

```yaml
name: jira-state-review
trigger:
  on: event
  event: issue.entered_state
  filter: "{{ event.state.after.name == 'Ready for Review' }}"
```

The named aliases are preferred where possible. They are easier to read and safer to reuse.

---

## 7. Important architectural constraint

This was repo-shaped at the start:

- `poll_sources` assumed `<platform>:<owner>/<repo>`
- `EventConnector::poll_events` took `RepoRef`

That foundation is now generalized in code to a source model that can represent both:

- repo sources (`github:owner/repo`, `gitlab:group/project`)
- tracker-project sources (`linear:<team-id>`, `jira:<project>`)

That means future Linear/Jira polling no longer requires a fake repo model. The remaining work is connector-specific transport and payload hydration, not another core trigger-shape rewrite.

---

## 8. Recommended implementation order

### Phase 1 — foundation

- normalize native state-event aliases in `rupu-orchestrator`
- freeze payload contract in docs and tests
- no tracker connector yet

### Phase 2 — Linear webhook path

- add `rupu webhook serve` support for Linear
- map Linear `updatedFrom` issue webhooks onto normalized events
- initial implementation may only know state / project / cycle IDs; named aliases improve automatically once a future connector can hydrate names

### Phase 3 — non-repo event sources

- generalize poll sources away from repo-only scope
- add tracker/project event source references
- support Linear polling if the API surface is good enough

### Phase 4 — Jira webhook path ✅ shipped

- `rupu webhook serve` accepts Jira Cloud deliveries on `/webhook/jira`
- validates `X-Hub-Signature`
- maps `jira:issue_updated` changelog events to normalized transitions
- emits native aliases such as `issue.entered_workflow_state.ready_for_review`

### Phase 5 — GitHub Projects mapping

- map Projects field/column transitions into the same normalized shape
- treat this as lower priority because the upstream model is less clean

---

## 9. Non-goals

This design does **not**:

- invent a second workflow syntax
- replace existing SCM semantic aliases
- require SaaS / cloud infrastructure
- require polling support before native tracker webhooks are useful

---

## 10. Acceptance criteria for Phase 1

- `rupu-orchestrator` derives portable native aliases from normalized tracker payloads
- exact named aliases like `issue.entered_workflow_state.ready_for_review` match
- docs define the payload contract clearly enough that connector work is mechanical
- future connectors can choose webhook or polling transport without redefining event semantics
