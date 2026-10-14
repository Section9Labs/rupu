# One query line for every CP list

Status: approved direction (matt, 2026-10-14); plan below.

## Goal

Every control-plane view that shows a list or table filters through one
full-width spotlight query line — the findings `QueryBar` — and nothing else:
no filter pills, status dropdowns, host selects, search boxes or per-host
status strips beside it. The line looks and behaves the same on every page;
a page supplies only its fields and its rows.

Already on it (PR #868): Findings (global, project, run, agentiflow), the
Messages tab, Assets.

## Decisions

1. **Same architecture as findings.** A list the browser holds in full is
   filtered in the browser. A list that loads page by page (runs, sessions)
   is filtered by the server from `?q=`, with the same grammar, so a query
   finds an old run on any host, never only the rows already loaded. Remote
   hosts' rows are filtered on the coordinator, paging past peer clamps, the
   way `?customer=` already is.
2. **Filters fold into the query; tables don't.** `is:archived` replaces the
   Archived pill (and switches the endpoint behind it); time is `since:` /
   `until:`; the host select is `host:`. A control that swaps the table
   itself (columns and row type) is a view, not a filter, and stays a tab.
3. **Host health lives in the dropdown, plus one alert.** `host:` suggests
   every host with its state (live / offline / unavailable) and row count.
   Under the line, one quiet line appears only when a host is offline or
   unavailable ("sutajio offline: its runs are missing"), so an incomplete
   list is never silent. The `PerHostStrip` goes.
4. **Autoflows are the autoflows session's.** That session rebuilds the
   autoflow run list in the shape of workflow runs and adopts the query line
   in its own branch, extending the workflow-run registry. This work never
   edits `pages/runs/AutoflowRuns.tsx` or the autoflow endpoints.
5. **Netflow opens small.** With no query, the explorer shows the shorter of
   the last hour and the most recent run's span (start to finish), so it
   loads fast; the query (`since:` `until:` `workflow:` `origin:` `org:`
   `host:`) widens or narrows it, and graph clicks write terms into it.

## Data shape

One registry per list: a `RowField<Row>` is the grammar's field spec plus
presentation (label, description, icon) plus how it reads a row and matches:

```ts
interface RowField<Row> extends QueryField {
  get: (row: Row) => string | string[] | number | null | undefined;
  match: 'exact' | 'contains' | 'prefix' | 'ordered' | 'address';
  facet?: boolean; // suggest the values in use, with counts
}
```

`filterRows(rows, q, fields)` and `rowFacets(rows, fields)` in
`web/src/lib/query/rows.ts` evaluate any registry: a term's values OR, terms
AND, `-` negates, bare words search the fields marked `text: true`. The
assets and messages queries move onto it. Registries live in
`web/src/lib/query/lists/<list>.ts` (`WORKFLOW_RUN_FIELDS`, …).

Server side, a leaf crate `rupu-query` holds the grammar made generic over a
field registry (what `rupu-coverage`'s `query_lang` is today for findings,
which becomes one registry on it, its `fields.json` / `cases.json` lockstep
fixtures kept) and a row matcher with the same match modes. Per-list
registries live in `crates/rupu-cp/src/query/<list>.rs`, each in lockstep
with its web twin through a generated fields fixture.

`components/query/ListQueryBar.tsx` is the standard row: the full-width line,
the "N of M" count, and the host alert. Every list page mounts it in the same
place, above its table.

## Plan

- **PR A (web, stacked on #868).** `lib/query/rows.ts`, `ListQueryBar`,
  assets + messages migrated, and the fully loaded lists: Agents, Workflows,
  Skills (its project select becomes `layer:` / `project:`), Projects, Hosts,
  Workers, Coverage, Coverage templates, Customers (status + range fold into
  `is:` / `since:`), Agentiflows. A windowed list filters before its reveal
  slice.
- **PR B (server).** `rupu-query`; findings moved onto it; `?q=` on
  `/api/runs/workflows`, `/api/runs/agents`, `/api/sessions` and the project
  runs / sessions endpoints, coordinator-side for remote hosts; a 400 with the
  parse error's position for a bad query, as findings does.
- **PR C (web).** Workflow runs, agent runs, sessions and the project Runs /
  Sessions tabs on `?q=`; `FilterBar`, `FilterPills`, `SearchInput`'s list use,
  the list `HostSelect` and `PerHostStrip` deleted; host health per
  decision 3.
- **PR D.** Netflow (decision 5) and the Events stream.
