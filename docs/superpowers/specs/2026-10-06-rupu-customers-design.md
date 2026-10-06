# rupu — Customers: group projects, layer their config, ship it to remote hosts

**Date:** 2026-10-06 · **Status:** design approved in conversation; spec for review

## Context

rupu has projects but nothing above them. A project is a `rupu_workspace::Workspace`
record (`crates/rupu-workspace/src/record.rs`), created automatically by `upsert` the
first time rupu runs in a directory and keyed by its canonical path. It carries a path,
the git remote, the branch at registration and timestamps; it has no owner and no
metadata. Config is two layers — the global `~/.rupu/config.toml` and the repo's
`.rupu/config.toml` — merged by `rupu_config::layer_files` / `layer_files_locked` /
`resolve`, with the global `[policy].lock` pinning keys against the project.

Work for different customers needs different defaults — above all, a different
account to bill. The account machinery already exists: named provider accounts
(`[providers.anthropic-acme] kind = "anthropic"`), each with its own stored
credential, and `[[scm.rules]]` routing repos to SCM accounts. What is missing is a
place that says "every project of Acme's uses `anthropic-acme`", a way to see work by
customer in the CP, and a way for that choice to survive a run being placed on a remote
host — which today resolves its own `RUPU_HOME` config and would bill its own default
account.

## Goals

1. A **customer** record that groups projects (explicit assignment), carries metadata,
   and adds a config layer between global and project.
2. Customer config **wins against the repo only where the customer locks it**, so a
   committed `.rupu/config.toml` cannot silently move an Acme run onto another account.
3. Usage, cost, runs and findings **roll up per customer** in the CP, with a global
   customer filter and a Customers page; customers are managed from the CP and the CLI.
4. A run placed on a remote host runs under the **same customer layer**, and the
   **credentials it needs are shipped** to that host when it lacks them — over
   encrypted channels only, never silently falling back to another account.

## Non-goals

- **Data isolation.** A customer is a grouping, filtering and config dimension, not a
  security boundary inside one CP. Anyone who can open the CP sees every customer.
- **Hierarchy.** One level: customer → projects. No sub-customers, no teams.
- **Assignment rules.** No auto-assignment from remotes or paths; every assignment is
  explicit.
- **Remote-only projects.** Only the coordinator's projects (what the CP Projects page
  lists today) can be assigned. A project that exists only in a remote host's own
  workspace store stays unassigned.
- **Central credential management.** Credentials stay in each host's auth store and are
  logged in by hand as today; this design only ships the ones a specific run needs and
  the host lacks. Pushing secrets over the tunnel (`ws://`, no TLS in this build) or
  through a bucket is out of scope until those transports are encrypted.

## Decisions (from the design conversation)

| Question | Decision |
|---|---|
| What a customer carries | Config defaults, credential selection (via existing named accounts), usage/cost rollup. Not data isolation. |
| Assignment | Explicit only (CLI or CP). |
| Customer vs repo config | Normal inheritance — project wins — **plus customer locks** (`[policy].lock` in the customer layer). |
| Storage | Files in `RUPU_HOME` (approach A), not a section of the global config, not a JSON store. |
| Remote hosts | The resolved customer layer ships with the launch; credentials the run needs and the host lacks are shipped too. |
| Shipped credentials | API keys **persist** into the remote auth store; SSO credentials travel as a **per-run access-token lease** (never the refresh token). |
| Transports for credentials | SSH and HTTPS only. Tunnel, bucket and plain-`http://` hosts refuse with a "log in on that host" message. |
| Metadata | Slug id + display name, notes/contact, color, archived flag. |
| Deleting a customer with projects | Refused until no project is assigned. |
| CP first cut | Global customer filter, Customers page, management in the CP, customer + billing account on launch and run. |

## 1. Model and resolution

### Storage

```
<RUPU_HOME>/customers/<slug>/customer.toml   # metadata
<RUPU_HOME>/customers/<slug>/config.toml     # the customer's config layer
<RUPU_HOME>/workspaces/<ws_id>.customer      # assignment sidecar: the slug, one line
```

