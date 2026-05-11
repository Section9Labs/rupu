# Workflow triggers

Workflows in rupu can fire on three trigger paths:

| Trigger | When it fires | Where it lives |
|---|---|---|
| `manual` (default) | `rupu workflow run <name>` | every install |
| `cron` | system cron / launchd invokes `rupu cron tick` | every install |
| `event` | matching events appear in either `rupu cron tick` (polled) or `rupu webhook serve` (live) | polled tier ships in this PR |

For the architectural background, see
[`docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md`](./superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md).

---

## Schema

Add a top-level `trigger:` block to any `.rupu/workflows/<name>.yaml`:

```yaml
trigger:
  on: manual | cron | event       # default: manual
  cron: "0 4 * * *"                # required when on: cron (5-field UTC)
  event: github.issue.opened       # required when on: event
  filter: "{{ event.repo.full_name == 'foo/bar' }}"
                                    # optional, only when on: event
```

Cross-field rules (validated at parse):
- `cron:` only allowed when `on: cron`.
- `event:` and `filter:` only allowed when `on: event`.
- Missing `on:` defaults to `manual`.

Reminder: step `actions:` is not a tool allowlist. Keep `actions: []` unless your agent intentionally emits the workflow action protocol; the agent file's `tools:` frontmatter controls tool access.

---

## Cron-scheduled triggers

```yaml
# .rupu/workflows/nightly-audit.yaml
name: nightly-audit
trigger:
  on: cron
  cron: "0 4 * * *"   # every day at 04:00 UTC

steps:
  - id: scan
    agent: security-reviewer
    actions: []
    prompt: |
      Audit the repo for newly-added secrets in the last 24h.
```

Install one entry in your crontab / launchd plist:

```
* * * * *  /usr/local/bin/rupu cron tick
```

`rupu cron tick` walks all cron-triggered workflows, fires those whose schedule matched between the persisted `last_fired` timestamp and now. Idempotent at 1-minute granularity. Use `--dry-run` to verify a crontab line without actually firing anything.

`rupu cron list` is read-only — prints every cron-triggered workflow + its next firing time. Use it before adding the cron entry.

---

## Event-triggered workflows (polled)

The polled tier is the **CLI-native** way to react to SCM events without running a server. `rupu cron tick` calls each configured connector for new events between ticks; matched workflows fire with the event payload bound as `{{event.*}}`.

### 1. Configure the sources you want polled

In `~/.rupu/config.toml` (global) or `<project>/.rupu/config.toml` (project shadows global):

```toml
[triggers]
poll_sources = [
  "github:Section9Labs/rupu",
  { source = "gitlab:my-org/my-repo", poll_interval = "15m" },
]

# Optional: cap events processed per source per tick. Default 50.
# Prevents a backlog from chewing the rate-limit budget.
max_events_per_tick = 50
```

Empty by default — rupu doesn't poll anything until you ask it to.

`poll_sources` accepts either:

- a bare string source like `"github:owner/repo"` which is eligible on every event tick
- an inline table with a source-local cadence override:

```toml
[triggers]
poll_sources = [
  { source = "github:hot-org/hot-repo", poll_interval = "1m" },
  { source = "github:slow-org/archive", poll_interval = "30m" },
]
```

`poll_interval` uses the same `<int><unit>` shape as other duration fields:

- `30s`
- `5m`
- `2h`
- `1d`

This is an operational control only. It does not change workflow matching semantics; it only decides whether a source is due to be polled on a given `rupu cron tick --only-events`.

The source model is now generic enough for both repo and tracker-native polling:

- `github:owner/repo`
- `gitlab:group/project`
- `linear:<team-id>`
- `jira:<site>/<project>`
- `jira:<project>` when `[scm.jira].base_url` is configured

Today, shipped polled connectors are:

- GitHub repo feeds
- GitLab repo feeds
- Linear team feeds via `linear:<team-id>`
- Jira project feeds via `jira:<site>/<project>` or `jira:<project>` with `[scm.jira].base_url`

Linear polling is team-scoped because Linear issues belong to one team and carry team-native workflow states. The first poll warms a local snapshot and emits zero events; later polls diff that snapshot to emit `linear.issue.opened` and normalized `linear.issue.updated` events.

