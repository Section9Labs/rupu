# rupu CLI output polish (design)

Date: 2026-07-30

## Goal

Bring the CLI's list and report output up to the visual standard of the web
CP, without adding a new interactive surface and without breaking any
scripting contract.

The CLI and the CP render the same data from the same stores. The gap is
rendering policy, not architecture — and most of the vocabulary the CP uses
already exists in `crates/rupu-cli/src/output/`, unused by the tables.

## Scope

In scope:

- All 55 `new_table()` call sites across 16 modules.
- Identifier compaction in table cells, paired with fragment resolution.
- Adoption of the existing `palette::Status` glyph vocabulary in tables.
- Relative timestamps in tables.
- Empty-column suppression and summary lines for entity lists.
- Enrichment of `workflow list`.
- Snapshot test coverage over table renderers (currently zero).

Out of scope:

- Any interactive/full-screen TUI (`rupu top` or similar).
- Relative session selectors (`--last`, resume-by-agent-name). Considered
  and deliberately deferred; they need their own rules for what "last"
  means per agent, per workspace, and across archived scope.
- Changes to `--output json` shape. Frozen by contract, see below.

## Background

Observed from `rupu 0.71.0` on real data:

1. `tables.rs:34` loads `presets::UTF8_FULL`. The docstring at line 30
   claims "no separator lines inside the body", but UTF8_FULL draws
   `├╌╌╌┼╌╌╌┤` between every row. Twenty sessions render as 41 lines.
2. Timestamps print as raw ISO with microseconds: 32 columns to say
   "2 weeks ago".
3. Full 30-character ULIDs occupy every ID cell.
4. Dead columns render regardless of content. `session list` showed
   `TARGET` and `RUN` as `—` for every row; `workflow runs` showed
   `EXPIRES` blank for every row.
5. `workflow list` emits only NAME and SCOPE.
6. No aggregate line anywhere, though the data is present.

Two pieces of prior art already in the tree and under-used:

- `compact_session_run_id` (session.rs:5897) renders
  `run_01KRJDKSBE7X4J49094149WFJS` as `run_01KRJDKS…WFJS`, keeping both
  ends. Correct for ULIDs specifically: the leading 10 characters are a
  millisecond timestamp and therefore collide for records created close
  together (three sessions in the live listing share `ses_01KWA`), while
  the trailing 16 are random and are what actually disambiguate.
- `palette::Status` (palette.rs:233) already implements the Slice C glyph
  sketch: `○` waiting, `●` active, `◐` working, `✓` complete, `✗` failed,
  `!` soft-failed, `⏸` awaiting, `↺` retrying, `⊘` skipped. It is used by
  the session live view and by nothing in `tables.rs`.

## Architecture

Five units, each usable and testable in isolation.

### 1. `crates/rupu-cli/src/output/ids.rs` (new)

Owns both halves of the identifier problem. The governing rule:

> Never display an identifier the CLI will not accept back.

```rust
/// Compact form: `<prefix>_<8 ULID chars>…<4 tail chars>`.
/// Identifiers shorter than the compact form are returned unchanged.
pub fn compact_id(id: &str) -> String;

pub enum Resolution<'a> {
    Unique(&'a str),
    NotFound,
    Ambiguous(Vec<&'a str>),
}

/// Match `fragment` against `candidates`.
pub fn resolve<'a>(candidates: &'a [String], fragment: &str) -> Resolution<'a>;
```

`resolve` normalizes a fragment by splitting on `…`. Given `head…tail` it
requires `id.starts_with(head) && id.ends_with(tail)`. Given a fragment
with no ellipsis it accepts either a prefix match or a suffix match. Every
*partial* form — bare prefix, bare suffix, and both halves of an ellipsis
fragment combined — must carry at least `MIN_FRAGMENT_LEN` (4) characters;
only an exact full-id match is exempt. This bound exists because `resolve`
backs destructive commands (`workflow delete-run`, `cancel`, `reject`,
`archive-run`): without it, a two-character typo such as `0S` — or the
same two characters spelled as an ellipsis fragment, `…0S` — could match
and delete an unrelated run. This makes all four of these resolve to the
same record:

```
ses_01KWA7HTYEDX0ACG93ZW26FG3M   full
ses_01KWA7HT…FG3M                the string the table printed
26FG3M                           bare suffix
ses_01KWA7HTY                    unambiguous prefix
```

`Ambiguous` is always a hard error. It prints every candidate at full
length with disambiguating context and exits non-zero. It never picks.

```
$ rupu session send ses_01KWA "..."
error: ambiguous session id — 3 sessions match `ses_01KWA`
  ses_01KWA7HTYEDX0ACG93ZW26FG3M  oracle-assessor  2w ago
  ses_01KWAA6X3KH6BNCPXF5Q63FJM0  oracle-assessor  2w ago
  ses_01KWA8RA3NE9J88X6DHSH76AC8  oracle-assessor  4w ago
```

