# Workflow attempt history

Date: 2026-11-06. Baseline: `d9728af6`. Status: implemented after the approved bounded API audit; 28 focused/adjacent tests pass; package Clippy passes with the documented local compatibility allowances. Pinned-toolchain CI remains an integration gate.

## Problem and approved scope

The workflow graph groups logical step/unit slots. The children view collapses continued handles. Neither exposes every recorded agent attempt. Add a read-only workflow attempt listing that preserves existing run identities, supports stable filtered pages, reads active/archive artifacts, and advertises host support. Preserve CP55578; deployment and provider acceptance remain with the integration/native owners.

## Options and decision

1. Re-read and slice the ledger on every request. Small implementation, but repeated work grows with the full population and append activity can shift pages. Rejected for the approved scaling requirement.
2. Capture a bounded immutable ledger snapshot in memory, index matching line ranges once, and page that buffer. This preserves exact evidence without adding durable run artifacts. Selected. The trade-offs are a capture-size limit, bounded cache retention, and explicit cursor expiry.
3. Add a persistent indexed database and durable cursors. This could support larger populations, but adds migration, lifecycle, invalidation, and recovery contracts outside this focused change. Deferred.

## Contract

`GET /api/runs/:id/attempts?host=local&step_id=...&unit_index=...&sub_id=...&limit=50&cursor=...`

The read-only MCP operator tool `runs.attempts` calls this same route. Its
required `id` and optional `host`, `step_id`, `unit_index`, `sub_id`, `limit`,
and `cursor` arguments use the handler's query schema. It has no execution
or external-network side effects; all ownership, filter, and cursor checks
remain in the API handler.

- Feature: `workflow.attempt_history.v1` in `GET /api/host/info.features`.
- Version 1 serves the directly connected host's active/archive RunStore. An explicit non-local host returns 501 and requires a direct connection to that owner. No remote probing, mirror fallback, or inferred ownership.
- Filters are optional, combined with AND, and must be repeated unchanged with a cursor. String filters contain 1–1024 bytes. The unit index is a nonnegative integer. Omitted optional filters match all values, including null.
- Limit defaults to 50 and clamps to 1–200. Changing the page size does not change the snapshot/filter identity.
- Order is ledger append order. Physical `ledger_line` is one-based and is an evidence locator within the snapshot, never a job/run identity or inferred attempt ordinal.
- Optional row fields serialize as explicit null. `agent_run_id`, `continued_from`, host and transcript path come directly from the ledger. A null host retains the ledger convention: local to the owning workflow host.
- A non-null row host is recorded executor provenance, not a guaranteed CP connector identifier or transport grant. Resolve it through the owner's known execution/connector mapping before fetching a transcript. Otherwise display the attempt with transcript access unavailable. Never read a returned server-side path on the native machine by assuming it is local.

```typescript
{
  schema_version: 1,
  workflow_run_id: string,
  owner_host_id: "local",
  snapshot: string,
  rows: [{
    ledger_line: number,
    step_id: string,
    unit_index: number | null,
    sub_id: string | null,
    agent_run_id: string,
    host: string | null,
    continued_from: string | null,
    transcript_path: string,
    started_at: string
  }],
  next_cursor: string | null,
  recorded_matching_total: number | null,
  evidence: {
    ledger: "present" | "missing",
    parsing: "clean" | "partial",
    execution_history_complete: "unknown",
    malformed_rows: number,
    unsupported_rows: number,
    oversized_rows: number,
    incomplete_tail_bytes: number,
    snapshot_bytes: number
  }
}
```

Counts describe the captured ledger before filtering, except `recorded_matching_total`, which counts valid matching rows. That total is exact for the snapshot and null when the ledger is absent. It is not the count of all executions that occurred. A present empty ledger has total zero. Unsupported record versions, malformed/invalid rows, oversized lines, and an incomplete trailing line make parsing partial. Blank lines are ignored. Tail bytes remain outside the complete-line snapshot boundary.

No final status or loop iteration is inferred. The start ledger has neither final outcome nor loop occurrence identity, and its writer is best effort. Missing ledger data cannot prove that no agent ran. Skipped/deterministic steps may have no rows. Session attachment does not imply dispatch ancestry. Graph selection and attempt inspection never change message routing.

## Snapshot and resource limits

The first request captures at most 16 MiB of ledger bytes and indexes matching line ranges. Lines over 64 KiB are reported and skipped. A larger ledger returns 413 `attempt_snapshot_too_large`; it is not silently truncated. The process cache has a 64 MiB accounting budget, at most 64 snapshots, and a 10-minute TTL. Cache entries include captured bytes and line-index capacity; response allocations and fixed process overhead are additional. Captures are serialized, so concurrent initial requests do not each allocate a full input buffer. Cached pages decode only their selected rows and perform metadata checks; they do not re-read ledger contents, events, or transcripts.

Cursors are MAC-authenticated with a process-local random key and bound through the snapshot to the owning global directory/RunStore, workflow run, and filters. They carry no caller-supplied filesystem paths. Cursor strings are opaque and at most 1024 bytes. Snapshot IDs identify API evidence snapshots only.

