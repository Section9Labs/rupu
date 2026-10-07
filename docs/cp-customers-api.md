# Customers — control-plane API

What `rupu cp serve` serves for customers (spec:
`docs/superpowers/specs/2026-10-06-rupu-customers-design.md`; backend plan:
`docs/superpowers/plans/2026-10-06-rupu-customers-plan-2a-cp-backend.md`). The
config layer and the `rupu customer` CLI are in [`configuration.md`](configuration.md#customer-layer).
Writes need a `cp serve` deployment and answer **501** otherwise.

## Routes

| Route | Success | Errors |
|---|---|---|
| `GET /api/customers?archived=1&range=7d\|30d\|all` | 200, rows: customer + `tint` + `rollup` + `default_account` + `layer_error` (default range `30d`; archived only with `archived=1`) | 400 bad `range`; 500 an unreadable assignment (names the workspace) |
| `POST /api/customers` `{slug, name, notes?, contact?, color?}` | 201, the customer | 400 invalid slug / color / empty name, or the reserved slug `none` (the filter's "no customer"); 409 slug exists; 501 |
| `GET /api/customers/:slug?range=` | 200, `{customer, rollup, projects[], default_account, layer_error, config_path}` | 404 unknown; 400 bad `range`; 500 unreadable assignment |
| `PATCH /api/customers/:slug` | 200; absent fields stay, `""` clears `notes` / `contact` / `color` | 400; 404; 501 |
| `POST /api/customers/:slug/archive`, `…/unarchive` | 200, the customer | 404; 501 |
| `DELETE /api/customers/:slug` | 204 | 409 `{error, projects: [{ws_id, path}]}` while projects are assigned; 404; 501 |
| `PUT /api/customers/:slug/projects/:ws_id` | 200, the project's row (any earlier assignment is replaced) | 404 unknown customer / project; 409 customer archived; 400 malformed id; 501 |
| `DELETE /api/customers/:slug/projects/:ws_id` | 204 | 404 when the project is not assigned to `:slug`; 501 |
| `GET /api/config?customer=<slug>` | 200, effective config + provenance + `customer_lock`, `raw_customer`, `customer`, `layer_error` (+ `layer_error_kept`: `global_customer` when only `?project=`'s own layer is broken and its customer's layer resolves, else `global`) | 400 with `?project=` (mutually exclusive); 404; 500 a dangling customer on `?project=` |
| `PUT /api/config/customer/:slug` | 200 `{ok: true}` | 400 the layer breaks the merged config, or sets a key the GLOBAL `[policy].lock` enforces; 404; 501 |
| `PUT /api/config/project/:id` | 200 | 400 also for a key the project's customer locks (`key … is enforced by customer … policy`) |
| `POST /api/launch/preview` | 200, see below | 400 / 404 / 409, see below |

A customer's `tint` is its `color` for both themes when set, else a light/dark
pair derived from the slug (`rupu_codename::crew_for` + `crew_tint`), so clients
never invent colors. `default_account` is `{account, locked_by, inherited}`:
the provider account the customer's runs default to over global + its layer
(`inherited: true` when the value is the global one); `null` when nothing sets
one or the layer does not resolve (`layer_error` says why on the detail).
`config_path` on the detail is the customer layer's real `config.toml` under the
CP's global dir (any `RUPU_HOME`), with a leading `$HOME` shown as `~` — for
display only. The detail's `projects[]` are the customer's current projects;
each row's `usage`, `run_count` and `last_active` cover only this customer's
work in that project over the same `range` as `rollup` (attributed and priced
the same way) — not the project's all-time spend, which can include work billed
to another customer. (`GET /api/projects` keeps the all-time figures.)

Rollups (`projects`, `run_count`, `usage`, `findings_open`, `last_active`) are
computed in one pass over the run store. Standalone agent runs and session
turns add spend and activity, never `run_count`. Each run is priced with the
pricing of the customer it is attributed to (global + that customer's layer,
cached per slug and revalidated by the stat of the two `config.toml` files);
work with no customer is priced at the global pricing. The Projects page's
rollups use the same per-customer pricing.

A customer whose layer does not resolve (a malformed
`customers/<slug>/config.toml`) is still listed, with `default_account: null`
and `layer_error` saying why, and its work is priced at the GLOBAL rates —
visibly: every usage summary that includes such work (the customer's
`rollup.usage`, project rollups, run / agent-run / session rows, `/api/usage`'s
`summary`, `/api/usage/timeline` buckets and `/api/usage/runs` rows) carries
`pricing_error` naming the customer. Work whose customer can't be known (a
row listed without customer keys, below) is priced at the global rates too,
with `pricing_error: "customer unknown; priced at global rates"`.
`pricing_error` is absent otherwise (it is unrelated to `partial`, which flags
missing token counts). `/api/usage/outliers` rows carry it as well, and such
runs are left out of every outlier baseline (their cost is not comparable)
while still being flagged against it.

**A run costs the same everywhere.** Its list row, `GET /api/runs/:id`,
`/api/runs/:id/usage`, `/api/runs/:id/graph`, the workflows list's and the
agents list's usage, autoflow rows and the rollups all price it at its
attributed customer. A session's list row and `GET /api/sessions/:id` price
each TURN at that turn's own attribution (not the session's latest customer),
as the rollups do.

