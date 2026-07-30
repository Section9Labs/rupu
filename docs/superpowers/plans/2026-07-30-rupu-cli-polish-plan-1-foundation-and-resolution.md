# CLI Output Polish — Plan 1: Foundation and Identifier Resolution

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the shared vocabulary the CLI polish depends on — identifier compaction paired with fragment resolution, relative timestamps, a lifecycle-glyph mapping, and the quiet table preset — then wire resolution into the session and run lookups so short identifiers work everywhere.

**Architecture:** Four pure, independently testable helpers land first (`ids.rs`, `relative_time`, `status_of`, the `new_table()` preset). Two wiring tasks then adopt resolution inside `read_session` and the run-store lookup, so every existing caller inherits it without touching call sites. No table is restyled beyond the global preset change in this plan; applying the entity/report profiles is Plan 2 and Plan 3.

**Tech Stack:** Rust 2021, `comfy-table` 7.2.2 (`custom_styling`), `owo-colors`, `chrono`, `insta` (snapshot tests), `anyhow` (CLI errors).

**Spec:** `docs/superpowers/specs/2026-07-30-rupu-cli-output-polish-design.md`

## Global Constraints

- Workspace dependency versions are pinned in the root `Cargo.toml` only. Never add a version to a crate `Cargo.toml` — use `insta.workspace = true`.
- `#![deny(clippy::all)]` is workspace-wide via `[workspace.lints]`. `unsafe_code` is forbidden.
- `rupu-cli` uses `anyhow` for errors; libraries use `thiserror`.
- Unit tests live in-module under `#[cfg(test)] mod tests`, matching `output/fmt.rs` and `output/tables.rs`.
- `--output json` output must remain byte-identical. Identifiers in JSON are always full-length.
- Never run a package-wide `cargo fmt` — this repo is fmt-dirty under the pinned toolchain. Format only the files you touched: `rustfmt --edition 2021 <path>`.
- Verified baseline: `cargo check -p rupu-cli --lib` exits 0 on this worktree (rustc 1.97.1). Any error you see is yours.
- Glyphs come from `palette::Status` and are never invented. Classification values (scope, issue state, severity) get colour only, never a glyph.

## Scope of this plan

In: `output/ids.rs`, `relative_time`, `status_of`, the `new_table()` preset change, insta wired into `rupu-cli`, session-ID resolution, run-ID resolution.

Out (later plans): `EntityTable` / `ReportTable` profiles, empty-column suppression, summary lines, applying compaction to individual table cells, `workflow list` enrichment.

Note the ordering consequence: after this plan, tables are quieter and short IDs resolve, but table cells still print full identifiers and absolute timestamps. Cell-level adoption is Plan 2. This is deliberate — the helpers get their own review gate before 55 call sites depend on them.

---

### Task 1: Identifier compaction and resolution

**Files:**
- Create: `crates/rupu-cli/src/output/ids.rs`
- Modify: `crates/rupu-cli/src/output/mod.rs` (add `pub mod ids;`)