Jira polling is project-scoped. It also uses a local snapshot diff, so the first poll warms the snapshot and emits zero events; later polls emit `jira.issue.opened` and `jira.issue.updated` when Jira issue fields actually change. Normalized transitions currently cover workflow state, priority, project, and sprint changes.

### 2. Write the workflow

```yaml
# .rupu/workflows/triage-incoming-issues.yaml
name: triage-incoming-issues
trigger:
  on: event
  event: github.issue.opened
  filter: "{{ event.repo.full_name == 'Section9Labs/rupu' }}"

steps:
  - id: classify
    panel:
      panelists:
        - security-reviewer
        - performance-reviewer
        - maintainability-reviewer
      subject: |
        Issue #{{ event.payload.issue.number }}
        Title: {{ event.payload.issue.title }}

        {{ event.payload.issue.body }}

  - id: comment_back
    agent: issue-commenter
    actions: []
    prompt: |
      Post a triage summary on issue
      {{ event.repo.full_name }}#{{ event.payload.issue.number }}
      with severity {{ steps.classify.max_severity }}.
```

### 3. Inspect what's wired

```
$ rupu cron events
NAME                         EVENT                            SOURCES                                       CURSOR
triage-incoming-issues       github.issue.opened              github:Section9Labs/rupu,gitlab:foo/bar@15m   etag:W/"abc"|since:2026-05-07T00:00:00Z
```

### 4. Tick

The same `rupu cron tick` that fires `cron`-triggered workflows also polls events. Two flags split the tiers if you want to run them at different cadences:

```
* * * * *      rupu cron tick --skip-events     # cron only, every minute
*/5 * * * *    rupu cron tick --only-events     # events every 5 minutes
```

### What you can match against

The polled connector lifts events from each vendor's events API and maps them onto the rupu vocabulary. Glob wildcards (`*`) work in `trigger.event:` — see the next section.

**GitHub canonical ids:**

| Category | Event ids |
|---|---|
| Issue lifecycle | `github.issue.opened` / `github.issue.closed` / `github.issue.reopened` / `github.issue.edited` |
| Issue queue (label/assign/milestone) | `github.issue.labeled` / `github.issue.unlabeled` / `github.issue.assigned` / `github.issue.unassigned` / `github.issue.milestoned` / `github.issue.demilestoned` |
| Issue comments | `github.issue.commented` / `github.issue.comment_edited` |
| PR lifecycle | `github.pr.opened` / `github.pr.closed` / `github.pr.merged` / `github.pr.reopened` / `github.pr.edited` / `github.pr.updated` / `github.pr.ready_for_review` |
| PR review | `github.pr.labeled` / `github.pr.unlabeled` / `github.pr.assigned` / `github.pr.unassigned` / `github.pr.review_requested` / `github.pr.review_submitted` |
| Push | `github.push` |

**GitLab canonical ids:**

| Category | Event ids |
|---|---|
| Issue lifecycle | `gitlab.issue.opened` / `gitlab.issue.closed` / `gitlab.issue.reopened` / `gitlab.issue.updated` |
| MR lifecycle | `gitlab.mr.opened` / `gitlab.mr.closed` / `gitlab.mr.merged` / `gitlab.mr.reopened` / `gitlab.mr.updated` |
| Comments | `gitlab.comment` |
| Push | `gitlab.push` |

GitLab's events API is still less detailed than GitHub's for queue-like metadata moves. When discrete label / assignment / milestone events are not surfaced upstream, use webhook mode or match `gitlab.issue.updated` / `gitlab.mr.updated` and inspect `event.payload.changes.*` in `trigger.filter:`.

### Semantic aliases

In addition to the canonical vendor ids above, rupu derives a broader semantic vocabulary from those deliveries. These are **matchable** in `trigger.event:` and `autoflow.wake_on:`:

| Alias family | Meaning |
| --- | --- |
| `issue.queue_changed` | queue-ish issue metadata changed |
| `issue.queue_entered` | issue moved into a queue-like state |
| `issue.queue_left` | issue moved out of a queue-like state |
| `issue.activity` | issue comment/edit/update activity |
| `pr.queue_changed` | queue-ish PR/MR metadata changed |
| `pr.queue_entered` | PR/MR moved into a review/ready queue |
| `pr.queue_left` | PR moved out of a queue-like state |
| `pr.review_activity` | review-request / review-submission activity |
| `pr.activity` | broader PR/MR activity updates |

