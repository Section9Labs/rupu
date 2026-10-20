# Operator MCP, Plan 3a: CLI-only capabilities into the CP API and the MCP

> **For agentic workers:** this plan is split into **work packages** (A–F). Each package is built by its own session in its own worktree, which opens one PR. In each package, use superpowers:test-driven-development: write the API test first, then the endpoint, then the operator tool entry.

**Goal.** Every capability in spec M8 becomes a CP endpoint and an operator MCP tool. The CP endpoint is the one implementation; the web UI and the MCP both reach it. Today these capabilities exist only in the CLI.

**Spec.** `docs/superpowers/specs/2026-10-19-rupu-operator-mcp-design.md`. This plan implements M8, "Delivery order" item 3. It covers the API and MCP half (3a). The web UI half (3b) is a separate plan: no package here changes `crates/rupu-cp/web/`.

**Builds on.** Plan 1 (#884) and Plan 2 (#893). These give the operator tool table in `crates/rupu-cp/src/mcp/ops/`, the `op(...)` builder, the lockstep test `tests/it/mcp_routes.rs` and the pinned tool names `tests/fixtures/mcp/tool_names.json`. Plan 2 is the worked example: its "Area tasks: the shared recipe" section is the recipe every endpoint here also follows.

## Global constraints

Every package follows all of Plan 2's "Global Constraints", with one exception: this plan **adds** CP endpoints and handler logic, which Plan 2 forbade. Restated briefly:

- **Full access, no guardrail layer.** Hints are informational only.
- **Tool names and descriptions.** Tool names are `<area>.<verb>`. A description's first line stands on its own, ends with a period and is at most 120 characters. Hint defaults depend on the method. Override them per spec M4: destructive for prune, discard, hard stop and force installs; open_world for launches and SCM-touching calls.
- **Repo rules.**
  - Workspace deps only; `#![deny(clippy::all)]`; no `unsafe`.
  - Tests are modules under `tests/it/`.
  - Format with per-file `rustfmt` only.
  - Run targeted tests only.
  - Never `git stash`.
  - Never bind or touch port 7878, where matt's live CP runs.
  - Every change lands through a PR. CI is release-gated, so verify locally before opening it.

And these rules for new endpoints (spec M8):

1. **One implementation.** An endpoint calls the library function the CLI command calls. If that logic lives inside `rupu-cli`, choose one of two routes:
   - (a) **Move it** into the library crate it belongs in, and make the CLI call it from there. This is preferred for pure logic: validation, listings, manifest reads.
   - (b) **Inject it** as a port on `AppState`, implemented in `rupu-cli`. Use this for anything that spawns a `rupu` process or needs a feature only the CLI enables, such as `rupu-skills`'s `git` feature.

   Never copy the logic. `rupu-cli` stays a thin dispatcher (CLAUDE.md rule 2).
2. **Long-running work runs detached.** A launch, stop, compact, install or prune that the CLI runs for a long time is spawned as a detached `rupu` subprocess. Use W6's `RunArgv` + `rupu_runtime::spawn::spawn_detached`, adding a `RunArgv` variant if one is missing, plus its `argv_roundtrip` case. The endpoint returns as soon as the work is recorded: an id, or 202. A short synchronous operation, such as validate or a listing, answers inline.
3. **Record before acting.** This is the pending-state rule, not optimistic success. A 200 or 202 means recorded, never done.
4. **Remote hosts.** A `host` argument works only where a `HostConnector` method already has a transport path. Otherwise the endpoint is local-only: `?host=<remote>` gets a 501 whose body says the operation is local-only. No package adds a new `HostConnector` method.
5. **Writable gate.** A mutating endpoint goes through the same `require_writable` gate as the existing writes, so a CP without `cp serve`'s ports answers 501.
6. **Errors.** Use the existing statuses: 400 invalid, 404 unknown, 409 conflict (not live, already running, exists), 501 can't here, 502 host offline. The CP's error body shape applies.
7. **Sibling paths.** A new `:param` route next to literal siblings, such as `/api/autoflows/:name` next to `/api/autoflows/claims`, gets Plan 2's Review Focus 1 test: a path argument equal to a sibling's literal segment is refused before any handler runs.
8. **Lockstep.** Every new route gets an `op(...)` entry in its package's ops file, and `tool_names.json` is re-blessed (`BLESS=1`). `EXCLUDED` gains nothing.
9. **Docs.** Each package adds its endpoints to `docs/cp-api.md`, in that file's existing style, under a heading for its area. The `docs/mcp.md` rewrite belongs to Plan 4, so packages don't touch it.

### Shared touch points (merge protocol)

Every package edits a few shared files. Keep each edit small and confined to its own place:

| File | What a package adds |
|---|---|
| `crates/rupu-cp/src/server.rs` `api_routes()` | One `.merge(<area>::routes())` line, only if the package adds a new api file |
| `crates/rupu-cp/src/mcp/ops/mod.rs` | One `mod` line and one `<area>::ops()` line, only for a new ops file |
| `crates/rupu-cp/src/state.rs` | New `AppState` port fields, appended at the end of the struct |
| `crates/rupu-cli/src/cmd/cp.rs` | The port adapters' construction: one block per port, next to the existing ones |
| `crates/rupu-cp/tests/fixtures/mcp/tool_names.json` | Re-blessed |
| `crates/rupu-cp/tests/it/main.rs` | New test modules |
| `docs/cp-api.md` | One section per area |
| `CHANGELOG.md` | One line under Unreleased |

PRs merge one at a time. After each merge, the next package rebases onto the new `main`. If any test files conflict, it resolves them, then re-blesses `tool_names.json` and re-runs its tests. The integrating session (the one that wrote this plan) merges them in this order: A, B, C, D, E, F.

### Per-package definition of done

- Every endpoint in the package's table exists, with API tests: the happy path, each error status the table names, and `?host=<remote>` → 501 for local-only endpoints.
- Every endpoint has its operator tool. Add `tests/it/mcp_ops_<package>.rs` following Plan 2's recipe: the tools are listed with their required arguments; a read through MCP equals the HTTP answer; a write records through MCP.
- The lockstep test (`mcp_routes::`) and the tool-name pin pass.
- `cargo clippy -p rupu-cp -p rupu-cli --all-targets` is clean, plus any crate the package moved logic into.
- For a detached spawn: a test over the injected port or `RunArgv` asserts the exact argv, through the existing argv round-trip test for a new variant.
- One live check per package, against a `cp serve` on a free port (never 7878) with an isolated `RUPU_HOME`: call one write tool through `/mcp` with `curl` JSON-RPC, then confirm its effect on disk. Record it in the PR body.
- The PR body lists every endpoint and tool, and the calls the package made itself.

---

## Package A — Agentiflows

| Capability | Endpoint | Tool | Notes |
|---|---|---|---|
| Launch an agentiflow, detached | `POST /api/agentiflows` `{definition, goal?, inputs?, workspace?/scope?, …the flags `agentiflow run` takes}` → 202 `{id}` | `agentiflows.run` (open_world) | `RunArgv::Agentiflow` (exists, `--detach`) through a new `AgentiflowLauncher` port implemented in `rupu-cli`. The id is minted before spawning so it can be returned and recorded. Local-only. |
| Hard-stop an agentiflow | `POST /api/agentiflows/:id/stop` `{now: bool}` | `agentiflows.stop` (destructive when `now`) | The graceful stop stays on `steer {stop}`; `now: false` does the same thing here. `now: true` is CLI `agentiflow stop --now`: the same library path. 409 when the flow is not live. |
| Agentiflow definitions | `GET /api/agentiflow-defs`, `GET/PUT/DELETE /api/agentiflow-defs/:name`, `POST /api/agentiflow-defs` (create), `POST /api/agentiflow-defs/validate` | `agentiflow_defs.list/get/save/delete/create/validate` | Mirror `api/workflows.rs`'s definition CRUD: global and project layers, `scope_kind`/`scope_id`. Use the parser the CLI's `agentiflow run` loads definitions with. |

**Files:** `api/agentiflows.rs` (launch, stop), new `api/agentiflow_defs.rs`, `mcp/ops/agentiflows.rs`, new port module `crates/rupu-cp/src/agentiflow_launcher.rs`, plus its adapter in `rupu-cli`. **Tests:** `tests/it/agentiflow_api.rs` (or extend the existing one), `mcp_ops_agentiflows.rs`.

## Package B — Runs and launches

| Capability | Endpoint | Tool | Notes |
|---|---|---|---|
| Continue an agent run | `POST /api/runs/agents/:id/continue` `{prompt?, model?, provider?}` → 202 `{run_id}` | `agent_runs.continue` (open_world) | `RunArgv::Agent` with `continue_from`, through the existing `AgentLauncher` port, extended if it needs to be. `--continue` restores the original launch flags (`RunStart.launch_args`); the explicit fields override one each. A sub-run id `sub_…` works too. 409 when the run is still running. |
| Retry a workflow run | `POST /api/runs/:id/retry` `{restart_interrupted?: bool}` | `runs.retry` (open_world) | CLI `workflow resume [--restart-interrupted]` on a failed, rejected or cancelled run, via `RunArgv::WorkflowControl::Resume`. Flip the record to `Running` first, as the CLI does. 409 on a run that is running, paused or completed. Remote hosts: only if `HostConnector::resume_run` already carries it; otherwise 501. |
| Launch body gaps | `POST /api/agents/:name/run` gains `model`, `provider`, `engagement_profiles`. `POST /api/workflows/:name/run` gains `engagement_profiles`, plus a raw-YAML launch: `POST /api/workflows/run` `{yaml, inputs…}`, a workflow that is not saved under a name. | the existing `agents.run`, `workflows.run`; new `workflows.run_yaml` (open_world) | Each new field becomes the `RunArgv` flag the CLI already has. The raw YAML is validated first (400 with the validator's errors), then written to a run-scoped temp file and launched with `--file` (or the CLI's equivalent). A remote launch forwards the new fields only where the connector carries them; otherwise it refuses, per the CP's capability rule. |
| Agent validation | `POST /api/agents/validate` `{raw}`, `GET /api/agents/validate` (every agent file, both layers) | `agents.validate`, `agents.validate_all` | `rupu_agent::check_agent_files` / `load_agent` + `validate_tools` + `check_agent_skills`: what `rupu agent validate` runs. |

**Files:** `api/runs.rs`, `api/run_streams.rs` (if agent runs live there), `api/agents.rs`, `api/workflows.rs`, the launcher port modules, `mcp/ops/runs.rs` and `mcp/ops/definitions.rs`, and the CLI adapters. **Tests:** API tests next to the existing ones, plus new `mcp_ops_launch.rs`.

## Package C — Sessions, skills, the stock fleet, init, engagement profiles

| Capability | Endpoint | Tool | Notes |
|---|---|---|---|
| Stop a session | `POST /api/sessions/:id/stop` | `sessions.stop` (destructive) | `rupu session stop`, through `SessionMutator` with a new `Stop` action and its subprocess adapter. Remote: only through an existing connector path, else 501. |
| Compact a session | `POST /api/sessions/:id/compact` `{window?}` | `sessions.compact` | `rupu session compact`, detached. 409 when a turn is running. |
| Install skills | `POST /api/skills/install` `{source, ref?, skills?, project?: ws_id, force?}` → `{installed, skipped}` | `skills.install` (open_world; destructive when `force`) | `rupu_skills::install_from` with `GitFetcher`. The `git` feature is enabled only in `rupu-cli`, so this goes through an `AppState` `SkillInstaller` port implemented there. It answers inline, since an install is bounded. |
| Update skills | `POST /api/skills/update` `{names?, project?, force?}` | `skills.update` (open_world) | `rupu_skills::update_skills`, through the same port. |
| Validate skills | `POST /api/skills/validate` `{path}` or `{layer, name}` | `skills.validate` | Run what `rupu skill validate` runs. |
| The stock fleet | `GET /api/fleet` (the manifest, plus each file's installed status per layer), `POST /api/fleet/install` `{force?, project?}` | `fleet.list`, `fleet.install` (destructive when `force`) | `templates::FLEET_MANIFEST` lives in `rupu-cli`. Either move the manifest and the install into a library (preferred, if its `include_str!` templates move with it), or inject a `FleetInstaller` port. Choose based on build cost. |
| Project init | `POST /api/projects/init` `{path, with_samples?}` | `projects.init` | `rupu init`, same choice as for the fleet. The path must be absolute and must exist: 400 otherwise, 409 when it is already initialised (unless the CLI has a force flag). |
| Engagement profiles | `GET /api/engagement-profiles`, `GET /api/engagement-profiles/:id` | `engagement_profiles.list`, `engagement_profiles.get` | The built-in profiles plus any layered ones, each with its bundle, asset kinds and checks. Use the registry the run assembler resolves profiles from. |

**Files:** `api/sessions.rs`, `session_mutator.rs`, `api/skills.rs`, new `api/fleet.rs` (fleet and init), new `api/engagement_profiles.rs`, `mcp/ops/sessions.rs`, `mcp/ops/definitions.rs` (skills; one block, appended), new `mcp/ops/setup.rs`, plus the ports and adapters. **Tests:** API tests, plus `mcp_ops_setup.rs`.

## Package D — Findings, coverage, query help

| Capability | Endpoint | Tool | Notes |
|---|---|---|---|
| Import findings | `POST /api/findings/import` `{paths, id?, dry_run?}` → the import report | `findings.import` | `rupu_coverage::tools::attach_report::attach_reports` / `rupu-findings-report`'s `import`: what `rupu findings import` runs. `paths` must be absolute files readable on this machine. Keep the CLI's backup and lock behaviour. Local-only. |
| Finding schema | `GET /api/findings/schema?advertised=` | `findings.schema` | `rupu findings schema`: the embedded draft-07 schema, or the advertised profile schema. |
| Coverage gap | `GET /api/coverage/:target/gap` | `coverage.gap` | `rupu coverage gap`'s computation. |
| Coverage rerun | `POST /api/coverage/:target/rerun` `{run}` | `coverage.rerun` (open_world) | What `rupu coverage rerun` launches, detached through the existing launcher port. |
| Query help | `GET /api/query/:list/fields`, `POST /api/query/:list/check` `{q}` | `query.fields`, `query.check` | `:list` ∈ findings, workflow_runs, agent_runs, sessions, autoflow_runs, the registries the web twins are pinned to (`tests/fixtures/list_query/`, findings' `FIELDS`). `check` answers `{ok: true, terms}` or the same `{error, token, code, start, end}` a list's 400 gives, with status 200 for the result: the check itself succeeded. |

**Files:** `api/findings.rs`, `api/coverage.rs`, new `api/query.rs`, `mcp/ops/findings.rs`, new `mcp/ops/query.rs`. **Tests:** API tests, plus `mcp_ops_findings_extra.rs`.

## Package E — Autoflows, cron, maintenance

| Capability | Endpoint | Tool | Notes |
|---|---|---|---|
| Run an autoflow now | `POST /api/autoflows/:name/run` `{issue?}` → 202 | `autoflows.run` (open_world) | `rupu autoflow run`, detached. |
| Explain an autoflow | `GET /api/autoflows/:ref/explain` | `autoflows.explain` | |
| Autoflow doctor | `GET /api/autoflows/doctor` | `autoflows.doctor` | |
| Repair claims | `POST /api/autoflows/claims/repair` `{dry_run?}` | `claims.repair` (destructive unless `dry_run`) | |
| Wakes | `GET /api/autoflows/wakes` | `autoflows.wakes` | |
| Show one autoflow | `GET /api/autoflows/:name` | `autoflows.get` | |
| Cron | `GET /api/cron`, `GET /api/cron/events`, `POST /api/cron/tick` `{dry_run?}` | `cron.list`, `cron.events`, `cron.tick` | `tick` runs the same tick `cp serve`'s loop runs. It must not run concurrently with that loop: take the loop's own lock or single-flight. Answers 409 when a tick is already in progress. |
| Prune | `POST /api/maintenance/prune` `{kind: transcripts\|sessions\|netflow\|cleanup, older_than?, dry_run?}` → `{would_remove \| removed}` | `maintenance.prune` (destructive unless `dry_run`) | The CLI's prune and cleanup functions, moved into their library crates if they live in `rupu-cli`. `dry_run` is the default when the field is absent (my call: safe default). |

The route `GET /api/autoflows/:name` must not shadow `GET /api/autoflows/claims`, `/wakes` or `/doctor`. Register the literal routes, and add Plan 2's route-guard sibling test for `name: "doctor"` etc.

**Files:** `api/autoflows.rs`, `api/autoflow_claims.rs`, new `api/cron.rs`, new `api/maintenance.rs`, `mcp/ops/definitions.rs` (autoflow block, appended), new `mcp/ops/ops_admin.rs` (cron, maintenance). **Tests:** API tests, plus `mcp_ops_admin.rs`.

## Package F — SCM, repos, auth status

| Capability | Endpoint | Tool | Notes |
|---|---|---|---|
| SCM accounts | `GET /api/scm/accounts` | `scm_accounts.list` | `rupu scm accounts`: each account's platform, kind, host and whether credentials are present. **Never** a token or secret. Add a test that fails if a stored token's value appears anywhere in the response. |
| SCM binding rules | `GET /api/scm/rules`, `POST /api/scm/rules` (what `rupu scm bind` writes) | `scm_rules.list`, `scm_rules.bind` | Writes go through the config writer the CLI uses (the dotted-key contract, `config_write`). |
| Tracked repos | `GET /api/repos/tracked`, `POST /api/repos/tracked` (attach), `POST /api/repos/tracked/prefer`, `DELETE /api/repos/tracked/:id` (forget) | `repos.tracked`, `repos.attach`, `repos.prefer`, `repos.forget` (destructive) | `rupu repos attach/prefer/tracked/forget`. Issues stay on the catalog tools. |
| Auth status | `GET /api/auth/status` | `auth.status` | `rupu auth status`: providers and SCM accounts, auth mode, expiry, ok or needs-login. Never a secret (same test). `auth login` stays out (spec non-goal). |

**Files:** new `api/scm.rs`, `api/repos.rs`, new `api/auth_status.rs`, new `mcp/ops/scm.rs`. **Tests:** API tests, plus `mcp_ops_scm.rs`.

---

## After the packages

The integrating session:

1. Merges A–F in order, rebasing each onto the new `main` and running the lockstep tests plus every `mcp_ops_*` module after each merge.
2. Runs one end-to-end check across packages, over HTTP against an isolated `cp serve`: launch an agentiflow, wait for its round, stop it; retry a failed workflow run; install a skill from a local path; prune with `dry_run`.
3. Writes Plan 3b, the web UI. It mocks every capability's UI first and gets matt's sign-off before building (spec "Web UI for the additions").
