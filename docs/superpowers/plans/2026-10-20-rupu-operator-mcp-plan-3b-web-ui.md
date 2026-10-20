# Operator MCP, Plan 3b: web UI for the Plan 3a capabilities

> **For agentic workers:** this plan is split into **packages** UI-1 to UI-5. Each package is built by its own session, in its own worktree, and opens one PR. Use superpowers:test-driven-development: write the vitest test first, then the component.

**Goal.** Every capability Plan 3a added to the CP API and the MCP also gets web UI. The UI is designed for the human's task, not as a mirror of the tool list (spec M8, "Web UI for the additions").

**Approved design.** `docs/superpowers/specs/assets/cp-plan-3b/mocks.html` was approved by matt on 2026-10-20. It is the contract. Its placement table says where each capability goes. Its eleven screens show how each one looks. A package builds what its rows and screens show, in the CP's existing components. A change from the mock needs a reason, written in the PR body.

**Spec.** `docs/superpowers/specs/2026-10-19-rupu-operator-mcp-design.md`, M8. The API side is Plan 3a, `docs/superpowers/plans/2026-10-20-rupu-operator-mcp-plan-3a-cli-capabilities.md`; its endpoints are documented in `docs/cp-api.md`.

## Global constraints

- **Visual language.** Follow `docs/superpowers/specs/2026-10-19-rupu-cp-visual-language-design.md` and its plan.
  - `Button` variants: `solid` is the primary action, `secondary` the neutral one, `danger-outline` / `danger` destructive, `ring` row actions.
  - `StatusChip` for state pills.
  - `PageHeader` where the page already uses it, and the page's existing header otherwise.
  - `Field` / `Input` / `Select` for forms. `DialogFrame` for modals (focus trap, Escape).
  - `EmptyState`, `ListSkeleton`, `ErrorBanner` / the inline `role="alert"` line.
  - Tokens from `styles.css` only.
  - Every screen works in light and dark.
- **Pending, not optimistic.** A 2xx means recorded, not done (CLAUDE.md rule 9, carried over to the CP). After a write, show the pending state ("retry requested", "compacting", "install running…"). Confirm only from the data changing (the next fetch, the live stream), never from the POST alone.
- **Errors.** Show the server's message inline under the action that failed. A 409 is not a crash: show its message. A 501 means the host or CP can't do it here: hide or disable the control, with the reason, as the existing gated controls do.
- **Destructive actions** (Stop now, Prune, Release claims, force installs, Forget, Delete) confirm first. Where the API has a dry run, preview first and act second (prune, claims repair, findings import).
- **No secrets.** The Accounts tab renders what `auth.status` / `scm_accounts.list` return, and nothing more.
- **API client.** `lib/api.ts` exports `apiRequest` (its `request` helper). Put each package's new client functions in their own file, `web/src/lib/api3b/<package>.ts`, with their types. Add nothing to `api.ts`, so packages don't conflict. Test the client functions with vitest, using the patterns of the existing `lib/api.*.test.ts` files.
- **Process rules.**
  - Tests: `cd crates/rupu-cp/web && npx vitest run <files>` for your tests, then the full `npx vitest run` once before the PR. `npx tsc -b` must be clean.
  - Never `git stash`. Never touch port 7878.
  - Every change lands through a PR. Do not merge your own.
  - No Rust changes, except a bug in a Plan 3a endpoint that the UI exposes. Fix that in the same PR, with a Rust test, and say so in the body.

## Visual check (per package, before the PR)

matt asked for every page to be checked visually before it merges.

1. Build the UI and binary: `make cp-web`, then `cargo build -p rupu-cli`.
2. Run `cp serve` on a free port (never 7878), against an isolated `RUPU_HOME`. Use the preview recipe: copy only what you need; set the `[cp]` loops, `[agentiflow] reaper_enabled/serve_enabled` and `[netflow] asn_auto_refresh` off. Seed the records your screens need: a failed workflow run, a finished agent run, an idle session, an agentiflow definition, a findings report to import, and so on. Invent data from scratch: the repo is public, so never use assessment data.
3. Screenshot every changed screen with headless Chrome (`"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --screenshot=… --window-size=…`), in **dark and light**. Include the state after an action where the mock shows one: the pending chip, the dry-run preview, the 409 line.
4. Put the screenshots in `docs/superpowers/specs/assets/cp-plan-3b/screens/<package>/`, commit them in the PR, and embed them in the PR body.
5. In the PR body, list each mock screen and row with its screenshot, plus any difference from the mock and why.

The integrating session checks every PR's screenshots against the mock, and sends them to matt before merging.

## Shared touch points

| File | Rule |
|---|---|
| `web/src/App.tsx` (routes) | Add only new route lines (UI-2: the agentiflow definition page) |
| `web/src/components/v2/pages/LibraryV2.tsx` | UI-2 adds the Agentiflows tab |
| `web/src/pages/Settings.tsx` | UI-5 adds three tabs. Nobody else edits it |
| `web/src/lib/api.ts` | Only the `apiRequest` export, which lands with this plan |
| `CHANGELOG.md` | One line under Unreleased |

