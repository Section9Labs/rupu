# rupu Workflow Triggers — Design

**Status:** Approved (foundation shipped; polled-event tier in plan)
**Date:** 2026-05-07
**Companion docs:** [Slice A design](./2026-05-01-rupu-slice-a-design.md), [Slice B-2 design](./2026-05-03-rupu-slice-b2-scm-design.md), [Slice C TUI design](./2026-05-05-rupu-slice-c-tui-design.md)
**Companion plans:** [Plan 1 — polled events on cron tick](../plans/2026-05-07-rupu-workflow-triggers-plan-1-polled-events.md)

---

## 1. What this is

A horizontal initiative — not a numbered slice — that makes rupu workflows fire on schedule and on external events. The CLI today only fires workflows on explicit `rupu workflow run <name>` invocation. This design layers in three trigger paths that share a single dispatch core, while remaining honest about the architectural mismatch between "CLI tool" and "long-running server."

## 2. Why a separate spec rather than an existing slice

The trigger surface crosses every slice:

- Schema lives in **rupu-orchestrator** (Slice A primitive).
- Polling runtime lives in **rupu-scm/issue connectors** (Slice B-2).
- Webhook receiver lives in **rupu-webhook** (Slice B-2 follow-on).
- Cloud relay belongs to **Slice E** (rupu.cloud) when that lands.

A single design captures the contract between layers so each can evolve independently — and so the cloud-relay contract is **frozen at the CLI side** before Slice E begins, preventing a re-architecture later.

## 3. The architectural tension

CLI tools execute, exit, and aren't running between invocations. Webhooks need a process listening on a port. These two requirements are fundamentally incompatible without picking one of:

- **Don't run continuously** → poll on a scheduler (cron / launchd) for both scheduled fires AND event fires.
- **Do run continuously** → the user opts into a long-lived process they themselves manage (systemd, launchd, container).
- **Outsource the listener** → a stable cloud endpoint receives webhooks and the CLI consumes them later.

We support all three, layered. Most users get coverage from layer 1 alone. Power users who already run servers add layer 2. Layer 3 ships with rupu.cloud and closes the latency gap for users who want webhook responsiveness without operating infrastructure.

## 4. Three-tier architecture

```
┌──────────────────── tier 1 ────────────────────┐    ┌── tier 2 ───┐    ┌── tier 3 ───┐
│ system cron / launchd  ──N min──▶ rupu cron tick  │    │ user-managed │    │ rupu.cloud   │
│                                       │            │    │ long-running │    │ webhook      │
│   ┌───────────────────────────────────┴───────┐    │    │ rupu webhook │    │ relay        │
│   │ scheduled workflows (trigger.on: cron)    │    │    │ serve        │    │ (Slice E)    │
│   │   → fire if schedule matched              │    │    │   ↑          │    │   ↑          │
│   ├───────────────────────────────────────────┤    │    │ (HMAC + HTTP │    │ (HTTPS +     │
│   │ event-triggered workflows                 │    │    │  listener)   │    │  durable     │
│   │ (trigger.on: event, polling mode)         │    │    │              │    │  queue)      │
│   │   → poll connector for events             │    │    └──────────────┘    └──────────────┘
│   │     since last cursor                     │    │                                │
│   │   → match → fire with {{event.*}}         │    │           shared                │
│   └───────────────────────────────────────────┘    │      ┌────────────────┐         │
└────────────────────────────────────────────────────┘      │ dispatch core  │◀────────┘
                                                             │ (run_workflow  │
                                                             │  + RunRecord   │
                                                             │  + idempotency)│
                                                             └────────────────┘
```

All three tiers feed the same dispatch core: `rupu_orchestrator::run_workflow` with `OrchestratorRunOpts.event` populated and a deterministic run-id used for idempotency.

### 4.1 Tier 1 — `rupu cron tick` (CLI-native, no daemon)

System cron / launchd invokes `rupu cron tick` every N minutes. Each tick:

1. Walks `<global>/workflows/` and `<project>/.rupu/workflows/` for workflows where `trigger:` is `cron` or `event`.
2. **For `cron` triggers** (already shipped): checks if the schedule matched between persisted `last_fired` and `now`. Fires if so.
3. **For `event` triggers in polling mode** (this initiative): reads the per-workflow / per-source cursor, asks the connector for new events since that cursor, matches against `trigger.event:` and the optional `trigger.filter:`, and fires with `{{event.*}}` bound. Updates the cursor.

