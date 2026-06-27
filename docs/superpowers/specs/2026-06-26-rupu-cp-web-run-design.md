# rupu CP web — smart Run target pickers + agent Run

Date: 2026-06-26 (corrected 2026-06-27)
Status: approved (design)

## Reality check (corrected)

The CP web UI **already has workflow Run** (this was missed in the first
exploration). On `main` (v0.18.0):
- `WorkflowDetail` has a Run button → `LauncherSheet` modal
  (`crates/rupu-cp/web/src/components/LauncherSheet.tsx`): declared/free-form
  inputs, a Mode picker (Ask/Bypass/Read-only), and a single free-text
  **Target** field.
- `api.launchRun(workflow, body)` → `POST /api/workflows/:name/run` →
  `launch_run` handler → the `RunLauncher` port. `rupu-cli`'s `cp serve`
  installs `SubprocessLauncher`, which spawns `rupu workflow run …
  --run-id … --plain` and returns the new run id.
- Cancel is also fully built: `POST /api/runs/:id/cancel` → `RunStore::cancel`
  (marks `Cancelled`, SIGTERMs the run's `runner_pid`) + Cancel buttons on
  `RunDetail`.

So "build Run" is done. The actual asks are improvements to this surface:

1. **Repository fuzzy-complete** in the Target field, from the logged-in repo
   list.
2. **Directory picker** — browse + fuzzy-complete from previous projects/paths —
   as a target (run in that directory).
3. **Agent Run** — the same Run experience on `AgentDetail` (none exists today).

The one already-kept change from the initial detour: `SubprocessLauncher` now
spawns the run **detached** (own process group + null stdio) so a run survives
`cp serve` being closed (commit on this branch).

## Architecture pattern

The runtime-dependent backend work follows the existing **port** pattern
(`RunLauncher`, `session_sender`): `rupu-cp` defines a thin trait + HTTP handler;
`rupu-cli`'s `cp serve` installs the adapter that has the full runtime (SCM
registry, keychain). Read-only `rupu cp` (no adapter) returns 501 / empty. This
keeps `rupu-cp` free of `rupu-scm`/`rupu-auth` deps.

## Phasing (3 PRs)

- **PR1 — Repo picker** (+ the detach fix). `RepoLister` port + `GET /api/repos`
  + `cp serve` adapter (wires `rupu-scm` `Registry::discover` → `list_repos`);
  frontend repo fuzzy-complete in `LauncherSheet`'s Target.
- **PR2 — Directory picker.** `GET /api/fs/browse` + a `working_dir` field on
  `LaunchRequest` (and `--dir`/`current_dir` plumbing through the launcher);
  frontend directory browse + fuzzy-complete (sourced from `getProjects()` +
  browse).
- **PR3 — Agent Run.** Agent launch port/endpoint + `cp serve` adapter spawning
  `rupu run <agent> …`; a Run modal on `AgentDetail` reusing the pickers; cancel
  works unchanged.

Each PR is independently shippable. This spec details PR1; PR2/PR3 are scoped.

---

## PR1 — Repository fuzzy-complete

### Backend
- **`rupu-cp` `RepoLister` port** (`crates/rupu-cp/src/repos.rs`, mirroring
  `launcher.rs`):
  ```rust
  pub struct RepoEntry { platform: String, repo: String /*owner/name*/,
                         default_branch: String, private: bool }
  #[async_trait] pub trait RepoLister: Send + Sync {
      async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError>;
  }
  ```
  Add `AppState.repos: Option<Arc<dyn RepoLister>>` + `with_repos(...)`
  (mirroring `with_launcher`).
- **`GET /api/repos`** (`crates/rupu-cp/src/api/repos.rs`): if `repos` port is
  `None` → `ApiError::not_available`; else return `port.list().await` (map
  errors to 500). Registered in the router.
- **`rupu-cli` adapter** (`crates/rupu-cli/src/cp_repos.rs`), installed in
  `cmd/cp.rs` `serve` alongside the launcher: builds the SCM registry the same
  way the CLI `rupu repos list` does (`Registry::discover(resolver, &cfg)` →
  for each configured platform `registry.repo(p).list_repos()`), maps
  `rupu_scm::Repo` → `RepoEntry` (`platform`, `"{owner}/{repo}"`,
  `default_branch`, `private`). Cache the result in the adapter with a short TTL
  (e.g. 60s) since it's a live API call; on error return what it can + log.

### Frontend
- **api.ts**: `RepoEntry` type + `getRepos(): Promise<RepoEntry[]>`
  (`GET /api/repos`). Tolerate 501 (no adapter) → treat as empty list.
- **`LauncherSheet` Target → fuzzy-complete combobox.** Replace the plain Target
  `<input>` with a small typeahead: as the user types, fetch-once the repo list
  and filter (substring on `platform:owner/repo`), show a suggestion dropdown;
  selecting sets the target to `"{platform}:{repo}"`. Keep free-text fallback
  (so PR/issue refs and arbitrary targets still work). The combobox is a new
  reusable component `components/Combobox.tsx` (no new deps; input + filtered
  listbox + keyboard up/down/enter/esc).

### Testing (PR1)
- `rupu-cp`: `GET /api/repos` → 501 when no port; with a mock `RepoLister`,
  returns its entries (handler test).
- `rupu-cli`: `rupu_scm::Repo` → `RepoEntry` mapping (pure fn) — platform string,
  `owner/repo` join, private flag.
- web (vitest): `Combobox` filtering (substring match, keyboard select) and that
  selecting a repo sets the launch `target` to `platform:owner/repo`; free-text
  still passes through.

### Notes / risks
- Repo listing is a live network call → the adapter caches (TTL) so reopening
  the sheet is instant; the endpoint may be slow on first call (frontend shows a
  loading state in the dropdown).
- No logged-in platforms / read-only `rupu cp` → empty suggestions; the Target
  field still accepts free text.

---

## PR2 — Directory picker (scoped)

- **`GET /api/fs/browse?path=<dir>`** (`rupu-cp`): list immediate
  subdirectories (`{ name, path, is_dir }`), canonicalized; default to `$HOME`
  when no path; tolerant of unreadable dirs. (Pure filesystem read on the CP
  host — fine for a local tool; clamp to existing readable dirs, no symlink
  escape surprises.)
- **`LaunchRequest.working_dir: Option<String>`** + launcher adapter spawns the
  child with `current_dir(working_dir)` (so "run in this directory" works);
  `launch_run` body gains `working_dir`. Mutually-informative with `target`
  (a repo target clones itself; a working_dir runs in place).
- **Frontend**: a directory target mode in `LauncherSheet` — a browse tree
  (drill via `/api/fs/browse`) + a fuzzy-complete input seeded from
  `getProjects()` paths and browse results. Reuses the `Combobox`.

## PR3 — Agent Run (scoped)

- **Agent launch port/endpoint**: `POST /api/agents/:name/run` → an
  `AgentLauncher` port (or extend `RunLauncher` with an agent variant);
  `cp serve` adapter spawns `rupu run <agent> [target] [prompt] --run-id <id>
  --mode <m>` detached. Requires `--run-id` on `rupu run` (the agent run CLI) —
  add if absent (the workflow one already exists).
- **Frontend**: a Run button + modal on `AgentDetail` — a prompt textarea +
  Mode + the same target picker (repo + directory). Navigate to `/runs/<id>`.
  Cancel works unchanged.

## Out of scope
- Re-building workflow Run / Cancel (already exist).
- Notarization, non-macOS signal specifics.
