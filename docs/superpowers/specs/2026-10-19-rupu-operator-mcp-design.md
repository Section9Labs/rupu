# Operator MCP: full operator parity over MCP — design

- **Status:** drafted from brainstorming, 2026-10-19; awaiting matt's approval.
- **Builds on:** W4 (`docs/superpowers/specs/2026-10-07-rupu-tool-and-launch-architecture/W4-mcp-as-transport.md`, the MCP server as a transport over the tool catalog), #879 (protocol compliance: ping, bad lines, version negotiation, concurrent calls with cancel), the CP API (`docs/cp-api.md`).

## Intent

An external agent, such as Claude Code, becomes rupu's operator. It can do everything matt does in the CP and from the CLI, end to end:

- **Author.** Agents, workflows, autoflows, agentiflow definitions, skills, the stock fleet.
- **Run.** Agent runs, sessions, workflow runs, agentiflows, on any host.
- **Control.** Approve, reject, pause, cancel, resume and retry runs. Steer and stop agentiflows. Release and requeue claims.
- **Talk.** Send operator messages to any participant of a live run, and read the message log.
- **Review.** Transcripts, events, run graphs, children, usage, netflow, coverage, assets. Query, verify, tag, export and import findings, and pull their artifacts.
- **Watch.** Follow long work without blind polling.

Today `rupu mcp serve` serves only the `rupu-tools` catalog: files, shell, SCM, issues, findings. It can launch nothing, control nothing and watch nothing. The CP has ~161 endpoints the MCP doesn't reach, and the CLI has capabilities that neither the CP nor the MCP has.

## Decisions (from brainstorming)

1. **Full access, no guardrail layer in rupu.** The MCP exposes everything. The harness of whoever runs the external agent decides what the agent may call: Claude Code's permission prompts, allowlists and so on. rupu adds no permission mode, admin tier or confirmation step to MCP calls. (matt: "it is the operator … the operator running the external agent controlling the MCP decides".)
2. **One command, one process.** `rupu cp serve` serves the web UI and the MCP from the same process and port:
   - `rupu cp serve`: full mode, web UI + `/mcp`.
   - `rupu cp serve --no-mcp`: web only.
   - `rupu cp serve --no-web`: MCP only.

   The MCP is **HTTP only**: clients connect to `/mcp` on the running daemon. The flag names are my call.
3. **stdio and `rupu mcp serve` are sunset.** Nothing uses stdio internally. No client config on matt's machine registers rupu as an MCP server, and in the repo only tests and docs reference `rupu mcp serve` (checked 2026-10-19). For one release, `rupu mcp serve` prints a deprecation notice on stderr and runs `rupu cp serve --no-web` (HTTP), then it is removed. It passes `--tools` through (M6) and ignores `--mode` / `--transport` with a notice. A client config that spawned it over stdio stops working, and the notice prints the `claude mcp add --transport http …` line that replaces it. The stdio transport code in `rupu-mcp` goes. The in-process transport stays for tests.
4. **Parity by construction: MCP tools call the CP's own endpoint code.** Each operator tool builds an HTTP request and sends it through the CP's axum router in process (`tower::ServiceExt::oneshot`, no socket). None of `rupu-cp`'s ~26k lines of handler logic is rewritten or duplicated. A new CP endpoint becomes a new tool once it gets a table entry: name, description, schema, hints.
5. **The "and more" goes into the CP API first.** Every capability the CLI has and the CP lacks (M8) is added as a CP endpoint, and so reaches the web UI and the MCP together. The MCP gets no logic of its own beyond `wait` and resources.
6. **Agent-oriented additions:**
   - a `wait` tool (M7)
   - MCP tool hints (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) on every tool, so the harness can tell a read from a delete. Hints gate nothing.
   - structured results (`structuredContent`)
   - docs, schemas and registries served as MCP resources (M9)
