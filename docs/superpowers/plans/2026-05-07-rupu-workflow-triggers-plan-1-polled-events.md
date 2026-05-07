# Workflow Triggers Plan 1 — Polled Events on Cron Tick

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `rupu cron tick` to poll SCM connectors for events between ticks, match against workflows whose `trigger.on: event`, and fire them with `{{event.*}}` populated. This is the path that lets users WITHOUT a long-running server use event triggers — no `rupu webhook serve`, no firewall rules, no port forwarding.

**Spec:** [docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md](../specs/2026-05-07-rupu-workflow-triggers-design.md), §4.1, §6.2, §9, §10, §12.

**Architecture:**
- New trait `EventConnector` in `rupu-scm` with GitHub + GitLab implementations.
- New `[triggers]` section in `rupu-config` with `poll_sources` + `max_events_per_tick`.
- Extend `cmd/cron.rs::tick` to drive the polling tier alongside the existing schedule tier.
- Per-source cursor state at `<global>/cron-state/event-cursors/<vendor>/<owner>--<repo>.cursor`.
- Deterministic run-id `evt-<workflow>-<vendor>-<delivery>` for idempotency via `RunStore::create`.

**Tech stack:** Rust 2021 (workspace MSRV), `octocrab` (existing) for GitHub Issues + Pulls + Events APIs, the existing GitLab REST client for GitLab.

**Files touched:**
```
crates/rupu-scm/src/lib.rs                   — re-export EventConnector
crates/rupu-scm/src/event_connector.rs       — NEW trait + types
crates/rupu-scm/src/connectors/github/events.rs — NEW poll impl
crates/rupu-scm/src/connectors/gitlab/events.rs — NEW poll impl
crates/rupu-scm/src/registry.rs              — register events()
crates/rupu-config/src/triggers_config.rs    — NEW [triggers] section
crates/rupu-config/src/config.rs             — wire TriggersConfig
crates/rupu-config/src/lib.rs                — re-export
crates/rupu-cli/src/cmd/cron.rs              — extend tick + new flags + new `events` action
crates/rupu-orchestrator/src/runs.rs         — accept caller-supplied run-id (idempotency)
crates/rupu-orchestrator/src/runner.rs       — plumb event into StepContext (already does)
docs/triggers.md                              — NEW user docs
```

---

## Task 1 — `EventConnector` trait + types

- [ ] Create `crates/rupu-scm/src/event_connector.rs`. Define:
  ```rust
  #[async_trait]
  pub trait EventConnector: Send + Sync {
      async fn poll_events(
          &self,
          repo: &RepoRef,
          cursor: Option<&str>,
          limit: u32,
      ) -> Result<EventPollResult, ScmError>;
  }

  #[derive(Debug, Clone)]
  pub struct EventPollResult {
      pub events: Vec<PolledEvent>,
      pub next_cursor: String,
  }

  #[derive(Debug, Clone)]
  pub struct PolledEvent {
      pub id: String,                    // rupu event id
      pub delivery: String,              // vendor unique-id
      pub repo: RepoRef,
      pub payload: serde_json::Value,
  }
  ```
- [ ] Wire the module into `rupu-scm/src/lib.rs`; re-export `EventConnector`, `EventPollResult`, `PolledEvent`.
- [ ] Add `events(&self, platform: Platform) -> Option<&dyn EventConnector>` to `Registry`. Default impl returns `None` for platforms without an event connector configured.

**Verify:** `cargo build -p rupu-scm`.

---

## Task 2 — GitHub `EventConnector` impl

