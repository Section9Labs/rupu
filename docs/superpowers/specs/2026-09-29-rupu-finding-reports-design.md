# rupu finding reports — design

Date: 2026-09-29
Status: approved in brainstorming, pending spec review

## Problem

A security finding is written twice today. The agent calls `report_finding`
(or an `action:` step calls `findings.record`), which stores a thin ledger
record: `summary`, `severity`, one `rationale` string, one `code_excerpt`,
`references`, `concern_id`. Separately, the agent's prompt asks it to write a
full assessment-style report as a markdown file on disk: ownership, ratings,
root cause, call chain, per-claim evidence, a patch, a CI/CD check, a
regression test, replication steps, and proof-of-concept files.

The control plane only knows about the thin record. So:

- The agent packs call chain, evidence, and remediation into `rationale`,
  and `FindingEvidence.tsx` renders that as one `<p class="whitespace-pre-wrap">`
  wall of text. Newlines, code, and emphasis are lost.
- The report file only reaches the CP as a transcript tool output. Nothing
  links it to the finding, so there's no patch view, no PoC browser, and no
  way to filter by owner or CWE.
- The two copies drift. On one real multi-unit assessment, the ledger held
  ~600 characters per finding while the matching report files averaged ~8 KB.

## Decisions (from brainstorming)

1. **One structured contract.** The agent writes the full report once, as
   typed data on the finding. rupu stores it and generates every
   presentation from it: UI sections, Markdown, HTML, and PDF. Agents stop
   writing report files.
2. **Profiles: `full` and `summary`.** `full` requires the complete report.
   `summary` is today's lightweight record. Selection precedence is
   **step → workflow `defaults` → agent frontmatter → built-in default
   (`full`)**.
3. **Validation is strict.** Under `full`, an incomplete report is rejected
   at write time with an error that names every missing or invalid field, and
   the agent fixes it in the same turn.
4. **Three field levels.** *Required*, *required with sentinel allowed*, and
   *optional*. Following the reporting standard this came from, nothing in the
   report body is optional. The honest escape hatches are the sentinels:
   `Unknown`, `Not Applicable`, `None Provided`, `None`, and
   `Not Provided — <justification>`.
5. **The schema ships inside rupu.** It is embedded in the binary. It is
   called a "finding report", never by an organisation-specific standard's
   name. `rupu findings schema` prints it for external tools and prompts.
6. **Artifacts are copied, content-addressed, capped at 500 MB per file.**
   Up to the cap, files are hashed and copied into rupu's store and
   deduplicated by hash. Over the cap, rupu records a reference (host, path,
   size, sha256) and fetches it on demand.
7. **Exports: Markdown, HTML, PDF**, generated server-side. Per finding, plus
   a **project report** covering all findings or a chosen subset.

## Data model

### FindingRecord (extended, backwards-compatible)

```rust
pub struct FindingRecord {
    // ... every existing field unchanged ...
    #[serde(default)]                     // absent on legacy lines ⇒ Summary
    pub profile: FindingProfile,          // full | summary
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<FindingReport>,    // Some iff profile == Full
}
```

Legacy records deserialize as `profile: summary, report: None`. No migration
is needed. Existing readers (the CP DTO flattens this record; macOS
`FindingsModels.swift`) get new optional keys and nothing else changes.

Under `full`, the existing top-level fields stay populated, so everything
that keys on them keeps working (sorting, the coverage ledger, Code-tab
anchoring, dedup):

- `summary` is the report's `title`.
- `severity` is derived from `rating.risk_rating`. `Critical/High/Medium/Low`
  map one-to-one. `info` is only reachable under `summary`.
- `evidence.rationale` is the report's `root_cause`, so old clients show
  something meaningful.
- `evidence.code_excerpt` is the first evidence claim's excerpt.

### FindingReport

Field names follow the source reporting standard. Where the standard used
prose for something with obvious structure, rupu uses the structure, because
the UI renders each part and export turns it back into the standard's prose
layout.

