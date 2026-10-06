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
5. **Storage:** an append-only tag-event log next to each ledger, folded on read. Finding lines are never rewritten for tags.

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

`CoveragePaths` gets `finding_tags: root.join("finding_tags.jsonl")`, and the file holds one `TagEvent` per line:

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

1. It takes `findings.jsonl.lock`, the same sidecar lock every findings append already takes in `append_record`. Tag writes and finding appends are therefore serialized with each other, and with `attach_reports`.
2. It reads `findings.jsonl` and `finding_tags.jsonl` and folds them.
3. It validates the whole batch before writing anything. The batch is rejected if:
   - any `finding_id` is unknown (`TagError::UnknownFindings(ids)`);
   - a tag appears in both `add` and `remove`;
   - `add` and `remove` are both empty;
   - any finding would end up with more than 32 tags.
4. It works out the events that actually change something: a removal of a tag that's present, then an addition of a tag that's absent. These are ordered by finding id, then tag, removals first. **Requests that change nothing write no events**, so running "tag every SQLi `class:sqli`" again leaves the log untouched.
5. It appends every event in one `write_all`, then fsyncs.
6. If `paths.run_stream` is set, it mirrors each event to the run's `coverage.jsonl` as a new `tags` line kind. This uses `stream_json`, the same path findings take.
7. It returns before and after tags for each requested finding, including findings that didn't change.

If locking is unsupported, it falls back the same way `append_record` does today.

**Batches that span ledgers.** Findings live in per-target ledgers (`<workspace>/.rupu/coverage/<target_id>/`) across workspaces. When the CP or CLI change findings spread over several ledgers, they first find which ledger holds each id, then call `apply` once per ledger. Each ledger's batch is atomic; **the whole request is not**. Results are reported per ledger, and any id that no ledger holds is listed as `unknown`. The id-to-ledger lookup is one shared function, `ledger::locate_findings(roots, ids)`, used by both the CLI and the CP.

## Read path

- `read_findings` (`ledger/views.rs`) returns records with **folded** `tags`. Every existing consumer therefore sees effective tags with no change of its own: the CP `FindingOut` DTO (which flattens `FindingRecord`), export, import and the new CLI list.
- `tag_history(paths, finding_id) -> Vec<TagEvent>` returns one finding's events in file order.
- `tags_in_use(ledgers) -> Vec<(Tag, usize)>` returns each tag with the number of findings that currently carry it, sorted by count descending, then by tag.
- `query(ledgers, &FindingQuery) -> Page<FindingRow>` is the one query implementation shared by the agent tool, MCP, CLI and CP:
  - `tags` + `tag_mode` (`all` by default, or `any`);
  - `untagged`, which can't be combined with `tags`;
  - `min_severity`, `concern_id`, `file_prefix`, `run_id`;
  - `limit` and `cursor`.
  - Results are ordered by severity, then `declared_at` descending, then id — the same order the CP list already uses.
  - The cursor is the last row's `(severity, declared_at, id)` key. Findings appended while someone is paging therefore never shift or repeat rows. Results are paged, never silently cut short.

## Remote units

- `coverage.jsonl` gets a `tags` line kind, which carries one `TagEvent`.
- `ledger::ingest::ingest_unit_stream` appends `tags` lines to the coordinator's `finding_tags.jsonl` for the matching scope. It takes the coordinator's findings lock and dedupes by event `id`, seeding the seen-set from the ids already on disk, the way findings are seeded today.
- No finding lookup happens at ingest, because folding already ignores orphan events.
- This needs no transport changes. Every connector already carries the coverage stream byte for byte.
- `strip_delta_coverage` / `Delta::without_coverage` already strip all of `.rupu/coverage/`, which covers `finding_tags.jsonl`.

## Surfaces

### Agent tools (`rupu-agent/src/coverage_tools.rs`)

There are two new built-ins: `query_findings` and `tag_findings`. Like `report_finding`, they are **explicit grants**: they are registered only when the agent's `tools:` lists them. A `coverage:` block does *not* register them automatically. The registration sits next to the `report_finding` opt-in in `runner.rs`.

