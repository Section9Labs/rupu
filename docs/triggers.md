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
  "gitlab:my-org/my-repo",
]

# Optional: cap events processed per source per tick. Default 50.
# Prevents a backlog from chewing the rate-limit budget.
max_events_per_tick = 50
```

Empty by default — rupu doesn't poll anything until you ask it to.

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
NAME                         EVENT                            SOURCES                                  CURSOR
triage-incoming-issues       github.issue.opened              github:Section9Labs/rupu,gitlab:foo/bar  etag:W/"abc"|since:2026-05-07T00:00:00Z
```

### 4. Tick

The same `rupu cron tick` that fires `cron`-triggered workflows also polls events. Two flags split the tiers if you want to run them at different cadences:

```
* * * * *      rupu cron tick --skip-events     # cron only, every minute
*/5 * * * *    rupu cron tick --only-events     # events every 5 minutes
```

### What you can match against

The polled connector lifts events from each vendor's events API and maps them onto the rupu vocabulary. Glob wildcards (`*`) work in `trigger.event:` — see the next section.

**GitHub (polled tier covers all of these):**

| Category | Event ids |
|---|---|
| Issue lifecycle | `github.issue.opened` / `github.issue.closed` / `github.issue.reopened` / `github.issue.edited` |
| Issue queue (label/assign/milestone) | `github.issue.labeled` / `github.issue.unlabeled` / `github.issue.assigned` / `github.issue.unassigned` / `github.issue.milestoned` / `github.issue.demilestoned` |
| Issue comments | `github.issue.commented` / `github.issue.comment_edited` |
| PR lifecycle | `github.pr.opened` / `github.pr.closed` / `github.pr.merged` / `github.pr.reopened` / `github.pr.edited` / `github.pr.updated` / `github.pr.ready_for_review` |
| PR review | `github.pr.labeled` / `github.pr.unlabeled` / `github.pr.assigned` / `github.pr.unassigned` / `github.pr.review_requested` / `github.pr.review_submitted` |
| Push | `github.push` |

**GitLab (polled tier covers a subset; webhook tier covers more):**

| Category | Event ids |
|---|---|
| Issue lifecycle | `gitlab.issue.opened` / `gitlab.issue.closed` / `gitlab.issue.reopened` |
| MR lifecycle | `gitlab.mr.opened` / `gitlab.mr.closed` / `gitlab.mr.merged` / `gitlab.mr.reopened` |
| Comments | `gitlab.comment` |
| Push | `gitlab.push` |

Some GitLab events (label changes, assignment changes) aren't surfaced as discrete entries by GitLab's events API — use webhook-serve for those, or write a workflow against `gitlab.issue.opened` and inspect `event.payload.labels` in `trigger.filter:`.

### Glob matching on `trigger.event:`

`*` matches any (possibly empty) sequence of characters. Useful when:

- You want one workflow to react to **all** events of a kind: `trigger.event: github.issue.*`
- You want to react cross-vendor: `trigger.event: "*.pr.merged"` matches both `github.pr.merged` and (when GitLab MR-merge support lands in the polled tier) `gitlab.mr.merged`. Note vendor differences — `*.mr.merged` matches GitLab; `*.pr.merged` matches GitHub.
- You want a "wake on anything" workflow: `trigger.event: "*"`

`*` is greedy and does not special-case `.` boundaries — `github.*` matches the entire suffix.

### Queue patterns

GitHub Issues doesn't have first-class "queue" semantics (Linear / Jira do). The convention on GitHub is to use **labels as queue indicators** (e.g. `triage`, `ready`, `in-review`). Compose those with the existing label events + a filter expression:

```yaml
# .rupu/workflows/triage-on-label.yaml
name: triage-on-label
trigger:
  on: event
  event: github.issue.labeled
  filter: "{{ event.payload.label.name == 'triage' }}"

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

- **"Issue moved between queues."** Listen on `github.issue.labeled` AND `github.issue.unlabeled`. Glob: `trigger.event: github.issue.*labeled`. Filter on the specific label name.
- **"PR awaiting review."** `trigger.event: github.pr.review_requested`. The reviewer is at `event.payload.requested_reviewer.login`.
- **"PR moved out of draft."** `trigger.event: github.pr.ready_for_review`.
- **"Issue assigned to me."** `trigger.event: github.issue.assigned` + `filter: "{{ event.payload.assignee.login == 'matt' }}"`.

Native `entered_queue:<queue>` / `left_queue:<queue>` event sugar (per design spec §6.3) is deferred until rupu ships a Linear or Jira connector that natively models board columns. Until then, label-based composition is the GitHub idiom.

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

Same workflow YAML; same event vocabulary. The webhook receiver:

- Validates `X-Hub-Signature-256` (GitHub) / `X-Gitlab-Token` (GitLab).
- Maps the raw vendor delivery onto the rupu event id.
- Fires matching workflows with `{{event.*}}` populated.

Secrets are read from environment variables — never config files, never the keychain. Webhook secrets are operational secrets and belong in your process supervisor's environment block.

Bind to `127.0.0.1` and front with a TLS-terminating reverse proxy in production. rupu does not terminate TLS itself.

---

## Choosing a tier

- **Want to just react to issues / PRs / pushes on a laptop?** Polled tier. Add `poll_sources`, set up `rupu cron tick` in launchd / cron, done.
- **Have an always-on server (homelab, VPS, container)?** Webhook serve. Lower latency, broader event coverage.
- **Both?** That works. The deterministic-run-id rule means the same logical event fires once, regardless of whether polling or webhook saw it first.

---

## Templating

Both tiers expose the same `{{event.*}}` shape inside step prompts and `when:` filters:

```
event:
  id: github.issue.opened
  vendor: github
  delivery: <vendor unique id for this delivery>
  repo:
    full_name: Section9Labs/rupu
    owner: Section9Labs
    name: rupu
  payload: <vendor's raw JSON, untouched — reach inside via event.payload.*>
```

Common patterns:

```
{{ event.payload.issue.number }}              # GitHub: issue #
{{ event.payload.pull_request.head.ref }}     # GitHub: PR head branch
{{ event.payload.object_attributes.iid }}     # GitLab: issue / MR iid
```

The `filter:` field is the same minijinja you'd write in a `when:` expression — but evaluated at trigger-match time, against the event payload only. It must render to `true` or `false`.