Appending does not alter an existing snapshot. Refresh without a cursor to see new attempts. File replacement, observed truncation, disappearance, or a same-length rewrite invalidates the snapshot. Archive rename within the owner preserves a snapshot when the file identity is unchanged. Captured bytes remain immutable even if an external edit followed by growth cannot be distinguished from append by metadata; this API assumes the runtime's append-only ledger contract and never claims a complete mutation audit.

## Errors and UI gating

- 400: invalid cursor/MAC, mismatched owner/run/filter, invalid filter, standalone agent root, or invalid/symlink/special-file source. A process restart changes the MAC key; refresh an old cursor after 400.
- 404: recorded local run absent.
- 409 `attempt_snapshot_expired`: expiry, eviction, or source invalidation. Refresh without cursor.
- 413 `attempt_snapshot_too_large`: ledger exceeds capture limit. Keep graph/current-state details available and expose the history limitation.
- 501: non-local host requested. Connect directly to the owner and check its capability.
- I/O and decoding failures of the run record remain errors, not empty history.

Enable attempt browsing only after the connected owner advertises the feature. Distinguish unsupported host, missing ledger, partial evidence, and zero recorded matches. This read-only capability requires no execution adapter, provider credential, or mutation grant. It grants no resume, dispatch, or messaging authority.

## Dependencies and build order

Existing AttemptRecord, active/archive recorded-store resolver, CP router and host feature discovery are sufficient. No writer/schema migration or native code is needed. Implement reader/cache and route, validate with disposable fixtures, then publish the DTO and commit for native integration. Native integration can proceed independently against this contract; the native owner owns provider acceptance and the integration owner coordinates runtime replacement. No extra agents are required for this focused backend slice.

## Validation and merge gates

Focused fixtures cover retries/restarts/continuation, sub-step and unit filters, exact nullable DTO, cursor MAC and owner binding, archive movement, missing/empty/malformed/torn/unsupported/oversized evidence, 12,000 attempts, append-stable pages, truncation/replacement, TTL/eviction, capability advertisement, and source containment. Compare recorded bytes before/after reads. Confirm later pages work without content-read permission and reuse the captured buffer. Run focused CP tests and Clippy, plus adjacent history/graph capability tests. Keep source fixture results separate from live provider/native acceptance.

Open limitations: capture-size/cache bounds and direct-owner-only v1 are deliberate; persistent indexing, remote aggregation, Agentiflow adaptation, and loop/outcome enrichment remain separate contracts.


## Validation record

Validated in the isolated `codex/workflow-attempt-history` worktree, with the shared target at `/private/tmp/rupu-session-harness-20261106/target`. Environment: `CARGO_BUILD_JOBS=4`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`; all Cargo commands use `--no-default-features --offline`.

- `cargo test -p rupu-cp --no-default-features --offline --lib attempt_history`: 12 passed, 0 failed; 0.90 seconds in the final focused run after compilation. Includes 12,000 attempt rows, exact filtered counts, append-stable pages, duplicate run IDs, archive movement, cursor binding/MAC, evidence gaps, containment, and read-only artifact preservation.
- The freshly built `target/debug/deps/rupu_cp-e868a41d30bfdfb4 api::graph::tests`: 13 passed, 0 failed. Cargo reproduction: `cargo test -p rupu-cp --no-default-features --offline --lib api::graph::tests`.
- The same test binary with `workflow_history`: 3 passed, 0 failed. Cargo reproduction: `cargo test -p rupu-cp --no-default-features --offline --lib workflow_history`.
- Scoped `rustfmt --edition 2021 --config skip_children=true --check` and `git diff --check`: passed.
- Package Clippy passed: `cargo clippy -p rupu-cp --no-deps --no-default-features --offline --lib --tests -- -D warnings -A clippy::double_must_use -A clippy::result_large_err` (exit 0).
- Strict Clippy without those allowances did not pass on this host's Homebrew Rust 1.99. The initial dependency check stopped at unchanged `rupu-netflow/src/sink.rs` async-trait expansions. The package-only run reported 90 `double_must_use` diagnostics in unchanged async-trait expansions and one `result_large_err` in unchanged `api/findings.rs:890`. It also caught one new test-style issue, corrected from permission literal `0` to the equivalent `0o0`. No runtime behavior changed. The repository and CI pin Rust 1.95, unavailable locally; normal strict pinned-toolchain CI remains required. The allowances are command-line-only; no repository lint policy or unrelated source was changed.

The first focused pass found two fixture assumptions: RunStore creation already writes an empty attempt ledger. The legacy-missing and symlink fixtures now explicitly remove that initial file. The corrected focused run passes. No product behavior was changed to accommodate those assumptions.

No provider call, live data mutation, CP55578 restart, deployment, native edit, or cross-chat send was performed by this change. Native/provider acceptance remains separate from these backend fixture results. The integration touches the shared CP route module, router, and host feature list; retain other parallel work's routes and feature flags when integrating.
