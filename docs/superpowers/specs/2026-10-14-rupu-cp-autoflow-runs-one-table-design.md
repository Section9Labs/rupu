# rupu CP — Autoflow runs as one table

Status: design approved in conversation (2026-10-14), spec pending review
Date: 2026-10-14
Builds on: `2026-07-07-rupu-autoflow-runs-firstclass-design.md` (autoflow runs
open in the shared `RunDetail`; unpersisted runs resolve from history).
Coordinates with: the unified-filter work (PR #868 and its follow-ups, "PR A"
web query bar, "PR B" `rupu-query` + server `?q=`).

## Why

Runs → Autoflows is the only run list in the CP with three tables (Runs /
Cycles / Claims), each at a different grain: history events, worker passes,
claimed issues. None answers "what did my autoflows run last, and did it
work?". An autoflow run is a workflow run that something launched
automatically, so the page should look and behave like Runs → Workflows: one
table, newest first, row click opens the run.

## Decisions (approved)

1. **One table, Workflows-shaped.** Rows are autoflow-launched workflow runs,
   newest first, plus one failed row per attempt that never produced a
   recorded run. Built with the CP's existing components and conventions
   (`SortableTable`, `FilterBar`, `StatusPill`, `CrewChip`, `Button`,
   `ErrorBanner`, `EmptyState`, `usePerHostPagedList`, and the unified query
   bar once it lands) — the approved mock fixed layout and content, not
   styling.
