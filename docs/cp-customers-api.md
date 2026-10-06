# Customers — control-plane API

What `rupu cp serve` serves for customers (spec:
`docs/superpowers/specs/2026-10-06-rupu-customers-design.md`; backend plan:
`docs/superpowers/plans/2026-10-06-rupu-customers-plan-2a-cp-backend.md`). The
config layer and the `rupu customer` CLI are in [`configuration.md`](configuration.md#customer-layer).
Writes need a `cp serve` deployment and answer **501** otherwise.

## Routes

| Route | Success | Errors |
|---|---|---|
| `GET /api/customers?archived=1&range=7d\|30d\|all` | 200, rows: customer + `tint` + `rollup` + `default_account` (default range `30d`; archived only with `archived=1`) | 400 bad `range`; 500 an unreadable assignment (names the workspace) |
| `POST /api/customers` `{slug, name, notes?, contact?, color?}` | 201, the customer | 400 invalid slug / color / empty name; 409 slug exists; 501 |
| `GET /api/customers/:slug?range=` | 200, `{customer, rollup, projects[], default_account, layer_error}` | 404 unknown; 400 bad `range`; 500 unreadable assignment |
| `PATCH /api/customers/:slug` | 200; absent fields stay, `""` clears `notes` / `contact` / `color` | 400; 404; 501 |
| `POST /api/customers/:slug/archive`, `…/unarchive` | 200, the customer | 404; 501 |
| `DELETE /api/customers/:slug` | 204 | 409 `{error, projects: [{ws_id, path}]}` while projects are assigned; 404; 501 |
| `PUT /api/customers/:slug/projects/:ws_id` | 200, the project's row (any earlier assignment is replaced) | 404 unknown customer / project; 409 customer archived; 400 malformed id; 501 |
| `DELETE /api/customers/:slug/projects/:ws_id` | 204 | 404 when the project is not assigned to `:slug`; 501 |
| `GET /api/config?customer=<slug>` | 200, effective config + provenance + `customer_lock`, `raw_customer`, `customer`, `layer_error` | 400 with `?project=` (mutually exclusive); 404; 500 a dangling customer on `?project=` |
| `PUT /api/config/customer/:slug` | 200 `{ok: true}` | 400 the layer breaks the merged config, or sets a key the GLOBAL `[policy].lock` enforces; 404; 501 |
| `PUT /api/config/project/:id` | 200 | 400 also for a key the project's customer locks (`key … is enforced by customer … policy`) |
| `POST /api/launch/preview` | 200, see below | 400 / 404 / 409, see below |

A customer's `tint` is its `color` for both themes when set, else a light/dark
pair derived from the slug (`rupu_codename::crew_for` + `crew_tint`), so clients
never invent colors. `default_account` is `{account, locked_by, inherited}`:
the provider account the customer's runs default to over global + its layer
(`inherited: true` when the value is the global one); `null` when nothing sets
one or the layer does not resolve (`layer_error` says why on the detail).

Rollups (`projects`, `run_count`, `usage`, `findings_open`, `last_active`) are
computed in one pass over the run store. Standalone agent runs and session
turns add spend and activity, never `run_count`. Each run is priced with the
pricing of the customer it is attributed to (global + that customer's layer,
cached per slug and revalidated by the stat of the two `config.toml` files);
work with no customer is priced at the global pricing. The Projects page's
rollups use the same per-customer pricing.

## Attribution

A run records the customer it ran under at launch: `RunRecord.customer`
(workflow and standalone runs), `RunStart.customer` in every agent transcript
(standalone runs, session turns, workflow steps) and `SessionRecord.customer`
(the session's directory as of its latest turn). A resumed run keeps its
recorded customer even if the project has been reassigned since (a recorded
customer that no longer exists is an error); only a run recorded before this
feature falls back to the launch's `customer_dir` sidecar.

A run with no recorded customer **derives** it from its project's CURRENT
assignment and the row says so: `customer_derived: true`. A session turn whose
transcript predates customers inherits its session's customer (also reported
derived). Findings and projects record no customer — they follow the project's
current assignment.

Run, session, agent-run, project and finding rows always serialize `customer`
(`null` = no customer; `customer_derived` on run-like rows). A row that omits
the `customer` key cannot say whose it is (see Remote hosts below) — a
coordinator never reads absence as "no customer".

## `?customer=<slug>|none`

`<slug>` selects one customer's work; `none` selects work with no customer (the
picker's "Unassigned"). A malformed slug is **400**. A slug no customer carries
is not an error — it matches nothing.

| Endpoint | What the filter keys on |
|---|---|
| `GET /api/runs`, `/api/runs/workflows`, `/api/runs/agents` | the run's attribution (recorded, else derived); applied BEFORE paging |
| `GET /api/sessions` | the session's recorded customer, else derived |
| `GET /api/findings` | the project's CURRENT assignment (the summary counts only the kept findings) |
| `GET /api/projects` | the project's CURRENT assignment |
| `GET /api/usage`, `/usage/timeline`, `/usage/runs` | the attribution of each LOCAL source (runs, standalone agent runs, session turns), priced per customer |
| `GET /api/dashboard` | the LOCAL host's runs by attribution, `findings_open` by the customer's projects, autoflow cycles by the projects of the repos they touched (a cycle no project resolves for is left out); the `fleet` counts are not run-scoped and stay unfiltered |

### Remote hosts

- **Run and session lists** (runs, workflows, agents, sessions): a remote host's
  rows are filtered on the coordinator by their `customer` key. If any row
  **lacks the key** — a peer older than customers, a mirror holding a worker's
  runs recorded before attribution, or rows the peer could not attribute — the
  host "can't report a customer for every run": a single-host request
  (`?host=<id>`) answers **501** (the web shows it "unavailable"), and the
  fan-out skips the host and names it in the **`X-Rupu-Hosts-Without-Customer`**
  response header (comma-separated host ids). It is never counted as zero. A
  `null` value is "no customer"; an empty page stays filterable.
- **Filtered reads page past peer limits.** A remote's page is read page after
  page until the requested window is full or the host runs out, so a peer that
  clamps its page size (an HTTP peer caps `limit` at 200) never makes a filtered
  page come back short while more matches exist.
- **Aggregates (`/api/usage`, `/api/dashboard`) are local-only under a filter.**
  A remote host's totals arrive already summed and cannot be filtered:
  `?host=<remote>&customer=…` is **501**, and the fan-out reports each remote
  host `unavailable` rather than counting it. An unknown `?host=` is 404 first.

### An unreadable assignment fails closed

The assignment of a project is `workspaces/<id>.customer`. If the CP cannot read
it (an unreadable sidecar), its local handlers answer **500** naming the workspace
and the file to repair — work is never counted under "no customer". Two
documented exceptions: `/api/usage` marks the local host `offline` with that
reason (its per-host contract), and the CLI's display listings (`rupu run list`,
`transcript list`, `session list`, which an SSH coordinator reads) instead
**omit the `customer` keys** on the affected rows and print one warning per
workspace to stderr — the coordinator then reads that host as unable to report.
A malformed legacy workspace id reads as unassigned.