**Interfaces:**
- Consumes: nothing.
- Produces: `output::ids::compact_id(&str) -> String`, `output::ids::Resolution` (`Unique(String)` / `NotFound` / `Ambiguous(Vec<String>)`), `output::ids::resolve(&[String], &str) -> Resolution`. Tasks 5 and 6 consume all three.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-cli/src/output/ids.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<String> {
        vec![
            "ses_01KWA7HTYEDX0ACG93ZW26FG3M".to_string(),
            "ses_01KWAA6X3KH6BNCPXF5Q63FJM0".to_string(),
            "ses_01KWA8RA3NE9J88X6DHSH76AC8".to_string(),
            "ses_01KS1PC047QS6NG1RG644AWXTW".to_string(),
        ]
    }

    #[test]
    fn compact_shortens_long_ids() {
        assert_eq!(
            compact_id("run_01KRJDKSBE7X4J49094149WFJS"),
            "run_01KRJDKS…WFJS"
        );
    }

    #[test]
    fn compact_leaves_short_ids_untouched() {
        assert_eq!(compact_id("run_short"), "run_short");
        // Exactly at the threshold: 18 chars, returned verbatim.
        assert_eq!(compact_id("run_0123456789ABCD"), "run_0123456789ABCD");
    }

    #[test]
    fn resolves_full_id() {
        let c = candidates();
        assert_eq!(
            resolve(&c, "ses_01KWA7HTYEDX0ACG93ZW26FG3M"),
            Resolution::Unique("ses_01KWA7HTYEDX0ACG93ZW26FG3M".to_string())
        );
    }

    #[test]
    fn resolves_the_compact_form_it_printed() {
        let c = candidates();
        let shown = compact_id(&c[0]);
        assert_eq!(shown, "ses_01KWA7HT…FG3M");
        assert_eq!(resolve(&c, &shown), Resolution::Unique(c[0].clone()));
    }

    #[test]
    fn every_compact_form_round_trips() {
        // The governing rule: never display an identifier we won't
        // accept back. Assert it for every candidate, not just one.
        let c = candidates();
        for id in &c {
            assert_eq!(
                resolve(&c, &compact_id(id)),
                Resolution::Unique(id.clone()),
                "compact form of {id} failed to resolve"
            );
        }
    }

    #[test]
    fn resolves_bare_suffix() {
        let c = candidates();
        assert_eq!(
            resolve(&c, "26FG3M"),
            Resolution::Unique(c[0].clone())
        );
    }

    #[test]
    fn resolves_unambiguous_prefix() {
        let c = candidates();
        assert_eq!(
            resolve(&c, "ses_01KS"),
            Resolution::Unique(c[3].clone())
        );
    }

    #[test]
    fn ambiguous_prefix_lists_all_matches_sorted() {
        // ULIDs are timestamp-prefixed, so records created close
        // together collide on prefix. This is the common failure.
        let c = candidates();
        match resolve(&c, "ses_01KWA") {
            Resolution::Ambiguous(matches) => {
                assert_eq!(matches.len(), 3);
                // Sorted, so error output is stable across runs.
                assert_eq!(matches[0], "ses_01KWA7HTYEDX0ACG93ZW26FG3M");
                assert_eq!(matches[1], "ses_01KWA8RA3NE9J88X6DHSH76AC8");
                assert_eq!(matches[2], "ses_01KWAA6X3KH6BNCPXF5Q63FJM0");
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn unknown_fragment_is_not_found() {
        assert_eq!(resolve(&candidates(), "zzzzzz"), Resolution::NotFound);
    }

    #[test]
    fn empty_fragment_is_not_found_never_ambiguous() {
        // An empty fragment is a prefix of everything. Returning
        // Ambiguous(all) would be technically true and useless.
        assert_eq!(resolve(&candidates(), ""), Resolution::NotFound);
    }

    #[test]
    fn exact_match_wins_over_partial() {
        let c = vec![
            "run_0123456789".to_string(),
            "run_0123456789_extra".to_string(),
        ];
        assert_eq!(
            resolve(&c, "run_0123456789"),
            Resolution::Unique("run_0123456789".to_string())
        );
    }

    #[test]
    fn empty_candidate_set_is_not_found() {
        assert_eq!(resolve(&[], "anything"), Resolution::NotFound);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib output::ids`
Expected: FAIL — the module isn't registered and `compact_id` / `resolve` / `Resolution` don't exist.

- [ ] **Step 3: Register the module**

In `crates/rupu-cli/src/output/mod.rs`, add `pub mod ids;` to the module list, keeping alphabetical order (between `formats` and `jsonl_reader`):

```rust
pub mod formats;
pub mod ids;
pub mod jsonl_reader;
```

- [ ] **Step 4: Write the implementation**

Prepend to `crates/rupu-cli/src/output/ids.rs`, above the test module:

```rust
//! Identifier compaction and fragment resolution.
//!
//! Governing rule: never display an identifier the CLI will not accept
//! back. `compact_id` shortens for display; `resolve` accepts that
//! exact compact form, plus full identifiers, bare suffixes, and
//! prefixes.
//!
//! Both halves matter for ULIDs specifically. The leading 10 characters
//! after the type prefix encode a millisecond timestamp, so records
//! created close together share a prefix — three sessions in a real
//! listing shared `ses_01KWA`. The trailing 16 characters are random
//! and are what actually disambiguate. Keeping both ends means the
//! displayed form stays sortable by eye AND uniquely identifying.

/// Leading characters kept by `compact_id`, including the `ses_` /
/// `run_` type prefix. Four prefix characters plus eight ULID
/// characters.
const HEAD: usize = 12;

/// Trailing characters kept by `compact_id`, taken from the ULID's
/// random tail.
const TAIL: usize = 4;

/// Identifiers this long or shorter are returned unchanged — below
/// this, compaction saves nothing and only costs legibility.
const MIN_COMPACT_LEN: usize = 18;

/// Shorten an identifier for display as `<12 chars>…<4 chars>`.
///
/// Identifiers of `MIN_COMPACT_LEN` characters or fewer are returned
/// verbatim. The result always resolves back via [`resolve`].
pub fn compact_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= MIN_COMPACT_LEN {
        return id.to_string();
    }
    let head: String = chars[..HEAD].iter().collect();
    let tail: String = chars[chars.len() - TAIL..].iter().collect();
    format!("{head}…{tail}")
}

/// Outcome of matching a user-supplied fragment against known
/// identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one identifier matched.
    Unique(String),
    /// Nothing matched.
    NotFound,
    /// Two or more matched. Always an error at the call site — never
    /// silently pick one. Sorted for stable output.
    Ambiguous(Vec<String>),
}

/// Match `fragment` against `candidates`.
///
/// Accepted forms, in order of precedence:
/// 1. An exact full identifier.
/// 2. The compact `head…tail` form produced by [`compact_id`].
/// 3. A bare prefix or a bare suffix.
pub fn resolve(candidates: &[String], fragment: &str) -> Resolution {
    if fragment.is_empty() {
        return Resolution::NotFound;
    }

    // An exact match always wins, even when the fragment is also a
    // prefix of some longer identifier.
    if let Some(exact) = candidates.iter().find(|c| c.as_str() == fragment) {
        return Resolution::Unique(exact.clone());
    }

    let mut matches: Vec<String> = candidates
        .iter()
        .filter(|c| matches_fragment(c, fragment))
        .cloned()
        .collect();
    matches.sort();
    matches.dedup();

    match matches.len() {
        0 => Resolution::NotFound,
        1 => Resolution::Unique(matches.remove(0)),
        _ => Resolution::Ambiguous(matches),
    }
}

/// True when `fragment` identifies `id`. Splitting on the ellipsis is
/// what lets a user paste back the exact string a table printed.
fn matches_fragment(id: &str, fragment: &str) -> bool {
    if let Some((head, tail)) = fragment.split_once('…') {
        // Guard against a fragment longer than the identifier, where
        // head and tail would overlap and both match spuriously.
        if head.chars().count() + tail.chars().count() > id.chars().count() {
            return false;
        }
        return id.starts_with(head) && id.ends_with(tail);
    }
    id.starts_with(fragment) || id.ends_with(fragment)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib output::ids`
Expected: PASS — 12 tests.

- [ ] **Step 6: Format and lint**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/ids.rs crates/rupu-cli/src/output/mod.rs
cargo clippy -p rupu-cli --lib 2>&1 | tail -5
```
Expected: no warnings introduced by these files.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cli/src/output/ids.rs crates/rupu-cli/src/output/mod.rs
git commit -m "feat(cli): identifier compaction and fragment resolution

compact_id shortens for display; resolve accepts that exact form back,
plus full ids, prefixes, and suffixes. Ambiguity is a distinct variant
so callers must handle it rather than silently picking.

Keeps both ends of a ULID: the leading chars are a millisecond
timestamp and collide for records created close together, while the
trailing chars are random and actually disambiguate."
```

---

### Task 2: Relative timestamps

**Files:**
- Modify: `crates/rupu-cli/src/output/fmt.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `output::fmt::relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String`. Plan 2's entity tables consume it.

`now` is an explicit parameter rather than read from the clock, so tests are deterministic. Callers pass `Utc::now()`.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` in `crates/rupu-cli/src/output/fmt.rs`:

```rust
    use chrono::{TimeZone, Utc};

    fn at(secs_ago: i64) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
        let now = Utc.with_ymd_and_hms(2026, 7, 30, 12, 0, 0).unwrap();
        (now - chrono::Duration::seconds(secs_ago), now)
    }

    #[test]
    fn under_a_minute_is_just_now() {
        let (then, now) = at(0);
        assert_eq!(relative_time(then, now), "just now");
        let (then, now) = at(59);
        assert_eq!(relative_time(then, now), "just now");
    }

    #[test]
    fn minute_boundary() {
        let (then, now) = at(60);
        assert_eq!(relative_time(then, now), "1m ago");
        let (then, now) = at(3_540); // 59m
        assert_eq!(relative_time(then, now), "59m ago");
    }

    #[test]
    fn hour_boundary() {
        let (then, now) = at(3_600);
        assert_eq!(relative_time(then, now), "1h ago");
        let (then, now) = at(82_800); // 23h
        assert_eq!(relative_time(then, now), "23h ago");
    }

    #[test]
    fn day_boundary() {
        let (then, now) = at(86_400);
        assert_eq!(relative_time(then, now), "1d ago");
        let (then, now) = at(518_400); // 6d
        assert_eq!(relative_time(then, now), "6d ago");
    }

    #[test]
    fn week_boundary() {
        let (then, now) = at(604_800);
        assert_eq!(relative_time(then, now), "1w ago");
        let (then, now) = at(1_209_600);
        assert_eq!(relative_time(then, now), "2w ago");
    }

    #[test]
    fn beyond_a_year_falls_back_to_absolute_date() {
        // "63w ago" is not useful. Show the date instead.
        let (then, now) = at(31_536_000);
        assert_eq!(relative_time(then, now), "2025-07-30");
    }

    #[test]
    fn future_timestamps_clamp_to_just_now() {
        // Clock skew between the writer and this process is real and
        // must not render as a negative age.
        let (then, now) = at(-500);
        assert_eq!(relative_time(then, now), "just now");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib output::fmt`
Expected: FAIL — `relative_time` not found.

- [ ] **Step 3: Write the implementation**

Add to `crates/rupu-cli/src/output/fmt.rs`, after `format_cost_compact` and before `mod tests`:

```rust
use chrono::{DateTime, Utc};

/// Seconds in each grade. Beyond a year, relative ages stop being
/// useful and we show the date instead.
const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
const YEAR: i64 = 365 * DAY;

/// Render how long ago `then` was, relative to `now`.
///
/// Grades: `just now` under a minute, then minutes, hours, days, and
/// weeks. Past one year it falls back to an absolute `YYYY-MM-DD`.
///
/// `now` is a parameter rather than a clock read so this is
/// deterministic under test. Future timestamps clamp to `just now`,
/// because clock skew between whatever wrote the record and this
/// process must not surface as a negative age.
pub fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds();
    if secs < MINUTE {
        return "just now".to_string();
    }
    if secs < HOUR {
        return format!("{}m ago", secs / MINUTE);
    }
    if secs < DAY {
        return format!("{}h ago", secs / HOUR);
    }
    if secs < WEEK {
        return format!("{}d ago", secs / DAY);
    }
    if secs < YEAR {
        return format!("{}w ago", secs / WEEK);
    }
    then.format("%Y-%m-%d").to_string()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib output::fmt`
Expected: PASS — the four pre-existing tests plus seven new ones.

- [ ] **Step 5: Format, lint, and commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/fmt.rs
cargo clippy -p rupu-cli --lib 2>&1 | tail -5
git add crates/rupu-cli/src/output/fmt.rs
git commit -m "feat(cli): relative_time formatter

Grades just-now / m / h / d / w, falling back to an absolute date past
a year. Takes now as a parameter so tests are deterministic, and clamps
future timestamps so writer clock skew can't render a negative age."
```

---

### Task 3: Lifecycle status mapping, with glyph and colour deliberately split

**Files:**
- Modify: `crates/rupu-cli/src/output/tables.rs:42-71` (`status_color`)

**Interfaces:**
- Consumes: `palette::Status` (already exists, `palette.rs:233`).
- Produces: `output::tables::status_of(&str) -> Option<palette::Status>` and `output::tables::status_glyph(&str) -> Option<char>`. Plan 2's entity tables consume both.

**Critical finding — read before implementing.** The spec's §3 says colour and glyph both derive from the returned `Status`. That is wrong and must not be implemented as written. `Status::Waiting.color()` resolves to `palette.skipped` = `rgb(203, 213, 225)`, but `status_color` currently gives `pending` / `eligible` / `released` the `palette.dim` = `rgb(100, 116, 139)`. Deriving colour from `Status` would silently restyle three states from mid-slate to light-slate.

So: `status_of` drives the **glyph only**. The existing string→colour mapping is left exactly as-is. The two tables are intentionally separate, and the comment in the code must say why so nobody "simplifies" them back together.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` in `crates/rupu-cli/src/output/tables.rs`:

```rust
    #[test]
    fn status_of_maps_lifecycle_states() {
        use crate::output::palette::Status;
        assert_eq!(status_of("running"), Some(Status::Active));
        assert_eq!(status_of("claimed"), Some(Status::Active));
        assert_eq!(status_of("completed"), Some(Status::Complete));
        assert_eq!(status_of("complete"), Some(Status::Complete));
        assert_eq!(status_of("failed"), Some(Status::Failed));
        assert_eq!(status_of("blocked"), Some(Status::Failed));
        assert_eq!(status_of("awaiting_approval"), Some(Status::Awaiting));
        assert_eq!(status_of("await_human"), Some(Status::Awaiting));
        assert_eq!(status_of("paused"), Some(Status::Awaiting));
        assert_eq!(status_of("rejected"), Some(Status::Retrying));
        assert_eq!(status_of("retry_backoff"), Some(Status::Retrying));
        assert_eq!(status_of("pending"), Some(Status::Waiting));
        assert_eq!(status_of("idle"), Some(Status::Waiting));
        assert_eq!(status_of("skipped"), Some(Status::Skipped));
    }

    #[test]
    fn status_of_rejects_classification_values() {
        // Scope and issue state are not lifecycle positions. Giving
        // them a glyph would imply progress semantics they don't have.
        assert_eq!(status_of("project"), None);
        assert_eq!(status_of("global"), None);
        assert_eq!(status_of("open"), None);
        assert_eq!(status_of("closed"), None);
        assert_eq!(status_of("merged"), None);
        assert_eq!(status_of("critical"), None);
    }

    #[test]
    fn status_glyph_matches_the_palette_vocabulary() {
        assert_eq!(status_glyph("running"), Some('●'));
        assert_eq!(status_glyph("completed"), Some('✓'));
        assert_eq!(status_glyph("failed"), Some('✗'));
        assert_eq!(status_glyph("awaiting"), Some('⏸'));
        assert_eq!(status_glyph("rejected"), Some('↺'));
        assert_eq!(status_glyph("idle"), Some('○'));
        assert_eq!(status_glyph("skipped"), Some('⊘'));
        assert_eq!(status_glyph("project"), None);
    }

    #[test]
    fn pending_keeps_its_dim_colour_not_the_waiting_colour() {
        // Regression guard. Status::Waiting.color() is palette.skipped
        // (203,213,225) but pending has always rendered as palette.dim
        // (100,116,139). Deriving colour from Status would silently
        // restyle it. Glyph and colour are mapped separately on
        // purpose — see status_of's docstring.
        let prefs = prefs_color_always();
        let dim = crate::output::palette::active_palette().dim.into_table();
        assert_eq!(status_color("pending", &prefs), Some(dim));
        assert_eq!(status_color("eligible", &prefs), Some(dim));
        assert_eq!(status_color("released", &prefs), Some(dim));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib output::tables`
Expected: FAIL — `status_of` and `status_glyph` not found. The `pending` colour test compiles and passes already; it is a guard for later, not a driver.

- [ ] **Step 3: Write the implementation**

Insert into `crates/rupu-cli/src/output/tables.rs`, immediately above the existing `status_color`:

```rust
/// Map a status string to its lifecycle position, for glyph selection.
///
/// Returns `None` for classification values — scope (`project` /
/// `global`), issue state (`open` / `closed` / `merged`), and severity.
/// Those get colour but never a glyph: in this CLI a glyph always means
/// "where is this in its lifecycle", and attaching one to a
/// classification value would imply progress that doesn't exist.
///
/// Deliberately NOT used to pick colours. `Status::Waiting.color()` is
/// `palette.skipped` (203,213,225) while `pending` / `eligible` /
/// `released` have always rendered as `palette.dim` (100,116,139).
/// Routing colour through here would silently restyle them, so
/// `status_color` below keeps its own explicit mapping. Do not merge
/// the two.
pub fn status_of(status: &str) -> Option<crate::output::palette::Status> {
    use crate::output::palette::Status;
    Some(match status {
        "running" | "claimed" => Status::Active,
        "completed" | "complete" => Status::Complete,
        "failed" | "blocked" => Status::Failed,
        "awaiting_approval" | "awaiting" | "paused" | "await_human" | "await_external" => {
            Status::Awaiting
        }
        "rejected" | "retry_backoff" => Status::Retrying,
        "pending" | "eligible" | "released" | "idle" => Status::Waiting,
        "skipped" => Status::Skipped,
        _ => return None,
    })
}

/// The glyph for a lifecycle status, or `None` for classification
/// values and unknown strings.
pub fn status_glyph(status: &str) -> Option<char> {
    status_of(status).map(|s| s.glyph())
}
```

Leave `status_color` unchanged. Add this line to its docstring so the separation is discoverable from either side:

```rust
/// Foreground color for a status string. Returns `None` when colors
/// are disabled OR when the status doesn't match a known semantic
/// bucket — caller falls back to a plain `Cell::new(status)`.
///
/// Kept separate from `status_of` on purpose: the palette's `dim` and
/// `skipped` differ, so deriving these colours from `Status` would
/// change `pending` / `eligible` / `released`. See `status_of`.
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib output::tables`
Expected: PASS — all pre-existing tests plus four new ones.

- [ ] **Step 5: Format, lint, and commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/tables.rs
cargo clippy -p rupu-cli --lib 2>&1 | tail -5
git add crates/rupu-cli/src/output/tables.rs
git commit -m "feat(cli): status_of / status_glyph lifecycle mapping

Adopts the palette::Status vocabulary (already used by the session live
view) for table glyphs. Classification values return None so a glyph
always means lifecycle position.

Glyph and colour are mapped separately on purpose: palette.dim and
palette.skipped differ, so deriving colour from Status would silently
restyle pending/eligible/released. Regression test guards it."
```

---

### Task 4: Quiet table preset and snapshot harness

**Files:**
- Modify: `crates/rupu-cli/src/output/tables.rs:29-37` (`new_table`)
- Modify: `crates/rupu-cli/Cargo.toml` (add `insta.workspace = true` to `[dev-dependencies]`)

**Interfaces:**
- Consumes: nothing.
- Produces: the restyled `new_table()`, inherited by all 55 call sites. Plan 2 and Plan 3 build their profiles on it.

This is the single highest-leverage change in the plan: one function, and every table in the CLI stops drawing per-row separators.

**Verified during design** against comfy-table 7.2.2: `presets::NOTHING` plus `set_style(TableComponent::HeaderLines, '─')` alone renders **one continuous rule** across the full width. The segmented per-column rule additionally requires blanking `MiddleHeaderIntersections`. Do not omit that line.

- [ ] **Step 1: Add insta to dev-dependencies**

In `crates/rupu-cli/Cargo.toml`, under the existing `[dev-dependencies]`, add:

```toml
insta.workspace = true
```

No version — it is pinned at the workspace root (`Cargo.toml:94`).

- [ ] **Step 2: Write the failing snapshot test**

Append to `mod tests` in `crates/rupu-cli/src/output/tables.rs`:

```rust
    #[test]
    fn table_style_is_a_segmented_header_rule() {
        // Guards the house style for all 55 call sites. Colour is off,
        // so this asserts structure only.
        let mut t = new_table();
        t.set_header(vec!["SESSION", "AGENT", "STATUS", "UPDATED"]);
        t.add_row(vec![
            "ses_01KWA7HT…FG3M",
            "oracle-assessor",
            "✗ failed",
            "2w ago",
        ]);
        t.add_row(vec![
            "ses_01KWAA6X…FJM0",
            "oracle-assessor",
            "○ idle",
            "2w ago",
        ]);
        insta::assert_snapshot!(t.to_string());
    }

    #[test]
    fn table_draws_no_per_row_separators() {
        // The specific regression: UTF8_FULL drew ├╌╌┼╌╌┤ between every
        // row, doubling the height of every list in the CLI.
        let mut t = new_table();
        t.set_header(vec!["A", "B"]);
        t.add_row(vec!["1", "2"]);
        t.add_row(vec!["3", "4"]);
        let out = t.to_string();
        assert!(!out.contains('┼'), "found row-separator intersection");
        assert!(!out.contains('╌'), "found dashed row separator");
        assert!(!out.contains('│'), "found vertical border");
        // The header rule itself must survive.
        assert!(out.contains('─'), "header rule missing");
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib output::tables`
Expected: FAIL — `table_draws_no_per_row_separators` fails on `┼` / `│` because `UTF8_FULL` is still loaded, and the snapshot test fails as unreviewed.

- [ ] **Step 4: Change the preset**

Replace `new_table` in `crates/rupu-cli/src/output/tables.rs`:

```rust
/// Build a `comfy-table::Table` with rupu's default visual style: a
/// segmented rule under the header, no borders, and no per-row
/// separators.
///
/// The previous `UTF8_FULL` preset drew `├╌╌┼╌╌┤` between every row —
/// twenty rows rendered as forty-one lines.
///
/// `MiddleHeaderIntersections` must be blanked explicitly. Without it
/// the header rule renders as one continuous run across the full table
/// width, because `NOTHING` leaves the intersection as a space that
/// `HeaderLines` then overdraws. Verified against comfy-table 7.2.2.
pub fn new_table() -> Table {
    let mut t = Table::new();
    t.load_preset(presets::NOTHING);
    t.set_style(TableComponent::HeaderLines, '─');
    t.set_style(TableComponent::MiddleHeaderIntersections, ' ');
    t.set_content_arrangement(ContentArrangement::Dynamic);
    t
}
```

Extend the import at line 27 to include `TableComponent`:

```rust
use comfy_table::{presets, Cell, Color as TableColor, ContentArrangement, Table, TableComponent};
```

- [ ] **Step 5: Review and accept the snapshot**

```bash
cargo insta test -p rupu-cli --review
```

If `cargo-insta` is not installed, run `cargo test -p rupu-cli --lib output::tables` and then rename the generated `.snap.new` file to `.snap` after reading it. Expected content — a segmented rule, no verticals:

```
 SESSION             AGENT             STATUS     UPDATED
─────────────────── ───────────────── ────────── ─────────
 ses_01KWA7HT…FG3M   oracle-assessor   ✗ failed   2w ago
 ses_01KWAA6X…FJM0   oracle-assessor   ○ idle     2w ago
```

Do not accept a snapshot showing a single unbroken rule — that means `MiddleHeaderIntersections` was not blanked.

- [ ] **Step 6: Run the full crate test suite**

Run: `cargo test -p rupu-cli 2>&1 | tail -20`
Expected: PASS. This is the moment 55 call sites change output at once. Design-time verification found no test anywhere asserting on box-drawing characters, so nothing should break — but confirm rather than assume, and report any failure rather than adjusting the assertion to match.

- [ ] **Step 7: Eyeball the real binary**

```bash
cargo run -p rupu-cli -- session list 2>&1 | head -20
cargo run -p rupu-cli -- workflow list 2>&1 | head -10
```
Expected: header rule, no per-row separators, roughly half the previous vertical height. Cells still show full IDs and absolute timestamps — that is correct for this plan; cell-level adoption is Plan 2.

- [ ] **Step 8: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/tables.rs
git add crates/rupu-cli/src/output/tables.rs crates/rupu-cli/Cargo.toml \
        crates/rupu-cli/src/output/snapshots/
git commit -m "feat(cli): quiet table preset, segmented header rule

Replaces UTF8_FULL, which drew a dashed separator between every row and
doubled the height of every list. One change, inherited by all 55
new_table() call sites.

MiddleHeaderIntersections is blanked explicitly; without it the rule
renders as one continuous line. Adds insta and the first snapshot
coverage of table output in this crate."
```

---

### Task 5: Session identifier resolution

**Files:**
- Modify: `crates/rupu-cli/src/cmd/session.rs:243-246` (`SessionScope`), `:5897-5914` (delete `compact_session_run_id`), `:7256-7267` (`read_session`)

**Interfaces:**
- Consumes: `output::ids::{compact_id, resolve, Resolution}` from Task 1.
- Produces: `read_session` accepting any fragment form. All twelve existing callers inherit it unchanged.

Resolution goes inside `read_session` rather than at each call site, so the ~12 callers listed at `session.rs:1166, 1428, 1470, 1532, 1688, 1708, 1752, 2832, 2860, 2921` get it for free.

- [ ] **Step 1: Add `as_str` to SessionScope**

`SessionScope` has no string form, and ambiguity errors need one. At `crates/rupu-cli/src/cmd/session.rs:243`:

```rust
impl SessionScope {
    fn as_str(self) -> &'static str {
        match self {
            SessionScope::Active => "active",
            SessionScope::Archived => "archived",
        }
    }
}
```

Confirm `SessionScope` derives `Clone, Copy` — if not, add them, since `as_str(self)` takes by value and the scope is stored in a map below.

- [ ] **Step 2: Write the failing tests**

Append to `mod tests` in `crates/rupu-cli/src/cmd/session.rs`:

```rust
    #[test]
    fn read_session_resolves_a_compact_fragment() {
        let tmp = assert_fs::TempDir::new().expect("tempdir");
        let global = tmp.path();
        let id = "ses_01KWA7HTYEDX0ACG93ZW26FG3M";
        write_test_session(global, SessionScope::Active, id);

        let compact = crate::output::ids::compact_id(id);
        let (record, scope) = read_session(global, &compact).expect("resolves");
        assert_eq!(record.session_id, id);
        assert_eq!(scope, SessionScope::Active);
    }

    #[test]
    fn read_session_resolves_a_bare_suffix() {
        let tmp = assert_fs::TempDir::new().expect("tempdir");
        let global = tmp.path();
        let id = "ses_01KWA7HTYEDX0ACG93ZW26FG3M";
        write_test_session(global, SessionScope::Active, id);

        let (record, _) = read_session(global, "26FG3M").expect("resolves");
        assert_eq!(record.session_id, id);
    }

    #[test]
    fn read_session_finds_archived_sessions() {
        let tmp = assert_fs::TempDir::new().expect("tempdir");
        let global = tmp.path();
        let id = "ses_01KWA7HTYEDX0ACG93ZW26FG3M";
        write_test_session(global, SessionScope::Archived, id);

        let (record, scope) = read_session(global, "26FG3M").expect("resolves");
        assert_eq!(record.session_id, id);
        assert_eq!(scope, SessionScope::Archived);
    }

    #[test]
    fn read_session_errors_on_ambiguous_fragment_listing_scopes() {
        // A fragment matching one record per scope must not silently
        // prefer Active.
        let tmp = assert_fs::TempDir::new().expect("tempdir");
        let global = tmp.path();
        let active = "ses_01KWA7HTYEDX0ACG93ZW26FG3M";
        let archived = "ses_01KWA8RA3NE9J88X6DHSH76AC8";
        write_test_session(global, SessionScope::Active, active);
        write_test_session(global, SessionScope::Archived, archived);

        let err = read_session(global, "ses_01KWA").expect_err("ambiguous");
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "got: {msg}");
        assert!(msg.contains(active), "got: {msg}");
        assert!(msg.contains(archived), "got: {msg}");
        assert!(msg.contains("active"), "got: {msg}");
        assert!(msg.contains("archived"), "got: {msg}");
    }

    #[test]
    fn read_session_still_accepts_a_full_id() {
        let tmp = assert_fs::TempDir::new().expect("tempdir");
        let global = tmp.path();
        let id = "ses_01KWA7HTYEDX0ACG93ZW26FG3M";
        write_test_session(global, SessionScope::Active, id);

        let (record, _) = read_session(global, id).expect("resolves");
        assert_eq!(record.session_id, id);
    }

    #[test]
    fn read_session_reports_unknown_fragment() {
        let tmp = assert_fs::TempDir::new().expect("tempdir");
        let err = read_session(tmp.path(), "zzzzzz").expect_err("unknown");
        assert!(err.to_string().contains("unknown session"));
    }
```

Add the fixture helper alongside them. `test_session_record()` already exists in this test module (used by `session_route_detail_includes_repo_target_and_issue`); reuse it and override the id:

```rust
    fn write_test_session(global: &std::path::Path, scope: SessionScope, id: &str) {
        let mut record = test_session_record();
        record.session_id = id.to_string();
        let dir = session_dir(global, scope, id);
        std::fs::create_dir_all(&dir).expect("create session dir");
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_vec(&record).expect("serialize"),
        )
        .expect("write session.json");
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib cmd::session::tests::read_session`
Expected: FAIL — compact, suffix, and ambiguity cases all bail with "unknown session", because `read_session` still does an exact path lookup. The full-id case passes already.

- [ ] **Step 4: Write the implementation**

Replace `read_session` at `crates/rupu-cli/src/cmd/session.rs:7256`:

```rust
/// Load a session by identifier.
///
/// Accepts a full id, the compact `head…tail` form printed by tables,
/// a bare suffix, or an unambiguous prefix. Ambiguity is always an
/// error listing every candidate — never a silent pick, and never a
/// preference for `Active` over `Archived`.
fn read_session(global: &Path, fragment: &str) -> anyhow::Result<(SessionRecord, SessionScope)> {
    // Fast path: an exact id, no directory scan.
    for scope in [SessionScope::Active, SessionScope::Archived] {
        let path = session_record_path(global, scope, fragment);
        if path.is_file() {
            let bytes = fs::read(&path)
                .with_context(|| format!("read session record {}", path.display()))?;
            return Ok((serde_json::from_slice(&bytes)?, scope));
        }
    }

    let (id, scope) = resolve_session_fragment(global, fragment)?;
    let path = session_record_path(global, scope, &id);
    let bytes =
        fs::read(&path).with_context(|| format!("read session record {}", path.display()))?;
    Ok((serde_json::from_slice(&bytes)?, scope))
}

/// Session directory names in one scope. Reads directory entries only —
/// no `session.json` parsing — so resolution stays cheap as history
/// grows.
fn session_ids_in_scope(global: &Path, scope: SessionScope) -> anyhow::Result<Vec<String>> {
    let dir = match scope {
        SessionScope::Active => paths::sessions_dir(global),
        SessionScope::Archived => paths::archived_sessions_dir(global),
    };
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.path().join("session.json").is_file() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            out.push(name.to_string());
        }
    }
    Ok(out)
}