Vendor-scoped semantic aliases are also emitted where they help:

- `github.issue.queue_changed`
- `github.issue.queue_entered`
- `github.issue.queue_left`
- `github.pr.queue_changed`
- `github.pr.queue_entered`
- `github.pr.queue_left`
- `github.pr.review_activity`
- `gitlab.issue.activity`
- `gitlab.issue.queue_changed`
- `gitlab.mr.activity`
- `gitlab.mr.queue_changed`
- `gitlab.issue.commented`
- `gitlab.mr.commented`

These aliases do **not** replace canonical ids. One delivery still fires a workflow at most once; rupu just allows more than one pattern vocabulary to match that delivery.

### Glob matching on `trigger.event:`

`*` matches any (possibly empty) sequence of characters. Useful when:

- You want one workflow to react to **all** events of a kind: `trigger.event: github.issue.*`
- You want to react cross-vendor: `trigger.event: "*.pr.merged"` matches both `github.pr.merged` and (when GitLab MR-merge support lands in the polled tier) `gitlab.mr.merged`. Note vendor differences — `*.mr.merged` matches GitLab; `*.pr.merged` matches GitHub.
- You want a "wake on anything" workflow: `trigger.event: "*"`

`*` is greedy and does not special-case `.` boundaries — `github.*` matches the entire suffix.

### Queue patterns

GitHub Issues doesn't have first-class "queue columns" the way Linear or Jira do. In practice, labels / assignment / milestones act as queue signals. With the new semantic aliases you can now express that intent directly and still refine it with filters.

Example: any issue entering a queue-like state:

```yaml
name: issue-entered-queue
trigger:
  on: event
  event: issue.queue_entered

steps:
  - id: classify
    agent: triage-classifier
    prompt: |
      Issue {{ event.repo.full_name }}#{{ event.payload.issue.number }}
      entered a queue-like state via {{ event.canonical_id }}.
```

Example: only the `triage` label should count as queue-entry:

```yaml
# .rupu/workflows/triage-on-triage-label.yaml
name: triage-on-triage-label
trigger:
  on: event
  event: issue.queue_entered
  filter: "{{ event.canonical_id == 'github.issue.labeled' and event.payload.label.name == 'triage' }}"

steps:
  - id: classify
    agent: triage-classifier
    prompt: |
      Issue {{ event.repo.full_name }}#{{ event.payload.issue.number }}
      just entered the triage queue.

      Title: {{ event.payload.issue.title }}
      Body: {{ event.payload.issue.body }}
```

Variations:

- **"Issue moved between queues."** `trigger.event: issue.queue_changed` and inspect `event.canonical_id` plus the changed label/assignee/milestone fields.
- **"PR awaiting review."** `trigger.event: pr.queue_entered` or the raw `github.pr.review_requested`. The reviewer is at `event.payload.requested_reviewer.login`.
- **"PR moved out of draft."** `trigger.event: github.pr.ready_for_review` or the broader `pr.queue_entered`.
- **"Issue assigned to me."** `trigger.event: github.issue.assigned` + `filter: "{{ event.payload.assignee.login == 'matt' }}"`.

True board-column events like `issue.entered_queue:<queue>` remain a future connector feature for trackers that model queues natively. For GitHub today, semantic aliases plus filters are the correct level of abstraction.

### Coverage caveats

- **Warmup tick.** On the very first poll (no cursor), rupu emits zero events and sets the cursor to "now." This avoids a workflow stampede on the last 90 days of history.
- **Latency.** Polling latency is one tick interval (1-5 minutes typical). Use webhook-serve for sub-second responsiveness.
- **At-most-once.** A workflow that crashes after rupu advances the cursor won't be re-processed. This is intentional — re-firing a triage workflow is worse than dropping a single event during a crash.
- **Idempotent re-fires.** The deterministic run-id (`evt-<workflow>-<vendor>-<delivery>`) lets the polled tier and the webhook tier coexist on the same workflow without double-firing the same logical event.

---

## Event-triggered workflows (webhook serve)