- **`query_findings`**
  - Input is a `FindingQuery`. `limit` defaults to 50 and is capped at 500; `cursor` is optional.
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
  - `--ids-only` prints one id per line.
  - Ledger discovery uses the same `--project` / `--run` resolution as `findings export`.
- `rupu findings tag <ID>… [--add T]… [--remove T]…`
  - An `<ID>` of `-` reads ids from stdin, one per line, ignoring blank lines.
  - It prints `id: before → after` for each finding, and exits non-zero if any id is unknown. Ledgers that succeeded keep their changes; this is stated in the output.
  - Example: `rupu findings list --tag class:sqli --severity high --ids-only | rupu findings tag - --add needs-poc`.
- `rupu findings tags [--project P] [--run R]` lists tags in use, with counts.

### CP (`rupu-cp/src/api/findings.rs`)

- `GET /api/findings` gains `tag=` (repeatable), `tag_mode=all|any` and `untagged=true`. Filtering happens on the server, through `ledger::query`'s filter.
- `GET /api/findings/tags?ws_id=&run_id=` returns `[{tag, count}]`.
- `POST /api/findings/tags`
  - Body: `{finding_ids, add, remove}`.
  - Response: `{ledgers: [{ws_id, target_id, outcomes: [TagOutcome]}], unknown: [id]}`.
  - Status codes: 200 when at least one id was found; 404 when none were; 400 for invalid tags, add and remove of the same tag, an empty change, or a change over the cap.
  - It is a plain local append under the lock, with no runtime or worker involved. The CP already writes config and workflow YAML, and this follows the same pattern.
- `GET /api/findings/:id` adds `tag_history: [TagEvent]`.
- Findings shown in the CP are the coordinator's local ledgers, including ingested remote units. Tagging applies to exactly those findings.

### Web (`rupu-cp/web`)

- **Findings table:** tag chips on each row. A tag filter (multi-select with autocomplete from `/api/findings/tags`, plus an all/any toggle and "untagged") sits next to the existing severity, profile, owner and CWE filters.
- **Bulk editing:** row checkboxes and a bulk action bar ("Tag…" / "Untag…", with autocomplete) that posts once for the whole selection.
- **Report page (`/findings/:id`):** a tag editor (removable chips plus an add input with autocomplete) and a collapsible tag history showing who, when and add/remove.
- **Visual check first:** a mock of the table chips, the bulk bar and the editor goes to matt *before* the UI is built.

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
- **`rupu-cp`:**
  - Handler tests: the tag filter modes, tags-in-use counts, a bulk `POST` spanning two ledgers plus one unknown id, the 400 and 404 cases, and `tag_history` on the detail endpoint.
  - Vitest for the filter state and the bulk bar's request shape.
- **`rupu-cli`:** `list` / `tag` / `tags` against a fixture workspace, including stdin `-` and the exit code for unknown ids. These tests touch no env or cwd, so they belong in the `it` binary.

## Docs

- A new "Tagging findings" section in `docs/coverage.md`, covering syntax, the fold, the CLI, the CP and the agent tools.
- The `rupu-coverage` entry in CLAUDE.md gets one line: `finding_tags.jsonl` is folded on read, and `ledger::tags::apply` is the only writer.

## Plans

1. **Plan 1: core, CLI, agent and MCP tools.**
   - `Tag`, the record field, the tag log, `apply`, fold, query, tags-in-use, the stream line kind, ingest.
   - The `report_finding` / `findings.record` `tags` input.
   - `query_findings` / `tag_findings` and their MCP mirrors.
   - `rupu findings list|tag|tags`.
   - Docs.
2. **Plan 2: CP API and web UI.** The three endpoint changes, plus the table, filter, bulk bar and report-page editor, with the mock shown first.

## Out of scope (follow-ups)

- Workflow `for_each` fan-out over a tag query. This is the next plan once tags exist.
- Carrying tags forward to a re-reported vulnerability via finding fingerprinting.
- Tag descriptions or colours in config. These can be added later without changing the data model.
- Tagging findings that exist only in a remote host's own ledgers and were never ingested into the coordinator.
- The macOS app (deprecated).
