# Evidence-block artifacts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Files referenced by `image` / `hexdump` / `pcap_ref` evidence blocks are verified and stored like `report.artifacts`, served by the CP (images inline), reach the coordinator from remote units, and render on the web finding page together with every other block kind.

**Architecture:**
- **Store at report time.** `report_finding` runs each block's file through `ArtifactStore::ingest`, sharing the finding's ingest budget with `report.artifacts`, and replaces the block's ref with the verified one. Block files stay in their blocks; they are not copied into `report.artifacts`.
- **One iterator for consumers.** `FindingReport::artifact_refs()` (artifacts, then block artifacts) replaces `report.artifacts` in the three places that mean "any file this report references":
  - the CP endpoint's sha lookup;
  - Plan A's `mark_external`;
  - Plan B's bucket `referenced_artifacts`.
- **Inline images.** `artifact_response` serves PNG/JPEG/GIF/WebP inline as `image/*`, chosen by magic bytes.
- **Web.** A new `EvidenceBlocks` component renders `report.blocks` on the finding page.

**Tech Stack:** Rust 2021 (rupu-coverage, rupu-cp, rupu-cli), axum 0.7, tokio; React + TypeScript + vitest (`crates/rupu-cp/web`).

**Spec:** `docs/superpowers/specs/2026-10-02-rupu-evidence-block-artifacts-design.md`

## Global Constraints

- Workspace deps only; `#![deny(clippy::all)]`; no `unsafe`. CI clippy 1.95 also flags `!x.is_some_and(..)` (use `is_none_or`) and a match arm whose body is only an `if` (use a guard). Clippy gate: `cargo clippy -p <crate> --all-targets -- -D warnings -A clippy::question_mark`.
- Never package-wide `cargo fmt`; `rustfmt --edition 2021 --check <file>` first, never on `lib.rs`/`mod.rs`. Never `git stash`.
- Integration tests live in `crates/<c>/tests/it/` (one binary per crate); run with `cargo test -p <c> --test it <file>::`.
- Public repo: invent every fixture value.
- Block artifact field path in errors: `report.blocks[{i}].artifact.path`.
- Inline image types, decided ONLY by magic bytes of the served file (never extension or agent input):
  - PNG `89 50 4E 47 0D 0A 1A 0A` → `image/png`
  - JPEG `FF D8 FF` → `image/jpeg`
  - GIF `GIF87a` / `GIF89a` → `image/gif`
  - WebP `RIFF????WEBP` → `image/webp`
  - Everything else is unchanged. Every artifact response keeps `X-Content-Type-Options: nosniff` + `Content-Security-Policy: sandbox`.
- Web: a block artifact with a `host` is never fetched on render (only on a click). A block artifact without `host` may load on render.

---

### Task 1: Verify and store block files (`rupu-coverage`)

**Files:**
- Modify: `crates/rupu-coverage/src/report/types.rs` (`impl EvidenceBlock`, `impl FindingReport`)
- Modify: `crates/rupu-coverage/src/report/validate.rs` (next to the `report.artifacts` path loop, ~:271)
- Modify: `crates/rupu-coverage/src/tools/report_finding.rs` (after the `report.artifacts` ingest, ~:167-176)
- Modify: `crates/rupu-coverage/src/ledger/ingest.rs` (`mark_external`, ~:147)
- Tests: unit tests in those files

**Interfaces:**
- Produces:
  - `EvidenceBlock::artifact(&self) -> Option<&ArtifactRef>`
  - `EvidenceBlock::artifact_mut(&mut self) -> Option<&mut ArtifactRef>`
  - `FindingReport::artifact_refs(&self) -> impl Iterator<Item = &ArtifactRef>`: `artifacts` first, then block artifacts in block order.
  - `FindingReport::artifact_refs_mut(&mut self) -> impl Iterator<Item = &mut ArtifactRef>`