`compact_session_run_id` is deleted and its call sites moved to
`compact_id`; its existing test moves with it.

#### Scope traversal obligation

`read_session` (session.rs:7256) currently does an exact filesystem
lookup — `session_record_path(...)` then `is_file()` — looping
`[Active, Archived]`. Resolution replaces the exact lookup but must
preserve the two-scope search, and must report explicitly when a
fragment matches one record in Active and a different one in Archived
rather than silently preferring Active.

The run store gets the equivalent treatment.

### 2. `crates/rupu-cli/src/output/tables.rs` — two profiles

`new_table()` changes preset and becomes the shared base for both
profiles:

```rust
pub fn new_table() -> Table {
    let mut t = Table::new();
    t.load_preset(presets::NOTHING);
    t.set_style(TableComponent::HeaderLines, '─');
    // Required for a segmented rule. Without this the header line
    // renders as one continuous run across the full table width,
    // because `NOTHING` leaves the intersection glyph as a space
    // that `HeaderLines` then overdraws. Verified against
    // comfy-table 7.2.2.
    t.set_style(TableComponent::MiddleHeaderIntersections, ' ');
    t.set_content_arrangement(ContentArrangement::Dynamic);
    t
}
```

Verified rendering (comfy-table 7.2.2, colour disabled):

```
 SESSION             AGENT             STATUS     UPDATED
─────────────────── ───────────────── ────────── ─────────
 ses_01KWA7HT…FG3M   oracle-assessor   ✗ failed   2w ago
 ses_01KWAA6X…FJM0   oracle-assessor   ○ idle     2w ago
```

Note that rule segments span each column's padding, so they run one
character wider than the header text on each side, and `NOTHING`
renders the left border as a space, which supplies the leading indent.
Both are cosmetic and acceptable; the alternative is hand-rolling the
header line, which is not worth abandoning comfy-table's width solver
for.

The stale "no separator lines" docstring is corrected rather than
carried forward.

Two profiles sit on top, because entity lists and numeric reports are
scanned differently:

- **`EntityTable`** — session, run, workflow, cron, repos, issues, agent,
  models. Rows the operator acts on. Gets empty-column suppression, the
  summary line, compact IDs, relative time.
- **`ReportTable`** — coverage (18 tables), usage, auth. Dense grids the
  operator reads. Keeps every column: a zero in a coverage grid is data,
  not absence. Gets the header rule, alignment, and colour, but not
  suppression or summaries.

Shared by both: two-space gutters, left alignment by default, right
alignment for numeric columns.

Rendered result:

```
  8 sessions · 2 failed · 5 idle · 1 stopped

  SESSION            AGENT            STATUS    UPDATED
  ─────────────────  ───────────────  ────────  ───────
  ses_01KWA7HT…FG3M  oracle-assessor  ✗ failed  2w ago
  ses_01KWAA6X…FJM0  oracle-assessor  ○ idle    2w ago
  ses_01KV94XJ…YS9S  oracle-assessor  ✗ failed  4w ago
```

### 3. Single status mapping

`status_color` (tables.rs:42) matches status strings straight to palette
colours, duplicating knowledge that `palette::Status` already holds. It
is replaced by:

```rust
/// Lifecycle states only. Returns `None` for classification values.
pub fn status_of(status: &str) -> Option<Status>;

/// The glyph for a lifecycle status.
pub fn status_glyph(status: &str) -> Option<char>;
```

`status_of` drives the **glyph only**. Colour keeps its existing
explicit string mapping, and the two are deliberately not merged.

This corrects an earlier version of this section, which said colour and
glyph should both derive from the returned `Status`. They must not.
`Status::Waiting.color()` resolves to `palette.skipped` =
`rgb(203, 213, 225)`, while `status_color` currently renders `pending` /
`eligible` / `released` as `palette.dim` = `rgb(100, 116, 139)`.
Deriving colour from `Status` would silently restyle those three states
from mid-slate to light-slate. The implementation carries a regression
test pinning their colour, and a comment on both functions explaining
why they stay separate.

Lifecycle strings map to glyphs as follows:

| String                                    | `Status`     |
| ----------------------------------------- | ------------ |
| `running`, `claimed`                      | `Active`     |
| `completed`, `complete`                   | `Complete`   |
| `failed`, `blocked`                       | `Failed`     |
| `awaiting_approval`, `awaiting`, `paused`, `await_human`, `await_external` | `Awaiting` |
| `rejected`, `retry_backoff`               | `Retrying`   |
| `pending`, `eligible`, `released`, `idle` | `Waiting`    |
| `skipped`                                 | `Skipped`    |

Classification values — scope (`project`/`global`), issue state
(`open`/`closed`/`merged`), and severity — keep a separate colour-only
path. They receive no glyph. A glyph in this CLI always means "where is
this in its lifecycle", and attaching one to a classification value
would imply progress semantics that do not exist.