The integrating session merges the PRs in order UI-1 … UI-5, rebasing each one.

---

## UI-1 — Runs and sessions

Mock §1–3, and the placement-table rows Retry, Continue, and Stop/compact a session.

- **Run page** (`pages/RunDetail.tsx`), terminal states only (failed, rejected, cancelled).
  - Add a `solid` **Retry ▾** in the header, before Archive and Delete. It opens a popover with:
    - **Mode**: the run's recorded `permission_mode`, labelled "(as launched)";
    - **Restart interrupted steps**: a checkbox with the mock's help line.
  - `POST /api/runs/:id/retry {mode?, restart_interrupted}` (`runs.retry`).
  - Then show a pending `retry requested` chip until the run's status is running. A 409 shows inline.
- **Inbox** (`components/inbox/InboxBoard.tsx`), "Recent failures" rows: a `solid` sm **Retry** beside View run. It uses the same call with the recorded mode. Replace the "no web retry" note.
- **Agent run transcript** (`pages/RunTranscript.tsx`), standalone runs that aren't running:
  - **Continue** opens the session-style composer under the transcript: an optional prompt, ⌘↵, and a "Model & provider" disclosure showing the original values.
  - `POST /api/runs/agents/:id/continue {prompt?, model?, provider?}`.
  - On success, "Continued as `<run_id>` →" links to the new run. A 409 shows inline.
- **Session page** (`pages/SessionDetail.tsx`):
  - **Compact** (`secondary`) opens a popover with the window field, defaulting to the session's own. `POST /api/sessions/:id/compact {window?}`.
  - While `GET /api/sessions/:id` reports `compacting: true`:
    - the header shows a `compacting` chip;
    - Compact and Stop are disabled;
    - the composer is locked, with the mock's line.
  - When it ends, the conversation shows the result line: "Compacted N turns into 1", or "Compaction discarded: …".
  - **Stop** (`danger-outline`, confirmed) calls `POST /api/sessions/:id/stop`.

Client file: `lib/api3b/runs.ts`.

## UI-2 — Agentiflows

Mock §4, and the placement-table rows Launch/hard-stop and Agentiflow definitions.

- **Agentiflows list** (`pages/runs/AgentiflowRuns.tsx`):
  - Add a `solid` **New agentiflow** to the header. It opens a launcher sheet in the `LauncherSheet` pattern: a definition picker (from `GET /api/agentiflow-defs`), the project/directory scope, and Launch. There is no host picker: flows are local-only.
  - `POST /api/agentiflows` returns `{id}`; navigate to `/agentiflows/:id`.
  - Replace the EmptyState's CLI hint with the button.
- **Agentiflow page** (`pages/AgentiflowDetail.tsx`), while running:
  - **Stop** (`secondary`) calls `POST …/stop {now:false}`; show the round-boundary pending state.
  - **Stop now** (`danger-outline`) opens the mock's warn confirm card, then calls `{now:true}`.
- **Library → Agentiflows tab** (`components/v2/pages/LibraryV2.tsx` + a new `pages/AgentiflowDefs.tsx`):
  - A list in the `Workflows.tsx` pattern: name, layer, lead and profiles, and a row **Launch**.
  - **New agentiflow** opens a modal with a YAML editor, validated as you type with `POST /api/agentiflow-defs/validate`.
- **Definition page** `/agentiflows/defs/:name` (new route): the `WorkflowDetail` editor layout. It has a YAML pane with a server `ValidityBadge`, plus Save (`PUT`), Revert, Delete (confirmed) and Launch.
- The v1 shell's Library equivalent gets the same tab, if v1 still has one; check `lib/shell.ts`.

Client file: `lib/api3b/agentiflows.ts`.

## UI-3 — Launchers, validation, stock fleet, project init

Mock §5, and the placement-table rows Launch fields, Agent validation, and Stock fleet / project init.

- **`components/AgentLauncherSheet.tsx`**:
  - A collapsed **Model & provider** section: provider and model selects, defaulting to the agent's own, from `GET /api/models`.
  - An **Engagement profiles** chips picker from `GET /api/engagement-profiles`. Each chip shows its bundle and asset kinds below, as in the mock.
  - For a remote host that can't carry these fields, disable them and give the API's reason.
- **`components/LauncherSheet.tsx`**: the same Engagement profiles picker.
- **New workflow modal** (`pages/NewWorkflowModal`): its YAML mode gains **Run without saving**, which calls `POST /api/workflows/run {yaml, inputs, mode}`.
- **Agent builder** (`components/agentBuilder/AgentBuilder.tsx`): the `ValidityBadge` and the warnings strip take their state from `POST /api/agents/validate {raw}`, debounced, as `WorkflowDetail` does with `validateWorkflow`.
  - The browser-only check stays as the instant first pass.
  - The server answer wins when it arrives.