## Attribution

A run records the customer it ran under at launch: `RunRecord.customer`
(workflow and standalone runs), `RunStart.customer` in every agent transcript
(standalone runs, session turns, workflow steps) and `SessionRecord.customer`
(the session's directory as of its latest turn). The field is **tri-state** on
disk:

| On disk | Means | Attributed as |
|---|---|---|
| `"customer": "acme"` | recorded slug | `acme`, `customer_derived: false` |
| `"customer": null` | recorded **no customer** | none, `customer_derived: false` — even if the project has been assigned since |
| no `customer` key | **legacy** — written before customers existed | the project's CURRENT assignment, `customer_derived: true` (none when unassigned) |

Reassigning a project never rewrites history: only a legacy record derives
from the current assignment. A resumed run keeps what it recorded — its slug
(even if the project has been reassigned since; a recorded customer that no
longer exists is an error), or no customer for a recorded `null` (even if the
project has been assigned since). Only a legacy run falls back to the launch's
`customer_dir` sidecar, else its workspace path.

A session turn whose own transcript is legacy inherits its session record's
customer, and the row reports it as derived (`customer_derived: true`); a turn
that recorded `null` stays none whatever the session says now. Findings and
projects record no customer — they follow the project's current assignment.

Run, session, agent-run, project and finding rows serialize `customer`
whenever it is known (`null` = no customer; `customer_derived` on run-like
rows). A row that omits the `customer` key cannot say whose it is (see Remote
hosts and "An unreadable assignment" below) — a coordinator never reads
absence as "no customer".

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
| `GET /api/usage/outliers` | the same LOCAL sources, as both the candidates and the per-workflow baselines (so a run is judged against that customer's own runs); a bad slug is 400 |
| `GET /api/dashboard` | the LOCAL host's runs by attribution, `findings_open` by the customer's projects, autoflow cycles by the projects of the repos they touched (a cycle no project resolves for is left out); the `fleet` counts are not run-scoped and stay unfiltered |

### Remote hosts

- **Run and session lists** (runs, workflows, agents, sessions): a remote host's
  rows are filtered on the coordinator by their `customer` key. If any row
  **lacks the key** — a peer older than customers, a mirror (tunnel / bucket)
  holding a worker's legacy runs (a mirrored run that recorded `null` carries
  `customer: null` and is filterable), or rows the peer could not attribute — the
  host "can't report a customer for every run": a single-host request
  (`?host=<id>`) answers **501** (the web shows it "unavailable"), and the
  fan-out skips the host and names it in the **`X-Rupu-Hosts-Without-Customer`**
  response header (comma-separated host ids). It is never counted as zero. A
  `null` value is "no customer"; an empty page stays filterable.
- **Filtered reads page past peer limits.** A remote's page is read page after
  page until the requested window is full or the host runs out, so a peer that
  clamps its page size (an HTTP peer caps `limit` at 200) never makes a filtered
  page come back short while more matches exist.
- **Peers have their own customer namespace (until Plan 3).** A remote run —
  including a placed unit of a coordinator's run — records the customer the
  WORKER resolved from its own customers and assignments; the coordinator's
  layer is not shipped to remote hosts yet. A peer's `acme` is that peer's
  customer, which the filter matches by slug.
- **Mirrored runs are never attributed through the coordinator.** A run the
  coordinator's store holds for a worker (`worker_id` set — tunnel, bucket,
  placed units) counts only by what it recorded. A LEGACY one (no `customer`
  key) is listed unfiltered without customer keys (priced as unknown); under a
  filter, and in counts and rollups, it is left out — never counted as "no
  customer" — and its worker host is named: in the
  `X-Rupu-Hosts-Without-Customer` header on run lists, `/api/usage/timeline`,
  `/api/usage/runs` and `/api/usage/outliers`, and in a
  `hosts_without_customer` array on `/api/usage`, `/api/dashboard` and each
  customer row's `rollup`.
- **Aggregates (`/api/usage`, `/api/dashboard`) are local-only under a filter.**
  A remote host's totals arrive already summed and cannot be filtered:
  `?host=<remote>&customer=…` is **501**, and the fan-out reports each remote
  host `unavailable` rather than counting it. An unknown `?host=` is 404 first.

