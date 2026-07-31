# CLI Output Polish — Plan 3: Entity Adoption

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Adopt the `EntityTable` profile in the five commands where it is a clear win, giving their identifiers, timestamps and classification values proper rendering — leaving the ones where adoption would regress behaviour explicitly alone.

**Architecture:** One vocabulary extension to `output/tables.rs` that the per-command adoptions then consume. No changes to `EntityTable` itself except where a command genuinely needs a capability it lacks.

**Tech Stack:** Rust 2021, `comfy-table` 7.2.2, `owo-colors`, `chrono`, `anyhow`.

**Spec:** `docs/superpowers/specs/2026-07-30-rupu-cli-output-polish-design.md`
**Builds on:** Plan 2 (PR #573) and Plan 1 (PR #570).

## Why this is not "adopt all 18"

The spec envisages the remaining entity tables adopting the profile wholesale. A survey of all 18 call sites found several where adoption would **lose** behaviour, and the directive is UX over standardisation. So this plan converts five and states why it skips the rest.

**Converted:** `transcript list`, `transcript prune`, `cron list`, `agent list`, `autoflow wakes`.

**Skipped, with reasons:**

| Site | Why not |
|---|---|
| `issues list` | LABELS carries a per-column `ColumnConstraint::UpperBoundary(Width::Fixed(48))` (`issues.rs:221`) and TITLE is hand-truncated to 60 chars (`issues.rs:218`). `EntityTable` can express neither. Adoption would drop both caps and change wrapping under a narrow terminal — a regression, not a polish. |
| `models list` | Two blockers. `models.rs` has no `UiPrefs` anywhere, so adoption needs new plumbing first. And CONTEXT is `Option<u64>` that is **`—` for 23 of 23 rows today** — a suppression-eligible `Missing` would delete the column in everyday use, the third instance of the bug class that already cost us `DURATION` and `COST`. |
| `cleanup` (both sites) | No `UiPrefs`, and `Args` has no `--no-color` flag at all (`cleanup.rs:10-27`). `--stats` is also a numeric aggregate, not an entity list. |
| `repos list` / `repos tracked` | Classification values (`github`/`gitlab`, `private`/`public`) aren't in the colour vocabulary, and whether a bare `owner/repo` qualifies as a `Name` is genuinely unclear — the CLI accepts `github:owner/repo`, a different string. Low value, real ambiguity. |
| `autoflow doctor` | Each row is a free-text diagnostic. No identity, no lifecycle, no timestamp. The weakest fit of all 18. |
| `autoflow status` (histogram) | Rows *are* status/count pairs. A summary line would duplicate the table's own content. |
| `autoflow list` / `autoflow history` | Both pre-flatten real `Option`s into the literal string `"-"` inside `Serialize`-frozen structs (`autoflow.rs:4969-5001`, `:5090-5108`), so `Missing` can't be attached without a fragile string-compare or a JSON shape change. `history`'s table path is also only reached when stdout is piped — interactively it renders a different snapshot view. |

Revisit `issues list` and `models list` if `EntityTable` later gains a per-column constraint hook and a non-suppressible cell kind.

## Global Constraints

- Workspace dependency versions pinned in the root `Cargo.toml` only.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden. `rupu-cli` uses `anyhow`.
- Unit tests live in-module under `#[cfg(test)] mod tests`.
- **`--format json` / `--format csv` byte-identical** for every command touched. Every row struct in this plan derives `Serialize` and is FROZEN — never add or rename its fields. Table headers are independent of struct field names; changing a header does not change JSON.
- **Never run `cargo fmt`** — reformats ~106 files here. Use `rustfmt --edition 2021 <path>` and verify with `git diff --stat`.
- **Never `rustfmt` module-root files** (`lib.rs`, `output/mod.rs`) — they cascade into siblings (`lib.rs` measured hitting 12).
- **Never `rustfmt` `cmd/autoflow.rs` or `cmd/workflow.rs`** — pre-existing drift; hand-write additions correctly formatted.
- **Never use `git stash` in any form.** The stash stack is shared across every worktree of this repo; a bare `stash pop` in an earlier session applied an unrelated entry and dirtied 104 files. Use `git checkout -- <paths>`.
- Never `git add -A`; always explicit paths.
- Baseline: `cargo clippy -p rupu-cli --lib` has ONE pre-existing error in `cmd/completers.rs` (`question_mark`). Not yours.
- Baselines: `cargo test -p rupu-cli --lib` → 648 / 0; `cargo build -p rupu-cli --lib` → **0 warnings**.

---

### Task 1: Extend the classification colour vocabulary

**Files:** Modify `crates/rupu-cli/src/output/tables.rs`

**Interfaces:** Extends `status_color`'s match. Consumed by Tasks 3, 5, and 6 via `EntityTable`'s `CellValue::Status`. Nothing else in the CLI reads it today — see the note below.

**Why first.** A dozen classification values have no arm in `status_color`, so any call site that renders them gets no colour. The later tasks in this plan route those values through `EntityTable`'s `CellValue::Status`, which consults `status_color` — so the vocabulary has to exist before they land.

**This task produces no visible change on its own.** An earlier draft of this plan claimed it would colour these values "everywhere at once, including in commands this plan skips." That was wrong, and verifying it is why the claim is gone: `cleanup.rs` contains **zero** `status_cell` / `status_color` calls and never colours anything, and `transcript list` renders SCOPE as a plain `Cell::new(&row.scope)` — only its STATUS column routes through `status_cell`. A vocabulary entry only reaches a screen when a call site actually asks for it.

So treat this as preparation, not payoff. The payoff arrives in Tasks 3, 5, and 6. Do not go looking for colour in untouched commands afterwards, and do not "helpfully" convert extra call sites to `status_cell` to manufacture the breadth — that is other tasks' scope, with their own tests and reviews.

Values confirmed present in real output but uncoloured:

| Value | Where | Suggested palette |
|---|---|---|
| `active` | transcript SCOPE, session SCOPE | `complete` |
| `archived` | transcript SCOPE, cleanup SCOPE | `dim` |
| `would_delete` | cleanup ACTION, transcript prune ACTION | `awaiting` |
| `deleted` | cleanup ACTION, transcript prune ACTION | `failed` |
| `queued` | autoflow wake State | `dim` |
| `due` | autoflow wake State | `awaiting` |
| `processed` | autoflow wake State | `complete` |
| `issue` / `pull_request` | autoflow Entity | `brand_subtle` |
| `session` / `transcript` | cleanup KIND | `brand_subtle` |

**Do NOT add these to `status_of`.** That function drives the lifecycle *glyph*, and these are classification values — a glyph would imply progress they don't have. This is the rule Plan 1 established and Plan 2 upheld; the regression test `pending_keeps_its_dim_colour_not_the_waiting_colour` guards the related separation and must still pass.

`would_delete` / `deleted` are a judgment call: they *are* lifecycle-ish (dry-run vs. done). Colour them, but keep them out of `status_of` — a `⊘` or `✓` on a deletion preview would read as reassurance the CLI hasn't earned.

- [ ] **Step 1: Write the failing tests**

Append to `tables.rs`'s existing `mod tests`:

```rust
    #[test]
    fn status_color_covers_scope_and_lifecycle_classifications() {
        let prefs = prefs_color_always();
        let p = crate::output::palette::active_palette();
        assert_eq!(status_color("active", &prefs), Some(p.complete.into_table()));
        assert_eq!(status_color("archived", &prefs), Some(p.dim.into_table()));
        assert_eq!(status_color("would_delete", &prefs), Some(p.awaiting.into_table()));
        assert_eq!(status_color("deleted", &prefs), Some(p.failed.into_table()));
    }

    #[test]
    fn status_color_covers_autoflow_wake_states() {
        let prefs = prefs_color_always();
        let p = crate::output::palette::active_palette();
        assert_eq!(status_color("queued", &prefs), Some(p.dim.into_table()));
        assert_eq!(status_color("due", &prefs), Some(p.awaiting.into_table()));
        assert_eq!(status_color("processed", &prefs), Some(p.complete.into_table()));
    }

    #[test]
    fn status_color_covers_entity_and_kind_classifications() {
        let prefs = prefs_color_always();
        let p = crate::output::palette::active_palette();
        for v in ["issue", "pull_request", "session", "transcript"] {
            assert_eq!(
                status_color(v, &prefs),
                Some(p.brand_subtle.into_table()),
                "no colour for {v}"
            );
        }
    }

    #[test]
    fn new_classification_values_get_no_lifecycle_glyph() {
        // A glyph means "where is this in its lifecycle". None of these
        // are lifecycle positions, so status_of must still reject them.
        for v in ["active", "archived", "would_delete", "deleted", "queued",
                  "due", "processed", "issue", "pull_request", "session",
                  "transcript"] {
            assert_eq!(status_of(v), None, "{v} must not get a glyph");
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cli --lib output::tables`
Expected: the three colour tests FAIL; `new_classification_values_get_no_lifecycle_glyph` passes already (it is a guard, not a driver — do not be alarmed).

Note `status_of` currently maps `"idle" | "stopped"` to `Status::Waiting` / `Status::Skipped`, so those are *not* in the list above.

- [ ] **Step 3: Implement**

Add arms to `status_color`'s match only. Do not restructure the function, and do not touch `status_of`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rupu-cli --lib output::tables`
Expected: PASS, including `pending_keeps_its_dim_colour_not_the_waiting_colour`.

- [ ] **Step 5: Confirm the vocabulary is reachable, not that it is visible**

There is nothing to see yet — see the note above. Instead, verify the arms are wired correctly by unit test only, and record in your report which call sites will consume them (Tasks 3, 5, 6) so the next implementer knows where the payoff lands.

Optionally, confirm the negative for yourself so it is not a surprise later:

```bash
grep -c "status_cell\|status_color" crates/rupu-cli/src/cmd/cleanup.rs   # expect 0
```

- [ ] **Step 6: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/tables.rs
git diff --stat
cargo clippy -p rupu-cli --lib 2>&1 | tail -5
git add crates/rupu-cli/src/output/tables.rs
git commit -m "feat(cli): colour the remaining classification values

A dozen values rendered as bare grey text because status_color had no
arm for them: active/archived, would_delete/deleted, queued/due/
processed, issue/pull_request, session/transcript.

Adding arms colours them everywhere at once, including in commands this
plan deliberately doesn't otherwise touch.

Kept out of status_of: these are classifications, not lifecycle
positions, and a glyph would imply progress they don't have. A ✓ on a
deletion preview would read as reassurance the CLI hasn't earned."
```

---

### Task 2: Thread `--absolute` / `--all-columns` into the commands this plan converts

**Files:** Modify `crates/rupu-cli/src/lib.rs`, `crates/rupu-cli/src/cmd/{transcript,cron,agent}.rs`

**`autoflow` is deliberately NOT in this task.** A survey during execution found `wakes()` builds no `UiPrefs` at all — it renders with raw `Cell::new` and its output struct has no `prefs` field. Threading the flags here would mean either an unused parameter or a discarded config read, both of which are dead code this project's standards reject. Task 6 adds the `UiPrefs::resolve` call when it converts that table, giving the flags a real consumer from their first commit.

**Interfaces:** Extends four `handle()` signatures with `absolute: bool, all_columns: bool` and threads each to `UiPrefs::with_table_flags(...)`.

Only `Cmd::Workflow` and `Cmd::Session` receive these flags today (`lib.rs:300-313`). Without this, the relative timestamps the later tasks add cannot be toggled back to ISO, and `--all-columns` silently does nothing — a silent no-op, which this project's standards explicitly reject.

Thread the three commands that already build a `UiPrefs` on a converted path. Do NOT thread `models`/`cleanup`/`repos`/`issues` — they aren't being converted, and adding parameters they don't use is dead plumbing.

Follow the established pattern: `cmd/session.rs:1211` and `cmd/workflow.rs:1170` show `UiPrefs::resolve(...).with_table_flags(absolute, all_columns)`.

- [ ] **Step 1: Survey**

```bash
grep -n "Cmd::Transcript\|Cmd::Cron\|Cmd::Agent\|Cmd::Autoflow" crates/rupu-cli/src/lib.rs
grep -n "UiPrefs::resolve" crates/rupu-cli/src/cmd/{transcript,cron,agent,autoflow}.rs
```
Record each `handle()` signature and every `UiPrefs::resolve` site inside those four files. Some files build `UiPrefs` in more than one place; all of them on a converted command's path need the flags.

- [ ] **Step 2: Write the failing tests**

```rust
    #[test]
    fn transcript_list_honours_absolute() {
        // Threading proof: the flag must reach the renderer, not just parse.
        let cli = crate::Cli::try_parse_from(["rupu", "transcript", "list", "--absolute"])
            .expect("parses");
        assert!(cli.absolute);
    }
```

That test alone is weak — it only proves parsing, which already worked. **The real proof is behavioural and belongs in Task 3's tests**, where `--absolute` must visibly change a rendered timestamp. State in your report that Task 2 is plumbing whose correctness Task 3 demonstrates, and do not claim more.

- [ ] **Step 3: Implement**

Extend the four `handle()` signatures, thread to every `UiPrefs::resolve` on a converted path, and update the four `lib.rs` arms to pass `cli.absolute, cli.all_columns`.

**Watch for transposition.** The two booleans are adjacent and same-typed; swapping them is silent. Name the parameters `absolute` and `all_columns` at every level so they line up positionally by name.

- [ ] **Step 4: Verify**

```bash
cargo test -p rupu-cli --lib
cargo build -p rupu-cli --lib 2>&1 | grep -c warning   # must be 0
cargo run -p rupu-cli -- transcript list --absolute | head -3
cargo run -p rupu-cli -- transcript list --all-columns | head -3
```
Both must parse and exit 0. They won't visibly change anything yet — Task 3 wires the renderer.

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/transcript.rs crates/rupu-cli/src/cmd/cron.rs crates/rupu-cli/src/cmd/agent.rs
git diff --stat   # confirm rustfmt didn't cascade; autoflow.rs and lib.rs hand-edited only
git add crates/rupu-cli/src/lib.rs crates/rupu-cli/src/cmd/transcript.rs crates/rupu-cli/src/cmd/cron.rs crates/rupu-cli/src/cmd/agent.rs crates/rupu-cli/src/cmd/autoflow.rs
git commit -m "feat(cli): thread table flags into transcript/cron/agent/autoflow

Only Workflow and Session received --absolute/--all-columns. Without
this the relative timestamps the next commits add can't be toggled back
to ISO, and --all-columns is a silent no-op.

Threaded only for the commands this plan converts — adding parameters
to handlers that don't use them would be dead plumbing."
```

---

### Task 3: `transcript list` adopts the entity profile

**Files:** Modify `crates/rupu-cli/src/cmd/transcript.rs`

**Interfaces:** Consumes `EntityTable`, `CellValue`, `UiPrefs::render_opts`.

**The best fit of all 18 sites**, and the only converted one with a literal `STATUS` header — so it gets a real summary breakdown, not just a count.

Header (`transcript.rs:306`): `["RUN ID", "SCOPE", "TITLE", "AGENT", "STATUS", "TOKENS", "STARTED"]`
Row: `TranscriptListRow` (`transcript.rs:183-192`), `#[derive(Serialize)]` — **FROZEN**:

```rust
struct TranscriptListRow {
    run_id: String, scope: String, title: Option<String>,
    agent: String, status: String, total_tokens: u64, started_at: String,
}
```

Mapping:
- `RUN ID` → `Id` (`run_<ULID>`)
- `SCOPE` → `Status` (`active` / `archived` — coloured as of Task 1)
- `TITLE` → `Option<String>` → `Missing`. **Already renders as a dimmed em dash today** (`transcript.rs:314-320`), so this is the closest existing match to `Missing` in the codebase. Live data shows a real mix (~40% `—`), so the column survives suppression in practice.
- `AGENT` → `Name`. **But check the `"nobody"` sentinel first** — live data shows several rows with the literal agent value `"nobody"`. Determine whether that is a real agent name or a placeholder for "unknown". If it's a placeholder, it should arguably be `Missing`, not a `Name` the CLI would fail to resolve. Report your finding; do not guess.
- `STATUS` → `Status` (`completed` / `failed`, already coloured)
- `TOKENS` → `Text` (`u64`, never absent)
- `STARTED` → `started_at: String` built via `.format("%Y-%m-%d %H:%M:%S")` (`transcript.rs:1482`) — **the same naive format `render_workflow_runs_table` already parses.** Reuse that exact three-tier fallback verbatim (`workflow.rs:957-960`): stored format → RFC3339 → verbatim text. Do not invent a fourth variant.

`.with_summary("transcript")`.

- [ ] **Step 1: Write the failing tests**

Extract into `render_transcript_list_table(&[TranscriptListRow], &UiPrefs, DateTime<Utc>) -> String`, leaving `render_table` a thin `println!` wrapper. Follow `render_session_list_table`'s shape (`cmd/session.rs:615`).

Assert: run id compacted and the full id absent; `STARTED` shows a relative age; a summary line breaking down by STATUS (`2 transcripts · 1 completed · 1 failed`); an unparseable `started_at` renders verbatim without dropping the row; and — under `RenderOpts { absolute: true, .. }` — the timestamp renders ISO. That last one is the behavioural proof Task 2's plumbing works.

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Verify against the real binary**

```bash
cargo run -p rupu-cli -- transcript list | head -8
cargo run -p rupu-cli -- transcript list --absolute | head -4
cargo run -p rupu-cli -- transcript list --format json | head -8   # diff vs a pre-change capture
```
Then paste a compacted run id from the listing back: `cargo run -p rupu-cli -- transcript show '<compact id>'`. **If it does not resolve, that is Critical — stop and report.** (Plan 1 wired `transcript show` through fragment resolution, so it should.)

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/transcript.rs
git diff --stat
git add crates/rupu-cli/src/cmd/transcript.rs
git commit -m "feat(cli): transcript list adopts the entity profile

Compact run ids, coloured scope and status, relative start times, and a
real STATUS breakdown in the summary — this is the only remaining site
with a literal STATUS header, so it gets more than a bare count.

Reuses render_workflow_runs_table's three-tier timestamp fallback
verbatim: started_at is the same naive %Y-%m-%d %H:%M:%S format."
```

---

### Task 4: `cron list` adopts the entity profile

**Files:** Modify `crates/rupu-cli/src/cmd/cron.rs`

Header (`cron.rs:166`): `["NAME", "SCHEDULE", "NEXT (UTC)", "IN"]`
Row: `CronListRow` (`cron.rs:105-111`), `#[derive(Serialize)]` — **FROZEN**:

```rust
struct CronListRow {
    name: String, schedule: String,
    next_utc: Option<String>, in_seconds: Option<i64>,
}
```

Mapping:
- `NAME` → `Name` (a workflow name; `workflow show`/`run` accept it back)
- `SCHEDULE` → `Text` (a cron expression)
- `NEXT (UTC)` → `next_utc` built via `.format("%Y-%m-%d %H:%M:%S")` (`cron.rs:258`) — same naive format as Task 3; reuse the same fallback.
- `IN` → currently `tables::relative_time_cell(seconds)` for `Some`, empty `Cell` for `None`.

**⚠️ The critical decision in this task.** `next_utc` and `in_seconds` are both `None` when the schedule failed to parse or has no future occurrence (`cron.rs:259-260`, computed together). That is **not** "this entity has no such dimension" — every cron workflow has a schedule. It is "we could not compute it," which is precisely the state an operator runs `cron list` to discover.

If both columns are mapped to `Missing`, then a listing where *every* schedule is broken suppresses both columns — hiding the exact failure the command exists to surface. This is the fourth instance of the bug class that already cost us `DURATION`, `COST`, and (caught before shipping) `models list`'s CONTEXT.

**Therefore: render `None` as a stable non-empty cell, not `Missing`.** Use `CellValue::Text("unscheduled".to_string())` — or check what the current empty `Cell::new("")` looks like to a user and pick wording that reads correctly for "this schedule doesn't produce a next run." State your choice and reasoning in your report.

`.with_summary("workflow")` — note there is no `STATUS` header, so this yields a bare count. That is fine; say so rather than renaming a header to force a breakdown.

- [ ] **Step 1: Write the failing tests**

Extract `render_cron_list_table(...) -> String`. Assert: name renders verbatim and never wraps; a populated `next_utc` renders relative; **a row with `next_utc: None` and `in_seconds: None` still shows both columns**; and — the guard for this task's whole point — **a table where EVERY row has both as `None` still shows both columns.**

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Verify**

```bash
cargo run -p rupu-cli -- cron list
cargo run -p rupu-cli -- cron list --absolute
cargo run -p rupu-cli -- cron list --format json   # diff vs pre-change
```

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/cron.rs
git diff --stat
git add crates/rupu-cli/src/cmd/cron.rs
git commit -m "feat(cli): cron list adopts the entity profile

Workflow names never wrap, next-run times render relative.

NEXT/IN render a stable label rather than Missing when the schedule
can't be computed. Both are None together when parsing fails — the very
state cron list exists to surface — and a suppression-eligible Missing
would delete both columns exactly when every schedule is broken."
```

---

### Task 5: `agent list` adopts the entity profile

**Files:** Modify `crates/rupu-cli/src/cmd/agent.rs`

Header (`agent.rs:220`): `["NAME", "SCOPE", "DESCRIPTION"]`
Row: `AgentListRow` (`agent.rs:153-158`), `#[derive(Serialize)]` — **FROZEN**:

```rust
struct AgentListRow { name: String, scope: String, description: Option<String> }
```

Mapping: `NAME` → `Name`; `SCOPE` → `Status` (`project` / `global`, already coloured via `status_cell` today); `DESCRIPTION` → `Option<String>` → `Missing`.

`DESCRIPTION` is the **safe** `Missing` case, in contrast to Task 4: `None` means the agent's frontmatter simply declares no `description:`. If every agent in a project lacks one, suppressing the column is *correct* — there is genuinely nothing to show. Note this contrast explicitly in your report; the difference between Task 4's mapping and this one is the judgment the plan is teaching.

`.with_summary("agent")` — no `STATUS` header, so a bare count.

The simplest of the five. Live data: ~29 rows, every DESCRIPTION populated.

- [ ] **Step 1: Write the failing tests**

Extract `render_agent_list_table(...) -> String`. Assert: name never wraps; scope coloured; a `None` description renders an em dash; an all-`None` DESCRIPTION column IS suppressed (the opposite of Task 4's assertion — make the contrast explicit in the test name).

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Verify**

```bash
cargo run -p rupu-cli -- agent list | head -8
cargo run -p rupu-cli -- agent list --format json | head -6   # diff vs pre-change
```

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/agent.rs
git diff --stat
git add crates/rupu-cli/src/cmd/agent.rs
git commit -m "feat(cli): agent list adopts the entity profile

Agent names never wrap, scope keeps its colour, and an absent
description renders an em dash.

DESCRIPTION is deliberately suppression-eligible, unlike cron's NEXT/IN:
None here means the frontmatter declares no description, so a project
whose agents all lack one has genuinely nothing to show."
```

---

### Task 6: `autoflow wakes` and `transcript prune`

**Files:** Modify `crates/rupu-cli/src/cmd/autoflow.rs`, `crates/rupu-cli/src/cmd/transcript.rs`

Two small conversions sharing a shape: an `Id` column plus an RFC3339 timestamp.

**`autoflow wakes`** (`autoflow.rs:601`), header `["Wake", "State", "Source", "Event", "Entity", "Not Before", "Repo"]`, row `AutoflowWakeRow` (`autoflow.rs:354-363`, all plain `String`, no `Option` — so no `Missing` candidates at all):
- `Wake` → `Id` (`wake_<ULID>`)
- `State` → `Status` (`queued` / `due` / `processed` — uncoloured today, coloured as of Task 1). This is the payoff of Task 1 landing first.
- `Not Before` → RFC3339 (confirmed: rendered today through `compact_timestamp`, which parses RFC3339 first, `autoflow.rs:6905`) → `Timestamp`
- Everything else → `Text`

**⚠️ `autoflow.rs` has pre-existing rustfmt drift. Do NOT run `rustfmt` on it.** Hand-write additions already correctly formatted, and verify with `git diff --stat` that only your lines moved.

**`transcript prune`** (`transcript.rs:357`), header `["RUN ID", "SCOPE", "LOCATION", "ARCHIVED", "ACTION"]`, row `TranscriptPruneRow` (`transcript.rs:218-225`, all plain `String`):
- `RUN ID` → `Id`
- `SCOPE` → `Status` (`global` / `project`, already coloured)
- `LOCATION` → `Text` (a filesystem path)
- `ARCHIVED` → RFC3339 (`archived_at.to_rfc3339()`, `transcript.rs:1694`) → `Timestamp`
- `ACTION` → `Status` (`would_delete` / `deleted` — coloured as of Task 1)

Neither gets a summary: `wakes` has no `STATUS` header and `prune`'s `ACTION` isn't literally `STATUS`. Do not rename headers to force one.

- [ ] **Step 1: Write the failing tests**

Extract `render_autoflow_wakes_table(...)` and `render_transcript_prune_table(...)`. Assert for each: id compacted and full id absent; timestamp relative; state/action coloured. Add one test proving an unparseable timestamp renders verbatim rather than dropping the row.

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Verify**

```bash
cargo run -p rupu-cli -- autoflow wakes | head -6
cargo run -p rupu-cli -- autoflow wakes --format json | head -6   # diff vs pre-change
```
`transcript prune` is destructive — exercise it via tests only, or with a dry-run flag if one exists. Say which you did.

Then paste a compacted wake id back into whatever command accepts one. **If none does, say so explicitly** — it is display-only and compaction is safe, but that must be verified rather than assumed.

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/transcript.rs   # NOT autoflow.rs
git diff --stat
git add crates/rupu-cli/src/cmd/autoflow.rs crates/rupu-cli/src/cmd/transcript.rs
git commit -m "feat(cli): autoflow wakes and transcript prune adopt the profile

Compact ids and relative timestamps on both. Wake State and prune ACTION
pick up the colour added in this plan's first commit — they rendered as
bare grey text before.

Neither gets a summary line: neither has a literal STATUS header, and
renaming one to force a breakdown would misrepresent the column."
```

---

## Verification

- [ ] `cargo test -p rupu-cli` passes in full; `cargo build -p rupu-cli --lib` emits 0 warnings.
- [ ] `cargo clippy -p rupu-cli --lib` introduces nothing beyond the pre-existing `completers.rs` error.
- [ ] **Every compacted identifier resolves.** Take a run id from `transcript list` and paste it into `transcript show`. This is the governing rule; a regression is Critical.
- [ ] `--format json` / `--format csv` byte-identical for all five converted commands.
- [ ] `--absolute` visibly changes timestamps on `transcript list` and `cron list`; `--all-columns` restores suppressed columns.
- [ ] **`cron list` still shows NEXT and IN when every row's schedule is broken.** The single most important behavioural check in this plan.
- [ ] Colour appears on the converted commands' Status columns (`transcript list` SCOPE, `autoflow wakes` State, `transcript prune` ACTION). It does NOT appear on `cleanup --stats` — that file has zero `status_cell` calls and is not converted here. Do not treat its absence as a bug.
- [ ] No identifier wraps at a narrow width (try `COLUMNS=60`).

## Deliberately not done

`issues list`, `models list`, `cleanup` (both), `repos list` / `tracked`, `autoflow doctor`, `autoflow status`, `autoflow list`, `autoflow history` — see the table at the top for the reason in each case. They still gain Task 1's colour where their values overlap the vocabulary.

## Follow-on candidates

- **A per-column constraint hook on `EntityTable`.** Would unblock `issues list`, whose LABELS column needs `UpperBoundary(48)` and whose TITLE is hand-truncated to 60.
- **A non-suppressible cell kind.** Task 4 works around the absence of one with a literal label. A `CellValue::Fixed(String)` — renders verbatim, never counts as empty — would express the intent directly and retire the workaround in three places (`workflow runs`' `(in flight)`, `usage`' `n/a`, `cron list`'s `unscheduled`).
- **`UiPrefs` plumbing for `models` and `cleanup`**, which would unblock those two.
- **`autoflow`'s `"-"` sentinel.** `AutoflowClaimRow` and `AutoflowHistoryRow` pre-flatten `Option`s into a literal `"-"` inside `Serialize`-frozen structs. Fixing it properly means changing those field types to real `Option<String>` — a JSON shape change needing a version bump.