- **Slug** is the customer's id: `[a-z0-9][a-z0-9-]{0,62}`, immutable. Renaming changes
  `name` only. It is what the CLI, `--customer-layer` launches, run records and the
  `?customer=` filter use.
- **`customer.toml`**: `name` (required), `notes`, `contact`, `color` (a `#rrggbb`
  value; absent ⇒ derived from the slug the same way codename crew tints are), `archived`
  (bool, default false), `created_at` (RFC 3339).
- **`config.toml`**: the ordinary `Config` schema, parsed by the same code as the global
  and project files. Its `[policy].lock` is the customer's lock list.
- **Assignment sidecar, not a `Workspace` field.** `rupu_workspace::upsert` rewrites the
  whole workspace record, unlocked, on every run (read-modify-write in `store.rs`). A
  field on the record would race: a run starting while you assign could write back its
  stale copy and drop the assignment. The sidecar is written atomically (temp file +
  rename) and `upsert` never touches it, so there is nothing to race and no lock to add.
  Unassigning removes the sidecar.

### Resolution

`layer_files`, `layer_files_locked` and `resolve` take a `LayerPaths { global, customer,
project }` (each `Option<&Path>`) instead of two paths. Precedence, highest first:

1. a key locked by the **global** `[policy].lock` — its global value;
2. a key locked by the **customer** `[policy].lock` — its customer value;
3. project;
4. customer;
5. global;
6. default.

- Locks are read only from the global and customer layers; a project's `[policy].lock`
  stays ignored, as today. A customer cannot unlock a globally locked key, and a
  customer lock on a key the customer layer does not set locks nothing (the resolver
  warns, naming the key).
- The resolved `config.policy.lock` stays the global list (the existing pin in
  `resolve.rs`); the customer's locks are reported separately so the UI can badge them.
- `KeySource` gains `Customer`; `KeyProvenance` gains `locked_by: Option<Global|Customer>`
  (replacing the bare `locked: bool`, which the CP reads — kept as a serialized alias for
  one release).
- Merge semantics are unchanged: tables merge key by key, scalars overwrite, **arrays
  replace**. A customer that declares `[[scm.rules]]` replaces the global rules for its
  projects; `docs/configuration.md` says so.
- Dotted keys follow the existing canonical quoted encoding in every new site (the
  customer config PUT, provenance keys, lock entries) — the dotted-key contract gains a
  customer-layer case in its lockstep tests.

### Behaviour rules

- **Dangling assignment** (a sidecar naming a slug with no `customers/<slug>/`) is an
  error on every launch path: the run fails, naming the project and the slug. It never
  falls back to global config.
- **Malformed customer layer** is the same error on launch paths. Several CLI paths load
  config with `.unwrap_or_default()`; on launch paths the customer-layer error is
  propagated before that fallback. Read-only views (CP display, `rupu customer show`)
  degrade but show the error.
- **Archived customers still resolve.** Archiving hides a customer from pickers, the
  filter's default list and `rupu customer list`; runs of its projects keep working.
- **Pricing.** A customer may override `[pricing]`. Usage rollups price each run with the
  pricing resolved for the customer it recorded (cached per customer; the cache is
  invalidated by the customer `config.toml`'s mtime, matching `FileCache`'s stat check).

## 2. Store, CLI and config loading

### Customer store

`rupu_workspace::customers::CustomerStore { root: <RUPU_HOME> }`:

- `list(include_archived)`, `get(slug)`, `create(slug, meta)` (refuses an existing slug),
  `update_meta(slug, patch)`, `set_archived(slug, bool)`;
- `delete(slug)` — refuses with `CustomerError::HasProjects { projects }` while any
  sidecar names the slug; otherwise removes the directory;
- `assign(slug, project)` — `project` is a path or a `ws_id`; a path with no workspace
  record yet is registered first through `upsert`; refuses an unknown or archived slug;
- `unassign(project)`, `customer_of(ws_id)`, `projects_of(slug)`.