### An unreadable assignment fails closed — except on an unfiltered list

The assignment of a project is `workspaces/<id>.customer`. If the CP cannot read
it (an unreadable sidecar), every **filtered** request (`?customer=`) and every
**count, rollup or price** (the customers API, project rollups,
`/api/usage/runs`, `/api/usage/timeline`, the dashboard) answers **500** naming
the workspace and the file to repair — work is never counted under "no
customer". On the fan-out, a filtered run list whose LOCAL rows can't all be
attributed is that same 500, never a header entry.

An **unfiltered** run / workflow-run / agent-run / session list (and a
project's session list, and a run's or session's detail) does not fail: the affected rows **omit the `customer`
/ `customer_derived` keys** and one warning per workspace goes to the server
log; every other row keeps its keys. The CLI's display listings (`rupu run
list`, `transcript list`, `session list`, which an SSH coordinator reads)
degrade the same way, warning on stderr — the coordinator then reads that host
as unable to report. `/api/usage` marks the local host `offline` with the reason
(its per-host contract). A recorded slug or `null` needs no assignment read and
is always reported. A malformed legacy workspace id reads as unassigned.

Surfaces that only PRICE work (no customer filter, no per-customer count)
degrade instead of failing: a run or turn whose customer can't be read — an
unreadable assignment, a legacy mirrored run, a `run.json` that won't load — is
priced at the global rates as `Unknown`, with `pricing_error: "customer
unknown; priced at global rates"`. These are the workflows list and detail,
the agents list, autoflow cycle and event rows, a run's detail, graph and
`/api/runs/:id/usage`, a session's detail, and the unfiltered run / agent-run /
session lists above. A remote session priced here (the `?host=` proxy, for a
peer that sent no `usage`) that has a customer carries `pricing_error: "the
peer's customer pricing isn't available here"`.

## Launch preview

`POST /api/launch/preview` says what a launch would authenticate as, before it
runs. Body: exactly one of `workflow` / `agent` (400 otherwise), plus the
launcher's `working_dir` or `scope_kind` / `scope_id` (mutually exclusive, as on
the launch routes; with neither, the server's cwd) and an echoed `host`.

It resolves exactly like a launch: the same launch directory
(`resolve_launch_scope`), the strict `rupu_workspace::config_paths` (a project
assigned to a customer that no longer exists, or a config that does not load, is
**409**, as it is a launch failure), `rupu_workspace::locate_workflow` and the
agent loader — agent files that do not load (one malformed file fails the
loader for every agent) are a **409** with the loader's message, for an agent
launch and for a workflow whose agents can't be loaded. 404 for an unknown
workflow, or an unknown agent on an agent launch. A `workflow` / `agent` name
containing `/`, `\` or `..` (or empty) is **400** before any file is read.

Response `{customer, accounts, warnings, host?}`:

- `customer` — the row reference (`slug`, `name`, `tint`, `archived`) or `null`.
- `accounts` — `rupu_runtime::credential_manifest` entries
  `{role: provider|fallback|scm, account, kind, auth_mode, agents[], source}`
  (`auth_mode` is the agent's `auth:` — `api-key` / `sso` — on its provider entry
  and on a fallback hop on that same provider, else `null`; entries are
  deduplicated by role + account + `auth_mode`); `source` says
  where the choice came from ("agent frontmatter", "customer default", "global
  default · locked", "customer [recovery].fallbacks", "rule owner = acme-corp",
  …). Provider and fallback entries follow the run's own resolution rules
  (`resolve_provider_name`, `RecoveryConfig::chain_for`, an unnamed fallback
  stays on the run's provider). The SCM entry is the account the launch
  directory's `origin` repo resolves to through `[[scm.rules]]`.
- `warnings` — resolver warnings; a workflow step's agent that was not found
  (its accounts are not listed); an `origin`
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

## Web

The control-plane web UI (`crates/rupu-cp/web/`) is a thin client of the routes
above. A global **customer scope** (`web/src/lib/customerScope.tsx`) decides
which customer every scoped page filters to; the pages pass it as `?customer=`.

### The scope

- **Values.** `null` = all customers (no `?customer=` sent), `none` = work with
  no customer (shown as "Unassigned"), otherwise a customer slug.
- **Persistence.** The scope is kept in `localStorage` under `rupu.cp.customer`
  (cleared when the scope returns to all customers). A browser that blocks
  storage still works for the session.
- **Deep link.** A `?customer=<slug>|none` on the URL the app first loads wins
  over storage: it is adopted as the scope and written back to storage, so a
  link such as `/runs?customer=acme` opens already filtered. It is read once, at
  load; changing the scope later does not rewrite the URL.
- **A scope the backend rejects is cleared.** A 400 from a request that carried
  the scope (a malformed slug, e.g. a stale stored value) clears it and shows a
  dismissible one-line notice ("The customer filter was rejected (…) — showing
  all customers."); the page refetches unfiltered. A slug the backend accepts
  but that no customer carries is cleared the same way once the customer list
  has loaded and neither the active nor the archived customers hold it
  ("Customer "acme" no longer exists — showing all customers."). A page
  embedded in a customer's own detail tab uses that customer and never clears
  the global scope.
- **Archived customers** stay valid scopes. The picker lists active customers;
  "Show archived" in its footer loads the archived ones on demand.

### Pickers and chips

- The **picker** (`components/customers/CustomerPicker.tsx`): the v1 sidebar
  block under the brand, and a compact button in the v2 top bar. A search box,
  "All customers", the customers (dot, name), "Unassigned", and a footer with
  "Show archived" and "Manage →" (the Customers page).
- A scoped page's header carries a **scope chip** (customer dot + name + ×,
  which clears the scope; "Unassigned" for `none`).

### Which pages filter

| Filters by the scope | Shows an "unscoped" note instead |
|---|---|
| Dashboard (runs, findings, autoflow cycles, spend), the workflow-run and agent-run lists (Activity / Runs), Sessions, Findings, Projects, Usage (headline, timeline, run rows, outliers) | autoflow listings (runs, cycles, claims), Coverage, the concern-template catalog, the dashboard's fleet counts, and the Workflows / Agents / Autoflows definition lists (their run counts and spend cover every customer) |

An unscoped page says so in a one-line info note (`UnscopedNote`) while a scope
is set, so a list or number that is not the customer's is never passed off as
theirs. The notes disappear when the scope is all customers.

### Hosts that can't be filtered

A scoped view names the hosts it had to leave out in a warn banner
(`HostsWithoutCustomerBanner`): hosts whose rows lack a `customer` key (the
single-host 501 and the `X-Rupu-Hosts-Without-Customer` header / the
`hosts_without_customer` arrays above) and remote hosts whose aggregate totals
cannot be filtered (`/api/usage`, `/api/dashboard`). They are never counted as
zero. The banner never shows unscoped.

### Where customers appear

- **Customers page** (`/customers`, "Customers" in the nav): one row per
  customer with its rollup (projects, runs, spend, open findings, last
  activity), a 7d / 30d / all range, search, an Active / Archived / All view,
  and create. The
  cost tile says when its total leaves out usage that could not be priced
  ("excludes unpriced usage from …"), and when part of a customer's usage was
  unreadable so the total may be understated ("some usage unreadable — may be
  understated"). Hosts without customer keys are named in a banner.
- **Customer detail** (`/customers/:slug[/:tab]`): tabs Overview, Projects,
  Runs, Findings, Usage and Config; edit, archive / unarchive, delete (a 409
  lists the projects still assigned), assign and unassign projects. The header
  shows the layer's real path from `config_path`. The **Config** tab edits the
  customer layer (`GET /api/config?customer=`, `PUT /api/config/customer/:slug`):
  a field is inherited from global, owned by the customer (with a "Lock for
  projects" switch), or pinned by the global `[policy].lock` (read-only; the
  server's 400 shows inline). A layer that does not parse (`layer_error`) shows
  a banner and opens the Raw tab. The banner says which layers the values
  shown come from (`layer_error_kept`): the global config alone for a broken
  customer layer; on a project's Config tab, the global config plus the
  customer's layer when only the project's own layer is broken.
- **Project header** (`ProjectCustomerMenu`): the project's customer chip and an
  "Assign to customer" menu (active customers with their default account, and
  "Unassign"). Focusing a customer previews the effect: a key the customer (or
  global) locks is what new runs use and the project can't override it; an
  unlocked default applies unless the project's own config sets
  `default_provider` (read from the project's config view, and said when it
  does); an agent that names its own provider keeps it either way. The Projects
  table has a customer column.
- **Runs**: the workflow-run and agent-run lists carry a customer column, and
  run detail a customer chip (`CustomerChip`); a derived attribution
  (`customer_derived`) and a row that can't say (no `customer` key) are marked
  as such. Rows priced at the global rates because the customer is unknown or
  its layer is broken carry a `pricing_error` mark.
- **Launcher billing panel** (`LaunchBillingPanel`, in the workflow and agent
  launchers): debounced `POST /api/launch/preview` for the chosen project and
  definition — the customer, the provider / fallback / SCM accounts and where
  each choice came from, and the warnings. A 409 (dangling assignment,
  unloadable config, malformed agent files) blocks the launch, as the launch
  itself would fail.

## Not yet

Remote aggregate filtering, shipping the customer layer and credentials to remote
hosts, and autoflow filtering are tracked in `TODO.md` (Customers section).