2. **Runs first** (approach 1 of 3). The run list is the source of truth for
   autoflow runs; the autoflow history is used only for failed attempts and
   the problems strip. Rejected: folding history events into rows (status
   lags, capped by history retention and the 200-event read), and folding in
   the browser (pages by event, can't take server `?q=`).
3. **Failed attempts are rows.** An attempt the autoflow made to run a
   workflow that failed is a `failed` row, like any failed run.
4. **Failures with no workflow attempt go in a strip** above the table (option
   A of A/B/C): a repo whose autoflow config fails to load, or a cycle that
   crashed. One line per ongoing problem.
5. **The Cycles and Claims tables go away.** Claim actions move to the run
   page; per-issue cycle history is already the run page's Cycles tab.

## What the page shows

- **Filter bar**: the Workflows status pills (All / Running / Completed /
  Failed·Rejected / Archived), `HostSelect allowAll`, the find box, Refresh.
  When the unified query bar lands it replaces the pills and find box, with
  the workflow-run fields (which then include `autoflow`, `issue`, `pr`) plus
  the attempt fields.
- **Columns**: Status, Autoflow, Workflow, Issue, Run, Trigger, Host, In,
  Out, Cached, Cost, Turns, Duration, Started, Customer (as Workflows), row
  actions (Archive / Delete, as Workflows; none on attempt rows).
  - **Autoflow**: the autoflow's name. Clicking it narrows the table to that
    autoflow (a removable chip in the filter bar).
  - **Issue**: the issue's display ref linking to the tracker, plus a PR link
    when the run opened one. `—` for an autoflow with no issue (cron).
- **Run rows**: identical to Workflows; click opens `/runs/:id`.
- **Attempt rows**: status `failed` (or whatever `run_resolve`'s claim-status
  mapping gives a not-yet-recorded launch, so the row and the run page agree),
  the error under the workflow name, no usage.
  - With a run id: click opens the run page, which already renders an
    unpersisted run from history.
  - Without one: click expands the row in place — the error, the cycle, the
    worker, the claim status, and **Requeue** / **Release** for its claim
    (local host only, as the claim endpoints are).
- **Problems strip** above the table, hidden when empty. One line per
  problem: what (repo `x`: config fails to load / worker `y`: cycle crashed),
  the latest error, `since <first seen> · <n> cycles · <host>`. A problem is
  shown while it was last seen within the past hour (idle cycles are not
  saved, so history can't tell a fix happened; the hour is the bound).
- **Removed**: the Runs/Cycles/Claims switcher and the Cycles and Claims
  tables.

## Data

### `RunListRow.autoflow`

`RunListRow` (`crates/rupu-cp/src/api/runs.rs`, also the JSON contract of
`rupu run list`, which SSH hosts return verbatim) gains:

```
autoflow: null | {
  name: string,                 // run_envelope.json → autoflow.name
  issue_ref?: string,           // RunRecord.issue_ref
  issue_display_ref?: string,   // the claim for issue_ref
  issue_url?: string,           // the claim for issue_ref
  pr_url?: string,              // the claim, only when its last_run_id is this run
}
```

- Serialized on every row from a rupu that knows the field: `null` = not
  launched by an autoflow (or no envelope recorded). A row WITHOUT the key
  comes from a host that can't report it; under an autoflow filter that host
  is "unavailable" (the customer rule — PR B implements it generically for any
  field a remote row can't report).
- The envelope is read through `rupu_runtime::file_cache::FileCache`
  (`RunStore::read_run_envelope` or a cached equivalent), so a listing costs a
  `stat` per run after the first read. No `RunRecord` change.
- Claims are read once per listing from the local claim store.
- Built in one place (a `RunListRow` helper) and used by every producer: the
  CP's local listing, `rupu run list`, and any connector that builds rows
  coordinator-side from a mirror (tunnel). Where a mirror doesn't hold the
  envelope, the row omits the key rather than claim `null`.

### `GET /api/runs/autoflows/runs`

Same parameters as `/api/runs/workflows` (`lifecycle`, `host`, `since` /
`until`, `customer`, `offset` / `limit`), plus `autoflow=<name>` until the
query bar's `autoflow:` field replaces it. Returns, newest first and paged
after merging:

- **Run rows**: workflow runs whose `autoflow` is non-null.
- **Attempt rows**: the same `RunListRow` shape (id = the run id if the
  attempt had one, else the history event id; `status`; `workflow_name`;
  `started_at` = the event time; `trigger`; zero usage) with `autoflow` set
  and an `attempt` object `{ detail, cycle_id, worker, claim_status }`.
  An attempt is:
  - a `cycle_failed` event naming a `workflow`, or
  - a `run_launched` event whose `run_id` has no run record.
  An attempt whose run id exists as a run is not listed (the run row is).
  Attempts appear only under All and Failed·Rejected (they can't be archived).
- History is read for the requested time range, not a fixed event count —
  `AutoflowHistoryStore` gains a range read over its day directories.
- **Customer scope**: run rows filter as on Workflows. An attempt carries the
  customer of the workspace its claim names, when that workspace has an
  assignment; otherwise it carries no customer keys and is not shown under a
  scope.
- **Remote hosts**: run rows via the existing `HostConnector::list_runs`
  (workflow kind), filtered coordinator-side to rows with a non-null
  `autoflow`; a host whose rows lack the key answers 501 (shown
  "unavailable"). Attempt rows via the existing `list_autoflow_events`, which
  covers only that host's newest events — the same limit today's page has. A
  connector without `list_autoflow_events` contributes runs only.
- After PR B, the endpoint takes `?q=` through `rupu-query`, with the
  workflow-run registry plus the attempt fields.

### `GET /api/runs/autoflows/problems`

`?host=` as the other per-host lists. Returns `cycle_failed` events that name
no workflow, seen within the last hour, grouped by host and subject (the
event's `repo_ref`, else the cycle's worker):

```
{ host_id, subject_kind: "repo" | "worker", subject, detail,  // latest error
  first_seen, last_seen, count }
```

Remote hosts via `list_autoflow_events`.

### Kept for compatibility

`GET /api/runs/autoflows` (cycles) and `GET /api/runs/autoflows/events` stay:
older coordinators read them from their peers over the HTTP connector. The web
stops calling them.

## Run page

`RunDetail` already shows an autoflow run's `AutoflowPanel` (issue, claim
status and lease, launching cycle) and its Cycles tab. `AutoflowPanel` gains
**Requeue** and **Release** (`api.requeueClaim` / `api.releaseClaim`, with the
confirmation the Claims table uses today). They show only for a local run; a
remote run's panel says claim actions are local-only.

## Code shape

- `crates/rupu-cp/src/api/runs.rs`: `RunListRow.autoflow` + the helper that
  fills it.
- `crates/rupu-cli/src/cmd/run.rs`: `rupu run list` fills it (same helper).
- `crates/rupu-cp/src/api/run_streams.rs` (or a new `api/autoflow_runs.rs`):
  the two endpoints. Attempt synthesis reuses `run_resolve.rs`'s claim-status
  mapping.
- `crates/rupu-runtime/src/autoflow_history.rs`: range read of events.
- Web: `pages/runs/AutoflowRuns.tsx` rewritten; the Workflows column builders
  move to a shared module both pages use (the unified-filter session agreed to
  rebase onto that); a `AutoflowProblems` strip component; `AutoflowPanel`
  claim actions; `lib/api.ts` client for the two endpoints.

## Coordination with the unified-filter work

- This work owns `RunListRow.autoflow`, landing in a small first PR.
- `autoflow:`, `issue:`, `pr:` go into the workflow-run registries
  (`crates/rupu-cp/src/query/workflow_runs.rs`, its fixture
  `crates/rupu-cp/tests/fixtures/list_query/workflow_runs.fields.json`, web
  `lib/query/lists/workflowRuns.ts`), so Runs → Workflows can filter by
  autoflow too. Whichever of the two PRs merges second adds them.
- The autoflow runs endpoint's `?q=` uses `rupu-query` and `WORKFLOW_RUN_FIELDS`
  plus the attempt fields; no autoflow-only grammar.

## Delivery

1. **PR 1**: `RunListRow.autoflow` (CP local listing, `rupu run list`,
   mirror-built rows) + tests.
2. **PR 2**: the two endpoints, the history range read, the rewritten page,
   the problems strip, `AutoflowPanel` claim actions. matt checks the page
   with real data before it merges.

## Testing

Server (`rupu-cp` `tests/it`, `rupu-runtime`):
- `RunListRow.autoflow` is set from the envelope for an autoflow run, `null`
  for a manual run and for a run without an envelope; `pr_url` only for the
  claim's last run.
- `/api/runs/autoflows/runs`: runs and attempts merged newest first and paged
  after the merge; an attempt whose run exists is not listed twice; lifecycle,
  `since`/`until`, `customer` and `autoflow` filters; a remote host whose rows
  lack `autoflow` → 501.
- `/api/runs/autoflows/problems`: grouping, counts, first/last seen, events
  older than an hour dropped, events naming a workflow excluded.
- History range read returns events across day directories within bounds.

Web (vitest):
- One table; attempt row with no run id expands with the error and claim
  actions; attempt row with a run id links to the run; clicking an autoflow
  name filters; the strip renders and hides when empty; per-host
  "unavailable" for a 501.
- `AutoflowPanel` shows Requeue / Release for a local run, not for a remote
  one.

## Not in scope

- The Workflows page itself (beyond rows carrying the new field).
- Build → Autoflows (definitions), autoflow history retention, worker health
  pages.
- Making claim actions work on remote hosts.
- A per-repo health record that would let the strip clear the moment a
  problem is fixed.