| Field | Type | Level |
|---|---|---|
| `title` | string | required |
| `ownership.owner` / `.product` / `.affected_component` | string | sentinel allowed (`Unknown`) |
| `ownership.source_repository` | string | sentinel allowed (`Unknown`, `Not Applicable`) |
| `tickets` | `"None Provided"` \| `"Unknown"` \| `[{type, identifier, url, notes}]` | sentinel allowed |
| `rating.impact` / `.risk_rating` / `.risk_factor` | enum Low/Medium/High/Critical | required |
| `rating.likelihood` | enum Low/Medium/High | required |
| `rating.cvss_v3` | string (score, optional vector) | sentinel allowed (`Unknown`) |
| `category` | string | required |
| `attack_vector` | string | sentinel allowed (`Unknown`) |
| `cwe` | string[] (`CWE-306` form) | required; may be empty (the list is extracted for chips/filters) |
| `description` | markdown | required |
| `impact` | markdown | required |
| `location.input` / `location.output` | markdown | required |
| `root_cause` | markdown (1–2 sentences) | required |
| `call_chain` | `[{label, file?, lines?, binary_va?, gate?, passes_because?, role: source\|hop\|sink}]` \| `"Not Provided — …"` | sentinel allowed |
| `evidence` | `[{claim, file?, lines?, binary_va?, excerpt?, lang?, sha256?, artifact?}]`, min 1 | required |
| `remediation` | markdown | required |
| `recommended_patch` | `{diff, notes?}` \| `"Not Provided — …"` | sentinel allowed |
| `ci_cd_detection` | `{stage, body, command?, expect}` \| `"Not Provided — …"` | sentinel allowed |
| `regression_test` | `{body, command, expect_vulnerable, expect_patched}` \| `"Not Provided — …"` | sentinel allowed |
| `replication_steps` | string[], min 1 | required |
| `cross_references` | `[{finding_id, relation: duplicate\|sibling\|prerequisite\|supersedes, note?}]` \| `"None"` | sentinel allowed |
| `references` | markdown | required |
| `artifacts` | `[{path, sha256, size, kind, stored: copied\|external, host?}]` | optional (rupu-populated) |
| `verification` | `{status: unverified\|confirmed\|disputed\|inconclusive, by_run?, notes?}` | optional (rupu/verifier-populated) |

Rules enforced by validation (under `full`):

- Required strings must be non-empty after trimming.
- A sentinel is accepted only on the fields that allow it, and only in its
  exact form. `Not Provided — ` must be followed by a non-empty justification.
  `Unknown` is **not** accepted for patch, CI/CD, or regression test.
- Evidence and call-chain `file` values are workspace-relative and
  `lines` is `[start, end]` with `start ≤ end`. The same rule as the existing
  `line` scope locator, reused rather than duplicated.
- `cross_references[].finding_id` must be an existing `fnd_` id in the same
  project ledger.
- The whole report must fit a size budget (default 256 KB serialized,
  excluding artifacts). Over budget is rejected with the size named, so a
  pasted log can't swell the ledger.

### Identifiers

The source standard has a per-report `identifier` (`SEC-001`). In a fan-out
run, every unit's agent starts at `SEC-001`, so agent-chosen identifiers
collide. rupu therefore:

- keeps `fnd_<ULID>` as the one stable identity; agents do not supply
  `identifier`,
- numbers findings **at export time**, using a configurable prefix
  (`[findings].export_id_prefix`, default `SEC`), in the report's sort order
  (severity, then declared_at). The project report includes the
  number ↔ `fnd_` mapping,
- has agents cross-reference by the `fnd_` id the tool call returned; exports
  render those as the export numbers.

## The embedded schema

- Source of truth: `crates/rupu-coverage/schema/finding_report.schema.json`,
  loaded with `include_str!`.
