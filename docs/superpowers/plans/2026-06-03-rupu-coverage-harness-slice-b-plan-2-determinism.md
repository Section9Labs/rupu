# Coverage Harness Slice B — Plan 2: Level-1 Determinism Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Guarantee that everything the coverage harness controls about what the model sees — the rendered concern catalog, the catalog snapshot on disk, and the live `coverage_remaining` file list — is byte-stable and input-order-independent, locked by contract tests, so the model becomes the only source of run-to-run variance.

**Architecture:** The coverage prompt path is *already* deterministic — `flatten` builds a `BTreeMap<id, Concern>`, `render_prompt_section` iterates `catalog.concerns` in that sorted order, `coverage_remaining` iterates sorted concerns × path-sorted `file_views`, and `write_snapshot` serializes a `FlatCatalog` whose `concerns` is an ordered `Vec` and whose `sources`/`render_modes` are `BTreeMap`s. The audit (Plan B-2 §determinism) is therefore "prove and lock," not "fix": this plan adds (1) order-independence + byte-stability contract tests, (2) an `insta` snapshot that pins the exact rendered format, and (3) one defensive explicit sort in `coverage_remaining` so its ordering guarantee is *local* (not dependent on two helper functions' implicit ordering). No change to prompt content or behavior.

**Tech Stack:** Rust 2021, `insta` (workspace dev-dep, `yaml` feature), `serde_yaml`, `BTreeMap`/`BTreeSet`. Tests use `tempfile`.

**Spec:** `docs/superpowers/specs/2026-06-02-rupu-coverage-harness-slice-b-design.md` (Plan B-2 section).

**Depends on:** nothing (independent of B-1). B-3's `rerun` relies on this plan's guarantee that a replay renders the same prompt.

---

## Audit finding (context for the implementer — no code change required for this part)

Before writing tests, the prompt-construction path was audited for the three classic nondeterminism sources, and is **clean**:

- **Timestamps / wall-clock:** none in `catalog/render.rs`, `catalog/mode_selection.rs`, or the coverage block of `rupu-agent/src/runner.rs` (the catalog is flattened, snapshotted, and rendered with no time interpolation).
- **RNG:** none.
- **`HashMap` iteration:** the catalog path uses `BTreeMap` exclusively (`flatten`, `mode_selection`); no `HashMap` feeds a rendered string.
- **`read_dir` / filesystem ordering:** the rendered catalog comes from the in-memory `FlatCatalog`, not a directory walk.

Task 4 adds a guard test that greps the path to keep it clean. The substantive work is the contract tests (Tasks 1-3).

---

## File Structure

**`rupu-coverage` crate:**
- `Cargo.toml` *(modify)* — add `insta.workspace = true` to `[dev-dependencies]`.
- `tests/determinism.rs` *(create)* — the contract: render order-independence + byte-stability, catalog-snapshot order-independence, and an `insta` snapshot of a rendered curated catalog. Integration test (uses only the crate's public API), mirroring the existing `tests/cwe_index_mode_end_to_end.rs`.
- `src/tools/coverage_remaining.rs` *(modify)* — add an explicit final `sort_by` on the output so its `(concern_id, file_path)` ordering is a local guarantee; add an order-independence test.
- `tests/snapshots/` *(created by `insta` on first run)* — the accepted `.snap` file, committed.

---

## Task 1: Render determinism contract — order-independence + byte-stability

**Files:**
- Modify: `crates/rupu-coverage/Cargo.toml` (`[dev-dependencies]`)
- Create: `crates/rupu-coverage/tests/determinism.rs`

- [ ] **Step 1: Add the `insta` dev-dependency**

In `crates/rupu-coverage/Cargo.toml`, under `[dev-dependencies]` (which currently has `tempfile` and `tokio`), add:

```toml
insta.workspace = true
```

(`insta` is already pinned in the root `Cargo.toml` workspace deps with the `yaml` feature — do not add a version here.)

- [ ] **Step 2: Write the render contract tests**

Create `crates/rupu-coverage/tests/determinism.rs`:

```rust
//! Level-1 determinism contract (Slice B Plan 2).
//!
//! Locks the guarantee that everything the coverage harness controls
//! about the model's view is byte-stable and independent of the order
//! catalog inputs are declared in. If any of these fail, prompt
//! construction has become nondeterministic and run-to-run diffs would
//! conflate harness variance with model variance.

use rupu_coverage::{
    flatten, render_prompt_section, write_snapshot, CatalogMode, ConcernsBlock, ConcernsEntry,
    IncludeDirective, DEFAULT_FULL_MODE_THRESHOLD,
};

/// A `ConcernsBlock` that includes `a` then `b`.
fn block_two_includes(a: &str, b: &str) -> ConcernsBlock {
    ConcernsBlock {
        entries: vec![
            ConcernsEntry::Include(IncludeDirective {
                include: a.to_string(),
                overrides: vec![],
                mode: CatalogMode::Auto,
                filter: None,
            }),
            ConcernsEntry::Include(IncludeDirective {
                include: b.to_string(),
                overrides: vec![],
                mode: CatalogMode::Auto,
                filter: None,
            }),
        ],
    }
}

#[test]
fn render_is_byte_stable_across_repeated_calls() {
    let catalog = flatten(&block_two_includes("stride", "secrets-in-source")).unwrap();
    let first = render_prompt_section(&catalog, DEFAULT_FULL_MODE_THRESHOLD);
    let second = render_prompt_section(&catalog, DEFAULT_FULL_MODE_THRESHOLD);
    assert_eq!(first, second, "render_prompt_section must be a pure function");
}

#[test]
fn render_is_independent_of_include_order() {
    // The SAME logical catalog, declared in two different include orders,
    // must render to identical bytes — proving concern ordering is
    // canonical (sorted by id), not input-order-dependent.
    let ab = flatten(&block_two_includes("stride", "secrets-in-source")).unwrap();
    let ba = flatten(&block_two_includes("secrets-in-source", "stride")).unwrap();
    let rendered_ab = render_prompt_section(&ab, DEFAULT_FULL_MODE_THRESHOLD);
    let rendered_ba = render_prompt_section(&ba, DEFAULT_FULL_MODE_THRESHOLD);
    assert_eq!(
        rendered_ab, rendered_ba,
        "render must not depend on the order includes are declared in"
    );
}

#[test]
fn catalog_snapshot_is_independent_of_include_order() {
    // The persisted catalog.yaml must also be order-independent so a
    // re-run (B-3) reconstructs an identical effective catalog.
    let ab = flatten(&block_two_includes("stride", "secrets-in-source")).unwrap();
    let ba = flatten(&block_two_includes("secrets-in-source", "stride")).unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    let path_ab = tmp.path().join("ab/catalog.yaml");
    let path_ba = tmp.path().join("ba/catalog.yaml");
    write_snapshot(&ab, &path_ab).unwrap();
    write_snapshot(&ba, &path_ba).unwrap();

    let yaml_ab = std::fs::read_to_string(&path_ab).unwrap();
    let yaml_ba = std::fs::read_to_string(&path_ba).unwrap();
    assert_eq!(
        yaml_ab, yaml_ba,
        "catalog snapshot YAML must not depend on include order"
    );
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p rupu-coverage --test determinism render_ catalog_snapshot_`
Expected: 3 PASS. If `render_is_independent_of_include_order` or `catalog_snapshot_is_independent_of_include_order` FAILS, that is a real latent nondeterminism bug — STOP and report it (do not "fix" the test to match buggy output). It should pass given the current `BTreeMap`-based flatten.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage/Cargo.toml crates/rupu-coverage/tests/determinism.rs
git commit -m "test(coverage): render + snapshot determinism contract (order-independence)"
```

---

## Task 2: Lock the exact rendered format with an `insta` snapshot

**Files:**
- Modify: `crates/rupu-coverage/tests/determinism.rs`
- Create (by insta): `crates/rupu-coverage/tests/snapshots/determinism__stride_catalog_render.snap`

- [ ] **Step 1: Write the snapshot test**

Append to `crates/rupu-coverage/tests/determinism.rs`:

```rust
#[test]
fn stride_catalog_render_matches_snapshot() {
    // Pins the exact rendered bytes for a curated catalog. A diff here
    // means the prompt format changed — intentional changes are accepted
    // with `cargo insta review`; unintended ones are caught in review.
    let catalog = flatten(&ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto,
            filter: None,
        })],
    })
    .unwrap();
    let rendered = render_prompt_section(&catalog, DEFAULT_FULL_MODE_THRESHOLD);
    insta::assert_snapshot!("stride_catalog_render", rendered);
}
```

- [ ] **Step 2: Generate and accept the snapshot**

Run: `cargo test -p rupu-coverage --test determinism stride_catalog_render_matches_snapshot`
Expected: the test FAILS the first time (no stored snapshot) and `insta` writes a `.snap.new` file. Inspect it to confirm it contains the rendered STRIDE catalog (a `## Coverage Catalog` heading, the intro text, and `### stride:spoofing` etc. sections). If correct, accept it:

```bash
cargo insta accept --package rupu-coverage
```

(If `cargo insta` is not installed, accept manually by renaming the generated file: `mv crates/rupu-coverage/tests/snapshots/determinism__stride_catalog_render.snap.new crates/rupu-coverage/tests/snapshots/determinism__stride_catalog_render.snap`.)

- [ ] **Step 3: Re-run to verify the snapshot is now committed-and-matching**

Run: `cargo test -p rupu-coverage --test determinism stride_catalog_render_matches_snapshot`
Expected: PASS (snapshot matches).

- [ ] **Step 4: Commit (including the `.snap` file)**

```bash
git add crates/rupu-coverage/tests/determinism.rs crates/rupu-coverage/tests/snapshots/
git commit -m "test(coverage): insta snapshot pins rendered STRIDE catalog format"
```

---

## Task 3: Make `coverage_remaining` ordering a local guarantee

**Files:**
- Modify: `crates/rupu-coverage/src/tools/coverage_remaining.rs`
- Test: same file (`#[cfg(test)] mod tests`)

**Context:** `coverage_remaining` today produces output ordered by `(concern_id, file_path)` *implicitly* — because it iterates `catalog.concerns` (sorted by id) in the outer loop and `file_views` (sorted by path) in the inner loop. That is correct but fragile: a future change to either helper's ordering would silently change the file list the model sees. This task adds an explicit final sort so the guarantee is local to this function, and a test that proves order-independence against an unsorted input ledger.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/rupu-coverage/src/tools/coverage_remaining.rs`. (The module already imports `flatten`, `ConcernsBlock`, `ConcernsEntry`, `IncludeDirective`, `Attribution`, `FileTouchEvent`, `Surface`, `Utc`, and has a `write_events` helper and an `attribution()` helper — reuse them. Add a `read_event_at` helper if one is not already present.)