Writes are atomic (temp + rename). The store has no business logic beyond validation;
the CLI and the CP both call it.

### One way to build the layer paths

`rupu_workspace::customers::layer_paths_for(global_dir, project_root) ->
Result<LayerPaths, CustomerError>`: project root → workspace record (by canonical path,
as `upsert` matches) → sidecar → `customers/<slug>/config.toml`. A project with no
record or no sidecar gets `customer: None`.

Every config load that serves a project goes through it. The `LayerPaths` signature
change makes the compiler flag each of the ~21 existing call sites in `rupu-cli` and
`rupu-cp`; each is converted explicitly, so no path can quietly skip the customer layer.
Global-only loads (e.g. `rupu run`'s pricing read with no project) pass `customer: None`
on purpose and say so in a comment.

### Which runs get it

`rupu run`, `rupu workflow run` / `resume`, session turns, autoflow and cron ticks, and
CP launches all load through `layer_paths_for`. A run records the customer it ran under:

- `RunRecord.customer: Option<String>` and the transcript `RunStart.customer`
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`, so older readers and
  records are unaffected).
- Rollups and the filter use the **recorded** slug, so reassigning a project later does
  not rewrite history.

### Credential manifest (pure)

`rupu_runtime::credential_manifest(config, workflow_or_agent, repo) -> CredentialManifest`
lists the accounts a run will use, each as `{ account, kind, auth_mode }`:

- each agent's provider account — the agent's `provider:` / `auth:`, else the resolved
  `default_provider`;
- every account on the `fallbacks:` chain (agent, else `[recovery].fallbacks`), since a
  recovery hop can land on any of them;
- the SCM account `[[scm.rules]]` selects for the run's repo, when it has one.

Accounts that tool calls reach dynamically (another repo, an issue tracker named at
run time) cannot be known at launch; the manifest does not claim them. It is pure and
used three ways: the CP launch preview (§4), local display, and remote shipping (§3).

### `rupu customer`

A new thin subcommand (CLI rule 2: parse arguments, call the store):

| Command | Does |
|---|---|
| `list [--archived]` | slug, name, project count, archived |
| `show <slug>` | metadata, assigned projects, effective config with provenance and locks |
| `create <slug> --name <name> [--notes …] [--contact …] [--color …]` | |
| `set <slug> [--name …] [--notes …] [--contact …] [--color …]` | metadata only |
| `edit <slug>` | opens `customers/<slug>/config.toml` in `$EDITOR` via the existing `editor.rs` flow; validates on save, re-opens on a parse error |
| `archive <slug>` / `unarchive <slug>` | |
| `delete <slug>` | refuses while projects are assigned, listing them |
| `assign <slug> [--project <path\|ws_id>]` | defaults to the project in the current directory |
| `unassign [--project <path\|ws_id>]` | |

`rupu config get` resolves through the customer layer and reports `customer` as a
source. `CLAUDE.md`'s subcommand list gains `customer`.

## 3. Remote shipping

### The layer

`AgentLaunchRequest` and the workflow launch request gain
`customer: Option<CustomerLaunch { slug, layer_toml, expected_accounts }>`, where
`layer_toml` is the customer's `config.toml` as stored and `expected_accounts` the
manifest's account names.

- Every connector delivers it — local, SSH, HTTP, tunnel, bucket — as it delivers
  `findings_profile`. The remote `rupu run` / `rupu workflow run` gets
  `--customer-layer <file>` (a 0600 temp file the connector writes on the host; the run
  copies it into its run directory, so a resume re-reads the same layer) and
  `--customer <slug>`.
- Peers advertise `run.customer_layer` (HTTP `/api/host/info` features, tunnel
  `Hello.capabilities`, bucket worker markers, SSH `rupu __features`). A connector whose
  peer does not advertise it **refuses** the launch — never drops the layer.
- The layer holds no secrets (credentials live in the auth store, never in config), so
  it may travel over every transport.
- On the remote, the shipped file takes the customer slot: remote global → shipped
  layer → the repo's `.rupu/config.toml` from the staged workspace.

### Account check on the remote

A remote's own global locks still beat the shipped layer (§1 precedence), which could
silently change the billed account. So the remote resolves the run's manifest itself and
compares it with `expected_accounts`. A difference refuses the run before any model
call, naming the key and the lock: *"host `mini` locks `default_provider` to `anthropic`;
Acme's run expects `anthropic-acme`."*

### Check, then ship credentials

1. **Ask.** The coordinator asks the host which of the manifest's accounts it has:
   names, auth mode, and whether each is valid (present and not expired) — never values.
   SSH runs the hidden `rupu __accounts --check <names…>`; HTTP calls
   `POST /api/host/accounts/check`; tunnel sends an `AccountsCheck` frame; a bucket
   worker lists its account names and validity in its `nodes/<worker>.json` marker. Once
   per host per run: a 40-unit fan-out over 3 hosts
   makes 3 checks, cached for the run.
2. **API key missing → persist.** Written into the remote auth store under the same
   account name: SSH pipes it to the hidden `rupu __auth import --account <name>` on
   stdin (never argv); HTTPS `POST /api/host/accounts/import`. An account that already
   exists on the host is **never overwritten** — the name is the contract.
3. **SSO missing or expired → lease.** The coordinator refreshes its own token if it is
   near expiry, then leases the access token and its `expires_at` — never the refresh
   token, so the rotation that two copies of one refresh token would fight over cannot
   happen. The lease is written to a 0600 file in the run directory over the same
   channel; the run reads and unlinks it at startup and holds it in memory only; it is
   never in argv or the environment. A run that outlives its lease fails with a typed
   auth outcome whose hint names the host; `rupu workflow resume` takes a fresh lease, so
   recover-on-interrupt continues it.
4. **Transport gate.** Credentials travel only over SSH and `https://` HTTP. A tunnel,
   bucket or plain-`http://` host missing an account the run needs refuses the launch:
   *"`kuki` lacks `anthropic-acme` and its transport cannot carry credentials — log in on
   it: `rupu auth login --account anthropic-acme`."* A host that already has everything
   proceeds; only the layer ships.

Peers advertise `accounts.check`, `accounts.import` and `accounts.lease`; a missing
capability is a refusal with the same "log in on that host" message, never a skip.

### Audit

Each shipment appends a run event `CredentialShipped { account, host, mode:
persisted | leased, expires_at? }` to the coordinator's run (never the value). Refusals
are `StepFailed` with the message above. The CP shows both in the run's Events tab.

## 4. CP

### API

| Endpoint | Purpose |
|---|---|
| `GET /api/customers[?archived=1]` | rows: slug, name, color, archived, projects, runs, spend, findings, last active |
| `POST /api/customers` | create |
| `GET/PATCH/DELETE /api/customers/:slug` | detail · metadata patch · delete (409 with the assigned projects) |
| `POST /api/customers/:slug/archive`, `…/unarchive` | |
| `PUT/DELETE /api/customers/:slug/projects/:ws_id` | assign · unassign |
| `PUT /api/config/customer/:slug` | write the customer layer (same validation and dotted-key handling as `put_project`) |
| `GET /api/config?customer=<slug>` | effective config with `Customer` provenance and customer locks |
| `POST /api/launch/preview` | resolved customer, credential manifest, and (after Plan 3) the per-host shipping plan: persist / lease / refuse |

- **Rollups** come from each run's recorded `customer`, through the existing usage index
  and `FileCache` — no per-request rescans (the CP load-time rules).
- **`?customer=<slug>`** is accepted by the runs, sessions, findings, usage, projects and
  dashboard endpoints, including the per-host `?host=` paths. Rows from remote hosts are
  filtered on the coordinator by their `customer` field, because an older remote ignores
  the parameter. A host whose rows carry no `customer` field is excluded from a filtered
  view and reported (`hosts_without_customer: [...]`), so the UI can say so instead of
  showing a partial total as complete.
- `ProjectRow` and `RunListRow` gain `customer: Option<{ slug, name, color }>`.

### Web

- **Top-bar customer picker** — a new shell-level scope (the web has none today). Lives
  in the URL (`?customer=acme`) and is remembered per browser; narrows Dashboard,
  Activity, Usage, Findings and Projects. Archived customers are listed under a toggle.
  When some hosts cannot report customers, a quiet note says how many.
- **Customers page** — list with rollups and the customer's color; detail page with
  Projects, Usage, Findings and Config tabs. Config reuses `ConfigEditor` with the
  Customer provenance source and lock toggles writing the customer's `[policy].lock`.
- **Management** — create modal; metadata editing; archive/unarchive; delete shows the
  409's project list. Project detail's header gets a customer chip with an assign /
  unassign menu.
- **Launch and run** — the Launcher calls `/api/launch/preview` and shows
  *"Acme · anthropic-acme"* before launch (and the per-host shipping plan once Plan 3
  lands). Run detail's header shows the customer chip; `CredentialShipped` events render
  in the Events tab.
- **Visual first.** A mockup of the Customers page and the top-bar picker goes to matt
  before the web work is built.

## Error handling summary

| Situation | Behaviour |
|---|---|
| Dangling assignment / malformed customer layer, on a launch | Run fails before any model call, naming project + slug + parse error |
| Same, on a read-only view | View renders, error shown |
| Assign to unknown or archived customer | Refused |
| Delete customer with projects | Refused (CLI error, CP 409) listing them |
| Remote peer lacks `run.customer_layer` | Launch refused |
| Remote resolves different accounts than expected | Run refused, naming key + lock |
| Account missing on host, transport can't carry credentials | Launch refused with the `rupu auth login --account …` hint |
| Account exists on host | Used as-is, never overwritten |
| SSO lease expires mid-run | Typed auth outcome naming the host; resume re-leases |

## Testing

- **`rupu-config`** — the precedence matrix (global lock > customer lock > project >
  customer > global > default); a customer cannot unlock a global lock; a project lock
  list is still ignored; `Customer` provenance and `locked_by`; arrays replace across
  three layers; dotted-key lockstep cases for the customer layer.
- **`rupu-workspace`** — store CRUD; slug validation; delete refusal; assign registers an
  unknown path; the sidecar survives an `upsert` interleaved with `assign`;
  `layer_paths_for` on no record / no sidecar / dangling sidecar.
- **`rupu-runtime`** — manifest: agent provider, `auth:`, fallbacks chain, SCM rule
  selection, default-provider inheritance from the customer layer.
- **`rupu-cli`** (`tests/serial/`, holding `ENV_LOCK`) — `rupu customer` commands;
  `rupu run` and `workflow run` in an assigned project use the customer's
  `default_provider` (mock provider, `test_support::ENV_LOCK`); a customer lock beats the
  repo's `.rupu/config.toml`; a dangling assignment fails the run; the run records
  `customer`.
- **Remote** — each connector carries `CustomerLaunch` (argv / request / frame tests);
  capability refusal; account-mismatch refusal; the transport gate; existing accounts not
  overwritten; the lease file is 0600, unlinked at startup and absent from argv/env;
  `CredentialShipped` events; one account check per host per run. A live check against
  one real SSH host with probes batched into a single invocation.
- **`rupu-cp`** — endpoint tests (CRUD, 409, assignment, filter incl. remote rows without
  `customer`, preview); vitest for the picker, Customers pages, assignment and preview.

## Plans

1. **Model, config layer and CLI** — `CustomerStore`, sidecars, `LayerPaths` and the
   three-layer resolver, every call site converted, run records carry `customer`,
   `credential_manifest`, `rupu customer`, docs.
2. **CP surfaces** — API, rollups, `?customer=` filter, picker, Customers pages,
   management, launch preview (customer + accounts) and run chip. Mockup first.
3. **Remote shipping** — `CustomerLaunch` across connectors, `run.customer_layer`, the
   remote account check, `__accounts` / import / lease, the transport gate,
   `CredentialShipped`, and the shipping plan in the launch preview.

Each plan lands as its own PR.