7. **Attribution, not authorization.** The two places that record an operator surface today also record the MCP: operator message lines (`OperatorVia`, `rupu-fleet`) and finding tag events (`OperatorSurface`, `rupu-coverage`). Other writes (approve, launch, config…) record no surface today, and this design adds none (my call).

   **Compatibility (my call).** The existing `via` value stays `cp` on the wire. A new optional field, `client: "mcp"`, rides next to it. Both enums are closed and `read_log` drops a line it can't parse, so a new `via` value would make an older binary sharing the same `RUPU_HOME` miss operator messages. An older session worker or a live run would be enough to cause it. An additive field is ignored instead. Writes the CP forwards to remote hosts keep the host's own `cp` attribution.

## Goals

- Every operator capability the CP API offers is an MCP tool, and a test keeps it that way.
- Every operator capability only the CLI has is added to the CP API and the MCP (M8).
- An agent can wait for a gate, a run end, a message, a finding or a session turn, with a cursor, without polling blind.
- An agent can learn rupu from the MCP alone: formats, query languages, schemas, profiles.
- One process serves the UI, the API and the MCP, so the background loops run exactly as they do today.

## Non-goals

- A permission or approval layer in rupu for MCP calls (decision 1).
- MCP access to secrets. `rupu auth login` stays human-only, since it needs a browser or device code and handles credentials. `auth status` is in scope (M8).
- Infrastructure lifecycle through MCP: starting or stopping `cp serve`, `rupu node`, webhook or autoflow daemons, `rupu update`, shell completions.
- MCP for the macOS app.
- Redesigning the CP API's request or response shapes. The tools carry them as they are.

## Anti-goals

- Two implementations of one operation, one for HTTP and one for MCP. That is what decision 4 exists to prevent.
- A capability the web UI can't reach because it was added "MCP only".
- Agent-shaped UI: a page that mirrors the tool list or exposes every API knob instead of serving the human's task.
- An approve or resume that an MCP call records and that nothing ever carries out.
- A tool list that drifts from the router without a test failing.

## M1. Serve modes

`rupu cp serve [--bind] [--token] [--no-open] [--no-web] [--no-mcp] [--tools <grant>]`

| Mode | `/api/*` | Web UI (embedded SPA) | `/mcp` | Background loops |
|---|---|---|---|---|
| full (default) | yes | yes | yes | yes, as today |
| `--no-mcp` | yes | yes | no | yes |
| `--no-web` | yes | no | yes | yes |

- **`--no-web` keeps `/api/*` (my call).** Other rupu hosts and coordinators reach a host through `/api`: `/api/host/info`, runs, transcripts, workspace staging, skill bundles. Tunnel nodes dial `/api/node/connect`. Dropping `/api` would take the machine out of the fleet. So `--no-web` drops only the embedded SPA and its fallback, and an unmatched non-API path gets a plain 404.
- **`--no-web --no-mcp` is refused at startup**, since there is nothing to serve.
- **`/mcp` is the MCP streamable-HTTP transport**: one endpoint, POST for requests, SSE responses optional. It sits behind the same bearer-token layer as `/api/*`. Without `--token` there is no token check, the same as `/api` today.
- **`/mcp` always validates `Origin`**, as the MCP streamable-HTTP spec requires. A request whose `Origin` header is present and is not the CP's own origin is refused, so a web page can't reach `/mcp` through DNS rebinding. This is protocol compliance, not a guardrail on the operator's agent (my call).
- **`--no-open` is implied by `--no-web`.**
- **Startup prints the MCP endpoint and the `claude mcp add` line** (with the token when one is set), the way it prints the UI URL today.

To connect Claude Code to a running daemon:

```bash
claude mcp add --transport http rupu http://127.0.0.1:7878/mcp --header "Authorization: Bearer <token>"
```