For sub-second latency or events the polled tier doesn't deliver, run `rupu webhook serve` as a long-lived process under your own supervisor.

```
RUPU_GITHUB_WEBHOOK_SECRET=<your-webhook-secret> \
  rupu webhook serve --addr 0.0.0.0:8080
```

Linear now works on the same receiver path:

```sh
RUPU_LINEAR_WEBHOOK_SECRET=<your-linear-webhook-secret> \
  rupu webhook serve --addr 0.0.0.0:8080
```

Jira Cloud issue webhooks use the same server:

```sh
RUPU_JIRA_WEBHOOK_SECRET=<your-jira-webhook-secret> \
  rupu webhook serve --addr 0.0.0.0:8080
```

Same workflow YAML; same event vocabulary. The webhook receiver:

- Validates `X-Hub-Signature-256` (GitHub), `X-Gitlab-Token` (GitLab), `Linear-Signature` + `webhookTimestamp` freshness (Linear), or `X-Hub-Signature` (Jira Cloud).
- Maps the raw vendor delivery onto the rupu event id.
- Fires matching workflows with `{{event.*}}` populated.

Linear issue updates are normalized before dispatch so workflows can match native state aliases such as:

```yaml
name: linear-review-ready
trigger:
  on: event
  event: issue.entered_workflow_state
  filter: "{{ event.vendor == 'linear' and event.subject.ref == 'ENG-123' }}"

steps:
  - id: review
    agent: reviewer
    prompt: |
      Linear issue {{ event.subject.ref }} changed workflow state.
      Before: {{ event.state.before.id }}
      After:  {{ event.state.after.id }}
```

Jira issue updates are normalized the same way, but the transition source is `changelog.items`:

```yaml
name: jira-review-ready
trigger:
  on: event
  event: issue.entered_workflow_state.ready_for_review
  filter: "{{ event.vendor == 'jira' and event.subject.ref == 'ENG-123' }}"

steps:
  - id: review
    agent: reviewer
    prompt: |
      Jira issue {{ event.subject.ref }} entered review.
      Before: {{ event.state.before.name }}
      After:  {{ event.state.after.name }}
```

Secrets are read from environment variables — never config files, never the keychain. Webhook secrets are operational secrets and belong in your process supervisor's environment block.

Important current limits:

- Linear polling is available for event-triggered workflows, but autoflow ownership is still repo-backed. `poll_sources = ["linear:<team-id>"]` participates in `rupu cron tick` and the event trigger path; it does not yet give autoflows a tracker-native claim/ownership model.
- Jira native state support is currently webhook-only. `jira:<project>` polling is still future work.

Bind to `127.0.0.1` and front with a TLS-terminating reverse proxy in production. rupu does not terminate TLS itself.

---

## Choosing a tier

- **Want to just react to issues / PRs / pushes on a laptop?** Polled tier. Add `poll_sources`, set up `rupu cron tick` in launchd / cron, done.
- **Have an always-on server (homelab, VPS, container)?** Webhook serve. Lower latency, broader event coverage.
- **Both?** That works. The deterministic-run-id rule means the same logical event fires once, regardless of whether polling or webhook saw it first.

---

## Templating

Both tiers expose the same normalized metadata inside step prompts and `when:` filters:

```
event:
  id: issue.queue_entered            # the matched id for this workflow
  canonical_id: github.issue.labeled # the raw vendor-mapped id
  matched_as: issue.queue_entered
  aliases: [github.issue.queue_changed, github.issue.queue_entered, issue.queue_changed]
  vendor: github
  delivery: <vendor unique id for this delivery>
  repo:
    full_name: Section9Labs/rupu
    owner: Section9Labs
    name: rupu
  payload: <vendor's raw JSON>
```

Common patterns:

```
{{ event.payload.issue.number }}              # GitHub: issue #
{{ event.payload.pull_request.head.ref }}     # GitHub: PR head branch
{{ event.payload.object_attributes.iid }}     # GitLab: issue / MR iid
{{ event.subject.ref }}                       # Linear/Jira-style issue ref if normalized
{{ event.state.after.id }}                    # Native tracker state transitions
```

The `filter:` field is the same minijinja you'd write in a `when:` expression — but evaluated at trigger-match time, against the event payload only. It must render to `true` or `false`.