## Launch preview

`POST /api/launch/preview` says what a launch would authenticate as, before it
runs. Body: exactly one of `workflow` / `agent` (400 otherwise), plus the
launcher's `working_dir` or `scope_kind` / `scope_id` (mutually exclusive, as on
the launch routes; with neither, the server's cwd) and an echoed `host`.

It resolves exactly like a launch: the same launch directory
(`resolve_launch_scope`), the strict `rupu_workspace::config_paths` (a project
assigned to a customer that no longer exists, or a config that does not load, is
**409**, as it is a launch failure), `rupu_workspace::locate_workflow` and the
agent loader. 404 for an unknown workflow, or an unknown agent on an agent launch.

Response `{customer, accounts, warnings, host?}`:

- `customer` — the row reference (`slug`, `name`, `tint`, `archived`) or `null`.
- `accounts` — `rupu_runtime::credential_manifest` entries
  `{role: provider|fallback|scm, account, kind, agents[], source}`; `source` says
  where the choice came from ("agent frontmatter", "customer default", "global
  default · locked", "customer [recovery].fallbacks", "rule owner = acme-corp",
  …). Provider and fallback entries follow the run's own resolution rules
  (`resolve_provider_name`, `RecoveryConfig::chain_for`, an unnamed fallback
  stays on the run's provider). The SCM entry is the account the launch
  directory's `origin` repo resolves to through `[[scm.rules]]`.
- `warnings` — resolver warnings; an agent that was not found (its accounts are
  not listed); agent files that could not be loaded (listed once); an `origin`
  that is not a github.com / gitlab.com remote ("its SCM account can't be
  previewed" — a self-hosted remote is such a case, never silently omitted); an
  ambiguous or unresolvable SCM account; and, for a non-local `host`, that the
  preview resolves against the machine running the control plane.

**Credential presence parity.** The SCM candidates are only the accounts the
config declares for the repo's platform that **have a stored credential** —
the same set `rupu scm accounts` shows and the runtime registers — so an
uncredentialed `[scm.<name>]` table never makes the preview report an ambiguity
the run would not have. If the credential store cannot be read, those accounts
are kept, the entry's `source` says "credentials not checked" and a warning says
so. Provider accounts are reported from the config; their credentials are not
probed here.

## Not yet

Remote aggregate filtering, shipping the customer layer and credentials to remote
hosts, and the web UI are tracked in `TODO.md` (Customers section).