**Clients that only spawn stdio servers** (Claude Desktop's local config file) can add rupu as a connector by URL, or run a generic stdio-to-HTTP bridge such as `mcp-remote`. rupu ships no bridge. `docs/mcp.md` documents both options.

## M2. One state builder

The state wiring in `rupu_cp::serve_on` (`crates/rupu-cp/src/lib.rs`) and the adapter wiring in `crates/rupu-cli/src/cmd/cp.rs::handle` become one function, `build_state`. Every mode uses it.

- **Inputs:** the global dir, the config and the token.
- **What it wires:**
  - `AppState`, pricing and the host registry
  - the local connector with every subprocess adapter: launcher, agent launcher, session starter/sender/mutator, transcript mutator, generator, model catalog, repos, fleet inventory
  - the prewarm tasks

Today a CP built outside `cp serve` returns 501 for most writes. With `build_state`, every mode gets the same `AppState`, no mode is a partial CP, and the MCP's tools behave exactly like the web UI's calls.

## M3. Background loops

M3 needs nothing new. There is one process per daemon, and every mode runs the same loops `cp serve` runs today, with today's bind-before-loops rule (`rupu_cp::bind`): resume worker, gate sweep, cron and autoflow ticks, bucket poller, fleet inventory.

Because the MCP lives in that process, an approve or resume sent over MCP is carried out by the same resume worker as one sent from the UI.

## M4. Operator tools over the router

A new module in `rupu-mcp` (or a new crate `rupu-mcp-ops` if `rupu-mcp` must not depend on `rupu-cp`; builder's call) holds the **operator tool table**. Each entry has:

| Field | Meaning |
|---|---|
| `name` | Canonical tool name: `<area>.<verb>`, e.g. `runs.approve` |
| `method`, `path` | The CP route, with `:params` |
| `input_schema` | JSON Schema of the tool's arguments: path params, query params and body, flattened into one object. Hand-written, or derived with `schemars` from the handlers' serde types where they exist (`schemars` is a workspace dep). Builder's call per entry. |
| `description` | Model-facing description: what it does, when to use it, what it returns |
| `hints` | MCP tool hints, derived from the method and overridden per entry (below) |
| `result` | `json` (the default) or `bytes` (exports, artifacts: returned as an MCP embedded resource / blob) |

**A call:**

1. Validate the arguments against `input_schema`.
2. Build the `http::Request`: fill the path params, put the query params in the URI, put the rest in a JSON body.
3. Add an internal marker header that the router trusts only from in-process callers (see Auth), plus `X-Rupu-Via: mcp`.
4. Send it through the router with `oneshot`.
5. Map the response:
   - **2xx JSON:** `content` = one text block with the JSON; when the body is a JSON object, `structuredContent` = the body too (MCP clients reject a non-object `structuredContent`, so list results go as text only).
   - **2xx bytes:** an embedded resource with its content type.
   - **4xx/5xx:** `isError: true`, with the status and body. The CP's error bodies (`{error, token, code, …}`) stay intact.

**Auth.** An in-process call skips the bearer check. The process already holds the token, and the request never touches a socket. It is marked with a request extension that only the in-process caller can set. A header could be spoofed, so a header is never trusted for this. The `/mcp` endpoint itself sits behind the bearer layer.

**Hints.**

| | Default | Override when |
|---|---|---|
| `readOnlyHint` | true for `GET` | — |
| `destructiveHint` | true for `DELETE` | listed explicitly: cancel, reject, discard, host remove, config writes, prune |
| `idempotentHint` | true for `GET`/`PUT` | — |
| `openWorldHint` | — | true for launches and SCM-touching calls |

**Coverage test (lockstep).** A test enumerates every route the router registers and fails unless each is either:

- in the tool table, or
- on an explicit exclusion list with a reason.

The exclusions are:

| Excluded | Why |
|---|---|
| SSE streams (`/api/runs/:id/log`, `/api/events/stream`, `/api/transcript/stream`, `…/messages/stream`) | `wait` replaces them (M7) |
| Host-to-host transfer endpoints (`/api/runs/:id/coverage`, `/api/runs/:id/netflow/stream`, `/api/findings/artifacts/:sha256`, `/api/workspace/*`, `/api/skills/bundle/*`, `/api/node/connect`) | Machine-to-machine; an agent never calls them |
| `/healthz`, the SPA fallback | Not operations |
| `GET /api/fs/browse` | A UI directory picker (agents have their own file tools) |

A new route without a table entry or an exclusion fails the build.

**Tool names (my call).** One tool per operation, grouped by area, mirroring the API. A sample (the full list is generated from the table):

| Area | Tools (sample) |
|---|---|
| runs | `runs.list`, `runs.get`, `runs.graph`, `runs.children`, `runs.usage`, `runs.approve`, `runs.reject`, `runs.pause`, `runs.cancel`, `runs.resume`, `runs.archive`, `runs.restore`, `runs.delete`, `runs.source`, `runs.ast` |
| agent runs, transcripts | `agent_runs.list`, `transcripts.get`, `transcripts.archive`, `transcripts.delete` |
| sessions | `sessions.list`, `sessions.get`, `sessions.turns`, `sessions.start`, `sessions.send`, `sessions.archive`, `sessions.restore`, `sessions.delete` |
| agentiflows | `agentiflows.list`, `agentiflows.get`, `agentiflows.events`, `agentiflows.children`, `agentiflows.unit_output`, `agentiflows.steer` |
| messages | `messages.list`, `messages.send` (one tool each; the root kind is taken from the id prefix `run_` / `ses_` / `af_`) |
| definitions | `agents.list/get/create/save/delete/run/generate`, `workflows.list/get/create/save/delete/run/validate/generate`, `autoflows.list/enable/disable`, `claims.list/release/requeue`, `skills.list/get` |
| findings | `findings.list` (the CP's `GET /api/findings` with `q`), `findings.get`, `findings.tags`, `findings.tag`, `findings.export`, `findings.artifact` |
| coverage, assets | `coverage.list/get/catalog/audit/runs/diff`, `coverage.templates`, `assets.list` |
| netflow, usage | `netflow.run/project/global/graph/explorer`, `usage.get/timeline/runs/outliers`, `dashboard.get`, `events.recent` |
| fleet | `hosts.list/add/remove`, `workers.list`, `host.info` |
| config, customers, models | `config.get/put`, `customers.*`, `models.list/refresh`, `launch.preview`, `tools.list` |
| projects | `projects.list/get/runs/sessions/agents/workflows/autoflows/coverage/tree/source/files`, `repos.list` |

Every tool that takes `?host=` in the API takes a `host` argument, with the API's rules (501 = the host can't, 502 = offline).

## M5. Catalog tools next to the operator tools

The `rupu-tools` catalog tools (`scm.*`, `issues.*`, `github.*`, `gitlab.*`, `findings.report` / `findings.verify`, the core fs/shell tools) stay on the MCP.

- **Collisions.** Resolved dynamically: sources are ordered operator → `wait` → catalog, and a catalog tool is dropped only when an earlier source serves the same name (ruled during Plan 1; a fixed list hid catalog tools no operator tool served yet). As Plan 2 adds `findings.tag`, `agents.get` and the rest, they shadow their catalog namesakes automatically. A test pins that every name is served once.
- **Workspace.** The catalog tools act on a workspace. The MCP layer adds a required `workspace` argument (a project id, or the absolute path of a registered project) to every catalog tool that needs one: fs/shell, `findings.report` / `findings.verify`. It builds that call's `ToolContext` for that workspace. How contexts are cached is the builder's call. A daemon has no meaningful working directory, so nothing defaults to one.
- **Attribution.** Findings filed through the MCP keep today's `mcp` scope and `mcp_<ULID>` session run id.

## M6. Listing and `--tools`

- **Default:** everything (decision 1).
- **`--tools <grant>`** narrows what is listed, in the agent `tools:` grammar extended to operator names (`runs.*`, `findings.*`, `*`). It's a convenience for clients that pay for a long tool list, not a guardrail. Unknown names fail startup with a did-you-mean, as today.
- **Large lists.** The list will be roughly 150–200 tools. Clients with tool search (Claude Code defers MCP tools) handle that. For others, `--tools` trims it. Each description's first line must say what the tool is for on its own, since tool search ranks on it.

## M7. `wait`

`wait({watch, cursor?, timeout_ms?}) → {fired, events, cursor, timed_out}`

- **`watch`:** one or more conditions:

  | Condition | Fires when |
  |---|---|
  | `run_settled { run, host? }` | The workflow run is terminal and settled (`sse::run_is_settled`, made `pub`) |
  | `run_gate { run, host? }` | The run has a parked gate (`RunRecord.awaiting` non-empty, or a `step_awaiting_approval` event). Returns gate ids and `expires_at` |
  | `run_events { run, host?, types? }` | Any new executor event (optionally of given types) |
  | `messages { root, host?, kinds?, to_operator? }` | New message-log lines. `seen` lines skipped |
  | `findings { project?, run?, q? }` | New finding records, optionally matching a findings query |
  | `session_turn { session, run? }` | The turn's transcript has `run_complete`, or the session status is not `running`. Returns the turn's final answer |
  | `agentiflow { id, types? }` | New agentiflow `events.jsonl` lines (`round`, `run_stopped`) |
  | `agent_run_end { run }` | A standalone run's transcript has `run_complete` |

- **Current state first.** Each call evaluates every condition against current state before it waits. A run already parked or finished fires at once, never times out.
- **Cursor.** Opaque. It maps each watched source to its high-water mark: line numbers in `events.jsonl`, `log.jsonl`, a transcript, `findings.jsonl` per target, an agentiflow's `events.jsonl`. These are the same ids the SSE streams use. Passing the returned cursor back resumes without loss or repeats.
- **Timeout.** Default 30 s, capped at 55 s (my call: under common client tool timeouts). On timeout: `timed_out: true`, the cursor advanced past anything read, no error. While waiting, the tool sends `notifications/progress` when the client sent a progress token.
- **Remote hosts.** Runs on a remote host use the host connector's existing event tail (`Last-Event-ID` passthrough). Remote message spaces and findings are polled through their list endpoints. Builder's call on interval; one request per poll per host, never a burst.
- **Where it lives.** `wait` is the one operator tool that is not a route. It is MCP-layer code over the same readers the SSE streams use (`NumberedTail`, the message log reader, the transcript tail), not a copy of them. If a reader the SSE stream uses is private, it is made `pub` in `rupu-cp`, not duplicated.

## M8. CLI-only capabilities, added to the CP API (and so to MCP)

Each becomes a CP endpoint with an operator tool entry, and gets web UI designed for humans (see "Web UI for the additions" below). The endpoints delegate to the library functions the CLI uses, and run as detached `rupu` subprocesses through W6's `RunArgv` where the CLI launch is long-running.

| Capability | Today | New endpoint (sketch) |
|---|---|---|
| Launch an agentiflow (detached) | CLI `agentiflow run --detach` | `POST /api/agentiflows` `{definition, inputs…, host?}` |
| Hard-stop an agentiflow | CLI `agentiflow stop --now` | `POST /api/agentiflows/:id/stop {now}` (graceful stays on `steer`) |
| Agentiflow definitions: list, get, create, save, delete, validate | hand-written files | `/api/agentiflow-defs[/:name]`, `POST …/validate` |
| Continue a finished or failed agent run | CLI `run --continue` | `POST /api/runs/agents/:id/continue {prompt?, model?, provider?}` |
| Retry a failed or cancelled workflow run | CLI `workflow resume [--restart-interrupted]` | `POST /api/runs/:id/retry {restart_interrupted}` |
| Launch body gaps | CLI flags | `agents/:name/run` gains `model`, `provider`, `engagement_profiles`; `workflows/:name/run` gains `engagement_profiles`, and a `file` / raw-YAML launch |
| Session stop, compact | CLI | `POST /api/sessions/:id/stop`, `POST /api/sessions/:id/compact {window?}` |
| Skills: install, update, validate | CLI | `POST /api/skills/install {source, ref?, skills?, project?, force?}`, `POST /api/skills/update`, `POST /api/skills/validate` |
| Stock fleet, project init | CLI `fleet install`, `fleet list`, `init` | `GET /api/fleet`, `POST /api/fleet/install {force, project?}`, `POST /api/projects/init {path, with_samples}` |
| Agent validation | CLI `agent validate` | `POST /api/agents/validate {raw}` and `GET /api/agents/validate` (all files) |
| Findings import, schema | CLI | `POST /api/findings/import {paths, id?, dry_run}`, `GET /api/findings/schema?advertised=` |
| Coverage gap, rerun | CLI | `GET /api/coverage/:target/gap`, `POST /api/coverage/:target/rerun {run}` |
| Engagement profiles | no listing anywhere | `GET /api/engagement-profiles[/:id]` (bundle, asset kinds, checks) |
| Query-language help | 400s only | `GET /api/query/:list/fields` (the registries the web twins are pinned to), `POST /api/query/:list/check {q}` |
| Autoflow operations | CLI `autoflow run/explain/doctor/repair/wakes/show` | `POST /api/autoflows/:name/run {issue}`, `GET /api/autoflows/:ref/explain`, `GET /api/autoflows/doctor`, `POST /api/autoflows/claims/repair`, `GET /api/autoflows/wakes`, `GET /api/autoflows/:name` |
| Cron | CLI `cron list/events/tick` | `GET /api/cron`, `GET /api/cron/events`, `POST /api/cron/tick {dry_run}` |
| Housekeeping | CLI `transcript/session/netflow prune`, `cleanup` | `POST /api/maintenance/prune {kind, older_than, dry_run}` |
| SCM and repos | CLI `scm accounts/bind`, `repos attach/prefer/tracked/forget`, `issues list/show` | `GET /api/scm/accounts`, `POST /api/scm/rules`, `/api/repos/tracked[…]`. Issues stay on the catalog tools. |
| Auth status | CLI `auth status` | `GET /api/auth/status` (accounts and modes; never secrets) |

The endpoint shapes above are sketches; the plan fixes them. Each new mutating endpoint follows the CP's existing rules:

- records before it acts (pending-state, not optimistic)
- returns 501 for a host that can't do it
- for remote hosts, gets a `HostConnector` method only where a transport already has a path, else is local-only and says so

### Web UI for the additions

Every M8 capability also gets web UI (matt, 2026-10-19). The CP is for humans and the MCP is for agents, so the two are designed separately over the same API:

- **The API and the MCP tool follow the operation.** They are complete, flat and scriptable.
- **The UI follows the human's task.** It may combine several endpoints into one page, put an action where the object already lives (stop and compact on the session page, retry on the run page, profiles in the launcher, prune in Settings), add confirmations and previews (prune's `dry_run` as a preview before the real run), or leave a rarely used control out of the main path. It never mirrors the tool list.
- **Every new or changed page is mocked first** and shown to matt before it is built, and is checked visually before its PR merges.
- **Where each capability's UI goes** is decided per capability in the plan, from the existing pages and navigation. A capability with no natural home (cron, maintenance, autoflow doctor) gets a section under an existing area (Settings, Autoflows), not a new top-level page, unless matt asks for one.

## M9. Resources and prompts

MCP `resources` capability, read-only, served from the binary and the registries. A docs resource reads the doc embedded at build time, so the agent sees the docs of the version it talks to.

| URI | Content |
|---|---|
| `rupu://docs/<name>` | `using-rupu`, `agent-format`, `agent-authoring`, `workflow-format`, `workflow-authoring`, `agentiflows`, `triggers`, `engagement-profiles`, `coverage`, `messaging`, `skills`, `transcript-schema`, `cp-api`, `control-plane`, `response-outcomes`, `netflow`, `configuration` |
| `rupu://schema/finding-report` | `canonical_schema()` |
| `rupu://query/<list>` | Field registries: findings, workflow runs, agent runs, sessions, autoflow runs |
| `rupu://profiles/<id>` | Each engagement profile, as TOML |
| `rupu://tools` | The tool catalog (`ToolCatalog::all()`) |
| `rupu://examples/<path>` | `examples/` agents, workflows, contracts |

**Prompts (my call, small):** `author-agent`, `author-workflow`, `author-agentiflow`, `triage-findings`. Each is a short instruction plus the matching resource links, for clients that surface prompts as commands.

## Delivery order

Four plans, each shippable on its own:

1. **Foundation.**
   - M1 (modes, `/mcp`, `rupu mcp serve` deprecation, stdio removal)
   - M2 (one state builder)
   - M4's machinery (table, router call, auth, hints, lockstep test with every current route on the exclusion list)
   - M5
   - M7 `wait` for runs, gates and messages
   - About a dozen tools proving the path end to end: runs list/get/approve/reject/cancel, `workflows.run`, `agents.run`, `sessions.start/send`, `messages.send/list`.
2. **CP parity.** Every remaining route gets its table entry, and the exclusion list shrinks to the M4 list. Plus `wait` for findings, session turns, agentiflows and agent runs.
3. **The "and more".** M8, endpoint by endpoint, API and MCP first. Each capability's web UI is a separate step behind matt's sign-off on a mock (M8, "Web UI for the additions"). This plan will likely split into 3a (API + MCP) and 3b (UI).
4. **Resources and prompts** (M9), and the docs rewrite.

## Testing

- **Lockstep:** router routes ⊆ tool table ∪ exclusion list (M4). The tool-name list is snapshot-pinned.
- **In-process MCP tests** (`InProcessTransport`) over a temp `RUPU_HOME`: a tool call reaches the handler and returns its JSON. An error status becomes `isError`. A write records `via: mcp`.
- **`wait` tests:** fires at once on an already-parked or finished run. Resumes with no loss or repeats across cursors. Times out cleanly.
- **Mode tests:** `--no-web` serves `/api` and `/mcp` but no SPA, `--no-mcp` serves no `/mcp`, and `--no-web --no-mcp` is refused. A cross-origin `Origin` on `/mcp` is refused.
- **Live check:** Claude Code connected over HTTP to a `cp serve` with an isolated `RUPU_HOME`. Launch a workflow with a gate, `wait` for the gate, approve it, `wait` for the end, query its findings, tag one.

## Docs

- `docs/mcp.md` rewritten: modes, wiring for Claude Code and Cursor over HTTP (Claude Desktop through a connector URL or `mcp-remote`), the tool areas, `wait`, resources, full-access stance, `rupu mcp serve` deprecation.
- `docs/control-plane.md`: serve modes.
- `docs/cp-api.md`: the M8 endpoints.
- `CLAUDE.md`: the `rupu-mcp` and `rupu-cp` entries.

## Left to the builder

- Where the operator tool table lives (`rupu-mcp` gaining a `rupu-cp` dependency, or a new crate). Keep the hexagonal rule: the CLI stays thin.
- Hand-written vs `schemars`-derived input schemas, per entry.
- Exact tool names beyond the pattern in M4, and description wording.
- How HTTP-mode catalog tool contexts are built and cached per workspace (M5).
- The remote polling interval inside `wait`.
- The exact M8 endpoint paths and bodies, consistent with the API's existing style.

## Calls made without matt (check these)

- Flag names `--no-web`, `--no-mcp`.
- `rupu mcp serve` runs the `--no-web` daemon for one release, rather than refusing outright.
- `--no-web` keeps `/api/*` (the host protocol).
- `rupu mcp serve` passes `--tools` through, and ignores `--mode` / `--transport` with a notice.
- Operator tools win name collisions with catalog tools.
- HTTP-mode catalog tools take a required `workspace` argument.
- The tool hint rules.
- `/mcp` always checks `Origin`; the token rule is the same as `/api`'s.
- `mcp` attribution is an additive `client` field next to `via: cp`, only where a surface is recorded today.
- Where each M8 capability's UI lives is decided in the plan, preferring sections in existing areas over new top-level pages.
- `wait`'s 30 s default and 55 s cap.
- Resource URIs and the four prompts.
- `auth status` in scope, `auth login` out.
