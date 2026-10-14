# Control-plane HTTP API

`rupu cp serve` serves a JSON API under `/api/*`; the web UI is built entirely
on it, and so are peer control planes that drive this host remotely. This page
lists every route. The operator guide is [`control-plane.md`](control-plane.md);
customer scoping, attribution and the launch preview are covered in more depth
in [`cp-customers-api.md`](cp-customers-api.md).

Request and response bodies are named by their Rust type where they are large;
those types live in `crates/rupu-cp/src/api/` (and `rupu_orchestrator` /
`rupu_transcript` for events). The API is versioned with the binary: a feature
a peer needs is advertised in `GET /api/host/info` → `features`.

- [Conventions](#conventions)
- [Server-sent events](#server-sent-events)
- [Runs](#runs) · [Run control](#run-control) · [Run views](#run-views)
- [Events and transcripts](#events-and-transcripts)
- [Sessions](#sessions)
- [Launch, agents and workflows](#launch-agents-and-workflows) · [Skills](#skills)
- [Autoflows](#autoflows)
- [Findings](#findings) · [Coverage and assets](#coverage-and-assets)
- [Usage and dashboard](#usage-and-dashboard) · [Netflow](#netflow)
- [Projects and code](#projects-and-code)
- [Hosts, workers and fleet](#hosts-workers-and-fleet)
- [Config, models and tools](#config-models-and-tools)
- [Customers](#customers) · [Agentiflows](#agentiflows)
- [Node tunnel](#node-tunnel) · [Health](#health)

---

## Conventions

### Authentication

Started without `--token`, the API is open. Started with `--token <t>`, every
`/api/*` route requires the token, either as the header

```
Authorization: Bearer <t>
```

or as the browser cookie `rupu_cp_token_<port>`. A page load (any non-`/api`
path) with `?token=<t>` sets that cookie and answers `303` to the same URL
without the parameter. The cookie's value is derived from the token, not the
token, and is not accepted as a bearer. A cookie-authenticated request that
isn't `GET`/`HEAD`/`OPTIONS` must also send an `Origin` matching the `Host` it
was sent to, or it is a `403`. Comparisons are constant time. A missing or
wrong token is a `401` with the standard `{"error": ...}` body. `?token=` is
ignored on `/api` paths. `/healthz`, the static UI and `/api/node/connect`
(which authenticates nodes with their own enrollment token) are outside the
check. See [control-plane.md](control-plane.md#authentication).

```sh
curl -s -H "Authorization: Bearer $CP_TOKEN" \
  "http://127.0.0.1:7878/api/runs?limit=5"
```

### Errors

Handler errors are JSON:

```json
{ "error": "run 01JEXAMPLE0000000000000000 not found" }
```

A few endpoints add fields (a bad findings query adds `token`, `code`, `start`,
`end`; deleting a customer with projects adds `projects`; a failed artifact pull
is `{ "unavailable": "<reason>" }`). A malformed query string or JSON body is
rejected by the framework before the handler runs, with a plain-text 400/415/422.
An unknown `/api/...` path is a JSON 404 (`no API route for /api/...`), never
the web app.

**Path parameters** (`:id`, `:name`, `:ws_id`, …) are single plain path
components. Before any handler runs, a request is a 400 if any `/api/*` path
segment percent-decodes to something that can't be one: `.`, `..` or a name
starting with `.` or `-`, an encoded `/` or `\`, an encoded `?` or `#`, a
control character, or an invalid escape. Handlers validate their own
parameters on top of that.

| Status | Meaning across the API |
|--------|------------------------|
| 400 | Bad input: an id or path that fails validation, a bad slug, a malformed query. |
| 401 | Missing or wrong token. |
| 403 | A cookie-authenticated write from another origin; a `/api/fs/browse` path outside the browsable directories. |
| 404 | Unknown run / session / finding / host id. |
| 409 | The request conflicts with current state: approving a run not awaiting approval, deleting a run that isn't finished, resuming a run that isn't paused. |
| 500 | Internal error: an unreadable store, a fault on this server's own side. |
| 501 | This server can't do it: an adapter only `rupu cp serve` installs is missing, a remote host's transport doesn't support the operation, or a remote host can't answer this query (too old). |
| 502 | A remote host failed: unreachable, unauthorized, or an error or non-JSON reply. The same on lists (shown "offline" in the UI), detail views, streams and controls. |

### Common query parameters

- **`host=<id>`** — act on a registered remote host instead of this one. `local`
  is this host. An unknown id is a 404. On list endpoints an absent `host`
  fans out to every host and merges newest-first; per-host failures follow the
  501/502 rules above. One table holds for every `?host=<remote>` call:

  | The host… | List | Detail / graph / log / usage / netflow / session | Control (approve, reject, cancel, pause, resume, archive, restore, delete) |
  |---|---|---|---|
  | has no such run / session | 502 | 404 | 404 |
  | refuses the transition | — | — | 409 |
  | can't serve it (transport, too old) | 501 | 501 | 501 |
  | is unreachable or answers badly | 502 | 502 | 502 |

  A list's "not found" is a 502 because a reachable host answering a list
  route with 404 is broken, not empty. A 404 that names the *host* (`host
  <id> not found`) always means the id isn't registered.
- **`customer=<slug>|none`** — narrow to a customer's runs (by recorded
  attribution) or to projects currently assigned to it. See
  [`cp-customers-api.md`](cp-customers-api.md#customerslugnone).
- **`offset`, `limit`** — paging. `limit` defaults to 20 and is clamped to
  1–200.
- **`since`, `until`** — RFC 3339 timestamps bounding a list by start time.
- **`scope_kind=global|project`, `scope_id=<ws_id>`** — pin a definition to the
  global layer or one project's `.rupu/`.
- **`q=<query>`** — the one-line list query (the findings grammar:
  `status:failed,cancelled -workflow:nightly since:7d "two words"`) on
  `/api/runs/workflows`, `/api/runs/agents`, `/api/sessions` and
  `/api/projects/:ws_id/{runs,sessions}`. A term's values OR, terms AND,
  `-` negates, a bare word searches the list's text fields; comparisons are
  case-insensitive. It filters before `offset`/`limit` cut the page and ANDs
  with the list's other parameters (`lifecycle`, `scope`, `since`, `until`,
  `customer`). Remote hosts' rows are filtered on this server, reading past
  peer page clamps, as `customer` is: a host whose rows can't report a field
  the query names (no `customer` key: an old peer) is a 501 on
  `?host=<it>`, and left out of a fan-out and named in
  `X-Rupu-Hosts-Without-Customer` (the `customer` field) or
  `X-Rupu-Hosts-Unfilterable` (`<host>:<field>`, any other field). A `q`
  that doesn't parse is a 400 `{error, token, code, start, end}` (token
  index and char span). Time fields (`since`, `until`) take `30m`, `12h`,
  `7d`, `2w` (back from now, at most 10000 years), a UTC date (`2026-10-01`, the whole day) or an
  RFC 3339 time. Each list's fields, with their kind, values and match mode,
  are in `crates/rupu-cp/tests/fixtures/list_query/<list>.fields.json`:

  | List | Fields (match) | Bare words search |
  |---|---|---|
  | Workflow runs (`/api/runs/workflows`, `/api/projects/:ws_id/runs`) | `status` (one of the 8 run statuses), `workflow` (contains), `id` (prefix), `codename` (contains), `trigger` (`manual`/`cron`/`event`), `host`, `customer` (a slug or `none`), `since`, `until` (`started_at`), `is:archived` | workflow, id, codename |
  | Agent runs (`/api/runs/agents`) | `status` (exact), `agent` (contains), `provider`, `model` (contains), `source` (`standalone`/`session`), `session` (prefix), `trigger` (`trigger_source`), `codename`, `id` (prefix), `host`, `customer`, `since`, `until` (`started_at`) | agent, codename, id |
  | Sessions (`/api/sessions`, `/api/projects/:ws_id/sessions`) | `status` (`idle`/`running`/`failed`/`stopped`), `agent` (contains), `model` (contains), `provider`, `codename`, `id` (prefix), `host`, `customer`, `project` (alias `workspace`; the workspace id), `is:active`/`is:archived` (the session's `scope`), `since`, `until` (`created_at`) | agent, codename, id |

  `is:archived` on `/api/runs/workflows` lists the archive instead (rows
  carry `archived: true`); it is local only: a remote host is a 501, and on
  a fan-out each remote host is named `<host>:is`. On `/api/sessions` it
  reads each session's `scope` and asks remote hosts for their archived
  sessions; `scope=` still works.
- **`facets=1`** — on the same lists, answer `{rows, facets,
  facets_partial}` instead of the bare array: `facets` counts each facet
  field's values (`field → value → rows`) over the list's scope before `q`.
  `facets_partial: true` when some remote host's rows were read one page only
  (an unfiltered workflow-run listing), so its counts cover that page.

### Mutations are recorded, not done

Runs are detached processes. A control endpoint's 200 means the request was
recorded (a marker, a decision, a signal sent) — watch the run's status on a
stream to see it take effect.

---

## Server-sent events

Four endpoints stream `text/event-stream`:

| Endpoint | Frames |
|----------|--------|
| `GET /api/events/stream` | Without `run`: the **firehose** — every orchestrator run's step events, merged. With `run=<id>`: one run. |
| `GET /api/runs/:id/log` | One run's step events. |
| `GET /api/transcript/stream?path=…` | One transcript's events. |
| `GET /api/{runs,sessions,agentiflows}/:id/messages/stream` | One message space's log. See [Messages](#messages). |

Every frame is an unnamed `data:` line carrying one JSON event. On the one-run
and transcript streams it also carries an `id:` — the event's line number in
its file — and the stream ends with a named `end` event (see below). The
firehose has neither.

- Run streams carry `rupu_orchestrator::executor::Event`, internally tagged:
  `{"type": "step_started", ...}`. `dispatch_started` / `dispatch_completed`
  (a step's agent started or finished a child run) carry the child's `kind`:
  `subagent`, `agent` or `workflow`. Lines written before `kind` existed read
  as `subagent`.
- Transcript streams carry `rupu_transcript::Event`, adjacently tagged:
  `{"type": "...", "data": {...}}` (see [`transcript-schema.md`](transcript-schema.md)).

```
id: 2
data: {"type":"step_started","run_id":"01JEXAMPLE...","step_id":"triage",...}

: keep-alive

id: 9
data: {"type":"run_completed",...}

event: end
data: {}
```

Behaviour worth knowing:

- **Replay, then resume.** A stream sends the file's history from the start,
  then tails it (polled every 250 ms). On a one-run or transcript stream an
  event's `id` is its line number in the file (every line counts, so ids
  don't shift across rupu versions), and `Last-Event-ID` resumes after it
  instead of replaying. Browsers' `EventSource` sends the header on its own
  when it reconnects. If the file is rewritten (a mirror cache refilled from
  byte zero), the stream doesn't resend what it already sent.
- **Keep-alive** is a comment line every 15 s.
- **Ending.** A one-run stream ends once the run is over: its last event is a
  `run_completed` / `run_failed`, nothing follows it for 2 s, and the run is
  settled (terminal record, no gate-reject cleanup pending, no live runner).
  An earlier terminal event in the file (a failed attempt that was retried, a
  gate reject followed by its `on_reject` chain) never ends it. Then one
  `event: end` frame with `retry: 15000` follows and the connection closes:
  close your `EventSource` on it, or keep it and it reconnects 15 s later with
  `Last-Event-ID` — getting `end` again, or the events of a retry (the web
  run page does this). A transcript stream ends with `end` after
  `run_complete`. The firehose never ends.
- **Firehose scope.** The firehose follows runs in the run store — workflows and
  autoflows. Standalone agent runs and session turns never appear on it.
  It attaches to active runs at start, then to any new run id it sees (checked
  once a second), and drops a run's tail 30 s after it finishes.
- **Remote hosts.** With `host=<remote>`, the remote's run stream is passed
  through; `/api/events/stream` then requires `run`. `Last-Event-ID` reaches
  it: a mirror-backed host (SSH, tunnel, bucket) resumes from the
  coordinator's mirror, and an HTTP host gets the header forwarded. An HTTP
  host running an older rupu ignores it and replays without ids, as before.
- Sessions and run lists have no stream; the UI polls them.

```sh
curl -N -H "Authorization: Bearer $CP_TOKEN" \
  "http://127.0.0.1:7878/api/events/stream?run=01JEXAMPLE0000000000000000"
```

---

## Runs

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/runs` | Every orchestrator run, newest first (`RunListRow` + `host_id`). | `offset`, `limit`, `host`, `customer`. |
| GET | `/api/runs/workflows` | Manually launched workflow runs (no cron or event wake). | Also `lifecycle` (`active`, `completed`, `failed`), `since`, `until` (honoured on the local host), `customer`, `q` (`is:archived` lists the archive), `facets`. |
| GET | `/api/runs/archived` | Archived runs. | `kind=workflow` keeps manual runs only. Local only, unpaged. |
| GET | `/api/runs/agents` | Standalone and session agent runs (`AgentRunRow`). | `offset`, `limit`, `lifecycle`, `host` (`all` default, `local`, or an id), `since`, `until`, `customer`, `q`, `facets`. Offline hosts are skipped on fan-out. |
| GET | `/api/runs/autoflows` | Autoflow cycle ticks (`AutoflowCycleRow`). | `offset`, `limit`, `host`, `since`, `until`. |
| GET | `/api/runs/autoflows/events` | Actionable autoflow events: run launched, awaiting a human, cycle failed (`AutoflowEventRow`). | Same parameters. |
| GET | `/api/runs/:id` | Run detail: `{run, steps, usage}`. | `host`. 404 unknown. |
| DELETE | `/api/runs/:id` | Hard-delete a finished run. | `host`. 409 if the run isn't terminal ("cancel it first"). |
| POST | `/api/runs/:id/archive` | Move a finished run to the archive. | `host`. 409 if not terminal or already archived. |
| POST | `/api/runs/:id/restore` | Move an archived run back. | `host`. 404 not archived; 409 already present. |

## Run control

All take `host`. Locally they return the updated run detail plus `host_id`;
remotely `{ok, host_id}`. Their JSON bodies are optional: an empty body means
none, and a body that isn't the expected JSON is a 400.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| POST | `/api/runs/:id/approve` | Approve a parked gate. | `gate=<step_id>` picks one gate when several are parked. Optional body `{mode: "ask"\|"bypass"\|"readonly"}`. Records the decision and a resume marker; the `cp serve` resume worker (or the run's live runner) continues the run. 409: not awaiting approval, gate expired, gate already decided, ambiguous gate (several parked, no `gate`), unknown gate. |
| POST | `/api/runs/:id/reject` | Reject a parked gate. | `gate`. Optional body `{reason?}`, as for approve and cancel. Same 409s. |
| POST | `/api/runs/:id/cancel` | Cancel a run. | Optional body `{reason?}`. A live runner process is sent SIGTERM and the run is marked `Cancelled`; a run parked at a single gate with no live runner is rejected instead. 409 already finished. |
| POST | `/api/runs/:id/pause` | Pause a running run. | Writes a `.pause` marker the detached runner polls; it stops at its next safe boundary. 409 not running. 501 when the host's transport can't pause. |
| POST | `/api/runs/:id/resume` | Resume a paused run. | Marker only; the resume worker spawns `rupu workflow resume --if-unfinished`. 409 not `paused`; 501 without `rupu cp serve` or when the transport can't resume. |

## Run views

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/runs/:id/graph` | The run's DAG with step results, fan-out units, usage and codenames. | `host`. |
| GET | `/api/runs/:id/log` | SSE of the run's step events. | `host`. See [Server-sent events](#server-sent-events). |
| GET | `/api/runs/:id/usage` | Live usage: totals, per-step, per-call points. | `host`, `since`, `epoch` — pass back the previous response's `epoch` to receive only new points. |
| GET | `/api/runs/:id/usage-timeline` | Tokens per model call (`TurnPoint`). | `host`. |
| GET | `/api/runs/:id/children` | Every child the run launched with `dispatch` (all kinds), from the run's and its steps' children ledgers (`children.jsonl`) and their children's, each with its `parent_run_id` and `depth`. Pre-ledger sub-agents are read from `sub/` and marked `legacy`. | Local runs only (no `host`). |
| GET | `/api/runs/:id/autoflow` | Autoflow provenance: repo, issue, claim, prior cycles. | 404 means the run has no autoflow context. |
| GET | `/api/runs/:id/coverage` | **Host-internal.** The run's raw `coverage.jsonl` (`application/x-ndjson`); empty when none. | Read by a coordinator merging a remote unit's findings (`run.coverage_stream` feature). |
| GET | `/api/runs/:id/source` | A line-numbered slice of a file in the run's workspace (`SourceSlice`). | `path` (workspace-relative), `line`, `context` (default 20, max 200), `host`. A missing, binary, > 2 MiB or remote file is 200 with `available: false` and a `reason`. 400 on path traversal. |
| GET | `/api/runs/:id/ast` | The tree-sitter syntax subtree at a position (`AstResponse`). | `path`, `line`, `col` (1-based), `host`. Soft-fails like `source` when there's no grammar. |
| GET | `/api/runs/:id/netflow` | See [Netflow](#netflow). | |

## Events and transcripts

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/events/stream` | SSE firehose, or one run with `run`. | `run`, `host` (a remote host requires `run`). |
| GET | `/api/events` | Recent step events, newest first, each with `ts` and `pos`. | `limit` (default 200), cursor `before_ts` (unix ms), `before_run`, `before_pos`. Scans the 20 newest runs. |
| GET | `/api/transcript` | Read a transcript: `{events, summary, unparsed, partial?}`. | `path` (a `.jsonl` under the global dir or a registered workspace), `host`, `run`. A missing local file is an empty 200. With `host` naming an SSH, tunnel or bucket host, a path that isn't one of that host's mirrored transcripts must lie under the same local roots (400 otherwise). For a remote run the coordinator's mirror is served; `partial: true` when the host couldn't be reached to finish it; 502 when unreachable with nothing cached. |
| GET | `/api/transcript/stream` | SSE tail of a transcript. | `path`, `host`, `run` — `run` is required for a remote transcript not mirrored yet. 502 host unreachable. |
| POST | `/api/transcripts/:id/archive` | Archive a standalone agent-run transcript. | `host`, `ignore_liveness` (skip the "still running" check when a PID was reused). 501 without `cp serve`. |
| DELETE | `/api/transcripts/:id` | Delete a standalone transcript. | Same parameters. |

## Sessions

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/sessions` | Sessions, most recently updated first. | `offset`, `limit`, `scope` (`active` / `archived`), `host`, `since`, `until`, `customer`, `q`, `facets`. |
| GET | `/api/sessions/:id` | Session detail with usage and codename. | `host`. |
| GET | `/api/sessions/:id/runs` | The session's turns for the chat view (`SessionRunRow`). | `host`. |
| GET | `/api/sessions/:id/usage-timeline` | Tokens per model call. | `host`. |
| POST | `/api/sessions/:id/send` | Send a prompt as the next turn, or invoke skills in it. | `host`. Body `{prompt?, skills?}` → `{run_id, host_id}`; watch the turn with `/api/transcript/stream`. `skills` (skill entries) become `rupu session send --skill`; the prompt may be empty when a skill is given. 400 with neither, a malformed or wildcard entry, or a skill the session can't take (unknown, ambiguous, held from the other layer; the body is the CLI's message); 404 unknown session; 409 session stopped; 501 without `cp serve`. A remote host that does not advertise `session.skills` refuses a send with skills: 501, "upgrade rupu there", and nothing reaches it. |
| POST | `/api/sessions/:id/archive` | Archive a session. | `host`. |
| POST | `/api/sessions/:id/restore` | Restore an archived session. | `host`. |
| DELETE | `/api/sessions/:id` | Delete a session. | `host`. |

Start a session with `POST /api/agents/:name/session`.

## Messages

The operator's side of a run's message space (see
[`messaging.md`](messaging.md)): read what was said in it, and send into it as
the operator, the way `rupu message list|send` does. The same three endpoints
exist under each root that has a space:

| Root | Path prefix | Id |
|------|-------------|----|
| A workflow run or a standalone `rupu run` | `/api/runs/:id/messages` | `run_…` |
| A session | `/api/sessions/:id/messages` | `ses_…` |
| An agentiflow | `/api/agentiflows/:id/messages` | `af_…` |

An id is served only under the prefix its kind names: a session id under
`/api/runs` is a 404, as is an unknown id.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `…/messages` | The space: `MessagesView {participants, messages, legacy, space, next}`. | `after` (a message id; the page starts after it), `limit` (messages per page; all when absent), `kind` (comma-separated: `post`, `direct`, `broadcast`, `directive`, `retract`, `operator`; anything else is a 400), `host`. Each message is a log line with its `seen` receipts (`{by, turn, at}`) folded in. `space` is `absent` when nothing has joined or written yet. `legacy: true` is an agentiflow from before the log: its board posts and directives in the log's shape. |
| POST | `…/messages` | Send as the operator. | `host`. Body `{to?, body, urgent?}` → `202 {id, delivery}`, where `delivery` is `{resolved, undelivered?}` and each undelivered recipient carries its reason (e.g. `participant_left`). `to` is a participant, a codename, `role:<agent>`, `step:<id>`, `lead` or `broadcast`; absent, it is an agentiflow's lead and everyone elsewhere. `urgent` puts the message first, and ends an agentiflow lead's round early. 400 an empty body or a bad address; 404 unknown id; 409 the run isn't running (finished, or its recorded runner process is gone), or its log is full; 501 without `rupu cp serve`. The message is recorded with `via: "cp"`. The CP has no per-person identity, so `viewer` is never set. |
| GET | `…/messages/stream` | SSE tail of the space's log. | This host's runs only: with a remote `host` it is a 501 (poll `GET …/messages?host=` instead). See below. |

`POST /api/agentiflows/:id/steer` is kept as an alias. Without `stop` it is
exactly `POST /api/agentiflows/:id/messages {to: "lead", body: message, urgent:
now}`, answered the same way. With `stop: true` it queues a graceful stop on
the run's control queue instead (see [Agentiflows](#agentiflows)).

The stream replays the log from the start, then tails it, as the other streams
do. Each frame is an unnamed `data:` line with an `id:` (the line's number in
the log; `Last-Event-ID` resumes after it). The data is one of:

- `{"t": "msg", …}`: a message in the shape `GET` returns, with `seen: []`.
- `{"t": "seen", "id", "by", "turn", "at"}`: a receipt for message `id`.

The stream stays open after the run ends (a finished run's space can still be
read); close it when you're done.

```sh
curl -X POST -H "Authorization: Bearer $CP_TOKEN" -H 'Content-Type: application/json' \
  -d '{"to": "step:review", "body": "skip the vendored code", "urgent": true}' \
  "http://127.0.0.1:7878/api/runs/run_01JEXAMPLE0000000000000000/messages"
# 202 {"id":"msg_01JEXAMPLE...","delivery":{"resolved":["heron#2"]}}
```

### Remote hosts

With `host=<id>`, `GET` and `POST` go to that host. The stream does not.

| Transport | Messages |
|-----------|----------|
| local | Served here. |
| HTTP | Proxied to the host's own endpoints when its `/api/host/info` lists the `run.messages` feature. The coordinator answers from the features it last read (`/api/hosts` probes refresh them), so a host upgraded since that read is still a 501 until the next `/api/hosts` probe picks up its `run.messages`. It reads `/api/host/info` only for a host it has never read. A host that doesn't list it is a 501, "upgrade rupu there". The host's 400, 404 and 409 come back as the same status. |
| SSH | One remote command runs `rupu __features`. Only when the report lists `run.messages` does it run the hidden `rupu __message list\|send`. Any other remote is a 501, naming why: `rupu` isn't on its PATH, or its rupu predates `run.messages`. The helper's refusals keep their status. |
| Tunnel, bucket | 501 with a reason: these transports carry no messages. Message the run on its own host. |

A host that can't be reached is a 502, as for any remote read.

## Launch, agents and workflows

Launches return as soon as the detached process is spawned; all are 501
without `rupu cp serve`.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| POST | `/api/launch/preview` | Who a launch would bill and authenticate as (`PreviewResponse`). | Body `{workflow` *or* `agent, working_dir?, scope_kind?, scope_id?, host?}`. 409 when the launch itself would fail to resolve (dangling customer, bad config). See [`cp-customers-api.md#launch-preview`](cp-customers-api.md#launch-preview). |
| GET | `/api/workflows` | Workflow definitions, global and per project (`WorkflowDto`). | |
| POST | `/api/workflows` | Create a workflow in the global layer. | Body `{raw}` (YAML). 409 if the name exists. |
| GET | `/api/workflows/:name` | `{workflow, yaml, usage, scope, scope_kind, scope_id}`. | |
| PUT | `/api/workflows/:name` | Overwrite a workflow. | `scope_kind`, `scope_id`. Body `{raw}`; the YAML's name must match `:name`. |
| DELETE | `/api/workflows/:name` | Delete a workflow. | `scope_kind`, `scope_id`. |
| POST | `/api/workflows/:name/run` | Launch a run → `{run_id, host_id}`. | Body `LaunchBody {inputs, mode, target, working_dir, host, scope_kind, scope_id}`. `scope_kind` and `working_dir` are mutually exclusive; scope pinning is local-only. |
| POST | `/api/workflows/validate` | Parse-check YAML without saving. | Body `{raw}`. 400 with the parse error. |
| POST | `/api/workflows/generate` | Draft a workflow from a description with a model. | Body `{description, provider?, model?}`. |
| GET | `/api/generate/models` | Providers and models available for generation. | |
| GET | `/api/agents` | Agent definitions, global and per project (`AgentDto`). | Each row carries its frontmatter `skills` and `preload_skills` (omitted when empty) and, when it can't be launched, `load_error`: its `tools:` names an unknown tool, or a granted skill is not installed, does not load, or is ambiguous, checked as `rupu agent validate` checks it against the layers of the row's scope (global; global plus the project for a project agent). |
| POST | `/api/agents` | Create an agent in the global layer. | Body `{raw}` (the `.md` file). 409 if the name exists. |
| GET | `/api/agents/:name` | Agent detail with its prompt and raw file. | `load_error` as in the list. |
| PUT | `/api/agents/:name` | Overwrite an agent. | `scope_kind`, `scope_id`. Body `{raw}`. 400 for a malformed `skills:` / `preloadSkills:` entry (`skills: entry …`); a skill that isn't installed is accepted and shows in `load_error`. `POST /api/agents` checks the same. |
| DELETE | `/api/agents/:name` | Delete an agent. | `scope_kind`, `scope_id`. |
| POST | `/api/agents/:name/run` | Launch an agent run → `{run_id, host_id}`. | Body `AgentRunBody {prompt, mode, target, working_dir, host, scope_kind, scope_id, findings_profile, engagement_profiles, skills, preload_skills, skill_bundle, messaging, run_id, codename}`. `engagement_profiles` (ids; blank → 400) becomes `rupu run --engagement-profile`; a remote host that does not advertise `agent.engagement_profile` refuses it. `skills` / `preload_skills` (skill grant entries; malformed → 400) become `rupu run --skill` / `--preload-skill`; a remote host that does not advertise `agent.skills` refuses them. `skill_bundle` (a manifest `{version, skills: [{layer, name, content_hash}]}`; invalid → 400) becomes `rupu run --skill-bundle`; a coordinator sets it for a placed unit after shipping the skills, and a remote host that does not advertise `agent.skill_bundle` refuses it. `messaging` is a placed unit's workflow `messaging:` switch: `"off"` becomes `rupu run --no-messaging` and `{"tools": [...]}` one `rupu run --messaging-tool` per name (a name that isn't a `board.*` / `msg.*` tool → 400); absent, the agent file decides. A remote host that does not advertise `agent.no_messaging` (for `"off"`) or `agent.messaging_tools` (for tools) refuses it. `run_id` (a `run_…` id; malformed → 400) is the id the run executes under instead of a minted one (advertised as `run.supplied_run_id`); `codename` (malformed → 400) becomes the hidden `rupu run --codename` (advertised as `run.codename`). A coordinator sends either only to a host that advertises it, and a codename a host can't take never blocks the launch. |
| POST | `/api/agents/:name/session` | Start a session → `{session_id, host_id}`. | Body `SessionStartBody {prompt, mode, target, working_dir, host, scope_kind, scope_id, skills, preload_skills}`. `skills` / `preload_skills` (skill grant entries; malformed → 400) become `rupu session start --skill` / `--preload-skill`; a skill the session can't resolve is a 400 with the CLI's message. A remote host that does not advertise `session.skills` refuses them: 501, "upgrade rupu there". |
| POST | `/api/agents/generate` | Draft an agent from a description. | Body `{description, provider?, model?}`. |

```sh
curl -s -X POST -H "Authorization: Bearer $CP_TOKEN" -H 'Content-Type: application/json' \
  -d '{"inputs":{"pr":"42"},"mode":"readonly"}' \
  http://127.0.0.1:7878/api/workflows/review-pr/run
# {"run_id":"01JEXAMPLE0000000000000000","host_id":"local"}
```

## Skills

Installed [Agent Skills](skills.md), read-only: installing and updating stay
`rupu skill` operations. Both routes read the global layer and, with `ws_id`,
that project's layer (`.rupu/skills` in the workspace; none when the project's
`.rupu` is the global dir). An unknown `ws_id` is a 404.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/skills` | Every installed skill, as `rupu skill list` (`SkillsResponse`). | `ws_id`. Rows sorted by name, then layer. Each row loads its skill to show its status. |
| GET | `/api/skills/:layer/:name` | One skill, as `rupu skill show` (`SkillDetail`). | `ws_id`, required for `project`. 400 for a layer other than `global` / `project`, or `project` without `ws_id`; 404 `no installed skill <layer>:<name>`; 500 when the skill's layer can't be read. |

Hosts receive the skills of a placed unit through two more routes, behind the
`agent.skill_bundle` feature in `/api/host/info` (see
[Skills on placed units](skills.md#skills-on-placed-units)). They write only to
`<RUPU_HOME>/cache/skills`.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| POST | `/api/skills/bundle/missing` | Which shipped skills this host lacks. | Body `{skills: [{layer, name, content_hash}]}` → `{missing: [...]}`. 400 for a malformed ref. |
| PUT | `/api/skills/bundle/:hash` | Store one packed skill (the tar is the body). | `layer`, `name`. The tar is vetted and must hash to `:hash`; 400 otherwise, nothing kept. A hash already cached is a success. Body limit is the per-skill cap (32 MiB) plus 64 KiB for the tar headers. |

`SkillsResponse` is `{skills: [SkillRow], warnings: [string]}`. A layer that
can't be read is left out of `skills` and named in `warnings`
(`"project layer: <path>: <error>"`); the response is still 200. A row:

| Field | |
|---|---|
| `name`, `layer` | The directory name and `global` / `project`. |
| `status` | `ok` or `invalid`. |
| `description`, `stamp` | For a valid skill; `stamp` is the short stamp. |
| `error` | Why an invalid skill doesn't load (a name mismatch, a `SKILL.md` over 1 MiB, a symlinked project skill or project `skills/`, …). |
| `warnings` | Frontmatter warnings (an over-limit description), and `install record: <error>` when the skill's install record can't be read. |
| `source` | The install record's `source`, when there is a readable one. |
| `both_layers` | The other layer has a skill of the same name, so a bare grant of it is ambiguous. |

`SkillDetail` is the row's fields plus `path`, `license`, `compatibility`,
`allowed_tools`, `mapped_tools` (the rupu tools `allowed-tools` maps to;
informational, never a grant), `disable_model_invocation`, `metadata`, `files`
(relative to the skill, without `SKILL.md`), `content_hash` (short), `body`
(`SKILL.md` without its frontmatter), `source_record` (the install record as
written) and `source_error` (why the record couldn't be read). The fields
that need a loaded skill are empty for an invalid one. An install record is
read only when it is a regular file of at most 64 KiB, and never through a
symlinked project `skills/`.

## Autoflows

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/autoflows` | Workflows with an `autoflow:` block, with trigger kind and enabled state. | |
| POST | `/api/autoflows/:name/enable` | Set `autoflow.enabled: true` in the file (a `.bak` is kept). | `scope_kind`, `scope_id`. The file is `<name>.yaml`, else `<name>.yml` (as the list and the autoflow runtime read both; `GET /api/workflows/:name` resolves the same way). 404 not an autoflow. |
| POST | `/api/autoflows/:name/disable` | Set it to `false`. | Same. |
| GET | `/api/autoflows/claims` | Issue claims held by the entity engine (`ClaimRow`). | |
| POST | `/api/autoflows/claims/release` | Drop a claim. | Body `{issue_ref}` → `{released}`. |
| POST | `/api/autoflows/claims/requeue` | Queue a manual wake for a claimed issue. | Body `{issue_ref, not_before?}` → `{wake_id}`. 404 no claim. |

## Findings

Query language, tags and report export are covered in depth in
[`coverage.md`](coverage.md#querying-findings).

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/findings` | Every finding across all workspaces. | Query: `ws_id`, `workflow`, `run_id` (the run plus its direct sub-runs), `customer` (the project's *current* assignment), `q` (query language, e.g. `severity>=high tag:triage`). Rows are slim: no `report` body, a `report_summary` on full-profile rows. Response also carries `summary`, `facets` (counts over the scope-filtered, unqueried set) and `tags_unavailable`. A bad `q` is a 400 `{error, token, code, start, end}`. |
| GET | `/api/findings/:id` | One finding in full. | `ws_id`. Adds per-claim `evidence_status` (`current` / `changed` / `missing` / `unknown`), `tag_history`, `tags_editable`. 404 unknown id; **409** when the id exists in several workspaces and no `ws_id` picks one (the message names them). |
| GET | `/api/findings/:id/export` | One finding as a report file. | `format=md\|html\|pdf` (required), `ws_id`. 400 bad format; 501 PDF in a build without the `pdf` feature; 404; 409 ambiguous id, as above. |
| POST | `/api/findings/export` | Several findings as one report. | Body `ExportBody`: `format` (required), `title`, `ids`, `ws_id`, `run_id`, `min_severity`, `owner`, `cwe`, `include_summaries`, `split`. Returns `text/markdown`, `text/html`, `application/pdf`, or `application/zip` when `split`; always an attachment with `nosniff`. 404 when nothing matches. |
| GET | `/api/findings/tags` | Tags in use, most used first. | Query: `ws_id`. |
| POST | `/api/findings/tags` | Add / remove tags on many findings. | Body `{finding_ids, add, remove}` — each id tagged in every workspace that holds it — or `{findings: [{ws_id, id}], add, remove}` — each finding tagged only in its own workspace (what the web sends). Exactly one of `finding_ids` / `findings`; at most 1000. Atomic per workspace; response `TagAcrossResult {workspaces, unknown}` — a workspace whose tag log couldn't be written carries its `error` there. 400 for a bad change (empty, conflicting, too many tags); 500 if writing the tag log fails outright. 404 only when *every* finding is unknown. Recorded as `via: cp`. |
| GET | `/api/findings/:id/artifacts/:sha256` | An artifact or evidence-block file the finding references. | `ws_id`; 409 for an ambiguous id, as above. Text inline as `text/plain`, raster images inline as `image/*`, everything else as an attachment; always `nosniff` + `Content-Security-Policy: sandbox`. 404 if the finding doesn't reference it; 409 if a referenced workspace file changed since it was recorded; a remote artifact is pulled from its host on first view, and a failed pull is 404 `{"unavailable": reason}`. |
| GET | `/api/findings/artifacts/:sha256` | **Host-internal.** Raw blob from this host's artifact store. | Used by a coordinator pulling a remote unit's artifact (`findings.artifact_blob` feature). Not finding-scoped. 404 if not in the store. |

## Coverage and assets

All local-only. `ws_id` is optional; without it the first workspace containing
the target is used.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/coverage` | Every coverage target across workspaces. | `CoverageSummary` rows. |
| GET | `/api/coverage/templates` | Bundled concern templates. | `TemplateSummary` rows. |
| GET | `/api/coverage/templates/:name` | One template. | 404. |
| GET | `/api/coverage/:target` | Target detail: assertions, findings, files touched. | `ws_id`. 404 unknown target. |
| GET | `/api/coverage/:target/catalog` | The concern catalog snapshot (`FlatCatalog`). | `ws_id`. |
| GET | `/api/coverage/:target/audit` | Per-concern gap audit (`AuditReport`). | `ws_id`. |
| GET | `/api/coverage/:target/runs` | Runs that contributed to the target. | `ws_id`. |
| GET | `/api/coverage/:target/diff` | Two runs' contributions side by side (`RunDiff`). | `ws_id`, `base` (default `previous`), `compare` (default `latest`, or a run id). 400 unknown run. |
| GET | `/api/assets` | Asset inventory (`AssetListResponse`). | `ws_id`, `target`. |

## Usage and dashboard

`since` / `until` are RFC 3339 (default: the last 30 days). A value that doesn't
parse is a 400.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/usage` | Token and cost totals with a breakdown (`UsageResponse`). | `since`, `until`, `group_by` (`provider`, `model` (default), `agent`, `workflow`, `host`, `project`), `host`, `customer`. Without `host`, fans out to every host and reports each one's freshness in `hosts`. 404 unknown host; **501** for `?host=<remote>&customer=` (a remote aggregate arrives already summed). |
| GET | `/api/usage/timeline` | Gap-filled spend series. | `since`, `until`, `bucket` (`day` / `week`), `customer`. Local only. |
| GET | `/api/usage/runs` | One row per run per model (`UsageRunRow`). | `since`, `until`, `workspace_id`, `customer`. Local only. |
| GET | `/api/usage/outliers` | Runs costing at least 3× the median of their workflow (`OutlierRun`). | `since`, `until`, `customer`. Needs ≥ 3 runs for a baseline. Local only. |
| GET | `/api/dashboard` | The Overview dashboard (`DashboardResponse`). | `range` (`7d`, `30d`, `all`), `host`, `customer`. Same `host` / `customer` rules as `/api/usage`. |

Endpoints that take `customer` add an `X-Rupu-Hosts-Without-Customer` response
header naming hosts whose rows couldn't be attributed — see
[`cp-customers-api.md`](cp-customers-api.md#remote-hosts).

## Netflow

Netflow reads take `from` / `to` (inclusive RFC 3339) and comma-separated
dimension filters `workflow`, `origin`, `org` and `host`. Here `host` filters
by **remote endpoint** (`host:port`), not by registered rupu host.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/runs/:id/netflow` | One run's flows (`NetflowResponse`). | Proxied to the run's host when remote. 404 unknown run. |
| GET | `/api/projects/:id/netflow` | Project-scope flows. | `incomplete` lists remote hosts not included. |
| GET | `/api/netflow` | Every workspace's flows. | |
| GET | `/api/netflow/graph` | Source → endpoint graph (`GraphView`). | `scope` (`run:<id>`, `project:<id>`, absent = global), `from`, `to`. Dimension filters are not applied. |
| GET | `/api/netflow/explorer` | Explorer aggregates: sankey, timeline, histogram, KPIs. | `scope` plus the filters above. |
| GET | `/api/netflow/index` | Resident netflow index status (size, budget, evictions). | |

## Projects and code

An unknown `ws_id` is a 404 on every route except the list.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/projects` | Projects with rollups (`ProjectRow`). | `customer`. |
| GET | `/api/projects/:ws_id` | Project overview (`ProjectDetail`): run/session counts, coverage, 10 recent runs, usage. | |
| GET | `/api/projects/:ws_id/runs` | The project's runs. | `offset`, `limit`, `q`, `facets`. |
| GET | `/api/projects/:ws_id/sessions` | The project's sessions, most recently updated first. | `offset`, `limit`, `q`, `facets`. |
| GET | `/api/projects/:ws_id/coverage` | Per-target coverage summary. | |
| GET | `/api/projects/:ws_id/coverage/assessed` | Assessed percentage (`{assessed_pct}`). | Computed by a full audit; slower than the summary. |
| GET | `/api/projects/:ws_id/agents` | Agents the project can use (project overrides global by name). | Every row's `load_error` is checked against the global and this project's skill layers, so a bare skill name installed in both is ambiguous here. |
| GET | `/api/projects/:ws_id/workflows` | Workflows the project can use. | |
| GET | `/api/projects/:ws_id/autoflows` | Autoflow definitions the project can use. | |
| GET | `/api/projects/:ws_id/tree` | One directory of the checkout (`TreeResult`). | `path` (workspace-relative). 400 for an absolute path, `..`, or a path leaving the workspace. Skips `.git`, `.rupu`, `node_modules`, `target`. |
| GET | `/api/projects/:ws_id/source` | One file's contents (`FileContent`). | `path`. A missing, binary or > 2 MiB file is 200 with `available: false` and a `reason`. |
| GET | `/api/projects/:ws_id/files` | Flat file list for search. | Capped at 20 000 (`truncated`). |

## Hosts, workers and fleet

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/hosts` | Hosts with health (`HostView`). | Local first, always online. Remote status comes from the probe cache (fresh 15 s; stale is served while one background refresh runs; > 5 min or never probed is awaited). |
| GET | `/api/hosts/registered` | Registered hosts with no probe (`RegisteredHostView`). | What the per-host loaders iterate. |
| POST | `/api/hosts` | Register an HTTP control-plane host. | Body `AddHostBody {name, base_url, token?}`; the token goes to the keychain. 501 without `cp serve`. |
| POST | `/api/hosts/ssh` | Register an SSH host. | Body `AddSshHostBody {name, host, port?, identity_file?}`. |
| POST | `/api/hosts/bucket` | Register an object-store bucket host. | Body `AddBucketHostBody {name, url, prefix?}`. |
| POST | `/api/hosts/node` | Enroll a tunnel node. | Body `{name}`. Returns `EnrollNodeResponse {host, command, token}`; the plaintext token is shown once, only its hash is stored. 400 empty name. |
| DELETE | `/api/hosts/:id` | Remove a host. | 204; a tunnel host's live node connection is closed at once (its reconnect then fails enrollment). 400 for `local`; 404 unknown id. |
| GET | `/api/workers` | Workers with run activity (`WorkerView`). | |
| GET | `/api/host/info` | **Host-internal.** This host's version, capabilities and feature list (`HostInfoResponse`). | A coordinator's HTTP connector reads `features` before using an optional capability (e.g. `run.messages`, [Messages](#messages)). |
| GET | `/api/node/connect` | **Host-internal.** WebSocket for `rupu node` tunnel peers. | Outside the bearer check. See [Node tunnel](#node-tunnel). |

### Workspace sync (host-internal)

A coordinator placing a run on an HTTP host ships the workspace, runs there,
and pulls back the diff. The web UI never calls these.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| POST | `/api/workspace/stage` | Stage a packed workspace (`rupu_workspace::Payload`, raw bytes). | Returns `{working_dir}`. 413 over the compiled 256 MiB cap; 400 over `[cp].max_workspace_bytes` or a malformed payload. |
| GET | `/api/workspace/delta` | Diff a staged dir against its baseline (`Delta`, octet-stream) and remove it. | `dir`. 400 outside the sync root. |
| DELETE | `/api/workspace/discard` | Best-effort cleanup after a failed launch. | `dir`. |

## Config, models and tools

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/config` | Effective config with per-key provenance and each layer's raw TOML (`ConfigView`). | `project` (ws id) or `customer` (slug), not both. Includes `status {bind, token_set, restart_required_keys}` — the token itself is never returned. A malformed layer is 200 with `layer_error`. |
| PUT | `/api/config/global` | Write the global config. | Body `ConfigWriteBody {raw?, patch?}`. 400 invalid TOML. Validated, backed up, written atomically. Response `{ok, restart_required}`: the start-time-only keys (`cp.{autoflow_reconcile,cron_tick,gate_sweep}_{enabled,interval_secs}`, `agentiflow.reaper_enabled`, `netflow.asn_auto_refresh` — what `cp serve`'s background loops capture at start) whose saved value now differs from the one the server started with; they apply after a restart. Everything else applies at once. |
| PUT | `/api/config/customer/:slug` | Write a customer's layer. | 400 for a globally locked key or a layer that doesn't validate on top of global. |
| PUT | `/api/config/project/:id` | Write a project's `.rupu/config.toml`. | 400 for a locked key or a project whose `.rupu/` is the global dir. |
| PUT | `/api/config/policy` | Set the global `[policy].lock`. | Body `{lock: [...]}`. |
| GET | `/api/models` | Model catalog per provider, with limits and provenance (`CatalogProvider`). | 501 without `cp serve`. |
| POST | `/api/models/refresh` | Refetch provider model lists. | Body `{provider?}`; empty = all. 10 s per provider. 400 unknown provider. |
| GET | `/api/tools` | MCP tool catalog with JSON input schemas (`ToolsResponse`). | Feeds the workflow editor's action forms. |
| GET | `/api/repos` | Repositories visible to the configured SCM accounts. | 501 with no SCM credentials. |
| GET | `/api/fs/browse` | Subdirectories of a server path, for the folder picker. | `path` (default `$HOME`). Only under `$HOME` or a registered project: anything else (missing or not) is a 403, and `parent` is `null` at the top of a browsable directory. |

## Customers

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/customers` | Customers with rollups (`archived`, `range=7d\|30d\|all`). |
| POST | `/api/customers` | Create (201). |
| GET | `/api/customers/:slug` | Detail with projects and rollup. |
| PATCH | `/api/customers/:slug` | Update metadata. |
| DELETE | `/api/customers/:slug` | Delete (409 with the project list while projects are assigned). |
| POST | `/api/customers/:slug/archive` | Archive. |
| POST | `/api/customers/:slug/unarchive` | Unarchive. |
| PUT | `/api/customers/:slug/projects/:ws_id` | Assign a project. |
| DELETE | `/api/customers/:slug/projects/:ws_id` | Unassign a project. |

Bodies, status codes, attribution and `?customer=` semantics:
[`cp-customers-api.md`](cp-customers-api.md).

## Agentiflows

Local host only, except the message space ([Messages](#messages)), which takes `host`.

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/api/agentiflows` | All agentiflow runs, newest first (`{rows}`). | |
| GET | `/api/agentiflows/:id` | One run: record, definition, budget, events, units, lead transcripts (`AgentiflowDetail`). | 404 unknown or malformed id. |
| GET | `/api/agentiflows/:id/messages` | The run's message space (`MessagesView`). | See [Messages](#messages). |
| POST | `/api/agentiflows/:id/steer` | Message the lead, or stop the run. | Body `SteerRequest {message, now?, stop?}`. Without `stop`: the alias of `POST /api/agentiflows/:id/messages {to: "lead", body: message, urgent: now}`, with its answers (`202 {id, delivery}`). With `stop: true`: queues a graceful stop → `200 {queued: true}`; 400 empty message; 404 unknown run; 409 when nothing would read it — the run isn't `running`, or its recorded coordinator process is gone (a dead coordinator leaves `running` on disk until the reaper closes it); 501 without `rupu cp serve`. |

## Node tunnel

`GET /api/node/connect` upgrades to a WebSocket for `rupu node` peers that dial
home (a tunnel host). It is **host-internal** and sits outside the bearer check;
it authenticates with the node's own enrollment token:

1. Enroll the node with `POST /api/hosts/node`, which returns a one-time token
   and the `rupu node --cp-url ws://<cp>/api/node/connect --token … --node-id …`
   command to run on the node. Only the token's hash is stored.
2. Within 10 s of connecting, the node sends a `Hello` frame with its node id,
   token, capabilities and version. An unknown node or bad token closes the
   socket.
3. The CP answers `Welcome` with its own capabilities, then pings every 30 s.
   The node streams run artifacts (transcripts, coverage) and finished-run
   notices back over the same socket, and serves artifact pulls on request.

A newer connection for the same node replaces an older one. Frames are JSON
text messages; the shapes are `rupu-cp`'s `node::protocol::Frame`.

## Health

| Method | Path | Purpose | Notes |
|--------|------|---------|-------|
| GET | `/healthz` | Liveness: `200 ok` (plain text). | Never requires the token. Response headers `x-rupu-pid` and `x-rupu-version` identify the serving process; a second `cp serve` that loses the bind race uses them to name the PID holding the port. |

```sh
curl -si http://127.0.0.1:7878/healthz
# HTTP/1.1 200 OK
# x-rupu-pid: 41207
# x-rupu-version: 0.82.0
#
# ok
```
