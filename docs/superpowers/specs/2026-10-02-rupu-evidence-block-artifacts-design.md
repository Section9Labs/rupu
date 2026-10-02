# Evidence-block artifacts — design

- **Status:** approved in conversation, 2026-10-02.
- **Builds on:** engagement profiles (#716, typed evidence blocks), finding reports (`2026-09-29-rupu-finding-reports-design.md` §Artifacts), remote findings Plans A/B (#728, #730).

## Problem

#716 added typed evidence blocks (`report.blocks`). Three kinds point at a file through their own `ArtifactRef`: `image`, `hexdump` and `pcap_ref`. The builtin web, redteam and threat-model profiles list `image`; binary and firmware list `hexdump`; network lists `pcap_ref`. Today those files fall through every part of the artifact pipeline:

- **Never stored or verified.** `report_finding` hashes and copies only `report.artifacts`. A block's `path`, `sha256` and `size` stay whatever the agent wrote, unchecked.
- **Never served.** `GET /api/findings/:id/artifacts/:sha256` serves only shas in `report.artifacts`.
- **Never shown.** The web finding page doesn't render `report.blocks` at all, including the text-only kinds (disassembly, HTTP exchanges, scan output).
- **Wrong for remote units.** Plan A's ingest marks only `report.artifacts` external + `host`. Plan B's bucket worker uploads only those.

## Decisions

1. **Verify and store block files exactly like `report.artifacts`, but leave them in their blocks.** At `report_finding` each block's file goes through `ArtifactStore::ingest`. Agent-supplied `sha256`/`size`/`kind` are discarded, and the block's ref is replaced by the verified one (`stored: copied` or `external`).
   - Limits are shared with `report.artifacts`: one budget per finding.
   - A block names exactly one file; a directory is a field error at `report.blocks[i].artifact.path`.
   - Block files are NOT folded into `report.artifacts`. `has_poc` and the "PoC artifacts" section keep meaning PoC files. (This refines the conversation's "add it to `report.artifacts`".)
2. **One iterator for "every file a report references":** `FindingReport::artifact_refs()` returns `report.artifacts`, then block artifacts; `artifact_refs_mut()` is the mutable form. Every consumer that means "any referenced file" uses it:
   - the coordinator endpoint's sha lookup;
   - Plan A's `mark_external`;
   - Plan B's bucket `referenced_artifacts`.

   With that, remote pulls, bucket uploads and the local-external path cover block files with no transport changes.
3. **Validation:** a block artifact path follows the `report.artifacts` path rules: workspace-relative, no `..`, not the workspace root.
4. **Inline images.** `artifact_response` serves a non-text artifact whose first bytes are PNG, JPEG, GIF or WebP as `image/png|jpeg|gif|webp` with `Content-Disposition: inline`.
   - The type is decided by magic bytes, never by extension or agent input.
   - SVG and anything sniffed as text stay `text/plain`.
   - `X-Content-Type-Options: nosniff` and `Content-Security-Policy: sandbox` stay on every response.
5. **Web:** the finding page renders every block kind.
   - An image block whose artifact has no `host` loads inline (`<img>` on the artifact URL).
   - An image block whose artifact has a `host` shows a "Load image (from host X)" button. A remote pull may cost the host an SSH connection, so nothing remote is fetched on render.
   - `hexdump` shows its `rendered` text plus Download; `pcap_ref` shows its summary plus Download.
   - Text kinds render as text, code or tables.
   - An unknown kind renders as a labelled fallback, never a crash.

## Out of scope (follow-ups)

- Embedding images and captures in Markdown/HTML/PDF exports. They keep printing the path.
- Rendering blocks in the Code tab's inline finding card.
- Placed units running under an engagement profile (no `engagement` on `AgentLaunchRequest`/`RunSpec` yet).

## Testing

- **rupu-coverage:**
  - an image block's file is stored, its ref verified, and it is not added to `report.artifacts`;
  - a bogus agent sha is replaced;
  - a directory, missing file, `..` path or workspace root is refused with the right field path;
  - no store gives `NoStore`;
  - the shared budget is enforced;
  - `artifact_refs()` order;
  - `mark_external` marks block refs.
- **rupu-cp:**
  - a block artifact's sha is servable and an unreferenced sha is still 404;
  - a PNG is `image/png` inline with nosniff + CSP sandbox and its body intact;
  - non-image binaries stay octet-stream attachments;
  - text (incl. SVG) stays `text/plain`;
  - a remote block artifact pulls end to end.
- **rupu-cli:** the bucket worker uploads a block-referenced blob.
- **Web:**
  - each block kind renders;
  - a local image loads on render;
  - a host image loads only on click;
  - an unknown kind falls back.