```rust
    #[test]
    fn remaining_output_is_sorted_by_concern_then_path() {
        // Feed file events whose paths are NOT in sorted order. The output
        // must still come back ordered by (concern_id, file_path), and be
        // identical across two calls — the determinism contract for the
        // live file list the model sees.
        let catalog = flatten(&ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        })
        .unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(dir.path(), "tgt");
        // Events in deliberately reversed path order.
        let events = vec![
            FileTouchEvent::Read {
                path: "src/zeta.rs".to_string(),
                line_range: [1, 10],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            },
            FileTouchEvent::Read {
                path: "src/alpha.rs".to_string(),
                line_range: [1, 10],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            },
        ];
        write_events(&paths, &events);

        let out1 = coverage_remaining(&paths, &catalog, CoverageRemainingInput::default()).unwrap();
        let out2 = coverage_remaining(&paths, &catalog, CoverageRemainingInput::default()).unwrap();
        assert_eq!(out1.len(), out2.len());

        // Identical across calls.
        let key = |r: &RemainingItem| (r.concern_id.clone(), r.file_path.clone());
        let keys1: Vec<_> = out1.iter().map(key).collect();
        let keys2: Vec<_> = out2.iter().map(key).collect();
        assert_eq!(keys1, keys2, "remaining output must be stable across calls");

        // Globally sorted by (concern_id, file_path).
        let mut sorted = keys1.clone();
        sorted.sort();
        assert_eq!(keys1, sorted, "remaining output must be sorted by (concern_id, file_path)");
    }
```

- [ ] **Step 2: Run the test to confirm current behaviour**

Run: `cargo test -p rupu-coverage --lib tools::coverage_remaining::tests::remaining_output_is_sorted_by_concern_then_path`
Expected: This likely PASSES already (ordering is implicit today). That is fine — the test documents and pins the contract. Proceed to Step 3 to make the guarantee explicit so it survives future helper changes.

- [ ] **Step 3: Add the explicit sort**

In `crates/rupu-coverage/src/tools/coverage_remaining.rs`, at the end of `coverage_remaining`, replace the final `Ok(out)` with an explicit sort first:

```rust
    out.sort_by(|a, b| {
        a.concern_id
            .cmp(&b.concern_id)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    Ok(out)
}
```

- [ ] **Step 4: Run the test again to confirm it still passes**

Run: `cargo test -p rupu-coverage --lib tools::coverage_remaining::tests::remaining_output_is_sorted_by_concern_then_path`
Expected: PASS. Also run the full module to confirm no regression: `cargo test -p rupu-coverage --lib tools::coverage_remaining`.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/tools/coverage_remaining.rs
git commit -m "feat(coverage): explicit (concern, path) sort on coverage_remaining output"
```

---

## Task 4: Nondeterminism guard + full verification

**Files:** none (verification only) except a possible small assertion test.

- [ ] **Step 1: Grep the prompt path for nondeterminism sources**

Run:
```bash
rg -n "SystemTime|Instant|Utc::now|rand::|HashMap|read_dir" crates/rupu-coverage/src/catalog/ crates/rupu-coverage/src/tools/coverage_remaining.rs
```
Expected: **no matches** in `catalog/render.rs`, `catalog/mode_selection.rs`, or `coverage_remaining.rs`. (`Utc::now` may legitimately appear in *test* modules — that is fine; it must not appear in the non-test rendering/partition/remaining code paths.) If a match appears in non-test rendering code, STOP and report it — the contract tests in Tasks 1-3 would not have caught a nondeterministic value that varies *between processes* (e.g. a timestamp), so this grep is the backstop.

- [ ] **Step 2: Run the whole determinism + coverage suite**

Run: `cargo test -p rupu-coverage`
Expected: all PASS (the new `determinism` integration tests + existing 86 lib tests + other integration tests).

- [ ] **Step 3: Clippy + build**

Run: `cargo clippy -p rupu-coverage --lib --tests` and `cargo build -p rupu-coverage`
Expected: clean (no new warnings; `#![deny(clippy::all)]` is on).

- [ ] **Step 4: Format check on touched files**

Run: `cargo fmt -p rupu-coverage -- --check`
Expected: the new `tests/determinism.rs` and the `coverage_remaining.rs` sort are clean. If only new code shows diffs, run `rustfmt --edition 2021` on those specific files and re-stage. (The repo's `main` has pre-existing rustfmt drift under the pinned toolchain 1.88 / rustfmt 1.9.0 in other files — do NOT reformat files this plan didn't change.)

- [ ] **Step 5: Final commit if fmt changed anything**

```bash
git add -A
git commit -m "style(coverage): rustfmt determinism additions" || echo "nothing to format"
```

---

## Self-Review (completed by plan author)

**1. Spec coverage (B-2 section of the Slice B spec):**
- Concern ordering already deterministic → proven by `render_is_independent_of_include_order` (Task 1) + the `insta` snapshot (Task 2). ✅
- File ordering (the `coverage_remaining` file list) sorted by path → made an explicit local guarantee + tested (Task 3). ✅
- Nondeterministic interpolation (timestamps / RNG / HashMap) audited → confirmed clean, backstopped by the grep guard (Task 4). ✅
- "Guarantee mechanism: a determinism test that assembles the prompt section twice and asserts byte-equality" → `render_is_byte_stable_across_repeated_calls` + order-independence + the snapshot (Tasks 1-2). ✅
- "Explicit boundary: Level 1 makes prompt *construction* deterministic; model output still varies" → no behavioural change, no over-claim; this plan only adds tests + one defensive sort. ✅
- Catalog snapshot determinism (needed so B-3 replay reconstructs an identical effective catalog) → `catalog_snapshot_is_independent_of_include_order` (Task 1). ✅

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; every command has an expected result and a STOP-and-report instruction for the failure case. ✅

**3. Type consistency:** `ConcernsBlock` / `ConcernsEntry::Include` / `IncludeDirective { include, overrides, mode, filter }` / `CatalogMode::Auto` / `flatten` / `render_prompt_section` / `write_snapshot` / `CoveragePaths` / `DEFAULT_FULL_MODE_THRESHOLD` are all real crate-root exports used consistently (verified against `src/lib.rs`). `RemainingItem` fields `concern_id` / `file_path` match `coverage_remaining.rs`. ✅

**Out of scope for this plan (later B-plan):** run manifest + `rerun` (Plan B-3). Sampling-parameter control (`temperature` / `seed`) is the deferred "Level 2," not this plan.