State lives at:
- `<global>/cron-state/<workflow-name>.last_fired` — for `cron` triggers (shipped).
- `<global>/cron-state/event-cursors/<vendor>/<repo>.cursor` — for polled events (new).

Latency: 1-5 min. Sufficient for triage / review / CI-style flows. Not for sub-second integrations.

### 4.2 Tier 2 — `rupu webhook serve` (advanced, opt-in)

The same binary, run as a long-lived process under user-managed supervision (systemd, launchd, Docker). Listens on a configurable port, validates `X-Hub-Signature-256` (GitHub) / `X-Gitlab-Token` (GitLab), maps to the rupu event id via `rupu-webhook::event_vocab`, and dispatches via the same core. Already shipped.

We deliberately do **not** ship daemon-management tooling (`rupu daemon start`, install scripts, etc.). That's the user's job and the user's choice.

### 4.3 Tier 3 — rupu.cloud relay (Slice E, future)

Cloud receives the webhooks, persists events into a durable queue, and the CLI either:
- Polls the cloud API in `rupu cron tick` (treats cloud as another connector), or
- Subscribes via long-poll / SSE in a foreground `rupu listen` invocation (not a daemon — runs only while the user has a terminal open).

The contract surface is identical to tier 1's polled events: a list of `(event-id, payload, cursor)` tuples; whoever is calling fires `run_workflow` with `{{event.*}}` populated. **This means tier-3 wiring is a new connector implementation, not a new orchestration path.**

## 5. Schema

Already shipped. Refining the docs only.

```yaml
trigger:
  on: manual | cron | event       # default: manual

  # required when on: cron
  cron: "0 4 * * *"                 # 5-field cron expression (UTC)

  # required when on: event
  event: github.issue.opened        # rupu event id; see §6 for vocabulary

  # optional, only meaningful when on: event
  filter: "{{event.repo.name == 'rupu'}}"  # minijinja boolean expression
```