GitHub exposes events via the [Events API](https://docs.github.com/en/rest/activity/events). For our purposes we use **two endpoints**:
- `GET /repos/{owner}/{repo}/events` — last 90 days of repo events; covers `push`, `IssuesEvent`, `IssueCommentEvent`, `PullRequestEvent`. Paginated; returns `Etag` for cheap re-poll.
- `GET /repos/{owner}/{repo}/issues/events` — fine-grained issue lifecycle if we want it (not v0).

Cursor format: `etag:<etag>|since:<rfc3339>`. The `since` is the `created_at` of the last event we processed; `etag` short-circuits unchanged repos.

- [ ] Create `crates/rupu-scm/src/connectors/github/events.rs`.
- [ ] Implement `EventConnector` for the existing GitHub connector struct (or a new `GithubEventConnector` if cleaner). Use the existing `octocrab` client.
- [ ] Map raw GitHub event types → rupu event ids using a private helper that mirrors `rupu-webhook::event_vocab::map_github_event` (extract a shared mapper if it falls out cleanly).
- [ ] Set `delivery` = the GitHub event's `id` field (the `events` API exposes a unique-per-event id).
- [ ] Honor `limit`: stop adding events once we hit it; advance cursor to the last-emitted event's `created_at`.
- [ ] Handle 304 Not Modified (Etag match): return empty `events` + same cursor.
- [ ] Handle 403 (rate limit): return `ScmError::RateLimited` with the `X-RateLimit-Reset` time.

**Verify:**
```rust
#[tokio::test]
async fn poll_events_returns_events_since_cursor() { ... }

#[tokio::test]
async fn poll_events_with_etag_returns_empty_on_304() { ... }
```

Tests use `mockito` (already a workspace dep) to fake GitHub responses.

---

## Task 3 — GitLab `EventConnector` impl

GitLab exposes events via [`GET /projects/:id/events`](https://docs.gitlab.com/ee/api/events.html). Cursor format: `since:<rfc3339>|page:<n>`.

- [ ] Create `crates/rupu-scm/src/connectors/gitlab/events.rs`.
- [ ] Implement `EventConnector` against the existing GitLab REST client.
- [ ] Map raw GitLab `target_type` + `action_name` → rupu event ids using a helper that mirrors `rupu-webhook::event_vocab::map_gitlab_event`.
- [ ] Set `delivery` = GitLab event id (returned by the Events API).
- [ ] Honor `limit` and pagination per the GitLab API conventions.

**Verify:** mockito tests parallel to Task 2's.

---

## Task 4 — `[triggers]` section in config

- [ ] Create `crates/rupu-config/src/triggers_config.rs`:
  ```rust
  #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
  #[serde(default, deny_unknown_fields)]
  pub struct TriggersConfig {
      /// Repos to poll for event-triggered workflows. Format:
      /// "<platform>:<owner>/<repo>". Empty by default.
      pub poll_sources: Vec<String>,
      /// Max events processed per source per tick. Default 50.
      pub max_events_per_tick: Option<u32>,
  }
  ```
- [ ] Add a `triggers: TriggersConfig` field to `Config` (in `crates/rupu-config/src/config.rs`).
- [ ] Re-export from `lib.rs`.
- [ ] Add a parse test exercising the new section (round-trip).
- [ ] Add a layering test confirming project shadows global (project's `poll_sources` REPLACES global's per the existing array-replace rule).

**Verify:** `cargo test -p rupu-config`.

---

## Task 5 — Caller-supplied run-ids in `RunStore`

For idempotency we need to dispatch with a deterministic id and have `RunStore::create` return `AlreadyExists` rather than overwriting. Today run-ids are generated by `cmd::workflow::run`.

- [ ] In `crates/rupu-orchestrator/src/runs.rs`, surface a `RunStore::create_with_id(id: &str, ...)` constructor (or extend the existing `create`'s signature with `Option<String>`).
- [ ] Returns a typed `RunStoreError::AlreadyExists` when the directory exists.
- [ ] In `cmd/workflow.rs`, expose a `run_with_id(...)` entry point (private to the crate) that the cron-tick path can call. Manual-run path keeps generating `run_<ULID>` and using `create`.
- [ ] Add a unit test that calls `create_with_id` twice with the same id and asserts the second call errors.

**Verify:** `cargo test -p rupu-orchestrator`.

---

## Task 6 — Extend `rupu cron tick` with the polling tier

This is the integration step. The existing tick implementation (cron-scheduled fires) stays unchanged; we add a parallel pass.

- [ ] Add the new clap flags to `cmd::cron::Action::Tick`:
  ```rust
  /// Run only the cron-scheduled tier; skip event polling.
  #[arg(long, conflicts_with = "only_events")]
  pub skip_events: bool,
  /// Run only the event-polling tier; skip cron-scheduled fires.
  #[arg(long, conflicts_with = "skip_events")]
  pub only_events: bool,
  ```
- [ ] After the existing scheduled-fires loop, add `tick_polled_events(...)` if `!skip_events`. Steps:
  1. Load layered config; read `[triggers].poll_sources` and `max_events_per_tick` (default 50).
  2. For each source `<platform>:<owner>/<repo>`:
     a. Build the `RepoRef`.
     b. Look up the connector via `Registry::events(platform)`. Skip with a `tracing::info!` if absent ("no <platform> credential — run rupu auth login").
     c. Read cursor from `<global>/cron-state/event-cursors/<platform>/<owner>--<repo>.cursor` (None on first run).
     d. Call `poll_events(repo, cursor.as_deref(), max)`.
     e. Persist `next_cursor` BEFORE dispatching (idempotency — if a fire fails or the process is killed, we don't re-poll the same events).
  3. For each event:
     a. Walk all event-triggered workflows; filter to those whose `trigger.event:` equals `event.id` AND whose `trigger.filter:` (if set) renders `true` against the `{{event.*}}` context.
     b. For each match, build a deterministic run-id `evt-<workflow-name>-<vendor>-<delivery>`.
     c. Call into the workflow runner with the run-id + `event` payload set.
     d. On `RunStoreError::AlreadyExists`, log and skip (idempotent re-fire is the expected path on overlap).
- [ ] Wire `--only-events` to skip the existing scheduled-fires loop.
- [ ] In `--dry-run` mode, print the planned fires for both tiers without writing state.

**Verify:** `cargo build -p rupu-cli`. Manual smoke: write a `trigger.on: event, event: github.issue.opened` workflow; configure `poll_sources` for a test repo; run `rupu cron tick --only-events --dry-run`.

---

## Task 7 — `rupu cron events` read-only sanity check

- [ ] Add `Action::Events` to `cmd::cron::Action`.
- [ ] Implementation: walk workflows, find every `trigger.on: event`, print a table with columns: NAME / EVENT / SOURCES (sources from `[triggers].poll_sources` that COULD match — i.e. repos this workflow could fire for) / LAST CURSOR (best-effort: the most recent cursor across that workflow's sources).
- [ ] Read-only; doesn't update any state.

**Verify:** unit test against a fixture workflow set + tempdir state.

---

## Task 8 — Filter expression evaluation

`trigger.filter:` is a minijinja boolean expression evaluated against the same `{{event.*}}` context.

- [ ] In `cmd/cron.rs::tick_polled_events`, after building the event context, evaluate `wf.trigger.filter` (if Some) via a fresh `minijinja::Environment`. The expression must render to `"true"` or `"false"`; anything else is a parse-time error logged at WARN and treated as "filter excludes this event."
- [ ] Add `WorkflowParseError::FilterRequiresEventTrigger` if `filter:` is set without `on: event`. (Already covered by the existing schema validator? Check `validate_trigger`.)

**Verify:** unit tests for filter true / false / runtime-error / shape mismatch.

---

## Task 9 — User docs

- [ ] Create `docs/triggers.md` with three sections:
  1. **Cron-scheduled triggers** — `trigger.on: cron`, schedule syntax, `rupu cron tick` from system cron.
  2. **Event-triggered workflows (polled)** — `trigger.on: event`, `[triggers].poll_sources`, `rupu cron events` for inspection. Note coverage subset (§6.2 of the spec).
  3. **Event-triggered workflows (webhook serve)** — when to prefer this; secret env vars; reverse-proxy guidance.
  4. Pointer to the spec (`docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md`) for the architectural context.
- [ ] Sample workflow: a `triage-incoming-issues.yaml` that polls GitHub, fires on `github.issue.opened`, classifies with a panel.
- [ ] Update the top-level README's Triggers section (if any) to link to `docs/triggers.md`.

---

## Task 10 — Verification & polish

- [ ] `cargo test -p rupu-scm -p rupu-config -p rupu-orchestrator -p rupu-cli`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo fmt --check`
- [ ] Smoke test: configure `[triggers].poll_sources` against a test repo (Section9Labs/rupu itself); write a workflow that prints "saw issue #X" on `github.issue.opened`; open + close an issue; run `rupu cron tick --only-events`; verify the workflow fired exactly once.
- [ ] Update `TODO.md`: mark the "Workflow triggers PR 2/3" entries as superseded by this spec; remaining backlog items are Plan 2 (glob matching, extended vocab) and Plan 3 (Slice E hand-off).
- [ ] Update `CLAUDE.md` "Read first" section to reference the new spec.

---

## Risks / non-obvious considerations

- **Rate limit interaction with multi-source polling.** GitHub authenticated rate limit is 5000/hr. With `poll_sources = [10 repos]` and ticks every minute, that's 600 calls/hr — well under budget. Etag short-circuits trim further. But if a user configures 100 repos and ticks every minute, they'll hit ceilings. The `max_events_per_tick` cap limits the *processing* cost, not the *polling* cost; for now we accept that and tag adaptive backoff as a follow-up.
- **First poll on an empty cursor.** Returning every event from the last 90 days would be catastrophic (could fire hundreds of workflow runs). On first poll we set the cursor to "now" and return zero events; subsequent polls return events since that initial timestamp. Document this in `docs/triggers.md` so users know to expect a "warmup" first tick.
- **Cursor advance before dispatch is the right call.** A workflow that crashes after cursor-advance won't re-process the same event on the next tick. This is the correct trade-off — re-firing a triage workflow is a worse user experience than silently dropping a single event during a crash. If users want the opposite (at-least-once), they can wire failure-handling inside the workflow itself.
- **Polled events don't see webhook-only signal.** `github.pr.review_requested` doesn't appear in the public events feed; `github.issue.labeled` does but only when the bot account is the labeler. Document the coverage subset clearly so users don't write workflows expecting events that polling can't deliver.
- **The `delivery` id collision risk.** GitHub event ids are unique within the events API; webhook delivery ids are unique within the webhook stream. They use different namespaces. The deterministic run-id format `evt-<workflow>-<vendor>-<delivery>` accidentally collides if both webhook and polled tiers process the same logical event — which is exactly what we want (idempotent: only one fires).

---

## Acceptance criteria

- A workflow with `trigger.on: event, event: github.issue.opened, filter: "{{event.repo.full_name == 'foo/bar'}}"` fires exactly once when an issue is opened on foo/bar, regardless of whether `rupu cron tick` is invoked once or multiple times within the same tick window.
- The same workflow fires zero times for issues opened on a different repo.
- `rupu cron tick --skip-events` runs only the existing cron-scheduled path with no behavior change vs. today.
- `rupu cron events` prints the configured event-triggered workflows + their sources + last cursors.
- A first-time poll against a fresh cursor returns zero events (no historical re-fire).
- Build, tests, clippy, fmt all clean.