- [ ] **Step 1: Write the failing tests.** Add tests in `types.rs`, `validate.rs`, `report_finding.rs` and `ingest.rs`, following each file's existing test helpers (e.g. `report_finding.rs` has `full_profile_ingests_artifacts_and_hashes_claim_files` ~:995 — copy its setup):
  - **`types.rs` `artifact_refs_lists_artifacts_then_block_artifacts`:**
    - A report with one `artifacts` entry `a.bin` and blocks `[Text, Image{x.png}, PcapRef{c.pcap}]`.
    - `artifact_refs()` paths are `["a.bin", "x.png", "c.pcap"]`.
    - `artifact_refs_mut()` reaches the same three.
  - **`validate.rs` `a_block_artifact_path_follows_the_artifact_path_rules`:**
    - An `Image` block with path `../x.png` gives an error at `report.blocks[1].artifact.path` (block index 1, behind a `Text` block).
    - An image path of `.` names the workspace root and is also an error at that path.
    - A valid relative path gives no error.
  - **`report_finding.rs` `an_image_block_file_is_verified_and_stored_but_not_listed_as_a_poc`:**
    - Write `shots/login.png` (invented bytes, e.g. the PNG magic + `b"fake"`) in the workspace.
    - Report with `artifacts: []` and one `Image` block whose ref has a BOGUS `sha256` (`"0".repeat(64)`), `size: 1`, `kind: Some(Text)`.
    - After `report_finding`, the stored record's block ref has:
      - the real sha;
      - the real size;
      - `kind: Some(Binary)`;
      - `stored: Some(Copied)`.
    - The blob exists at `ArtifactStore::blob_path(sha)`.
    - `report.artifacts` is still empty.
  - **`report_finding.rs` `a_block_naming_a_directory_is_a_field_error`:** `Hexdump` with path `dumps/` (a directory holding two files) gives a `Report` error whose field path is `report.blocks[0].artifact.path`, and nothing is written to the ledger.
  - **`report_finding.rs` `a_block_naming_a_missing_file_is_refused`:** `PcapRef` with path `caps/none.pcap` gives the `Artifact(Missing { .. })` error.
  - **`report_finding.rs` `block_files_share_the_findings_ingest_budget`:**
    - Limits are `max_total_bytes = 10` (use whatever `FindingWriteOptions` field feeds `ingest_limits()`).
    - `artifacts: [a.txt (6 bytes)]` plus an `Image` block of `b.png` (6 bytes) gives `Artifact(TooLarge { .. })`.
    - With only the artifact, the same report succeeds.
  - **`report_finding.rs` `block_files_without_a_store_fail_loudly`:** an image block with `artifact_root: None` gives `Artifact(NoStore)`.
  - **`ingest.rs` `remote_ingest_marks_block_artifacts_external_with_the_host`:** extend or copy the existing remote-host ingest test so the streamed finding also has an `Image` block with `stored: Copied`. After `ingest_unit_stream` with `IngestSource { host: Some("host_x") }`, the block ref is `External` + `host_x`.

- [ ] **Step 2: Run them to confirm they fail.** Run: `cargo test -p rupu-coverage --lib` and expect compile errors or failures in the new tests.

- [ ] **Step 3: Accessors.** In `types.rs`:

```rust
impl EvidenceBlock {
    /// The file this block points at (`image`, `hexdump`, `pcap_ref`).
    pub fn artifact(&self) -> Option<&ArtifactRef> {
        match self {
            EvidenceBlock::Image { artifact, .. }
            | EvidenceBlock::Hexdump { artifact, .. }
            | EvidenceBlock::PcapRef { artifact, .. } => Some(artifact),
            _ => None,
        }
    }

    pub fn artifact_mut(&mut self) -> Option<&mut ArtifactRef> {
        match self {
            EvidenceBlock::Image { artifact, .. }
            | EvidenceBlock::Hexdump { artifact, .. }
            | EvidenceBlock::PcapRef { artifact, .. } => Some(artifact),
            _ => None,
        }
    }
}

impl FindingReport {
    /// Every file this report references: `artifacts`, then the files its
    /// evidence blocks point at. Consumers that mean "any referenced file"
    /// (serving, remote marking, bucket upload) use this, never `artifacts`.
    pub fn artifact_refs(&self) -> impl Iterator<Item = &ArtifactRef> {
        self.artifacts
            .iter()
            .chain(self.blocks.iter().filter_map(EvidenceBlock::artifact))
    }

    pub fn artifact_refs_mut(&mut self) -> impl Iterator<Item = &mut ArtifactRef> {
        self.artifacts
            .iter_mut()
            .chain(self.blocks.iter_mut().filter_map(EvidenceBlock::artifact_mut))
    }
}
```