`stopped` has no current `Status` variant and maps to colour-only until
one is justified.

### 4. Relative time

`output/fmt.rs` gains:

```rust
pub fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String;
```

Grades: `just now` under 60s, then minutes, hours, days, weeks. Beyond
one year it falls back to an absolute date, because "63w ago" is not
useful.

Tables render relative. Detail views and JSON keep full ISO. A global
`--absolute` flag forces ISO in tables, for correlating a run against
external logs.

`now` is an explicit parameter so tests are deterministic.

### 5. Empty-column suppression

`EntityTable` only. A column is dropped when every cell in the rows
being displayed is empty or `—`.

Suppression is computed over the rows *actually displayed*, not over the
whole store. This means the column set can differ between two invocations
of the same command with different filters. That is intended — the table
describes what is on screen — but it must be documented in `--help` so it
does not read as a bug.

`--all-columns` forces the full schema.

### 6. Summary line

`EntityTable` only. One line above the table, counting by lifecycle
status:

```
8 sessions · 2 failed · 5 idle · 1 stopped
```

Emitted only in the default human output format. Suppressed entirely
under `--output json` / `--output csv`.

### 7. `workflow list` enrichment

Today: NAME, SCOPE. After: NAME, SCOPE, STEPS, LAST RUN (glyph +
relative time), SCHEDULE.

LAST RUN requires a run-store read per workflow. A naive scan of every
run directory per row is O(workflows × runs) and will degrade as run
history grows. The implementation reads the run index once and joins in
memory; if no such index exists, adding a bounded most-recent-run lookup
is part of this work rather than an accepted regression.

## Contract guarantees

These are the rails that keep a 55-site change from breaking existing
usage:

1. `--output json` output is byte-identical before and after, and always
   carries full untruncated identifiers.
2. Colour and glyphs auto-disable off-TTY via the existing
   `color_disabled()` path. Glyph columns degrade to the bare status word.
3. Header rules still print when piped. They are structural, and no test
   or downstream consumer asserts on box-drawing characters (verified:
   no `.snap` files in `rupu-cli`, no assertions on `┌`/`├╌`/`╞═`).
4. Every identifier the CLI prints in compact form resolves back when
   pasted.

## Testing strategy

The 55-site blast radius currently has no regression net: nothing in
`rupu-cli` asserts on table output and the crate has no insta snapshots.
Changing every table in the CLI without one is the largest risk in this
work, above any individual styling choice.

- **Snapshot tests over the table renderers.** `insta` is already a
  workspace dependency (used by `rupu-app-canvas`); add it to
  `rupu-cli`'s dev-dependencies. One snapshot per distinct table shape,
  colour disabled, fixed clock.
- **`ids::resolve` unit tests** — full ID, compact form with ellipsis,
  bare suffix, prefix, zero matches, multiple matches, and the
  cross-scope Active/Archived collision.
- **`relative_time` grade boundaries** at 59s/60s, 59m/60m, 23h/24h,
  6d/7d, and the one-year absolute fallback.
- **Empty-column suppression** — all-empty column dropped, partially
  populated column retained, `--all-columns` override.
- **JSON contract test** — assert `--output json` is unchanged for at
  least one command per profile.

## Acceptance criteria

1. Every table in the CLI renders with a header rule and no per-row
   separators.
2. Any identifier displayed in compact form resolves when pasted back
   into a command that takes that identifier.
3. Ambiguous fragments error with all candidates listed, and exit
   non-zero.
4. Lifecycle statuses render with the `palette::Status` glyph;
   classification values render colour-only.
5. Entity lists show relative timestamps, suppress all-empty columns,
   and print a summary line.
6. `workflow list` shows steps, last-run status, and schedule.
7. `--output json` is byte-identical to pre-change output.
8. Snapshot coverage exists for every table shape touched.

## Implementation phases

1. **Foundation** — `ids.rs`, `status_of`, `relative_time`, the
   `new_table()` preset change, insta wired into `rupu-cli` dev-deps.
   Establishes the vocabulary; the preset change alone quiets all 55
   tables.
2. **Resolution wiring** — `read_session` and the run-store equivalent
   adopt `resolve`; ambiguity errors; cross-scope handling.
3. **Entity profile** — `EntityTable`, suppression, summary lines,
   applied to session / run / workflow.
4. **Remaining entity tables** — cron, repos, issues, agent, models,
   autoflow, cleanup, transcript.
5. **Report profile** — `ReportTable` applied to coverage, usage, auth.
6. **`workflow list` enrichment**, including the bounded last-run lookup.

## Open questions

None blocking. Two calls made during design, recorded here so they can be
revisited:

- Empty-column suppression is computed over displayed rows rather than
  the full store, accepting that column sets vary between filtered
  invocations.
- The summary line counts by lifecycle status only, not by scope or
  agent.