fn resolve_session_fragment(
    global: &Path,
    fragment: &str,
) -> anyhow::Result<(String, SessionScope)> {
    use crate::output::ids::{resolve, Resolution};
    use std::collections::HashMap;

    let mut candidates = Vec::new();
    let mut scope_of: HashMap<String, SessionScope> = HashMap::new();
    for scope in [SessionScope::Active, SessionScope::Archived] {
        for id in session_ids_in_scope(global, scope)? {
            scope_of.insert(id.clone(), scope);
            candidates.push(id);
        }
    }

    match resolve(&candidates, fragment) {
        Resolution::Unique(id) => {
            let scope = scope_of
                .get(&id)
                .copied()
                .expect("resolved id came from the candidate set");
            Ok((id, scope))
        }
        Resolution::NotFound => anyhow::bail!("unknown session: {fragment}"),
        Resolution::Ambiguous(matches) => {
            let mut msg = format!(
                "ambiguous session id — {} sessions match `{fragment}`",
                matches.len()
            );
            for id in &matches {
                let scope = scope_of.get(id).map(|s| s.as_str()).unwrap_or("unknown");
                msg.push_str(&format!("\n  {id}  {scope}"));
            }
            anyhow::bail!(msg)
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib cmd::session`
Expected: PASS — six new tests plus every pre-existing session test.

- [ ] **Step 6: Replace compact_session_run_id with compact_id**

Delete `compact_session_run_id` at `crates/rupu-cli/src/cmd/session.rs:5897-5914` and its test `compact_session_run_id_shortens_long_values` at `:7613`. Both are superseded by `output::ids::compact_id`, which has identical behaviour (12 head, 4 tail, 18-char threshold) and round-trip test coverage.

Update its call sites:

```bash
grep -n "compact_session_run_id" crates/rupu-cli/src/cmd/session.rs
```

Replace each with `crate::output::ids::compact_id`.

- [ ] **Step 7: Verify no behaviour changed and commit**

Run: `cargo test -p rupu-cli 2>&1 | tail -10`
Expected: PASS.

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/session.rs
cargo clippy -p rupu-cli --lib 2>&1 | tail -5
git add crates/rupu-cli/src/cmd/session.rs
git commit -m "feat(cli): resolve session ids from fragments

read_session accepted only exact ids, so any shortened id we displayed
would have been unusable for session send/resume. Resolution lives
inside read_session, so all twelve callers inherit it.

Ambiguity lists every candidate with its scope and errors — archived
sessions are never silently shadowed by active ones. Folds
compact_session_run_id into output::ids::compact_id."
```

---

### Task 6: Run identifier resolution

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (run-id argument handling)

**Interfaces:**
- Consumes: `output::ids::{resolve, Resolution}` from Task 1; `RunStore::list()` / `list_archived()` / `load()` (`rupu-orchestrator/src/runs.rs:1096, 1107, 897`).
- Produces: `resolve_run_id(&[String], &str) -> anyhow::Result<String>` (pure) and `resolve_run_fragment(&RunStore, &str) -> anyhow::Result<String>` (store glue), consumed by every `workflow` subcommand taking a run id.

`RunStore::load` takes an exact id, mirroring the session problem. Unlike sessions, `RunStore` already exposes `list()` and `list_archived()`, so candidates come from the store API rather than a directory walk.

**Two field-name traps, verified against the source — get these wrong and nothing compiles:**

- `RunRecord`'s identifier field is **`id`**, not `run_id` (`runs.rs:107`). Map with `.map(|r| r.id)`.
- `RunStore::create` takes a fully-populated `RunRecord` plus a YAML string (`runs.rs:830`), and the only helper that builds one (`sample_record`, `runs.rs:2567`) lives inside that crate's own `#[cfg(test)]` module, so it is **not reachable from `rupu-cli`**. Constructing a `RunRecord` here would mean spelling out ~30 fields.

That is why the logic is split: `resolve_run_id` is pure and takes a candidate list, so its tests need no store and no fixtures. `resolve_run_fragment` is thin glue over `list()` / `list_archived()` and needs no dedicated unit test.

- [ ] **Step 1: Locate the run-id call sites**

```bash
grep -n "RunStore::new\|\.load(\|run_id" crates/rupu-cli/src/cmd/workflow.rs | head -40
```

Record which subcommands take a user-supplied run id (`workflow show`, `approve`, `reject`, `cancel`, and similar). Each routes through the new helper.

- [ ] **Step 2: Write the failing tests**

Append to `mod tests` in `crates/rupu-cli/src/cmd/workflow.rs`:

```rust
    fn run_candidates() -> Vec<String> {
        vec![
            "run_01KYSMDNG84N9Z8XXHQZP3GKYJ".to_string(),
            "run_01KYSM3KE60KM2P2EDJR1V1BCP".to_string(),
            "run_01KYPASX18NYRER5NQPDWB2HZV".to_string(),
        ]
    }

    #[test]
    fn resolve_run_id_accepts_compact_form() {
        let c = run_candidates();
        let compact = crate::output::ids::compact_id(&c[0]);
        assert_eq!(compact, "run_01KYSMDN…GKYJ");
        assert_eq!(resolve_run_id(&c, &compact).expect("resolves"), c[0]);
    }

    #[test]
    fn resolve_run_id_accepts_bare_suffix() {
        let c = run_candidates();
        assert_eq!(resolve_run_id(&c, "P3GKYJ").expect("resolves"), c[0]);
    }

    #[test]
    fn resolve_run_id_accepts_full_id() {
        let c = run_candidates();
        assert_eq!(resolve_run_id(&c, &c[2]).expect("resolves"), c[2]);
    }

    #[test]
    fn resolve_run_id_errors_on_ambiguity_listing_candidates() {
        // ULIDs from the same era share a long prefix — the common case.
        let c = run_candidates();
        let err = resolve_run_id(&c, "run_01KYSM").expect_err("ambiguous");
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "got: {msg}");
        assert!(msg.contains(&c[0]), "got: {msg}");
        assert!(msg.contains(&c[1]), "got: {msg}");
        // The non-matching run must not be listed.
        assert!(!msg.contains(&c[2]), "got: {msg}");
    }

    #[test]
    fn resolve_run_id_reports_unknown() {
        let err = resolve_run_id(&run_candidates(), "zzzzzz").expect_err("unknown");
        assert!(err.to_string().contains("unknown run"));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib cmd::workflow::tests::resolve_run_id`
Expected: FAIL — `resolve_run_id` not found.

- [ ] **Step 4: Write the implementation**

Add to `crates/rupu-cli/src/cmd/workflow.rs`:

```rust
/// Resolve a run-id fragment against a candidate list.
///
/// Pure, so it is unit-testable without a `RunStore` — building a
/// `RunRecord` outside `rupu-orchestrator` would mean spelling out
/// every field, and that crate's `sample_record` helper is test-private.
///
/// Accepts a full id, the compact `head…tail` form printed by tables, a
/// bare suffix, or an unambiguous prefix.
fn resolve_run_id(candidates: &[String], fragment: &str) -> anyhow::Result<String> {
    use crate::output::ids::{resolve, Resolution};

    match resolve(candidates, fragment) {
        Resolution::Unique(id) => Ok(id),
        Resolution::NotFound => anyhow::bail!("unknown run: {fragment}"),
        Resolution::Ambiguous(matches) => {
            let mut msg = format!(
                "ambiguous run id — {} runs match `{fragment}`",
                matches.len()
            );
            for id in &matches {
                msg.push_str(&format!("\n  {id}"));
            }
            anyhow::bail!(msg)
        }
    }
}

/// Gather active and archived run ids and resolve `fragment` against
/// both, so an archived run is never shadowed by an active one.
fn resolve_run_fragment(
    store: &rupu_orchestrator::runs::RunStore,
    fragment: &str,
) -> anyhow::Result<String> {
    // NOTE: the field is `id`, not `run_id` (runs.rs:107).
    let mut candidates: Vec<String> = store
        .list()
        .context("list runs")?
        .into_iter()
        .map(|r| r.id)
        .collect();
    candidates.extend(
        store
            .list_archived()
            .context("list archived runs")?
            .into_iter()
            .map(|r| r.id),
    );
    resolve_run_id(&candidates, fragment)
}
```

- [ ] **Step 5: Route the subcommands through it**

For each site recorded in Step 1, resolve before loading. Use whatever
the clap argument is actually called on that subcommand — the field
name varies, so take it from the Step 1 survey rather than assuming
`args.run_id`:

```rust
let run_id = resolve_run_fragment(&store, &args.run_id)?;
let record = store.load(&run_id)?;
```

Resolve exactly once per command, at the point the argument enters, and
pass the resolved full id downstream. Do not re-resolve in helpers — a
second lookup would rescan the store and could report ambiguity for an
id that was already pinned.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib cmd::workflow`
Expected: PASS.

- [ ] **Step 7: Verify end to end against real data**

```bash
cargo run -p rupu-cli -- workflow runs | head -5
# Take a run id from the output, then confirm a 6-char suffix works:
cargo run -p rupu-cli -- workflow show <last-6-chars>
```
Expected: resolves to the full run.

- [ ] **Step 8: Format, lint, and commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/workflow.rs
cargo clippy -p rupu-cli --lib 2>&1 | tail -5
git add crates/rupu-cli/src/cmd/workflow.rs
git commit -m "feat(cli): resolve run ids from fragments

Mirrors the session resolver. RunStore::load takes an exact id, so
workflow subcommands taking a run id now resolve first. Searches active
and archived runs together; ambiguity errors with all candidates."
```

---

## Verification

Before opening the PR:

- [ ] `cargo test -p rupu-cli` passes in full.
- [ ] `cargo clippy -p rupu-cli --lib` introduces no new warnings.
- [ ] `cargo run -p rupu-cli -- session list` shows a header rule, no per-row separators, roughly half the previous height.
- [ ] A 6-character suffix from that listing resolves via `session show`.
- [ ] An ambiguous fragment (e.g. `ses_01KWA` against real data) errors, lists candidates with scopes, and exits non-zero.
- [ ] `cargo run -p rupu-cli -- workflow runs --output json | head -3` is unchanged from the pre-change output and carries full-length identifiers.

## Follow-on plans

- **Plan 2 — entity profile:** `EntityTable`, empty-column suppression over displayed rows with `--all-columns`, summary lines, `--absolute` flag, and cell-level adoption of `compact_id` / `relative_time` / `status_glyph` across the entity tables (session, run, workflow, cron, repos, issues, agent, models). Includes `workflow list` enrichment and its bounded last-run lookup.
- **Plan 3 — report profile:** `ReportTable` applied to `coverage.rs` (18 tables), `usage.rs`, `auth.rs`. No column suppression — a zero in a coverage grid is data, not absence.