If `EvidenceBlock` already has an `impl` block (it has `kind()`), add the two methods there.

- [ ] **Step 4: Validation.** In `validate.rs`, right after the `report.artifacts` loop:

```rust
    for (i, b) in r.blocks.iter().enumerate() {
        let Some(a) = b.artifact() else { continue };
        let field = format!("report.blocks[{i}].artifact.path");
        if let Some(why) = rel_path_problem(&a.path) {
            c.err(field, why);
        } else if crate::report::artifacts::names_workspace_root(&a.path) {
            c.err(
                field,
                "names the workspace root; name the one file this block shows",
            );
        }
    }
```

If `tests/it/report_schema_lockstep.rs` fails because the schema doesn't mirror a validator rule, add the same constraint to `schema/finding_report.schema.json` only if the schema already expresses the `report.artifacts` path rule. Otherwise leave the schema alone and record the reason in the report.

- [ ] **Step 5: Ingest block files.** In `report_finding.rs`, replace the `if !report.artifacts.is_empty() { … }` block with the code below. `ArtifactError`'s `From` already exists on `ReportFindingError`, and `IngestLimits` fields are `max_file_bytes`, `max_files`, `max_total_bytes`.

```rust
            let has_block_files = report.blocks.iter().any(|b| b.artifact().is_some());
            if !report.artifacts.is_empty() || has_block_files {
                let store = opts
                    .artifact_root
                    .as_ref()
                    .map(crate::report::ArtifactStore::new)
                    .ok_or(crate::report::ArtifactError::NoStore)?;
                let limits = opts.ingest_limits();
                if !report.artifacts.is_empty() {
                    report.artifacts = store.ingest(&paths.workspace, &report.artifacts, limits)?;
                }
                ingest_block_files(&store, &paths.workspace, &mut report, limits)?;
            }
```

and add:

```rust
/// Verify and store every file an evidence block points at, against what is
/// left of the finding's ingest budget after `report.artifacts`, and replace
/// each block's ref (whatever the agent wrote) with the verified one. A block
/// names exactly one file. Block files stay in their blocks: they are not
/// PoC artifacts.
fn ingest_block_files(
    store: &crate::report::ArtifactStore,
    workspace: &std::path::Path,
    report: &mut crate::report::FindingReport,
    limits: crate::report::IngestLimits,
) -> Result<(), ReportFindingError> {
    let copied = |a: &crate::report::ArtifactRef| {
        a.stored == Some(crate::report::ArtifactStorage::Copied)
    };
    let mut left = crate::report::IngestLimits {
        max_files: limits.max_files.saturating_sub(report.artifacts.len()),
        max_total_bytes: limits.max_total_bytes.saturating_sub(
            report.artifacts.iter().filter(|a| copied(a)).map(|a| a.size).sum(),
        ),
        ..limits
    };
    for i in 0..report.blocks.len() {
        let Some(requested) = report.blocks[i].artifact().cloned() else {
            continue;
        };
        let field = || format!("report.blocks[{i}].artifact.path");
        if workspace.join(&requested.path).is_dir() {
            return Err(ReportFindingError::Report(
                crate::report::ReportValidationError(vec![crate::report::FieldError {
                    path: field(),
                    message: "names a directory; an evidence block shows exactly one file".into(),
                }]),
            ));
        }
        let got = store.ingest(workspace, std::slice::from_ref(&requested), left)?;
        let [one] = got.as_slice() else {
            return Err(ReportFindingError::Report(
                crate::report::ReportValidationError(vec![crate::report::FieldError {
                    path: field(),
                    message: "must name exactly one file".into(),
                }]),
            ));
        };
        left.max_files = left.max_files.saturating_sub(1);
        if copied(one) {
            left.max_total_bytes = left.max_total_bytes.saturating_sub(one.size);
        }
        *report.blocks[i]
            .artifact_mut()
            .expect("the block had an artifact above") = one.clone();
    }
    Ok(())
}
```

