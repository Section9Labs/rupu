# rupu Autoflow Observability — Plan 1

**Date:** 2026-05-11
**Status:** Implemented
**Companion docs:** [Autoflow observability design](../specs/2026-05-11-rupu-autoflow-observability-design.md), [Autoflow Plan 2](../specs/2026-05-09-rupu-autoflow-plan-2-portable-runtime-design.md), [Tracker-native ownership design](../specs/2026-05-10-rupu-tracker-native-autoflow-ownership-design.md)

---

## Goal

Make autoflow execution observable enough to operate confidently through the CLI by adding:

- durable cycle/event history
- `rupu autoflow monitor`
- `rupu autoflow history`
- better drilldown from autoflow state into existing run/watch views

---

## Scope

This plan covers:

- a durable `AutoflowCycleRecord` / event history layer
- live `rupu autoflow monitor`
- durable `rupu autoflow history`
- `explain` / `claims` / `status` enhancements that use the new history
- consistent `table` / `json` / `csv` output

This plan does not cover:

- a brand-new TUI
- cloud control plane work
- workflow DSL changes
- changes to run execution semantics

---

## Why `monitor` is not PR 1

`monitor` depends on durable cycle history.

If we start by printing live worker activity directly:

- `tick` has no durable view
- `serve` becomes terminal-noise-only
- history/export is blocked
- future SaaS cannot reuse the model

So the correct order is:

1. cycle/event history
2. monitor

---

## PR 1 — Cycle/event history foundation

- add `AutoflowCycleRecord`
- add durable storage under `~/.rupu/autoflows/history/`
- emit cycle events from both `tick` and `serve`
- add event kinds for:
  - wake consumed / skipped
  - claim acquired / reused / released / takeover
  - run launched / resumed / completed / failed
  - awaiting human / awaiting external
  - retry scheduled
  - dispatch queued
  - cleanup performed
- keep execution behavior unchanged

**Acceptance**
- every `tick` pass emits a persisted cycle record
- every `serve` cycle emits a persisted cycle record
- old runtime behavior does not change
- existing autoflow tests still pass

---

## PR 2 — `rupu autoflow monitor`

- add `rupu autoflow monitor`
- build its view from:
  - latest cycle records
  - current claim store
  - current wake store
  - worker metadata
- support:
  - `--repo`
  - `--worker`
  - `--watch`
  - `--format json`
- keep the default UI table-oriented and palette-consistent

**Acceptance**
- operator can see current workers, claims, wakes, and recent activity in one command
- output is useful for both repo-backed and tracker-native claims
- no duplicate transcript rendering is introduced

---

## PR 3 — `rupu autoflow history`

- add durable history reader
- add `rupu autoflow history`
- support filtering by:
  - repo
  - source
  - issue
  - worker
  - event kind
  - limit
- support:
  - `table`
  - `json`
  - `csv`
  - optional `--watch`

**Acceptance**
- operator can answer “what happened recently?”
- one issue’s history is easy to inspect
- exports are stable enough for future SaaS ingestion

---

## PR 4 — Explain / claims / watch handoff polish

- extend `rupu autoflow explain` with recent cycle events
- add a watch handoff hint when `last_run_id` exists
- extend `rupu autoflow claims` with:
  - last cycle timestamp
  - last event kind
  - last run id
- keep `status` summary-first, but optionally include “recent change” data

**Acceptance**
- operator can move from control-plane view to execution drilldown quickly
- `explain` tells a coherent story, not just raw fields

---

## PR 5 — Optional watch-mode / TUI follow-on

- evaluate whether `monitor --watch` is sufficient
- if not, add a lightweight TUI over the same history model
- do not build this PR unless the first four slices show a real CLI ceiling

**Acceptance**
- only proceed if operator testing shows the table/watch UI is insufficient

---

## Validation strategy

For each PR:

1. start with focused tests around the new store/command
2. rerun affected autoflow runtime tests
3. rerun CLI integration tests for changed commands

Minimum final validation for the whole plan:

- `cargo test -p rupu-cli --lib cmd::autoflow::tests`
- `cargo test -p rupu-cli --test cli_autoflow`
- `cargo test -p rupu-runtime`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`

---

## Main risks

### 1. Event log bloat

Mitigation:

- append-only files by day
- small bounded event payloads
- defer compaction/indexing until needed

### 2. UI drift from existing CLI design

Mitigation:

- reuse the shared table/palette layer
- keep `monitor` and `history` as structured report commands

### 3. Duplicated run UI

Mitigation:

- treat `rupu watch <run_id>` as the execution drilldown
- do not render transcripts inline in monitor/history

### 4. Too much state in `status`

Mitigation:

- keep `status` summary-first
- place detailed chronology in `history` and `explain`

---

## Exit criteria

Plan 1 is complete when:

- `tick` and `serve` emit durable cycle records
- `rupu autoflow monitor` gives a coherent live operator view
- `rupu autoflow history` gives a coherent durable operator history
- `explain` and `claims` can point operators into `rupu watch <run_id>`
- repo-backed and tracker-native autoflows are equally visible through the CLI

## Closeout

Shipped in sequence:

- PR 1: durable cycle/event history foundation
- PR 2: `rupu autoflow monitor`
- PR 3: `rupu autoflow history`
- PR 4: explain / claims / status drilldown polish and `rupu watch <run_id>` handoff

The optional watch-mode / TUI follow-on is deferred. `rupu autoflow monitor --watch` is sufficient for the current CLI operator loop, so there is no active Plan 1 PR 5.
