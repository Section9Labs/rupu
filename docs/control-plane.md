# Control plane — operator guide

`rupu cp serve` runs the **control plane**: an HTTP server that serves the
embedded web UI and a JSON API over the same file-based stores the CLI uses
(`~/.rupu/` and each project's `.rupu/`). There is no database. Anything the
CLI started — a `rupu run`, a cron trigger, an autoflow — shows up in the
browser, and anything the browser starts is an ordinary run the CLI can see.

This guide covers starting and securing the server, the background work it
does, and every page of the web UI. The HTTP API is documented separately in
[`cp-api.md`](cp-api.md); the customer-scoping endpoints in
[`cp-customers-api.md`](cp-customers-api.md).

- [Starting it](#starting-it)
- [Authentication](#authentication)
- [Background loops](#background-loops)
- [Navigation shells](#navigation-shells)
- [Pages](#pages)
- [Launching runs](#launching-runs)
- [Approvals, cancel and pause](#approvals-cancel-and-pause)
- [Sessions](#sessions)
- [Authoring: workflow editor and Agent Builder](#authoring-workflow-editor-and-agent-builder)
- [Settings, config and policy](#settings-config-and-policy)
- [Hosts and per-host loading](#hosts-and-per-host-loading)
- [Live updates](#live-updates)
- [Findings, code and source previews](#findings-code-and-source-previews)
- [Usage and network](#usage-and-network)
- [Customers and agentiflows](#customers-and-agentiflows)

---

## Starting it

```sh
rupu cp serve
#   ➜  rupu Control Plane  →  http://127.0.0.1:7878
```

| Flag | Default | Meaning |
|------|---------|---------|
| `--bind <addr>` | `127.0.0.1:7878` | Address and port to listen on. |
| `--token <token>` | none | Require the token on every `/api/*` route — a bearer header, or the browser cookie the printed link sets (see [Authentication](#authentication)). |
| `--no-open` | off | Don't open the URL in a browser. By default the URL is opened when running in a terminal; it is always printed. |
| `--max-open-files <N>` | — | Raise the process's open-file limit (overrides `RUPU_MAX_OPEN_FILES` and `[runtime].max_open_files`). |

The web UI is compiled into the `rupu` binary; nothing else needs to be
installed. `RUPU_HOME` moves the global directory the server reads, which is
the easy way to try it against an empty store:

```sh
RUPU_HOME=$(mktemp -d) rupu cp serve --bind 127.0.0.1:9100 --no-open
```

**One server per port.** If the bind address is taken, `cp serve` exits
non-zero at once. When the occupant is another rupu control plane, the error
names its PID — `/healthz` answers `ok` with `x-rupu-pid` and `x-rupu-version`
response headers, which is how it finds out.

**Restart after upgrading.** `cp serve` is a long-running daemon: installing a
new `rupu` binary does not change a running server. Stop it and start it again.
`bind` and `token` are also restart-only settings.

### Read-only vs full runtime

The HTTP layer (`rupu-cp`) is a thin adapter. The pieces that execute things —
launching runs, resuming approved gates, writing config — are installed by
`rupu cp serve` as adapters. An endpoint whose adapter is missing answers
`501 Not Implemented` (for example `"resuming a paused run requires rupu cp
serve"`). Under `rupu cp serve` every adapter is present.

---

## Authentication

Without `--token` the API is open to anything that can reach the bind address.
That is the intended posture on `127.0.0.1`. **Do not bind a non-loopback
address without a token.**

With `--token`, every `/api/*` route needs the token, in one of two forms:

- **Scripts and peer control planes** send `Authorization: Bearer <token>`.
- **Browsers** sign in once. The URL `cp serve` prints (and opens) carries
  `?token=<token>`. Opening it sets a cookie and redirects (303) to the same
  page without the parameter, so the token leaves the address bar and the
  history at once. From then on the web UI's requests, live streams included,
  carry the cookie. To sign another browser in, open
  `http://<host>:<port>/?token=<token>` there. A wrong token in that link is
  a 401 and sets nothing.

The cookie is `HttpOnly`, `SameSite=Strict`, lasts 30 days, and is named
`rupu_cp_token_<port>`, so control planes on different ports never overwrite
each other. Its value is derived from the token (an HMAC), not the token
itself, and it is refused as a bearer: browsers send `localhost` cookies to
every port, so another local server could read it, but it never learns the
token. A write (anything but `GET`/`HEAD`/`OPTIONS`) authenticated by the
cookie must also carry an `Origin` naming this server, so a page served from
another origin (another `localhost` port included) can't drive runs with it.
Behind a reverse proxy, preserve the `Host` header: the cookie name and the
origin check both read it. Restarting with a different token makes the old
cookie a 401 until the browser opens the new link.

Tokens are compared in constant time. A missing or wrong one is a `401` with
the usual `{"error": ...}` body. `/healthz`, the node WebSocket
(`/api/node/connect`, authenticated by its own enrollment token) and the static
UI (HTML, JS, client-side routes) stay open. They hold no data: everything the
UI shows comes from `/api/*`.

```sh
rupu cp serve --bind 127.0.0.1:9100 --token "$CP_TOKEN" --no-open
curl -H "Authorization: Bearer $CP_TOKEN" http://127.0.0.1:9100/api/runs
```

The tracing log prints the URL without the token. Settings → Runtime status
only ever shows whether a token is set, never the token.

---

## Background loops

Besides the HTTP server, `cp serve` runs background work in-process so that
autoflows, triggers and gate timeouts work without an external `cron`:

| Loop | What it does | Keys |
|------|--------------|------|
| Resume worker | Every 4 s, claims runs the UI marked for resume (an approved gate, a resume of a paused run) and spawns a detached `rupu workflow approve` / `rupu workflow resume --if-unfinished` child for each. | always on |
| Autoflow reconcile | The same entry point as `rupu autoflow tick`: issue and PR entity autoflows. | `[cp].autoflow_reconcile_enabled`, `[cp].autoflow_reconcile_interval_secs` |
| Cron tick | `rupu cron tick`'s core: cron- and polled-event-triggered workflows. | `[cp].cron_tick_enabled`, `[cp].cron_tick_interval_secs` |
| Gate sweep | Fires overdue gates' `on_timeout` routing (reject runs the `on_reject` chain; approve spawns a detached `workflow approve`) and reaps orphaned `Running`/`Pending` runs whose recorded `runner_pid` is dead, marking them `Failed`. | `[cp].gate_sweep_enabled`, `[cp].gate_sweep_interval_secs` |
| Agentiflow reaper | On the gate-sweep tick: finalizes an agentiflow run whose coordinator died. | `[agentiflow].reaper_enabled` |
| ASN refresh | On the gate-sweep tick: refreshes the netflow ASN table when missing or stale. | `[netflow].asn_auto_refresh` |
| Bucket poller | Every 15 s, reads results from bucket-transport hosts. | always on (no-op without bucket hosts) |

All enabled flags default to `true`, all intervals to 60 s. The gate-sweep tick
runs if *any* of its three duties is enabled; each duty is then checked on its
own, so turning off gate timeouts doesn't stop the reaper. See
[`configuration.md#cp`](configuration.md#cp).

```toml
# ~/.rupu/config.toml
[cp]
cron_tick_interval_secs = 30   # tick triggers twice a minute
gate_sweep_enabled = false     # no gate timeouts; reaper and ASN refresh still run
```

---

## Navigation shells

The UI ships two sidebars over the same pages:

- **v1** (default) lists every page in grouped sections.
- **v2** folds them into seven destinations, each a tab bar whose active tab
  lives in `?tab=`: `/overview`, `/activity` (agents · workflows · autoflows ·
  sessions), `/projects`, `/security` (findings · coverage · catalog),
  `/library` (agents · workflows · autoflows), `/fleet` (hosts · workers), and
  `/customers`.

Pick one in the global config. It is read on every page load, so a browser
refresh applies it:

```sh
rupu config set ui.cp.shell v2
```

Under v2 the old URLs redirect (`/agents` → `/library?tab=agents`, `/hosts` →
`/fleet`, `/findings?q=…` → `/security?tab=findings&q=…`), so bookmarks keep
working. The ⌘K palette jumps to any page, run or session.

---

## Pages

| Route | Page |
|-------|------|
| `/dashboard` (v1), `/overview` (v2) | Counts, needs-you items, and token spend over time per model. |
| `/runs/workflows`, `/runs/agents`, `/runs/autoflows`, `/runs/agentiflows` | Run lists per kind, filterable by host, status, time window and customer. `/runs` redirects. |
| `/runs/:id` | Live run view: the workflow's real graph (forks, joins, gates, actions, fan-out units), the focused step's transcript, events, findings and network tabs, and the run's controls. |
| `/transcript?path=…` | A transcript on its own page, for agent runs, sessions and sub-agents with no workflow graph. Carries `host` and `run` for a remote transcript. |
| `/sessions`, `/sessions/:id` | Session list and chat view. |
| `/events` | Live Events: vitals strip, a merged stream of the event firehose and newly filed findings, and a per-project roster. |
| `/usage` | Spend over time, pivoted by model, provider, agent, workflow, host or project; outlier panel. |
| `/netflow` | Global network explorer. |
| `/agents`, `/agents/new`, `/agents/:name` | Agent definitions, the Agent Builder, and an agent's detail page (definition, recent runs, Edit / Delete / Run). |
| `/workflows`, `/workflows/:name` | Workflow definitions and the visual workflow editor. |
| `/autoflows` | Workflows with autoflow triggers, each with an enable/disable toggle. |
| `/projects`, `/projects/:wsId` | Project list; project detail with tabs `runs`, `findings`, `code`, `sessions`, `coverage`, `network`, `config` (each its own sub-route). |
| `/projects/:wsId/definitions` | Agents, workflows and autoflows the project can use, global and project-scoped. |
| `/findings`, `/findings/:id` | Findings table and the per-finding report page. |
| `/coverage`, `/coverage/templates`, `/coverage/:target` (+ `/catalog`, `/audit`, `/gap`, `/diff`) | Coverage status per target and its sub-views; bundled concern templates. |
| `/assets` | Asset inventory across every project. |
| `/agentiflows/:id` | Agentiflow run detail. |
| `/hosts`, `/hosts/:id` | Registered hosts with health; one host's detail and its runs. |
| `/workers` | Machines and identities that have run something, with capabilities and last-seen. |
| `/customers`, `/customers/:slug`, `/customers/:slug/:tab` | Customer management. |
| `/settings` | Settings (see below). |

Every route is bookmarkable. Unknown non-API paths serve the app (the router
resolves them client-side); an unknown `/api/...` path is a JSON 404.

---

## Launching runs

A **Run** button on a workflow or agent (list row, detail page, Library) opens
the launcher: target, inputs or prompt, permission mode, and — with remote
hosts registered — the host.

**Scope-aware launch.** A definition lives in a scope: global (`~/.rupu/`) or a
project (`<repo>/.rupu/`). The launcher sends the row's `scope_kind` and
`scope_id` with the launch so the run uses exactly that definition and runs in
that project, rather than whichever file of the same name a directory lookup
would find. Scope pinning applies to local launches only and is mutually
exclusive with an explicit `working_dir`.

**Launch preview.** Before you launch, the launcher's billing panel shows the
customer the run will be attributed to and the provider, fallback and SCM
accounts it will authenticate as, with where each came from. It is served by
`POST /api/launch/preview`, which resolves the same way a launch does — a
dangling customer assignment is a 409 here exactly as it would fail the launch.
See [`cp-customers-api.md#launch-preview`](cp-customers-api.md#launch-preview).

A launch returns as soon as the run is spawned: the run is a **detached
subprocess** (`rupu workflow run …` / `rupu run …`) owned by nobody, so closing
the browser or restarting `cp serve` doesn't stop it.

---

## Approvals, cancel and pause

Because runs are detached processes, a control action's `200` means *recorded*,
not *done*. The run's own status, arriving on the live stream, is what confirms
it.

**Approve / reject a gate.** A run parked at an approval gate shows Approve and
Reject — per gate, when a graph workflow has several parked at once.

- *Approve* records the decision on the run and sets a resume marker. The web
  process never executes the run itself: the **resume worker** inside `cp serve`
  (polling every 4 s) claims the marked run under a lease and spawns a detached
  `rupu workflow approve --gate <step>` (or `rupu workflow resume
  --if-unfinished` when the run has other recorded decisions), which resumes
  it. If the run's original runner is still alive (a DAG run with other
  branches executing), the decision is handed to that runner instead.
- *Reject* records the decision; a DAG run prunes only what is reachable solely
  through that gate and runs its `on_reject` chain, a linear run finalizes.
- A gate that was already decided, or a run that is no longer awaiting
  approval, is a 409.

**Cancel.** A running or pending run is marked `Cancelled` and its recorded
runner process is sent SIGTERM, which stops it at the next safe point. A run
parked at a single approval gate with no live runner is cancelled by
rejecting it (it ends `Rejected`).
Cancelling a run that already finished is a 409. Resumes started by the
resume worker are separate processes too, so a resumed run cancels the same
way.

**Pause and resume.** *Pause* writes a pause marker in the run's directory;
the detached runner polls for it and stops cooperatively at its next safe
boundary, and the run shows `paused`. Pausing a run that isn't running is a
409. *Resume* marks it for the resume worker, which spawns
`rupu workflow resume --if-unfinished` — the flag makes the child refuse a run
that finished in the meantime rather than retry it. On resume, work the dead
runner left mid-flight is continued from its transcript rather than restarted
(see [`using-rupu.md#resuming-interrupted-work`](using-rupu.md#resuming-interrupted-work)). Resuming a run
that isn't `paused` is a 409.

Controls carry `?host=` for a run on a remote host. Pause and resume are not
available on every transport (a 501 says so); see [Hosts](#hosts-and-per-host-loading).

**Archive, restore, delete.** Finished runs and sessions can be archived (out of
the active lists), restored, or deleted. `[storage].archived_session_retention`
and `archived_transcript_retention` are the default cutoffs for `rupu session
prune` / `rupu transcript prune`; archiving itself prunes nothing.

---

## Sessions

`/sessions/:id` is a chat view over a persistent agent session. Type a prompt
and the turn streams back token by token, with tool calls and thinking in
position. Start a session from an agent's page. Sending queues the turn for the
session's worker, which is a long-lived daemon: upgrading `rupu` doesn't change
a running session until it is stopped (`rupu session stop`).

---

## Authoring: workflow editor and Agent Builder

**Workflow editor** (`/workflows/:name`). A drag-and-drop graph on top and the
YAML below, kept in lock-step; YAML is the source of truth and every save goes
through the orchestrator's own validation (`POST /api/workflows/validate`). Each
step kind has its own silhouette (branch = diamond, action = parallelogram,
approval gate = trapezoid, `for_each` = hexagon). The right rail has Blocks
(palette of step kinds and every MCP connector tool), Step (a per-kind form;
action steps get a form generated from the tool's JSON schema via
`/api/tools`), Settings and Reference. Expression fields (`prompt`, `when`,
branch conditions) get highlighting and autocomplete for inputs, earlier steps'
outputs and loop locals. **New workflow** can generate a draft from a
description with a model.

**Agent Builder** (`/agents/new`). A card composer: drag field cards (identity,
prompt, model, tools, permission, reasoning, output…) onto a canvas and watch
the `.md` file build beside it. **Cards · Raw · AI** switches between the
composer, the raw file, and describing the agent for a model to draft. Existing
agents open in a code editor from their detail page.

There are no feature flags: `[cp].agent_authoring_ui` and
`[cp].workflow_editor_ui` are retired and ignored (with a warning).

---

## Settings, config and policy

`/settings` edits the **global** `~/.rupu/config.toml`:

- Typed tabs: **General, Providers, Models, Autoflow, SCM / Issues, Pricing,
  CP-Runtime.** Each field shows its effective value, the layer that won, and a
  lock toggle.
- **Raw** — the whole TOML file, read-only until you click Edit; validated on
  save.
- **Policy** — every key with a lock checkbox; writes `[policy].lock`.
- **Runtime status** — restart-required settings (`bind`, `token` — masked to
  "set / not set") and what this server has installed.

Each project's **Config** tab (`/projects/:wsId/config`) edits that project's
`.rupu/config.toml`; a customer's layer is edited from its Customers page. Keys
locked in a higher layer are read-only there. Every write is validated against
the layers above it, backed up, and written atomically; project paths are
confined to the project's `.rupu/`.

**Models** lists every model each configured provider offers, with input limit,
output cap and where each number came from (your `[[providers.X.models]]` entry
or the provider's live list, cached an hour). **Refetch** pulls a fresh list.

---

## Hosts and per-host loading

One control plane can drive rupu on other machines. Register hosts on
`/hosts` (v2: `/fleet`) — HTTP, SSH, a dial-home tunnel node, or an object-store
bucket — and launch, watch and control runs there. Setup is in
[`using-rupu.md#remote-hosts-rupu-host`](using-rupu.md#remote-hosts-rupu-host).

**Loaded host by host.** The Activity tables, the Usage headline and ⌘K's runs
and sessions ask each host separately (`?host=<id>`) and merge answers as they
arrive. A fast host paints at once; a slow one fills in later; each request is
bounded at 45 s. A host that doesn't answer is shown as:

| Shown | Status | Meaning |
|-------|--------|---------|
| *offline* | 502 | The host couldn't be reached or failed (SSH down, connection refused, an error). |
| *unavailable* | 501 | The host answered but can't serve this listing — typically an older `rupu` that lacks the command. |
| (row dropped) | 404 | The host id is no longer registered. |

`/api/hosts` serves each host's last health probe from a cache (fresh for 15 s;
stale answers are served while one background refresh runs), so opening a page
never waits on a fresh SSH probe. SSH listings are shared through a 5 s
single-flight cache that any mutation clears.

---

## Live updates

The UI updates over server-sent events:

- `/api/events/stream` — the global firehose behind Live Events, the new-run
  pill and status patching. It covers **orchestrator runs** (workflows,
  autoflows); a standalone `rupu run` agent run never enters the run store and
  so doesn't appear on it.
- `/api/runs/:id/log` — one run's step events, for the run view.
- `/api/transcript/stream` — one transcript file, tailed as it grows.

Runs the CP launched stream straight from the executor; runs started elsewhere
are tailed from their `events.jsonl`, so the live view looks the same however
the run began. Format details are in [`cp-api.md#server-sent-events`](cp-api.md#server-sent-events).

---

## Findings, code and source previews

`/findings/:id` renders a finding's report: a section rail, the ownership/risk
ledger, a completeness meter, evidence claims badged `current` / `changed` /
`missing` against the file on disk, an artifact browser (remote artifacts pulled
on first view), tags with history, and Markdown / HTML / PDF export. Tables
filter with the findings query language. Details:
[`coverage.md#viewing-reports-in-the-control-plane`](coverage.md#viewing-reports-in-the-control-plane),
[`coverage.md#querying-findings`](coverage.md#querying-findings),
[`coverage.md#tagging-findings`](coverage.md#tagging-findings).

**SCM permalinks.** When a finding's project has a known remote, its rows carry
a "View on repository" link to the file and line range on the forge's web UI,
built from the remote URL and the project's branch.

**Project code.** `/projects/:wsId/code` is a file tree and syntax-highlighted
viewer for the project's checkout, with findings shown inline at their lines.

**Transcript source previews.** In a run transcript, an `ast_grep` tool call's
matches open a line-numbered **source slice** around the match
(`GET /api/runs/:id/source`) and a **CST viewer** showing the parsed syntax tree
of that region (`GET /api/runs/:id/ast`). Both read the file in the run's own
working directory (or from its host); a file that's gone or outside the
workspace shows the reason instead.

---

## Usage and network

**Usage** (`/usage`) comes from a per-run ledger written as tokens are spent,
so in-flight steps, retried units, sub-agents and compaction calls all count.
Unpriced spend is counted in a banner rather than hidden. The outlier panel
flags runs that cost far more than their workflow usually does.

**Network** (`/netflow`, a run's Network tab, a project's Network tab) is the
netflow explorer: connections made by agents' tools and their subprocesses,
with ASN enrichment. The server keeps a resident index of the netflow ledgers
(`[netflow].cp_index_budget_mb`); its state is at `GET /api/netflow/index`. See
[`netflow.md`](netflow.md).

---

## Customers and agentiflows

- **Customers.** A header picker narrows the UI to one customer (or *no
  customer*); pages that can't be filtered say so. The Customers page manages
  customers, assignments, priced rollups and each customer's config layer. See
  [`cp-customers-api.md`](cp-customers-api.md).
- **Agentiflows.** `/runs/agentiflows` and `/agentiflows/:id` (Flow, Assets,
  Findings, Messages, Transcript, Events tabs; a steering box on Messages while
  the run is live). See [`agentiflows.md#10-in-the-control-plane`](agentiflows.md#10-in-the-control-plane).

## See also

- [`cp-api.md`](cp-api.md) — HTTP API reference
- [`cp-customers-api.md`](cp-customers-api.md) — customer scoping and launch preview
- [`configuration.md`](configuration.md) — `[cp]`, `[ui.cp]`, `[policy]`, `[netflow]`