Use the crate's real paths for `IngestLimits` and `ArtifactStorage` (check `report/mod.rs` re-exports). If `IngestLimits` isn't `Copy`, add `Clone, Copy` to its derive.

- [ ] **Step 6: Remote marking.** In `ingest.rs` `mark_external`, iterate `report.artifact_refs_mut()` instead of `report.artifacts.iter_mut()`.

- [ ] **Step 7: Run the tests.** Run `cargo test -p rupu-coverage` and `cargo clippy -p rupu-coverage --all-targets -- -D warnings -A clippy::question_mark`. Expect them to pass and clippy to be clean.

- [ ] **Step 8: Commit.**

```bash
git add crates/rupu-coverage
git commit -m "feat(findings): evidence-block files are verified and stored like artifacts; artifact_refs() names every referenced file"
```

---

### Task 2: Serve block files and show images inline (`rupu-cp`, `rupu-cli`)

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs`
  - `get_artifact`'s sha lookup, ~:624: `r.artifacts.iter().find(..)` becomes `r.artifact_refs().find(..)`.
  - `artifact_response`, ~:960.
- Modify: `crates/rupu-cli/src/cmd/node.rs`: `referenced_artifacts`, ~:1393, `.flat_map(|r| r.artifacts)` becomes the refs.
- Tests: the `findings.rs` test module, `crates/rupu-cp/tests/it/finding_artifacts.rs`, the `node.rs` tests.

**Interfaces:**
- Consumes: Task 1's `FindingReport::artifact_refs()`.
- Produces: `fn raster_image_type(head: &[u8]) -> Option<&'static str>` in `findings.rs`.

- [ ] **Step 1: Write the failing tests.**
  - **`findings.rs` unit `raster_image_type_is_decided_by_magic_bytes`:**
    - PNG / JPEG / GIF87a / GIF89a / WebP heads map to their types.
    - These are `None`: `b"<svg xmlns"`, `b"%PDF-1.7"`, `b"GIF8"` (truncated), `b"RIFF\0\0\0\0WAVE"`, and empty.
  - **`findings.rs` unit `a_png_artifact_is_served_inline_as_an_image`:**
    - Store a blob of PNG magic + `b"fake image body"`, listed as `kind: Binary`, copied.
    - GET returns 200 with:
      - `content-type: image/png`;
      - a `content-disposition` starting with `inline`;
      - `nosniff` and `content-security-policy: sandbox`;
      - the full body byte-exact (the peek must not consume bytes);
      - `content-length` equal to the body length.
  - **`findings.rs` unit:** a non-image binary (e.g. `b"%PDF-1.7 …"`) stays `application/octet-stream` + `attachment`. A `kind: Text` blob whose bytes start with the PNG magic is still `text/plain`, because `Text` is never sniffed.
  - **`findings.rs` unit `an_evidence_block_artifact_is_servable_by_its_sha`:**
    - A finding with `artifacts: []` and an `Image` block whose ref (copied, real sha) is in the store gives 200.
    - A sha present in neither `artifacts` nor any block is still 404 "this finding does not reference that artifact".
  - **`tests/it/finding_artifacts.rs` `a_remote_image_block_downloads_end_to_end`:** model it on the existing end-to-end test that builds a Plan A stream.
    - The streamed finding has an `Image` block whose file is in the remote CP's store.
    - Ingest it with `IngestSource { host: Some(id) }`.
    - GET through the coordinator gives 200, `image/png`, and the bytes; the blob is now in the coordinator store.
  - **`node.rs` `referenced_artifacts_includes_block_artifacts`:** a streamed finding whose only copied ref is in a `PcapRef` block is listed.

- [ ] **Step 2: Run them to confirm they fail.** Run `cargo test -p rupu-cp --lib api::findings`, `cargo test -p rupu-cp --test it finding_artifacts::` and `cargo test -p rupu-cli --lib referenced_artifacts`.

- [ ] **Step 3: Implement.**
  - Lookup: `.and_then(|r| r.artifact_refs().find(|a| a.sha256 == sha))`. Keep `.cloned()`.
  - `node.rs`: `.flat_map(|r| r.artifact_refs().cloned().collect::<Vec<_>>())`.
  - Image typing in `findings.rs`:

```rust
/// The `image/*` type of a raster image, by its magic bytes — never by name
/// or by anything an agent wrote. SVG (text) is never an image here.
fn raster_image_type(head: &[u8]) -> Option<&'static str> {
    match head {
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}
```

In `artifact_response`, when `kind` is not `Some(ArtifactKind::Text)`:
1. Read up to 12 bytes from `file`, looping until 12 bytes or EOF.
2. `seek(SeekFrom::Start(0))` back, with `tokio::io::AsyncSeekExt`.
3. On a read or seek error, fall back to octet-stream attachment. Never serve a half-consumed handle: if the seek fails, the response must not use that handle's remaining bytes as if complete. Return 500 via the existing error style, or re-open; pick the simplest correct option and say which.
4. Set the content type and disposition by `raster_image_type(&head)`: `Some(t)` gives `(t, inline; filename=…)`; `None` gives the current `application/octet-stream` + `attachment`.

Compute `len` from metadata before the peek, as today. `nosniff` and `CSP: sandbox` stay unconditional. The host blob endpoint also goes through `artifact_response(file, None, sha)`, so it gains image typing too; that is fine, because the coordinator re-verifies every pulled byte.

- [ ] **Step 4: Run tests and clippy.** Run `cargo test -p rupu-cp`, `cargo test -p rupu-cli --lib cmd::node` and clippy on both crates. Expect them to pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp crates/rupu-cli
git commit -m "feat(cp): serve evidence-block files; raster images inline as image/* by magic bytes"
```

---

### Task 3: Render evidence blocks on the finding page (web)

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/findingReport.ts`: types.
- Create: `crates/rupu-cp/web/src/components/findings/report/EvidenceBlocks.tsx`.
- Modify: `crates/rupu-cp/web/src/pages/FindingDetail.tsx`: a section after "Evidence".
- Test: `crates/rupu-cp/web/src/components/findings/report/report.test.tsx`.

**Interfaces:**
- Consumes: `findingArtifactUrl(id, sha256)` from `lib/api.ts`; `Markdown` from `components/transcript/Markdown`; `formatBytes` and `ArtifactRef` from `lib/findingReport`.
- Produces: `EvidenceBlocks({ findingId, blocks }: { findingId: string; blocks: EvidenceBlock[] })`.

- [ ] **Step 1: Types.** In `findingReport.ts`:

```ts
export interface DisasmLine { address: number; bytes: string; mnemonic: string; ops?: string }
export type EvidenceBlock =
  | { kind: 'text'; text: string }
  | { kind: 'code_slice'; file?: string; excerpt: string; lang?: string }
  | { kind: 'diff'; diff: string }
  | { kind: 'table'; headers: string[]; rows: string[][] }
  | { kind: 'image'; artifact: ArtifactRef; caption?: string }
  | { kind: 'hexdump'; base: number; artifact: ArtifactRef; rendered?: string }
  | { kind: 'disasm'; arch: string; listing: DisasmLine[] }
  | { kind: 'decompile'; lang: string; listing: string }
  | { kind: 'http_exchange'; request: string; response: string }
  | { kind: 'scan_output'; tool: string; output: string }
  | { kind: 'pcap_ref'; artifact: ArtifactRef; summary: string };
```

and add `blocks?: EvidenceBlock[];` to `FindingReport`.

- [ ] **Step 2: Write the failing tests** in `report.test.tsx`, as a new `describe('EvidenceBlocks')`. Use invented values.
  - **Text kinds:** each of `text`, `code_slice`, `diff`, `table`, `disasm`, `decompile`, `http_exchange` and `scan_output` renders its content.
    - The `disasm` address renders as hex `0x401000`.
    - The table header and cells are present.
    - `http_exchange` shows both the request and the response.
  - **Local image:** an `image` block without `host` renders an `<img>` whose `src` is `findingArtifactUrl('fnd_1', sha)` and whose `alt` is the caption, plus a Download link.
  - **Host image:** an `image` block with `host: 'kuki'` renders NO `<img>` and spies `fetch` not called. It shows a button named like `Load image (from host kuki)`; clicking it renders the `<img>`.
  - **Hexdump:** shows `rendered` in a `<pre>`, `base` as hex, and a Download link.
  - **Pcap:** `pcap_ref` shows its `summary` and a Download link.
  - **Fallback:** a block with an unknown `kind` (cast through `unknown`) renders a fallback naming the kind and does not throw.
  - **Broken image:** an `<img>` `onError` swaps to a message plus the Download link.

- [ ] **Step 3: Implement `EvidenceBlocks.tsx`.**
  - Render a `<div className="space-y-3">` with one block per item, matching the styling idioms of `EvidenceClaims.tsx` and `ArtifactBrowser.tsx`: bordered `rounded-md`, `font-mono text-note` for code, `bg-panel`. Read those files first.
  - Code-like kinds use a `<pre className="max-h-96 overflow-auto …">`.
  - **Image component:** state `show = !artifact.host`.
    - When `!show`, render a button labelled `Load image (from host ${host})`.
    - When `show`, render `<img src={findingArtifactUrl(findingId, artifact.sha256)} alt={caption ?? artifact.path} loading="lazy" className="max-h-[32rem] max-w-full rounded border border-border" onError={() => setBroken(true)} />` with the caption as `<figcaption>`.
    - Always show a Download link (`href` = the same URL, `download`) and `formatBytes(size)`.
  - Never use `dangerouslySetInnerHTML`.
  - Wire into `FindingDetail.tsx` right after the Evidence section:

```tsx
{report.blocks && report.blocks.length > 0 && (
  <Section id="s-blocks" title="Evidence blocks"><EvidenceBlocks findingId={detail.id} blocks={report.blocks} /></Section>
)}
```

If the page has a section index or table of contents listing the `s-*` ids, add `s-blocks` there too.

- [ ] **Step 4: Run the tests.** From `crates/rupu-cp/web` (run `npm ci` first if `node_modules` is missing): `npx vitest run src/components/findings src/lib` then `npx tsc -b`. Expect them to pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): render evidence blocks; images load inline (remote ones on click)"
```

---

### Task 4: Docs and the full gate

**Files:**
- Modify: `docs/coverage.md`: the evidence blocks / artifacts sections.
- Modify: `CLAUDE.md`: the `rupu-coverage` bullet (one clause), plus "Read first" (this spec + plan).
- Modify: `TODO.md`: follow-ups.

- [ ] **Step 1: `docs/coverage.md`.** Say four things:
  - files named by `image` / `hexdump` / `pcap_ref` blocks are verified and stored like `report.artifacts` and count against the same per-finding limits, but they are not PoC artifacts;
  - a block names exactly one file;
  - `GET /api/findings/:id/artifacts/:sha256` serves them, and remote ones are pulled from their host on first view like any artifact;
  - raster images (PNG/JPEG/GIF/WebP, by magic bytes) are served inline as `image/*`, and the finding page renders every block kind, loading remote images only on click.
- [ ] **Step 2: `CLAUDE.md`.** Add one clause to the `rupu-coverage` bullet: `FindingReport::artifact_refs()` is every file a report references (`artifacts` + evidence-block files); use it, not `artifacts`, wherever "any referenced file" is meant. Also add the spec and plan to "Read first".
- [ ] **Step 3: `TODO.md`.** Add two follow-ups: exports print block files by path only (embed images in HTML/PDF); the Code tab's inline finding card doesn't render blocks.
- [ ] **Step 4: Full gate.** Run each and record the counts:
  - `cargo check --workspace --all-targets`
  - `cargo test -p rupu-coverage`, `-p rupu-cp`, `-p rupu-orchestrator`
  - `cargo test -p rupu-cli --lib`, `--test it`, `--test serial -- --test-threads=1 < /dev/null`
  - `cargo clippy --workspace --all-targets -- -D warnings -A clippy::question_mark`
  - web: `npx vitest run` + `npx tsc -b`
- [ ] **Step 5: Commit.**

```bash
git add docs/coverage.md CLAUDE.md TODO.md docs/superpowers
git commit -m "docs: evidence-block files are stored, served and rendered"
```