Cross-field rules (validated at parse):
- `cron:` only allowed when `on: cron`.
- `event:` and `filter:` only allowed when `on: event`.
- Missing `on:` defaults to `manual` (today's behavior).

## 6. Event vocabulary

Stable rupu event ids — dotted, vendor-prefixed. The webhook receiver and the polled-events tier both produce these identifiers; the workflow's `trigger.event:` field matches against them with glob support and an additional layer of derived semantic aliases.

### 6.1 Shipped (via webhook receiver)

```
github.pr.opened
github.pr.reopened
github.pr.closed
github.pr.merged
github.pr.updated                    # synchronize
github.pr.review_requested
github.pr.review_submitted
github.issue.opened
github.issue.closed
github.issue.reopened
github.issue.edited
github.issue.labeled
github.issue.assigned
github.issue.commented
github.push
github.ping

gitlab.mr.{opened,reopened,closed,merged,updated}
gitlab.issue.{opened,closed,reopened,updated}
gitlab.comment
gitlab.push
```

### 6.2 Polled-event tier (this initiative)

The polled tier produces the **same event ids** as the webhook tier so the workflow author sees a single vocabulary. Coverage on first ship is a subset (events the SCM `events` / `issues` APIs expose without webhook-specific payload fields):

```
github.issue.opened
github.issue.closed
github.issue.reopened
github.issue.commented
github.pr.opened
github.pr.closed
github.pr.merged
github.push                           # via repo events API
```

Events that depend on webhook-only signal (`review_requested`, `labeled`, `assigned`) **stay webhook-only on first ship**. The vocabulary still includes them; the polled connector just doesn't emit them yet. Documented in `docs/triggers.md`.

### 6.3 Future vocabulary (deferred)

- **Native issue-tracker queue/state events**: true tracker-modeled workflow-state transitions such as `issue.entered_workflow_state.ready_for_review` and `issue.state_changed.todo.to.in_progress`. This is now split into a dedicated design + plan because it requires both a normalized payload contract and non-repo tracker connector work. See [`2026-05-10-rupu-native-tracker-state-events-design.md`](./2026-05-10-rupu-native-tracker-state-events-design.md).

## 7. Templates — the `{{event.*}}` binding

Both webhook and polled paths populate the orchestrator's `StepContext.event` with a `serde_json::Value` carrying:

```
event:
  id: github.issue.opened              # matched event id for this run
  canonical_id: github.issue.opened    # raw vendor-mapped id
  matched_as: github.issue.opened
  aliases: []                          # derived semantic aliases for this delivery
  vendor: github | gitlab              # for cross-vendor templates
  delivery: <opaque vendor delivery-id> # webhook X-GitHub-Delivery / poll cursor item id
  repo:
    full_name: Section9Labs/rupu
    owner: Section9Labs
    name: rupu
  payload: <vendor's raw JSON payload>  # full passthrough; templates can reach inside
```

Workflows access via `{{ event.repo.full_name }}`, `{{ event.payload.issue.number }}`, `{{ event.payload.pull_request.head.ref }}`, etc. The `filter:` expression is evaluated as a minijinja boolean against the same context.

## 8. Idempotency & the dispatch core

All three tiers eventually call `rupu_orchestrator::run_workflow(workflow, opts)`. Two invariants:

1. **Deterministic run-ids for triggered runs.** Triggered runs use a run-id of the form:
   - cron: `cron-<workflow-name>-<schedule-tick-iso8601>`
   - event: `evt-<workflow-name>-<vendor>-<delivery-id>`
   This means a re-delivered webhook or an over-running cron tick won't double-fire — `RunStore::create` returns AlreadyExists and the dispatcher skips.
   Manual runs keep `run_<ULID>` (no idempotency expectation).

2. **State writes happen before the run.** Cursor advance and `last_fired` write are committed *before* `run_workflow` starts, so a long-running workflow that overruns into the next tick doesn't get re-fired. This trades "skip if rupu crashes mid-run" against "double-fire if rupu crashes mid-run"; we pick the former because double-firing is the worse user experience.

## 9. Configuration

A `[triggers]` section in `config.toml` (project shadows global) gates which polled sources rupu queries each tick. Default: empty (no polling) — rupu doesn't surprise users with API calls they didn't ask for.

```toml
[triggers]
# Sources to poll for event triggers. Repo-backed examples:
# "<platform>:<owner>/<repo>". Tracker-native examples:
# "linear:<team-id>". Jira polling remains future work; Jira webhook ingress is shipped separately.
# Each tick: rupu queries the connector for events since the last cursor.
poll_sources = [
  "github:Section9Labs/rupu",
  { source = "gitlab:my-org/my-repo", poll_interval = "15m" },
]

# Optional cap on events processed per source per tick. Default: 50.
# Prevents a backlog from blowing rate-limit budget.
max_events_per_tick = 50
```

The `rupu webhook serve` path requires no config beyond the standard secrets-as-env-vars (`RUPU_GITHUB_WEBHOOK_SECRET`, `RUPU_GITLAB_WEBHOOK_TOKEN`).

## 10. The connector contract for polled events

New trait method on `rupu_scm::RepoConnector` (or sibling trait `EventConnector`):

```rust
#[async_trait]
pub trait EventConnector: Send + Sync {
    /// Return events since `cursor` (exclusive), oldest-first.
    /// Returns the list + a new cursor to persist for the next call.
    /// `limit` caps the call to honor rate-limit budgets.
    async fn poll_events(
        &self,
        repo: &RepoRef,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<EventPollResult, ScmError>;
}

pub struct EventPollResult {
    pub events: Vec<PolledEvent>,
    pub next_cursor: String,            // opaque to rupu; persisted as-is
}

pub struct PolledEvent {
    pub id: String,                     // the rupu event id, e.g. "github.issue.opened"
    pub delivery: String,               // vendor-side unique id (for idempotency)
    pub repo: RepoRef,
    pub payload: serde_json::Value,
}
```

Cursors are **opaque strings** managed by the connector (GitHub uses `Etag` + last-event-id; GitLab uses page+last-id). rupu stores them per repo per workflow without parsing.

## 11. Architecture rules preserved

- **Hexagonal separation.** Polling logic lives in connectors (`rupu-scm`). The cron tick handler in `rupu-cli` is thin — calls into connectors and dispatches via `rupu-orchestrator`.
- **`rupu-cli` stays thin.** No business logic; just clap parse + delegation.
- **Workspace deps only.** No new top-level deps for polled events — uses `octocrab` (already pinned) for GitHub and the existing GitLab client for GitLab.
- **`unsafe_code` forbidden** (workspace-wide).

## 12. Surfaces (CLI changes)

| Invocation | Status | Behavior |
|---|---|---|
| `rupu cron list` | Shipped | List cron-triggered workflows + next fire time. Read-only. |
| `rupu cron tick` | Shipped (cron) | Fire scheduled workflows. **Extend** to also poll event-triggered workflows. |
| `rupu cron tick --dry-run` | Shipped | Print what would fire; no state change. |
| `rupu cron tick --skip-events` | New | Run only the cron path; skip event polling. Useful for cron lines that want predictable cost. |
| `rupu cron tick --only-events` | New | Run only the event-poll path. Useful for splitting tick frequencies (cron at 1 min, events at 5 min). |
| `rupu cron events` | New | Read-only sanity check — show registered event-triggered workflows + which sources they cover + last cursor per source. |
| `rupu webhook serve` | Shipped | Long-lived HTTP receiver. |

## 13. Slice E hand-off contract

When rupu.cloud (Slice E) ships, it receives webhooks server-side and exposes them to the CLI. Two integration shapes are valid; both depend only on the contract frozen here:

### 13.1 Cloud-as-connector

The cloud presents an `EventConnector` (§10) backed by the persistent queue. `rupu cron tick` calls `cloud_connector.poll_events(...)` exactly like it calls `github_connector.poll_events(...)`. Cursor is an opaque cloud sequence number.

**Pros:** zero changes to the dispatch core. Drop-in additional connector. Works for users running `rupu cron tick` from any scheduler.

### 13.2 Cloud-as-stream

A foreground `rupu listen` subcommand opens an SSE / long-poll connection to the cloud, receives events as they arrive, dispatches inline. Tear down on Ctrl-C; no daemon.

**Pros:** sub-second latency without the user managing a server.
**Cons:** requires the user to keep a terminal session open. Acceptable for power-users who already have a dedicated tmux pane / always-on machine.

Both shapes consume the same `PolledEvent` shape and produce the same `{{event.*}}` template binding. Slice E can ship either or both without changing anything in `rupu-orchestrator`, `rupu-cli/cmd/cron.rs`, or the workflow YAML schema.

## 14. Non-goals

- A native rupu daemon (no `rupu daemon start`). Users who want always-on use Tier 2 with their own supervisor.
- Tunnel-as-a-service integration (ngrok, cloudflared, smee.io). Stays the user's concern.
- Event replay UI / console. Deferred to Slice E (cloud has the durable queue; CLI tier just consumes the latest cursor).
- Inter-workflow dependencies (`trigger.on: workflow_completed`). Possible future addition; not in scope here.
- Multi-tenant / per-user webhook routing. That's an explicit Slice E concern.
- Glob/regex on `trigger.event:`. Tagged 5-line follow-up.
- Rate-limit-aware backoff beyond the `max_events_per_tick` cap. The cap is the v0 lever; if users hit ceilings we add adaptive backoff later.

## 15. Plan map

- **Plan 1 — Polled events on cron tick** ([link](../plans/2026-05-07-rupu-workflow-triggers-plan-1-polled-events.md)) — implement §10 `EventConnector` for GitHub + GitLab, extend `rupu cron tick` to poll, add `[triggers].poll_sources` config, idempotent dispatch via deterministic run-id.
- **Plan 2 — Glob matching + extended event vocab** (future) — `github.issue.*`, queue events.
- **Plan 3 — rupu.cloud connector implementation** (Slice E) — cloud-as-connector or cloud-as-stream; consumes the §13 contract.

## 16. Open questions

- **Should `rupu webhook serve` also write events to a local queue and have `rupu cron tick` consume from it?** This would unify the two ingest paths. Pro: one dispatch entry. Con: an extra piece of state; the receiver path becomes "async fire" rather than "sync dispatch." Decision: defer — keep webhook serve as direct-dispatch, polled events as direct-dispatch, until we have a concrete reason to introduce the queue.
- **Native queue/state modeling for non-SCM trackers.** Semantic aliases now cover GitHub/GitLab queue-like activity, but true queue state transitions still need connector-native models (Linear/Jira-style column/status moves). Source of truth: [`2026-05-10-rupu-native-tracker-state-events-design.md`](./2026-05-10-rupu-native-tracker-state-events-design.md).
- **Filter expression sandboxing.** `trigger.filter:` is minijinja. If an attacker can write a workflow file in `.rupu/`, they can already execute arbitrary code via `bash:` agents — so the filter sandbox isn't the threat boundary. Document this; don't sandbox.