- Serde types (`FindingReport` and friends) plus a hand-written validator
  produce precise, field-path error messages ("`report.regression_test`:
  `Unknown` is not allowed here; provide the test or `Not Provided — <why>`").
  A lockstep test validates a shared fixture corpus (valid and invalid cases)
  against **both** the JSON Schema and the serde validator, and fails if they
  disagree.
- `rupu findings schema [--profile full]` prints the schema, so external
  prompts (for example, an external prompt file) can be regenerated from rupu
  rather than hand-maintained.
- Organisation-specific guidance (for example, which ticket-system URL
  patterns count as existing tickets) is **not** shipped. It is configurable
  as `[findings].ticket_patterns` and appended to the injected guidance when
  set.

## Profile selection

```yaml
# agent frontmatter
findings_profile: full

# workflow
defaults:
  findings_profile: full
steps:
  - id: sweep
    agent: quick-scanner
    findings_profile: summary
```

- Fields: `AgentSpec.findings_profile`, `WorkflowDefaults.findings_profile`,
  `Step.findings_profile` (also on `SubStep` for `parallel:`).
- Resolution happens in the orchestrator when it builds the step. The
  resolved profile is threaded into the agent runtime and into `FindingsContext`
  (for `action:` steps calling `findings.record`).
- Standalone `rupu run <agent>` uses the agent's value, or `full` if unset.
- The CP workflow editor (web + macOS Builder) gets the field in the step
  inspector and workflow settings.

What the resolved profile changes:

1. **Tool definition.** Under `full`, `report_finding` / `findings.record`
   advertise `report` as required, with the full schema inline. Under
   `summary`, they advertise today's fields only.
2. **Prompt guidance.** Under `full`, the runtime appends a short
   finding-writing guide to the system prompt: sentinels, don't invent owners
   or scores, patch/CI/regression expectations. Agents no longer point at an
   external file.
3. **Validation.** Under `full`, it is strict (above). Under `summary`, a
   `report` is not accepted. Sending one returns an error telling the agent
   the step is in summary mode, so data is never silently dropped.
4. **Labelling.** `profile` is stored on each record.

Deferred: upgrading a `summary` finding to `full` on the same id (a cheap
sweep followed by a deep dive).

## Artifacts

- Agents list PoC files in `report.artifacts[].path` (workspace-relative).
  At write time rupu hashes each one.
- ≤ 500 MB per file (`[findings].artifact_max_bytes`): the file is copied
  into a content-addressed store at `<RUPU_HOME>/findings/artifacts/<aa>/<sha256>`
  and recorded as `stored: copied`. Identical content is stored once, across
  runs and projects. The store is global, not per-workspace, so artifacts
  outlive the workspace.
- \> 500 MB, or a directory: recorded as `stored: external` with path, size,
  and sha256 (directories are recorded as a manifest of their files, each
  handled by the same rule). The UI opens external artifacts from the
  workspace while it exists.
- Placed/remote units: the tool runs on the remote host, so artifacts are
  recorded as `external` with `host` set. They are pulled into the
  coordinator's store on first view through the same host-connector pattern
  the SSH lazy transcript mirror uses (`HostConnector` gains
  `pull_finding_artifact`). The pulled copy is verified against the recorded
  sha256.
- Retention: artifact blobs are referenced by findings. A `rupu findings gc`
  pass deletes blobs that no remaining ledger line references. There's no
  automatic GC in v1.
- Content served from the store is always `Content-Disposition: attachment`
  for binaries and plain-text-escaped for text. It is never rendered as HTML.

## CP API

- `GET /api/findings`: unchanged shape, plus `profile`, `cwe[]`,
  `root_cause`, `completeness` (`{filled, total}`), `has_poc`, and
  `verification.status`. The heavy report body is **not** included, so
  global lists stay small.
- `GET /api/findings/:id`: the full record including `report`.
- `GET /api/findings/:id/artifacts/:sha256`: artifact bytes (copied, or
  pulled on demand for external artifacts; `404` + `{"unavailable": reason}`
  when the source is gone).
- `GET /api/findings/:id/export?format=md|html|pdf`: one finding.
- `POST /api/findings/export`: project report. Body
  `{format, ids? | filter?: {ws_id?, run_id?, severity?, owner?, cwe?, profile?}, include_summaries: bool}`.
  Returns a file, or a zip of per-finding files when `split: true`.
- `make macos-fixtures` regenerates the new DTOs. The drift test covers them.

Deviations as built (Plan 2): artifact errors for a path on this machine use
the CP's standard `{"error": …}` body with the usual status (`404`/`409`), not
`{"unavailable": …}`. A remote artifact (`external` with a recorded `host`) is
pulled from that host on first view per
`docs/superpowers/specs/2026-09-30-rupu-remote-findings-transport-design.md`
(§B1–B2, built by Plan B), and its failures answer `404` with
`{"unavailable": "<reason>"}`. Claim staleness hashes files up to 64 MiB only
(larger reports `unknown`).

## Rendering

### Web (`crates/rupu-cp/web`)

- **Quick fix, ships first and on its own:** `FindingEvidence.tsx` renders
  `rationale` through the existing `components/transcript/Markdown.tsx`
  (react-markdown + rehype-highlight). This alone fixes the formatting
  problem for every existing finding.
- **Finding detail route `/findings/:id`:**
  - a section rail with a completeness meter,
  - a header with a severity chip, CWE chips, a verification chip, and a PoC chip,
  - an ownership / classification ledger and a rating strip,
  - markdown prose sections,
  - root cause as a callout,
  - the call chain as a vertical hop list (source → hops → sink) with
    clickable `file:line` into the Code tab,
  - evidence as per-claim excerpt cards with a stale badge (current file hash
    ≠ recorded `sha256`),
  - the patch as a unified-diff viewer,
  - CI/CD and regression as command blocks with copy buttons and a
    vulnerable-vs-patched expected-signal pair,
  - numbered replication steps,
  - an artifact browser (file list plus text preview, download for binaries),
  - references, cross-reference links, and provenance (run, agent, model,
    declared_at).
  - Export buttons: Markdown / HTML / PDF.
- **Findings tables** (run / project / global): the expanded row becomes a
  triage card with root cause, a one-line call-chain path, owner and product,
  and the section map, with "Open full report". The long narrative never
  renders in a table row. Add a `Report` column (`17/18 · PoC ✓` /
  `summary only`) and filters for profile, owner, and CWE.
- **Code tab inline card** (`InlineFindingCard.tsx`): a compact tabbed card
  (Root cause / Call chain / Evidence / Patch / Repro). Evidence locations
  are jump chips, and the stale warning is per claim.
- **Project report export dialog:** pick by filter or checkbox, include
  summaries toggle, choose format, single document or split zip.

Deviations as built (Plan 2): provenance shows run, model, and declared-at —
the finding record carries no agent name, so none is shown. The run's Findings
tab (`FindingRow`) links each full-profile finding to its report page rather
than embedding the triage card or a Report column; the triage card and Report
column live on the findings tables (global, project, coverage). Exports (and
their buttons and dialog) are Plan 3.

### macOS (`apps/rupu-macos`)

Parity, in a later plan. A `FindingDetailScreen` in `RupuSecurity`, reusing
the transcript's markdown + HighlighterSwift path and the existing Code tab
for jumps. The findings table gains the triage card. Exports call the CP
endpoints.

A visual mockup was reviewed during brainstorming; it is not part of the repo.

## Export generation

- New crate **`rupu-findings-report`**. It is pure: it takes `FindingRecord`s
  and returns bytes, with no I/O. Renderers:
  - **Markdown:** the source standard's section order and headings, one
    document per finding, with the `Filename: <ID> - <Short Title>.pdf` line.
  - **HTML:** a self-contained document with inline CSS and no external
    loads, printable. The same section order.
  - **PDF:** generated in-process with **Typst** (the `typst` crate, pure
    Rust, so it works in the musl Linux build and needs no headless
    browser). rupu emits Typst markup from the report, with a bundled font.
    Code and diff blocks use Typst's built-in syntax highlighting.
  - **Project report:** a cover page (project, date, scope filter),
    severity summary, a findings index with the number ↔ `fnd_` map, then
    each finding as a standalone section starting on a new page (the source
    standard requires each finding to stand on its own).
- CLI: `rupu findings export [--run ID | --project PATH] [--id fnd_…]
  [--severity ≥high] [--format md|html|pdf] [--split] -o OUT`. The CLI stays
  thin and calls the crate.
- A snapshot test (insta) for Markdown and HTML over the fixture corpus. PDF
  gets a smoke test only: it compiles, it's non-empty, and the page count is
  as expected.

### Deviations as built (Plan 3)

Plan 3 (`docs/superpowers/plans/2026-09-29-rupu-finding-reports-plan-3-exports.md`)
shipped the export generation above, with these differences from the text:

- **CLI flag is `--to`**, not `--format`: `--format` is rupu's global output flag
  (`table`/`json`/`csv`), so the document format is `rupu findings export --to
  md|html|pdf`.
- **The display-number prefix is global-config only.** `[findings].export_id_prefix`
  is read from `~/.rupu/config.toml`; a project's `.rupu/config.toml` never
  changes it (it is repo-controlled and lands in file names and document text).
  It is validated (`^[A-Za-z][A-Za-z0-9_-]{0,15}$`, else `SEC`).
- **PDF is a default-on cargo feature `pdf`** (forwarded by `rupu-cp` and
  `rupu-cli`), because Typst and its bundled fonts add roughly 45-55 MB to a
  release binary. Without it, Markdown and HTML still export and PDF reports
  "compiled without PDF support".
- **HTML renders images as their alt text** and embeds a strict
  Content-Security-Policy (`default-src 'none'`, no `<base>`, no form posts) and a
  no-referrer policy, so an exported document loads nothing.
- **PDF fonts are bundled but limited** to Libertinus Serif, New Computer Modern
  and DejaVu Sans Mono: no CJK or emoji glyphs.
- **Tests are assertions, not snapshots.** There are no insta snapshots for
  Markdown or HTML: `rupu-findings-report`'s tests assert section order, field
  text, escaping and (for Markdown) what a CommonMark parser makes of the
  output. PDF has no page-count check: its tests assert a valid `%PDF` document
  (and that a project PDF is larger than one finding's) and that an adversarial
  corpus compiles through the real emitters.
- **The `POST /api/findings/export` body is flat**:
  `{format, title?, ids?, ws_id?, run_id?, min_severity?, owner?, cwe?,
  include_summaries?, split?}`, not `ids? | filter?: {…}`. `ids` and the
  filters combine (a finding must pass all of them), there is no `profile`
  filter (`include_summaries` covers it), and unknown fields are refused so a
  misspelt filter cannot widen a report.

## Backfill (last, optional)

`rupu findings import <dir>` parses finding reports written by the old
two-copy process (markdown in the source standard's layout) into
`FindingReport`, and attaches each one to its existing `fnd_` record by the
native id the report cites. It is best-effort. Reports that don't parse are
listed, never partially written. This is a one-time migration aid, not a
supported input path.

### Deviations as built (Plan 4 and Plan 5)

- **Plan 4 (macOS parity) was dropped:** the macOS app is deprecated, so new
  features target the CLI and the control-plane web UI only.
- **Plan 5** (`docs/superpowers/plans/2026-09-30-rupu-finding-reports-plan-5-import.md`)
  shipped the importer above. It matches a report to its finding by the id on
  the report's labelled id line (`Finding ID:`, `Native Finding:` and similar;
  never an id merely mentioned in prose), with `--id` naming the finding of a
  single file that has no such line (an `--id` that disagrees with the line
  fails). The rewrite is a single locked, backed-up, atomic replacement of the
  ledger (`rupu_coverage::tools::attach_reports`), a finding that already has a
  report is never changed, and imported evidence claims are stored unhashed
  (they describe the code as it was when the report was written). Missing content follows
  the no-invention rules: a field or section the schema has no sentinel for
  fails the file; one it has a sentinel for gets `Unknown`, `None`, or `Not
  Provided — section missing from the imported report`; and a missing part of
  a section that is present is `Not stated in the imported report.` See
  `docs/coverage.md#importing-reports-written-before-the-full-profile`.
- **Exporter changes for the importer** (Plan 5's final review): so an exported
  report reads back unambiguously, the Markdown export now prints a
  `**Command:**` line before the CI/CD Detection and Regression Test command
  blocks (Plan 3's block shape had the bare `sh` block), labels the code of
  typed evidence blocks it used to print as a bare fence (`**Diff**`,
  `**Decompiled** (lang)`, `**Code**`), and escapes a `;` inside one ticket
  reference as `\;` (Markdown shows `;`; HTML and PDF print it plain),
  since tickets are joined with `; `.

## Error handling

- Validation errors are returned to the agent as a tool error that lists
  every problem at once, each with its JSON path, so it can fix everything
  in one retry.
- Artifact hashing/copy failures reject the finding (the agent named a file
  that isn't there), except over-cap files, which are recorded as external
  by design.
- Export failures (Typst compile error) return `500` with the Typst
  diagnostic. Markdown/HTML never depend on Typst.

## Testing

- `rupu-coverage`:
  - lockstep schema ↔ validator corpus,
  - legacy-line deserialization,
  - severity derivation,
  - sentinel acceptance/rejection per field level,
  - size budget,
  - cross-reference existence,
  - artifact copy/dedup/over-cap/missing-file.
- `rupu-orchestrator`: profile precedence (a table test over the
  step/workflow/agent/default combinations, including `parallel:` substeps
  and `for_each:`).
- `rupu-agent` / `rupu-mcp`: the tool definition differs by profile; a
  mock-provider run that sends an incomplete report gets the error and
  succeeds on retry.
- `rupu-cp`: the list DTO excludes the report body; detail/artifact/export
  endpoints; fixture drift.
- Web: component tests for each section renderer and the triage card;
  markdown rationale rendering.
- GUI validation rule applies: matt runs the web and macOS views before merge.

## Plan breakdown

0. **Quick fix:** render `rationale` as markdown in web `FindingEvidence`
   Standalone PR (web only: the macOS findings table does not display the
   rationale). Shipped as Section9Labs/rupu#674.
1. **Contract:** model + embedded schema + validator + profile field
   and precedence + tool definitions + prompt guidance + artifact store.
   `rupu findings schema`.
2. **Web UI:** detail route, triage card, inline card, API detail/artifact
   endpoints.
3. **Exports:** `rupu-findings-report` crate (md/html/Typst PDF),
   per-finding and project report, CLI + CP endpoints + export dialog.
4. **macOS parity.** Dropped: the macOS app is deprecated.
5. **Backfill importer** (optional). Shipped.

## Out of scope

- Upgrading summary findings to full in place, other than the one-time
  `rupu findings import` backfill above.
- Ticket creation in external trackers (the mockup's "Create ticket" button
  is a later arc).
- Automatic artifact GC.
- Verification-lane semantics beyond storing `verification` when a verifier
  writes it.
