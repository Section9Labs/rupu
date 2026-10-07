# Finding tags — design

- **Status:** approved in conversation, 2026-10-06.
- **Builds on:** finding reports (`2026-09-29-rupu-finding-reports-design.md`), remote findings Plan A (#728, the coverage stream).

## Problem

A finding can't be changed once it's written. It has no tags, labels or triage state. The CP findings routes are read-only, and the web "triage card" is a read-only summary. `rupu findings` has no `list`. An agent can report a finding but can't look up existing ones or mark them.

So nobody can say "these are the SQLi findings that still need a PoC" and have that stick. An agent can't be pointed at "everything tagged `needs-poc`". An operator can't filter the findings table by their own categories.

## Goal

Agents and operators can attach any number of free-form tags to findings and remove them later. This works one finding at a time or in bulk, from:

- an agent tool,
- an MCP action,
- the CLI,
- the CP.

Findings can then be queried and filtered by tag on every one of those surfaces.

## Decisions

1. **Who tags:** agents and operators, both when a finding is reported and afterwards. Tags can be added and removed, there can be many per finding, and bulk changes are supported.
2. **How agents work on tagged items:** for now, through agent tools — `query_findings` and `tag_findings`, with MCP mirrors. A workflow `for_each` that fans out over a tag query is a follow-up plan, not part of this spec.
3. **Vocabulary:** free-form and normalized, with no registry. A "tags in use" listing (with counts) supports autocomplete and lets agents reuse existing tags. `namespace:value` (e.g. `status:needs-poc`, `class:sqli`) is a convention only.
4. **Identity:** tags belong to the finding id (`fnd_<ULID>`). A vulnerability reported again in a later run is a new finding with no tags. Carrying tags forward needs cross-run finding identity (fingerprinting), which is out of scope.
5. **Storage:** one append-only tag-event log per workspace (`.rupu/coverage/finding_tags.jsonl`), folded on read. Finding lines are never rewritten for tags.

   This was refined while planning. A per-target log can't work: a run stream labels each line with the writing agent's own scope, and the coordinator turns that scope back into a target id. So an event tagging a finding in *another* target would be filed under the wrong target and never applied. Finding ids are globally unique ULIDs, so a workspace-wide log needs no per-target routing.

## Tag syntax

`rupu_coverage::Tag` is a newtype, and constructing one is the only validation path.

- Input is trimmed and lowercased. `Needs-POC` becomes `needs-poc`.
- After that, a tag must be 1–64 characters from `[a-z0-9._:/-]`, and its first character must be `[a-z0-9]`. Anything else is rejected with an error naming the tag. It is never rewritten into something valid (`needs poc` fails; it doesn't become `needs-poc` or `needspoc`).
- A finding can have at most **32** effective tags. A change that would take any finding over the limit is rejected.

## Data model

### Tags given when a finding is reported

`FindingRecord` and its read mirror `FindingRecordWire` (`ledger/events.rs`) get the same new field:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub tags: Vec<Tag>,
```

- Old lines read as having no tags. Old readers ignore the field.
- `tags` lives on the record, not on `FindingReport`. That struct is `deny_unknown_fields` and is described by the draft-07 report schema, and both stay unchanged.
- When a record is written, its tags are deduplicated and sorted.

`ReportFindingInput` gets an optional `tags: Vec<String>`. `report_finding()` validates each entry into a `Tag`, and an invalid tag fails the call, as an invalid severity does. Both the summary and full input schemas of the `report_finding` built-in, and the MCP `findings.record` tool, advertise `tags`.

### The tag-event log

The log lives at `<workspace>/.rupu/coverage/finding_tags.jsonl`, one per workspace and shared by all of its targets. `discover_targets` lists directories only, so the file is never mistaken for a target. `TagLog` (workspace, log path, lock path, optional `RunStream`) addresses it, and `CoveragePaths::tag_log()` returns the workspace's log carrying that path's run stream. The file holds one `TagEvent` per line:

```json
{"id":"tge_01J…","finding_id":"fnd_01J…","op":"add","tag":"needs-poc",
 "by":{"kind":"agent","run_id":"…","model":"…","surface":"workflow","codename":"…","agent":"…"},
 "at":"2026-10-06T12:00:00Z"}
```

- `op` is `add` or `remove`.
- `by` is a `TagActor`, an internally tagged enum (`#[serde(tag = "kind")]`) with two variants:
  - `agent`: the existing `Attribution` fields, flattened.
  - `operator`: `{ user: String, via: "cli" | "cp" }`.
  - For the CLI, `user` is `$USER`. For the CP, it is the CP's operator identity, or `$USER` of the `cp serve` process when none is configured.
- The log is append-only and nothing ever rewrites it. Readers must accept a newer writer's lines: `TagEvent` doesn't use `deny_unknown_fields`, and a line with an unknown `op` or `kind`, or one that can't be parsed, is skipped when folding (and logged once). It is never an error.

### Fold rule

A finding's effective tags start from its record's `tags`. The events for that finding are then applied **in file order**: `add` inserts the tag and `remove` deletes it. File order is decided under the lock, so the result never depends on timestamps or clock skew. The last event for a given tag wins.

An event whose `finding_id` isn't in the ledger, for example one ingested before its finding, stays in the log and is ignored when folding. Once the finding arrives, the event applies.

## Write path

A new module, `rupu_coverage::ledger::tags`, provides `apply` — the **only** function that writes tag events:

```rust
pub struct TagChange { pub finding_ids: Vec<String>, pub add: Vec<Tag>, pub remove: Vec<Tag> }
pub fn apply(paths: &CoveragePaths, change: &TagChange, by: TagActor) -> Result<Vec<TagOutcome>, TagError>;
pub struct TagOutcome { pub finding_id: String, pub before: Vec<Tag>, pub after: Vec<Tag> }
```

`apply` does this in order:

1. It takes the tag log's own sidecar lock, `finding_tags.jsonl.lock`. This uses the same lock helper (and the same unlockable-filesystem fallback) as `findings.jsonl.lock`, so tag writers are serialized with each other.
   - Findings are read *without* their lock. That's safe: the findings ledger is append-only and `attach_reports` swaps it by atomic rename, so a reader always sees a whole file, and a finding id never disappears.
   - A workspace with no `.rupu/coverage/` directory has no findings. Every id is unknown there, and nothing (not even the lock file) is created.
2. It reads every target's declared findings in the workspace plus `finding_tags.jsonl`, and folds them.
3. It validates the whole batch before writing anything. The batch is rejected if:
   - any `finding_id` is unknown (`TagError::UnknownFindings(ids)`);
   - a tag appears in both `add` and `remove`;
   - `add` and `remove` are both empty;
   - any finding would end up with more than 32 tags.
4. It works out the events that actually change something: a removal of a tag that's present, then an addition of a tag that's absent. These are ordered by finding id, then tag, removals first. **Requests that change nothing write no events**, so running "tag every SQLi `class:sqli`" again leaves the log untouched.
5. It appends every event in one `write_all`, then fsyncs.
6. If `paths.run_stream` is set, it mirrors each event to the run's `coverage.jsonl` as a new `tags` line kind. This uses `stream_json`, the same path findings take.
7. It returns before and after tags for each requested finding, including findings that didn't change.

If locking is unsupported, it falls back the same way `append_record` does today. Finding lines are untouched.

**Batches that span workspaces.** When the CP or CLI change findings spread over several registered workspaces, they first find which workspace holds each id, then call `apply` once per workspace. Each workspace's batch is atomic; **the whole request is not**. Results are reported per workspace, and any id that no workspace holds is listed as `unknown`.

That lookup is `rupu_cp::api::findings::tag_findings_across(global_dir, ids, add, remove, by)`. It sits next to `collect_all_findings` / `finding_ledgers`, which already own the registered-workspace walk; `rupu findings export|import` call it from there the same way. An id found in two distinct workspaces (a checkout registered twice under different paths) is tagged in both.

## Read path

- `read_findings` (`ledger/views.rs`) returns records with **folded** `tags` (via `read_declared_findings` + the workspace's log). `read_workspace_findings(workspace)` returns every target's findings in a workspace, folded once. Every existing consumer therefore sees effective tags with no change of its own: the CP `FindingOut` DTO (which flattens `FindingRecord`), export, import and the new CLI list.
- `tag_history(paths, finding_id) -> Vec<TagEvent>` returns one finding's events in file order.
- `tags_in_use(ledgers) -> Vec<(Tag, usize)>` returns each tag with the number of findings that currently carry it, sorted by count descending, then by tag.
- `ledger::query` is the one filter, shared by the agent tool, MCP, CLI and CP. It has two entry points: `select(items, record_of, &FindingQuery)` returns every match unpaged (CLI, CP), and `query(records, &FindingQuery) -> Page` pages the matches (agent tool, MCP). The query fields:
  - `tags` + `tag_mode` (`all` by default, or `any`);
  - `untagged`, which can't be combined with `tags`;
  - `min_severity`, `concern_id`, `file_prefix`;
  - run scoping, `run_ids`, is **CLI/CP only**: `--run` resolves the run plus its sub-runs into a set of run ids, which is `serde(skip)` on `FindingQuery`. The agent tool and MCP input omit it (an agent or workflow step can't filter by run); their rows carry `run_id` instead;
  - `limit` and `cursor`.
  - `limit` and `cursor` apply to `query` only.
  - Results are ordered by severity, then `declared_at` descending, then id — the same order the CP list already uses.
  - The cursor is the last row's `(severity, declared_at, id)` key. Findings appended while someone is paging therefore never shift or repeat rows. Results are paged, never silently cut short.

## Remote units

- `coverage.jsonl` gets a `tags` line kind, which carries one `TagEvent`.
- `ledger::ingest::ingest_unit_stream` appends `tags` lines to the coordinator workspace's `finding_tags.jsonl`, whatever their `scope_name`. It takes the tag log's lock and dedupes by event `id`, seeding the seen-set from the ids already on disk, the way findings are seeded today.
- An *older* coordinator can't parse a `tags` line. It counts the line as malformed, so it doesn't strip the unit's `.rupu/coverage/` from the workspace-sync delta, and the delta carries the unit's tag log over. That's an acceptable degrade, and needs no version gate.
- No finding lookup happens at ingest, because folding already ignores orphan events.
- This needs no transport changes. Every connector already carries the coverage stream byte for byte.
- `strip_delta_coverage` / `Delta::without_coverage` already strip all of `.rupu/coverage/`, which covers `finding_tags.jsonl`.

## Surfaces

### Agent tools (`rupu-agent/src/coverage_tools.rs`)

There are two new built-ins: `query_findings` and `tag_findings`. Like `report_finding`, they are **explicit grants**: they are registered only when the agent's `tools:` lists them. A `coverage:` block does *not* register them automatically. The registration sits next to the `report_finding` opt-in in `runner.rs`.

- **`query_findings`**
  - Input is a `FindingQuery` without run scoping (see above). `limit` defaults to 50 and is capped at 500; `cursor` is optional.
  - It returns `{rows, next_cursor, tags_in_use}`, so a single call gives the agent both the findings and the existing vocabulary.
  - Each row is slim: id, title (or summary), severity, location (file and line range, or target ref), concern_id, tags, declared_at.
  - Scope is every target ledger under the agent's own workspace `.rupu/coverage/`. An agent can't see or tag findings in other workspaces.
- **`tag_findings`**
  - Input is `{finding_ids, add, remove}`. The call groups the ids by ledger within the agent's workspace, calls `apply` for each, and returns the per-finding outcomes plus any unknown ids.
  - It is attributed to the agent with the same `Attribution` that `report_finding` builds (run, model, surface, codename, agent).
  - Its writes go to the agent's run stream, so remote-placed units' tagging reaches the coordinator.
  - **Readonly mode:** `tag_findings` is allowed under `readonly`, as `report_finding` is. `ReadonlyDecider` denies only `bash`, `write_file` and `edit_file`, which change the workspace. A ledger annotation doesn't change the workspace, and the explicit `tools:` grant is the gate.

### MCP (`rupu-mcp/src/tools/findings.rs`)

- `findings.query` and `findings.tag` mirror the two built-ins for workflow `action:` steps and `rupu mcp serve`.
- `findings.record` gains `tags`.
- `findings.tag` is attributed the way `findings.record` is today (crew only, no agent).

### CLI (`rupu-cli/src/cmd/findings.rs`, thin)

- `rupu findings list [--project P] [--run R] [--tag T]… [--any-tag] [--untagged] [--severity MIN] [--limit N] [--ids-only]`
  - Output is a table; the global `--format json` gives JSON rows.
  - `--limit` truncates, and says so on stderr (`showing N of M`).
  - `--ids-only` prints one id per line.
  - Ledger discovery uses the same `--project` / `--run` resolution as `findings export`.
- `rupu findings tag <ID>… [--add T]… [--remove T]…`
  - An `<ID>` of `-` reads ids from stdin, one per line, ignoring blank lines.
  - It prints `id: before → after` for each finding, and exits non-zero if any id is unknown or any workspace failed. Workspaces that succeeded keep their changes; this is stated in the output.
  - Example: `rupu findings list --tag class:sqli --severity high --ids-only | rupu findings tag - --add needs-poc`.
- `rupu findings tags [--project P] [--run R]` lists tags in use, with counts.

### CP (`rupu-cp/src/api/findings.rs`)

- `GET /api/findings` gains `tag=` (repeatable), `tag_mode=all|any` and `untagged=true`. Filtering happens on the server, through `ledger::query`'s filter.
- `GET /api/findings/tags?ws_id=&run_id=` returns `[{tag, count}]`.
- `POST /api/findings/tags`
  - Body: `{finding_ids, add, remove}`.
  - Response: `tag_findings_across`'s result, `{workspaces: [{ws_id, outcomes: [TagOutcome]} | {ws_id, error}], unknown: [id]}`.
  - Status codes: 200 when at least one id was found; 404 when none were; 400 for invalid tags, add and remove of the same tag, an empty change, or a change over the cap.
  - It is a plain local append under the lock, with no runtime or worker involved. The CP already writes config and workflow YAML, and this follows the same pattern.
- `GET /api/findings/:id` adds `tag_history: [TagEvent]`.
- Findings shown in the CP are the coordinator's local ledgers, including ingested remote units. Tagging applies to exactly those findings.

### Web (`rupu-cp/web`)

- **Findings table:** tag chips on each row. A tag filter (multi-select with autocomplete from `/api/findings/tags`, plus an all/any toggle and "untagged") sits next to the existing severity, profile, owner and CWE filters.
- **Bulk editing:** row checkboxes and a bulk action bar ("Tag…" / "Untag…", with autocomplete) that posts once for the whole selection.
- **Report page (`/findings/:id`):** a tag editor (removable chips plus an add input with autocomplete) and a collapsible tag history showing who, when and add/remove.
- **Visual check first:** a mock of the table chips, the bulk bar and the editor goes to matt *before* the UI is built.

## Query language (Plans 2–3, decided 2026-10-06)

Findings are filtered everywhere with ONE single-line query language, modeled on Ghost's spotlight query bar (`/Users/matt/Security/Ghost`, `origin/main`: `crates/ghost-cp/web/src/graph3d/{SpotlightBar.tsx,query.ts,fuzzy.ts,spotlightResults.ts}` for the UI, `coordination/QueueFilter.tsx` for the tokenizer with explicit errors). It replaces the Plan 1 structured filters (`tags`/`tag_mode`/`untagged`/`min_severity`/`concern_id`/`file_prefix` and the CLI's `--tag`/`--any-tag`/`--untagged`/`--severity`/`--project`/`--run`). Those shipped in no beta, so nothing external depends on them.

**Grammar.**
- A query is whitespace-separated tokens, and every token must match (AND).
- `key:value` filters on a field.
  - `key:a,b` matches any of the values (OR within one token).
  - `-key:value` negates the token.
  - Values may be `"quoted"` or `'quoted'`, with `\` escapes.
- `severity` alone also takes `>=`, `>`, `<=` and `<`.
- A bare word, or a quoted phrase, is free text. It matches title, summary, id and file path, case-insensitively, and `-word` negates it.
- A token that looks like `word:`/`word>=`… names a key, so an unknown key is an error, never text. Quote a free-text value that contains `:`.
- These are errors, and the query does not run:
  - an unknown key
  - an empty value
  - an invalid value for an enum, tag or CWE
  - a comparison operator on a non-severity key, or a comparison with several values
  - an unclosed quote, or text glued to a closing quote

**Fields.**

| Key | Values and matching |
|---|---|
| `severity` (alias `sev`) | info/low/medium/high/critical; supports comparisons |
| `tag` | normalized like `Tag`. Repeated tokens AND, comma OR. Tags may contain `:`; only the first `:` after the key splits |
| `has` | tags / report / poc / cwe |
| `project` | workspace name or id |
| `cwe` | `79` or `CWE-79`, compared by number with `report.cwe`, the CWE a `concern_id` names and MITRE reference URLs — the same rule as the CWE column and export (`rupu_coverage::report::cwe::finding_cwes`) |
| `owner`, `product` | report ownership, case-insensitive |
| `verified` | unverified/confirmed/disputed/inconclusive. A finding with no verification counts as unverified |
| `profile` | full/summary |
| `scope` | line/file/repo/host/endpoint/resource |
| `concern` | concern id |
| `agent` | agent name or codename |
| `workflow` | workflow name |
| `file` | path prefix |
| `run` | run id. The CLI and CP expand it to the run plus its sub-runs; elsewhere it is an exact match |
| `id` | finding id |

- `project`, `workflow` and `run` expansion exist only where provenance is known (CP, CLI). The agent tool and MCP answer a query that uses them with an "unavailable here" error.

**One grammar, two parsers, one evaluator.**
- **Rust is authoritative.** `rupu_coverage::ledger::query_lang` parses, and `ledger::finding_filter` evaluates, shared by the CP API (`GET /api/findings?q=`), the CLI (`rupu findings list|tags [QUERY…]`), the agent tool and MCP (`{"q": …}`).
- **TypeScript only parses.** The web's copy (`web/src/lib/findingQuery/`) parses for chips, suggestions and inline errors. The page always asks the server to evaluate.
- **Lockstep.** The two parsers are held together by shared fixtures in `crates/rupu-coverage/tests/fixtures/finding_query/`, run by both `cargo test` and vitest:
  - `cases.json`: query → canonical AST, or `{token, code}` error
  - `fields.json`: keys, aliases, kinds, enum values

**API.**
- `GET /api/findings` gains `q`. An invalid `q` gives a 400 with `{error, token, code, start, end}` (char offsets).
- The response gains:
  - `facets` (per key, `[{value, count}]` over the scope-filtered but not q-filtered set). These feed autocomplete and the severity tiles.
  - `tags_unavailable: [ws_id]`, the workspaces whose tag log could not be read. This was decision A: findings are still served with their declared tags, flagged, and the page shows a banner.

**Web.**
- A generic `QueryBar` (chips, a fuzzy spotlight dropdown with lucide icons and per-value colors, keyboard handling, inline errors) driven by a per-view field registry. Plan 2 wires it into the global Findings page only.
- The query lives in the URL (`?q=`).
- Severity tiles toggle `severity:<x>`.
- The profile/owner/CWE controls are removed.
- A read-only Tags column shows each finding's tags.
- `/` focuses the bar. ⌘K stays the global command palette.

**Plans.**
- **Plan 2:** the query language everywhere, plus the query bar on Findings, the Tags column and the `tags_unavailable` banner.
- **Plan 3:** tag editing in the CP:
  - `POST /api/findings/tags`, `GET /api/findings/tags` and `tag_history`
  - row selection and the bulk tag/untag bar
  - the finding-page tag editor and history
  - editing disabled for `tags_unavailable` workspaces

## Testing

Each crate keeps one integration-test binary, with modules under `tests/it/`. All fixtures are invented; none are derived from an assessment.

- **`rupu-coverage`:**
  - `Tag` normalization and rejection, including the cap.
  - Fold: declared tags plus ordered events, last event wins, orphan events ignored, then applied once the finding arrives.
  - `apply`:
    - A request that changes nothing writes nothing.
    - An unknown id rejects the batch with the file unchanged byte for byte.
    - add and remove of the same tag is rejected.
    - The run-stream mirror receives the events.
  - Concurrency: two threads calling `apply` while a third calls `report_finding`, ending with no lost events and every line valid.
  - Back-compat: old `findings.jsonl` lines read with no tags; `FindingRecord` and `FindingRecordWire` stay in lockstep; unknown `TagEvent` fields, ops and actor kinds are tolerated when folding.
  - `query`: tag all/any/untagged, a stable cursor while findings are appended, ordering.
  - `ingest_unit_stream` with `tags` lines: deduped by event id across repeated ingests, orphans tolerated.
- **`rupu-agent` / `rupu-mcp`:**
  - Tool input schemas.
  - `report_finding` with `tags`.
  - `tag_findings` → `apply` round trip, with the agent's attribution.
  - The tools are registered only when listed in `tools:`.
  - The query tool pages with its cursor.
- **`rupu-cp`** (Plan 2):
  - Handler tests: the tag filter modes, tags-in-use counts, a bulk `POST` spanning two workspaces plus one unknown id, the 400 and 404 cases, and `tag_history` on the detail endpoint.
  - Vitest for the filter state and the bulk bar's request shape.
- **`rupu-cli`:** `list` / `tag` / `tags` against a fixture workspace, including stdin `-` and the exit code for unknown ids. These tests touch no env or cwd, so they belong in the `it` binary.

## Docs

- A new "Tagging findings" section in `docs/coverage.md`, covering syntax, the fold, the CLI, the CP and the agent tools.
- The `rupu-coverage` entry in CLAUDE.md gets one line: `finding_tags.jsonl` is folded on read, and `ledger::tags::apply` is the only writer.

## Plans

1. **Plan 1 (complete): core, CLI, agent and MCP tools** — `docs/superpowers/plans/2026-10-06-rupu-finding-tags-plan-1-core-cli-agent.md`.
   - `Tag`, the record field, the tag log, `apply`, fold, query, tags-in-use, the stream line kind, ingest.
   - `rupu_cp::api::findings::tag_findings_across`, a library function the CLI uses; it gets no HTTP route until Plan 2.
   - The `report_finding` / `findings.record` `tags` input.
   - `query_findings` / `tag_findings` and their MCP mirrors.
   - `rupu findings list|tag|tags`.
   - Docs.
2. **Plan 2 (complete): the query language** — `docs/superpowers/plans/2026-10-06-rupu-finding-tags-plan-2-query-language.md`. Rust parser + evaluator, the TS twin and fixtures, `GET /api/findings?q=` with `facets` / `tags_unavailable`, the CLI/agent/MCP `q` surfaces, the web query bar on the Findings page, docs.
3. **Plan 3 (remaining; originally "Plan 2: CP API and web UI", split by the Query language section above): bulk tagging and the report-page tag editor, with the tag-write endpoints.** The three endpoint changes, plus the bulk bar and report-page editor, with the mock shown first.

## Out of scope (follow-ups)

- Workflow `for_each` fan-out over a tag query. This is the next plan once tags exist.
- Carrying tags forward to a re-reported vulnerability via finding fingerprinting.
- Tag descriptions or colours in config. These can be added later without changing the data model.
- Tagging findings that exist only in a remote host's own ledgers and were never ingested into the coordinator.
- The macOS app (deprecated).