- **Library → Agents** (`pages/Agents.tsx`):
  - **Validate all** (`secondary`) opens a drawer with the rows from `GET /api/agents/validate`.
  - **Install stock fleet** opens a dialog listing `GET /api/fleet` with each file's status and a force toggle (danger), then calls `POST /api/fleet/install`.
- **Projects** (`pages/Projects.tsx`): **Initialize project** opens a dialog with an absolute path and "Add sample agents and workflows", then calls `POST /api/projects/init`. The EmptyState offers it too.

Client file: `lib/api3b/launch.ts`.

## UI-4 — Skills, findings, coverage, query help

Mock §6–8, and the placement-table rows Skills, Import findings, Coverage, and Query help.

- **Library → Skills** (`pages/Skills.tsx`): **Install skills** opens the mock's dialog.
  - Fields: source, ref, and install-to (global / a project).
  - The "found in source" list comes from a dry pass. Use the install endpoint's preview if it has one; if not, add a `dry_run` to `POST /api/skills/install` in this PR, with its Rust test.
  - Each found skill has a checkbox and a status chip.
  - "Replace locally edited copies" (danger) maps to `force`.
  - Results show inline.
  - Remove the read-only note and the CLI hint in the EmptyState.
- **Skill page** (`pages/SkillDetail.tsx`):
  - **Update** calls `POST /api/skills/update {names:[name]}`.
  - **Validate** calls `POST /api/skills/validate {layer, name}`; show the result inline.
- **Findings explorer** (`components/findings/FindingsExplorer.tsx`): **Import** sits next to Export.
  - The dialog takes paths and runs a dry run first (`POST /api/findings/import {paths, dry_run:true}`), showing the attach / skip / fail rows.
  - The button then reads "Import N reports" and calls the real import.
  - **Report schema ↗** opens a read-only viewer of `GET /api/findings/schema`: a JSON tree, or pretty-printed text in the `CodeEditor` read-only.
- **Coverage page** (`pages/CoverageDetail.tsx`):
  - The Gap tab reads `GET /api/coverage/:target/gap`, unless it already shows the same data client-side. If it does, keep that and note it in the PR.
  - **Rerun…** in the header picks a run from the coverage runs, confirms, then calls `POST /api/coverage/:target/rerun {run}` and links to the new run.
- **Query help** (`components/query/ListQueryBar.tsx` / `QueryBar.tsx`):
  - A trailing **?** button opens the fields popover from `GET /api/query/:list/fields`: key, kind, values, and an example. The list's key comes from the bar's registry.
  - Lists that are filtered in the browser use their local registry, the same one the server lockstep pins.

Client file: `lib/api3b/library.ts`.

## UI-5 — Autoflows, Settings (Automation, Maintenance, Accounts), project repositories

Mock §9–11, and the placement-table rows Autoflow ops, Cron/prune, Accounts, and Tracked repos.

- **Library → Autoflows** (`pages/AutoflowsDefs.tsx`), on each row:
  - **Run now** opens an inline issue-ref field. Only a fully qualified ref is accepted; show the API's 400 for a bare number. It calls `POST /api/autoflows/:name/run`.
  - **Explain** opens a drawer with the text from `GET /api/autoflows/explain?issue_ref=`.
- **Activity → Autoflows** (`pages/runs/AutoflowRuns.tsx`):
  - In the header, **Doctor** opens a drawer (`GET /api/autoflows/doctor`).
  - **Repair claims** shows the dry-run list first, then a danger "Release N claims" (`POST /api/autoflows/claims/repair`).
  - A **Wakes** strip above the table, from `GET /api/autoflows/wakes`, appears only when there are wakes.
- **Settings** (`pages/Settings.tsx`) gets three new tabs, after the existing ones and before the Raw/Policy divider:
  - **Automation**: the cron schedules table and the event log (`GET /api/cron`, `/events`), with **Dry run** and **Tick now** (`POST /api/cron/tick {dry_run}`). Show the 409 when a tick is already running.
  - **Maintenance**: one row per prune kind, each with an older-than field. The row shows the dry-run count, fetched on open and when the field changes, then **Prune N**, confirmed, which calls `POST /api/maintenance/prune {kind, older_than, dry_run:false}`.
  - **Accounts**: the account table from `GET /api/auth/status` + `/api/scm/accounts`, with the state chips and, on a needs-login row, the CLI command to run. Below it, the binding rules from `GET /api/scm/rules`, with **Add rule** (`POST /api/scm/rules`).
- **Project page → Config tab** (`pages/ProjectDetail.tsx`): a **Repositories** section listing the tracked checkouts from `GET /api/repos/tracked`, filtered to this project. Row actions are **Prefer** and **Forget** (confirmed). **Attach** opens a dialog with the repo and the path.

Client file: `lib/api3b/admin.ts`.
