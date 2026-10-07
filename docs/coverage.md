# Agentic coverage harness

`rupu coverage` turns "review this repo for X" from an unauditable one-shot into
a **persistent, accumulating, comparable record** of what an agent examined, for
which criteria, and what it concluded.

## At a glance

When a model performs a survey task — "review this repo for vulnerabilities",
"find every place we touch the database", "audit for accessibility" — findings
vary run-to-run, and you can't tell *"no issues found"* apart from *"the model
never looked."* The coverage harness fixes that:

- **Coverage is explicit.** For every `(file × concern)` the agent assesses, it
  records a verdict — `clean` / `finding` / `examined` / `not_applicable` — with
  evidence. File reads/greps/edits are tracked automatically.
- **Runs accumulate.** A second pass (same model or a different one) picks up
  where the first left off; cross-model verdicts are attributed and merge-able.
- **Industry-anchored catalogs.** Ship OWASP Top 10, CWE Top 25, the full CWE
  weakness list, STRIDE, secrets, and more; include or extend them.
- **Surface-uniform.** Works the same whether the agent loop is driven by
  `rupu run`, a workflow, an autoflow cycle, or an interactive session.

On top of that foundation, the harness also lets you **measure and reproduce**
variance: diff two runs, and replay a run to compare it against the original.

## How it works

Coverage data for a *target* lives under `<workspace>/.rupu/coverage/<target_id>/`
as append-only JSONL plus a catalog snapshot. Finding tags are the workspace-level
exception: their log sits one level up, in `<workspace>/.rupu/coverage/` itself,
because it spans every target (see [Tagging findings](#tagging-findings)). The other
exception is `rupu findings
import`, the one-time migration of older reports: every real import that attaches
anything replaces `findings.jsonl`, leaving a backup of it beside the ledger each
time (see [Importing reports](#importing-reports-written-before-the-full-profile)).

| File | Contents |
|------|----------|
| `files.jsonl` | every file touch (read / grep / glob / edit / cmd), with attribution |
| `concerns.jsonl` | every `(concern, file) → verdict` assertion |
| `findings.jsonl` | every reported issue |
| `catalog.yaml` | the effective concern catalog, snapshotted at run start |
| `runs.jsonl` | one manifest per run (its defining inputs, for replay) |
| `findings.jsonl.lock` | empty; the lock every writer of `findings.jsonl` takes |
| `findings.jsonl.pre-import-<UTC time>` | a copy of `findings.jsonl` from before a `rupu findings import` replaced it |

Workspace-level files, in `<workspace>/.rupu/coverage/` (not under a `<target_id>/`
directory):

| File | Contents |
|------|----------|
| `finding_tags.jsonl` | every tag add/remove event for the workspace's findings, across all targets |
| `finding_tags.jsonl.lock` | empty; the lock every writer of `finding_tags.jsonl` takes |

`<target_id>` is derived deterministically from `(workspace, scope_name)`, so the
same agent against the same repo accumulates into one target across runs, while
different workspaces stay distinct. Every row carries `run_id` + `model` +
`surface` attribution, which is what makes cross-run and cross-model analysis
possible.

## Turning it on: the `concerns:` block

An agent activates the harness by declaring a `concerns:` block in its
frontmatter. The block is a list of catalog includes (with optional overrides,
filters, and render mode). When present, the runtime flattens the catalog,
snapshots it, injects the catalog into the system prompt, and registers the
coverage tools.

```yaml
---
name: security-assessor
permissionMode: readonly
concerns:
  - include: owasp-top10-2021
    mode: full
  - include: cwe-top25-2023
    mode: full
  - include: secrets-in-source
    mode: full
  - include: stride
    mode: full
  - include: cwe-software-development   # ~399 CWEs
    mode: index                          # one-line table, searched on demand
---
You are a security assessor. For each (file × concern) you assess, call
coverage_mark; for each issue, call report_finding…
```

**Render modes.** `full` inlines each concern's body into the prompt; `index`
renders a compact one-line-per-concern table that the agent searches on demand
(use it for large catalogs like the full CWE list so the prompt stays small);
`auto` (default) picks based on catalog size.

### Bundled catalogs

`rupu coverage templates list` prints them all:

```
owasp-top10-2021        owasp-api-top10-2023
cwe-top25-2023          cwe-software-development   cwe-research
stride                  secrets-in-source          code-smells
web-security-default    api-security-default
```

User catalogs may live in `.rupu/concerns/` (project) or `~/.rupu/concerns/`
(global) and are discovered by name; project overrides global overrides builtin.

### Coverage tools the agent gets

When `concerns:` is set, these tools are injected automatically (you do **not**
list them in the agent's `tools:`):

| Tool | Purpose |
|------|---------|
| `coverage_mark` | record a `(concern, file)` verdict + evidence |
| `report_finding` | record an issue — a complete `report` under the `full` profile (default), or `summary` / `severity` / `evidence` under `summary`; see [Finding reports](#finding-reports) |
| `coverage_remaining` | list in-scope files still lacking an assertion |
| `coverage_status` | summary of assessed-vs-gap progress |
| `coverage_concerns_search` / `coverage_concerns_detail` | search / fetch full bodies for index-mode catalogs |

Two more tools are **not** injected: grant them explicitly in the agent's
`tools:` (like `report_finding` outside a `concerns:` agent). See
[Who can tag](#who-can-tag).

| Tool | Purpose |
|------|---------|
| `query_findings` | list the workspace's findings, filtered by tag, severity, concern or file; returns a page of slim rows plus `tags_in_use` (explicit `tools:` grant) |
| `tag_findings` | add or remove tags on one or many findings; returns each finding's tags before and after (explicit `tools:` grant) |

## Finding reports

A finding is recorded once, as structured data. That structured record is the
source of truth for the finding: rupu generates the rendered views and the
Markdown, HTML and PDF exports from it, so an agent never writes a separate
report file (see "Viewing reports in the control plane" and "Exporting reports"
at the end of this section).

### Profiles

| Profile | What `report_finding` / `findings.record` accepts |
|---------|----------------------------------------------------|
| `full` (default) | A complete `report` object. `summary`, `severity`, and `evidence` are **rejected**: rupu derives them (`summary` ← `title`, `severity` ← `rating.risk_rating`, `evidence.rationale` ← `root_cause`). |
| `summary` | The lightweight `summary` / `severity` / `evidence` record. A `report` is **refused**, never silently dropped. |

Pick a profile with the agent's `findingsProfile` frontmatter, a workflow's
`defaults.findings_profile`, a step's `findings_profile`, or `rupu run
--findings-profile` for a standalone run. Precedence is
step (or the flag) → workflow defaults → agent `findingsProfile` → `full`
(see `docs/agent-format.md` and `docs/workflow-format.md`). Remote workflow
units (`host:` / `distribute:`) get the same resolution: the step or default
value travels to the host as `--findings-profile`. The chosen profile
is stored on each finding record; records written before profiles existed read
back as `summary`.

> **Upgrading:** the built-in default is `full`, so an existing agent that
> records thin findings is rejected until it declares `findingsProfile: summary`
> or its prompt is updated to send a `report`. Agents outside the repo (for
> example under `~/.rupu/agents/`) need that one-line change.

### What `full` requires

Every field of the report is required except `cwe` (it may be an empty list)
and `artifacts`. The rules below are enforced at write time; a
rejected call returns **every** problem at once, each with its field path, so
the agent can fix them all in one retry. (A structurally malformed JSON argument
surfaces as a single parse error instead.) When a full-profile run can record
findings (the agent has a `concerns:` block or `report_finding` in `tools:`),
finding-writing guidance is also appended to its system prompt, so the agent
needs no external reporting-standard file.

`artifacts` is described under [Artifacts](#artifacts) below.

Some fields of a stored report are set by rupu, never by the reporting agent,
and are left out of the schema the agent is shown:

- `verification` (`{status: unverified|confirmed|disputed|inconclusive,
  by_run?, notes?}`) is set by verification runs, not by the agent that wrote
  the finding. A `report_finding` / `findings.record` call that supplies it is
  rejected at `report.verification`.
- Each evidence claim's `sha256` is the hash rupu takes of the claim's `file`
  at write time. Anything the agent sends there is discarded; a claim whose
  file is not in the workspace is stored without a hash.
- An artifact's `sha256`, `size`, `kind`, `stored`, and `host` are filled in
  when rupu stores it; the agent supplies only `path`.

- Required strings must be non-empty after trimming.
- Ratings (`impact`, `risk_rating`, `risk_factor`) are `Low`/`Medium`/`High`/`Critical`;
  `likelihood` is `Low`/`Medium`/`High`. `cwe` is a list of `CWE-<n>` ids and may be empty.
- `evidence` needs at least one claim and `replication_steps` at least one step.
- `file` values are workspace-relative and `lines` is `[start, end]` with `1 <= start <= end`.
- `cross_references[].finding_id` must be an existing finding id.
- The serialized report must fit `[findings].report_max_bytes` (default 256 KiB),
  so a pasted log cannot swell the ledger. The limit is checked again after
  artifact directories are expanded.

### Sentinels

Where information genuinely cannot be determined, a field uses a sentinel
instead of being omitted or guessed. On the structured fields (tickets, call
chain, patch, CI/CD detection, regression test, cross references) a sentinel is
accepted only in its exact form, and only the ones listed for that field:

| Sentinel | Used for |
|----------|----------|
| `Unknown` | owner, product, affected component, source repository, attack vector, CVSS score, tickets |
| `Not Applicable` | source repository, when no source-controlled code is involved |
| `None Provided` | tickets, when none are mentioned |
| `None` | cross references, when there is no related finding |
| `Not Provided — <justification>` (em dash) | call chain, recommended patch, CI/CD detection, regression test; the justification must be non-empty |

`Unknown` is **not** accepted for the patch, CI/CD detection, or regression
test: provide the item or say why it could not be produced.

### Artifacts

`report.artifacts[].path` lists proof-of-concept files (scripts, outputs,
harnesses) as workspace-relative paths. At write time rupu hashes each one:

- A file up to `[findings].artifact_max_bytes` (default 500 MiB) is copied into a
  content-addressed store at `<RUPU_HOME>/findings/artifacts/<aa>/<sha256>` and
  recorded `stored: copied`. Identical content is stored once across runs and
  projects, and the store outlives the workspace.
- A larger file is recorded `stored: external` with its path, size, and sha256.
- A directory expands to the files inside it, each handled by the same rule.
  Symlinks inside a directory are skipped.
- One report's `artifacts` are bounded before any of them is copied: at most
  `[findings].artifact_max_files` files (default 500), and the files that
  will be copied into the store may add up to at most
  `[findings].artifact_total_max_bytes` bytes (default 2 GiB). A file over
  `artifact_max_bytes` is recorded by reference and does not count toward
  that total, so it never rejects the finding. A larger set rejects the
  finding with the count or total named; list specific files instead of
  large directories. The check is per call: files named by evidence blocks
  are ingested afterwards, against what is left of the same limits, so a
  later block file can still be refused after earlier files were copied.
  Those copies are unreferenced, content-addressed blobs; a retry reuses
  them rather than storing them again.
- A path that escapes the workspace, names the workspace root itself (`.`),
  does not exist, or names something other than a regular file (a device,
  socket, or the like) rejects the finding, so a typo is not silently dropped.
- A remote workflow unit (`host:` / `distribute:`) runs `report_finding` on the
  host, so its artifacts are copied into **that host's** store. Every
  `rupu run` also streams its coverage (runs, file touches, concern
  assertions, findings, engagement assets) to
  `$RUPU_HOME/runs/<run_id>/coverage.jsonl`, which the host's connector
  delivers to the coordinator when the unit ends — success or failure — on
  every transport. The coordinator merges it under its own workspace's targets
  and records the unit's artifacts `stored: external` with `host` set to the
  host's id (their blobs stay in the host's store and are pulled on first
  download — see [Downloading artifacts](#downloading-artifacts); a unit
  placed on the coordinator's own `local` host shares its store, so its
  artifacts stay `copied`). In each of
  the cases below the step shows a `StepWarning` — in the control-plane run
  view (a marker on the run graph and a card in the event feed), in the
  Situation Room, in the CLI live view, and in the completion summary and
  `rupu workflow show-run` — and the unit is not failed:
  - no stream arrived (no begin line): the host predates coverage streaming,
    or its stream failed to start or was lost in transport;
  - the stream could not be collected or merged, or had malformed lines;
  - the stream may be incomplete — it was read while the unit may still have
    been running (the coordinator's poll failed after it saw the run, or its
    wall-clock budget ran out), or an SSH host's final copy of it did not
    arrive. It is merged, and the warning says findings recorded after it was
    collected may be missing.

  With `workspace: sync` the unit's returned delta still carries its
  `.rupu/coverage/`: the coordinator drops it when the unit's complete stream
  arrived and merged every line, and applies it otherwise — no stream, a
  stream that may be incomplete, or one with lines it could not read (a
  duplicate beats a loss). A failed unit returns no delta. When a unit did
  return one, the warning says whether the delta keeps its own copy or is
  the only one. A standalone run that
  never finished (no `run.json`) leaves `runs/<run_id>/` holding only its
  stream; it follows the run's transcript — `rupu transcript archive` moves
  it to `runs-archive/`, and `transcript delete`, `transcript prune` and
  `rupu cleanup` remove it.

Each evidence claim's `sha256` is taken only from a file that resolves inside
the workspace and is no larger than `artifact_max_bytes`; other claims are
stored without a hash.

#### Downloading artifacts

`GET /api/findings/:id/artifacts/:sha256` returns an artifact's bytes. The sha
must be one of that finding's artifacts or evidence-block files. A blob already in this control plane's
store is served directly. An `external` artifact with a `host` is pulled from
that host on first download and then kept in this store. How the host delivers
it depends on the transport:

- SSH runs the hidden `rupu __findings artifact <sha256>` on the host (one SSH
  invocation per pull).
- An HTTP host serves it from `GET /api/findings/artifacts/:sha256`
  (advertised as the `findings.artifact_blob` feature).
- A tunnel node streams it over the tunnel (`findings.artifact_pull`
  capability; the node must be online).
- A bucket worker uploaded every blob its run's findings reference to
  `artifacts/<sha256>` in the bucket when the run finished, whatever the
  outcome — before it published the run as finished, so a run the control
  plane sees as finished already has its blobs there — and the control plane
  reads it from there. A failed upload holds the run and is retried, but a
  blob that still fails after 3 passes is logged and released, and the
  control plane reports it unavailable. Coverage lines, by contrast, are held
  until they land.

A pull is capped at the recorded size, then checked against the recorded size
and sha256 before the blob enters the store. A recorded size over this control
plane's own `artifact_max_bytes` is refused without contacting the host. Concurrent first downloads of the
same blob share one pull. A local over-cap file (`external` with no `host`) is
served from the workspace while it still hashes to the recorded sha: `404` once
the file is gone, `409` once it has changed. Text artifacts are served as plain
text; PNG, JPEG, GIF and WebP images (recognised by their leading bytes,
never by file name) inline with their `image/*` type; everything else as an
attachment. Every response carries `X-Content-Type-Options: nosniff` and a
`Content-Security-Policy: sandbox`.

When a remote artifact's bytes cannot be had, the response is `404` with
`{"unavailable": "<reason>"}`: the host is unreachable or not registered, the
node is offline, the blob is not in the host's store (this includes a file the
host recorded by reference because it was over the copy cap), the host is too
old to serve it, the recorded size is over this control plane's
`artifact_max_bytes`, or the bytes failed the size or hash check. Nothing is stored
for a failed pull, and the next view tries again.

The web finding page's artifact browser uses the same endpoint. It previews
text up to 256 KiB when you open it and offers a download for every artifact. A
remote artifact is fetched from its host the first time it is previewed or
downloaded; a binary one only when you download it.

#### Evidence-block files

An `image`, `hexdump` or `pcap_ref` evidence block names exactly one file, in
its `artifact.path`. rupu verifies and stores that file the same way as a
`report.artifacts` entry: the same hashing, the same copy-or-reference rule,
and the same per-finding limits (`artifact_max_files` and
`artifact_total_max_bytes` are shared, so block files and PoC artifacts count
together). A block file is not a proof-of-concept artifact: it does not appear
in the PoC list. A bad path is rejected like an artifact path, with the field
named `report.blocks[<i>].artifact.path`.

`GET /api/findings/:id/artifacts/:sha256` serves block files exactly like
artifacts, including the first-view pull of a remote one from its host (see
[Downloading artifacts](#downloading-artifacts)); the bucket worker also
uploads them. The finding page renders every block kind in an "Evidence
blocks" section. An `image` block shows its picture inline; a block whose file
lives on a remote host loads only when you click it, so opening a finding
never pulls from a host.

### Configuration

```toml
[findings]
artifact_max_bytes = 524288000   # copy cap per artifact file (default 500 MiB)
artifact_max_files = 500         # files per report's artifacts (default 500)
artifact_total_max_bytes = 2147483648  # bytes copied into the store per report (default 2 GiB)
report_max_bytes = 262144        # serialized report budget (default 256 KiB)
ticket_patterns = ["ABC-[0-9]+"] # extra hints appended to the full-profile guidance
export_id_prefix = "VULN"        # number prefix for exported reports (default "SEC"; global config only)
```

`ticket_patterns` lets an organisation say which reference formats count as
existing tickets. Nothing organisation-specific ships in rupu. The keys are also
listed in [configuration.md](configuration.md#findings). `export_id_prefix` is
covered under [Exporting reports](#exporting-reports).

### `rupu findings schema`

```
rupu findings schema                 Print the embedded draft-07 JSON Schema of a finding report
rupu findings schema --advertised    Print the simplified copy used in tool definitions
```

To write reports out of the store, see [`rupu findings export`](#rupu-findings-export).

The schema is embedded in the binary and kept in lockstep with the validator by
a test, so external prompts and tools can be generated from rupu rather than
maintained by hand.

### Viewing reports in the control plane

The control-plane web UI (`rupu cp serve`) renders a stored `full` report; no
export step is needed to read one.

**Report page (`/findings/:id`).** A full-profile finding opens a report page
with a section rail (description, impact, location, root cause, call chain,
evidence, PoC artifacts, replication steps, remediation, patch, CI/CD and
regression commands, references, provenance), a header of CWE and verification-status chips above a ledger (ownership,
tickets, category, attack vector, impact, likelihood, risk rating, risk factor,
CVSS v3), and a completeness meter, `n/11`, that lists the gaps. The meter counts eleven
fields that can be left unanswered: owner, product, affected component, source
repository, tickets, CVSS v3, attack vector, call chain, recommended patch,
CI/CD detection, and regression test. `Unknown` and `Not Provided — …` count as
gaps; `None Provided`, `Not Applicable`, and `None` count as answers.
Call-chain steps link into the project's Code tab. Each evidence claim carries a
badge comparing the file hash recorded at write time with the file now
(`current`, `changed`, `missing`, or `unknown` when the file could not be
checked, for example above 64 MiB). The proof-of-concept browser lists the
artifacts, the patch renders as a diff, and the CI/CD and regression commands
have copy buttons. A `summary`-profile finding opens a compact page instead.

**Lists and triage.** The findings tables (the global Findings page, a
project's findings tab, and coverage detail) show a Report column with the
`n/11` count and a PoC marker for full-profile rows, and "summary" for the
rest. Expanding a
full-profile row shows a triage card (root cause, attack path, owner and
product with unknowns flagged, completeness, verification status) with an
"Open full report" link. The global Findings page also filters by profile,
owner, and CWE. Each full-profile finding row on a run's Findings tab has an
"Open report" link, and the command palette opens findings at `/findings/:id`. In the Code
tab, a full-profile finding's inline card has tabs (Root cause, Call chain,
Evidence, Patch, Repro) that load the report when the card is expanded.

**API.**

- `GET /api/findings` and the `findings` array in `GET /api/coverage/:target`
  return slim rows without the report body. A full-profile row carries
  `report_summary`: `owner`, `product`, `cwe`, `root_cause`, `chain`,
  `completeness` (`filled`, `total`, `gaps`), `has_poc`, and
  `verification_status`.
- `GET /api/findings/:id` returns the whole record with its `report` and an
  `evidence_status` per evidence claim (`current`, `changed`, `missing`,
  `unknown`). Claim files are hashed off the async runtime; a file over
  64 MiB reports `unknown` (a much lower limit than the artifact copy cap,
  because every detail request re-hashes every claim's file).
- `GET /api/findings/:id/artifacts/:sha256` serves an artifact only if that
  finding lists it; `:sha256` must be 64 lowercase hex characters. Every
  response carries `X-Content-Type-Options: nosniff` and
  `Content-Security-Policy: sandbox`. Where the bytes come from, how text and
  other kinds are served, and what a missing, changed or unpullable artifact
  answers are under [Downloading artifacts](#downloading-artifacts).
- `GET /api/findings/artifacts/:sha256` is the host-side half of that pull: it
  serves any blob in this control plane's own artifact store by hash. It sits
  behind the CP's bearer token when one is configured, and is open to anyone
  who can reach the CP otherwise. It is not finding-scoped, so that access
  control is its only boundary; browsers use the finding-scoped endpoint above.

### Exporting reports

A stored finding can be exported as Markdown, self-contained HTML, or PDF, for
one finding or for a whole project. The Markdown, HTML and PDF documents all
carry the same sections in the same order, and every field of the report is in
them: CWE, verification, ticket notes, claim-to-artifact links, each call-chain
hop's role (`source`, `hop`, `sink`), artifact kind and host, and provenance
(surface, concern, scope, location). Free-text fields that
the report stores as Markdown (the location input and output among them) render
as Markdown. A `summary`-profile finding exports as a short document built from
its summary, severity, location, rationale and provenance, marked as a summary
finding.

**Numbering.** Exports number findings within their project: by severity
(critical first), then by when the finding was declared, then by id, formatted
`<PREFIX>-NNN` (`SEC-001`, `SEC-002`, …). Numbers are assigned across all of a
project's findings before any filter is applied, so a finding keeps its number
whatever else a report leaves out, and the CLI and the control plane agree on
it. A number is a display label for a point in time, not a stored identifier:
recording a new critical finding renumbers the ones after it. The `fnd_` id is
the stable handle, and a project report's index lists number and id side by
side. A cross-reference to another finding of the same project prints that
finding's number, even when a filter leaves it out of the report.

The prefix comes from `[findings].export_id_prefix`, default `SEC`. It is read
from the **global** config (`~/.rupu/config.toml`) only: a project's
`.rupu/config.toml` never changes it, because that file is repo-controlled and the
prefix ends up in file names and document text. It must match
`^[A-Za-z][A-Za-z0-9_-]{0,15}$`; anything else is ignored with a warning and the
default is used.

**File names.** A finding is named `<NUMBER> - <Short Title>.<ext>`, for example
`SEC-003 - SQL injection in the search endpoint.pdf`. The title is shortened
(80 characters), whitespace is collapsed, and path separators, Windows-reserved
characters, control characters, bidirectional-override characters and
zero-width characters are removed. A whole name is at most 200 bytes, extension
included (a title in a multibyte script is cut, at a character boundary, to
fit), so it can be written or extracted where names are limited to 255 bytes.
Entries in a split zip are cleaned the same way, de-duplicated, and dated with
the time the report was generated. A project report is named after its title
(default "Findings report").

**Formats.**

- **Markdown** (`md`): plain text, with the sections in the same order as the
  other formats. Every finding's document opens with a `Filename:` line giving
  the suggested PDF file name (`<NUMBER> - <Short Title>.pdf`), in every export
  format, not only PDF. Report text is kept as Markdown, with three changes so
  it cannot restructure the document around it: a `<` that starts a line
  outside a code block is escaped (an unclosed `<script>` or `<!--` would
  otherwise turn every later section into raw HTML), headings are moved two
  levels down (`#` becomes `###`, as in HTML and PDF), and an unclosed code
  fence is closed. Code blocks are copied unchanged.
- **HTML** (`html`): one self-contained file with inline CSS. It loads nothing
  from the network and runs no script; a strict Content-Security-Policy
  (`default-src 'none'`, no `<base>`, no form posts) and a no-referrer policy are
  embedded in the document as defence in depth. Report text is escaped or passed
  through a Markdown converter that neutralises raw HTML and script-like URLs,
  and images in report text are rendered as their alt text rather than loaded.
  The only images it shows are `image` evidence blocks, inlined as `data:` URIs
  (see below).
- **PDF** (`pdf`): generated in process with Typst, with no headless browser
  and no external tools. Fonts are bundled (Libertinus Serif, New Computer
  Modern, DejaVu Sans Mono), so output does not depend on the machine; the flip
  side is that the bundle has no CJK or emoji glyphs, so those characters do not
  render in a PDF (Markdown and HTML keep them). The PDF renderer cannot read
  files from disk, so a report cannot pull in a local file or image; the only
  files it sees are the `image` evidence blocks' bytes, handed to it in memory.
  PDF export
  is behind the `pdf` cargo feature, on by default and forwarded by `rupu-cp`
  and `rupu-cli`; it adds roughly 45-55 MB to a release binary. A build without
  it (`--no-default-features`) still exports Markdown and HTML, and asking for
  PDF fails with "compiled without PDF support" (a non-zero exit from the CLI,
  `501` from the control plane).

**Evidence-block files.** An `image` block whose file is in this machine's
artifact store (recorded `stored: copied`, no `host`) is embedded when its
bytes are a PNG, JPEG, GIF or WebP image — recognised by their leading bytes,
never by name, so an SVG is never embedded — and the blob is at most 4 MiB and
still hashes to its recorded sha256. HTML inlines it as a `data:` URI and PDF
places the image itself, both with the caption under it; Markdown references
it by its recorded path (`![caption](<path>)`, never for a path that looks
like a URL) and inlines no bytes. A file that is `external`, lives on another
host, is missing from the store, too big or not a raster image is shown by
path, sha256 and size with a note saying why it was not embedded. An export
never fetches anything from another host, even for a file the finding page
would pull on view. An image Typst cannot decode becomes that note in the PDF
instead of failing the export. A `hexdump` block shows its `rendered` text as
a code block, its base in hex (exact for any 64-bit value), and its file; a
`pcap_ref` block shows its summary and its file. The files of those two are
never read by an export.

**Project reports.** A project report has a title, a summary of what it covers,
an index (number, severity, title, project, finding id, profile), then each
finding as its own section (its own page in the PDF). With **split**, the export
is a zip holding `index.md` (always Markdown) and one file per finding in the
chosen format. Summary-profile findings are left out unless asked for; naming
one with `--id` counts as asking.

#### `rupu findings export`

```
rupu findings export [--id <fnd_…>]… [--project <ws_id|path>] [--run <run_id>]
                     [--severity <critical|high|medium|low|info>]
                     [--owner <name>] [--cwe <CWE-n>] [--include-summaries]
                     [--to md|html|pdf] [--split] [--title <text>] -o <path>
```

The document format is `--to` (default `md`), not `--format`: `--format` is
rupu's global output flag (`table`, `json`, `csv`) and has nothing to shape here.
Filters combine: a finding must pass all of them. `--severity` keeps that
severity and worse. `--run` keeps findings declared by that run and its
sub-runs. `--cwe` compares CWE numbers: `CWE-79` (or `cwe-79`, or `79`) keeps
findings whose report lists CWE-79 or whose concern is CWE-79
(`cwe-top25-2023:cwe-79-xss`), never CWE-798 or CWE-179; a value that is not a
CWE id is a usage error. `--project` must be a registered project: a workspace
id, or the path of a registered project's checkout. Anything else is an error
("no project matches ...").

Exactly one `--id` on its own writes that finding as a stand-alone document.
Adding any of `--project`, `--run`, `--severity`, `--owner`, `--cwe`, `--split`
or `--title` turns it into a project report over the selection, and so do
several `--id`s or no `--id` at all. `--include-summaries` does not: a single
`--id` names its finding, so it is included whatever its profile, and the flag
changes nothing. `-o` is a file path, or an existing directory to receive the
generated file name. A write failure reports the OS cause, and an `-o`
extension that does not match the format (a zip written to `report.pdf`, say)
produces a warning but is still written.

```bash
# One finding as a PDF, named SEC-003 - <title>.pdf, into the existing ./reports
rupu findings export --id fnd_01J8… --to pdf -o ./reports

# Everything high or worse in one project, as a single HTML report
rupu findings export --project ~/src/service --severity high --to html \
  --title "Service assessment" -o assessment.html

# A finding per file plus an index, for one run's findings
rupu findings export --run run_01J8… --to pdf --split -o run-findings.zip
```

#### In the control plane

On a finding's report page, **Markdown**, **HTML** and **PDF** buttons download
that one finding; a failure shows inline on the page. The global Findings page
and a project's Findings tab have an **Export report** button that opens a
dialog for exactly the rows currently listed (filters applied): choose the
format, tick **One file per finding (zip)** to split, tick **Include summary
findings** to add the `summary`-profile rows (off by default), and set the
title. The download is named by the server.

- `GET /api/findings/:id/export?format=md|html|pdf` returns one finding.
- `POST /api/findings/export` takes `{format, title?, ids?, ws_id?, run_id?,
  min_severity?, owner?, cwe?, include_summaries?, split?}`. Unknown fields are
  rejected with `422` (a misspelt filter must not widen the report), and the
  body of that rejection is plain text rather than the API's usual JSON error.
  `cwe` is compared by number, as `--cwe` is. A missing or unknown `format`, an
  unknown `min_severity`, a `cwe` that is not a CWE id, or a `title` over 200
  characters is `400`; a selection that matches nothing is `404`; PDF from a
  build without the `pdf` feature is `501`.

Both are attachments with `Content-Disposition: attachment` and
`X-Content-Type-Options: nosniff`; an HTML response also carries
`Content-Security-Policy: sandbox`. The control plane uses the same numbering
and the same global-config prefix as the CLI. At most two PDF exports render at
once in `cp serve` (a Typst compile is CPU- and memory-heavy); further PDF
requests wait their turn, while Markdown and HTML are never held up.

### Importing reports written before the full profile

Findings recorded before the `full` profile existed are summary findings, with
their report written separately as Markdown. `rupu findings import` attaches
those reports to their findings. It is a one-time migration aid, not an input
path: nothing else writes a report onto an existing finding, and the parser is
best-effort.

```
rupu findings import <PATH>... [--id <fnd_…>] [--dry-run]
```

**Which files.** `PATH` is a report file, or a directory searched recursively
for `*.md` files. While searching, hidden files and directories are skipped and
symlinks are not followed (a symlinked file or directory is not found; name a
symlinked file outright and it is read). A file named outright is always read,
whatever its extension. A file named twice, or by two spellings (`a.md` and
`./a.md`), counts once. A file over 4 MiB fails. A named path that does not
exist, and a named directory that cannot be listed, fail the command before
anything is imported, naming the path and the OS error. A named file that
cannot be read, and a file or directory found while searching that cannot be
read, become a `failed` line for that path (with the error), and the rest go
on. A search that finds no Markdown file at all fails with `no Markdown
reports found under <paths>`.

**Layouts read.** Two spellings of the report layout: the one `rupu findings
export --to md` writes, and a plain-text one with bare heading lines
(`Description`, `Root Cause`, …) and `Label: value` fields (`Impact: High`,
`Finding ID: fnd_…`). A file with fewer than three of the layout's section
headings (a README, an index) is skipped, not failed, unless it has a Finding
ID line (below): then it fails with `has a Finding ID line but is missing the
report sections`. Named with `--id`, such a file fails too (`not a finding
report`). A file that holds several findings is refused; split it into
one file per finding first. The title is the file's `# ` heading; failing that,
the first line after `Filename:` when that line is not a field; failing that,
the first line that is not a field. A line before the title (a banner) is kept
as other text (below). A heading at the sections' level that names none of
them (`## Disclosure Timeline`) is kept with its text as other text, not run
into the section before it. The CVSS and Risk Factor lines are read where the
layout puts them, at the end of References (a line of the references
themselves that starts `CVSS:` stays a reference), else from a section of
their own after References (`## Scoring`), else, for the Risk Factor, one
before it.

**Which finding.** The one id the report's Finding ID line states. The labels
read, in any case and as `Label:`, `**Label:**` or `**Label**:`, are `Finding
ID`, `Native Finding`, `Native Finding ID`, `Rupu Finding`, `Rupu Finding ID`
and `Native Rupu Finding`. `rupu findings export` writes `**Finding ID:**` in
the report's Provenance. The line may be in the header or in any section but
Cross-References, and never inside a code block. An id mentioned anywhere else
(in prose, in Cross-References) is never taken for the report's own. The same
id on two lines counts once. A Finding ID line that says more than its id
(`Finding ID: fnd_… (merged from an earlier report)`) is kept as other text;
one that says only the id is not. A report with no Finding ID line fails with
`no Finding ID line; import it alone with --id`: `--id <fnd_…>` names the
finding for a single report file (`--id` with a directory, or with several
files, is refused). A report whose Finding ID lines name two or more different
findings fails (`cites several finding ids on Finding ID lines (…)`), with or
without `--id`, and so does an `--id` that differs from the id the report
states (`the report says <id>; --id says <id>`). The finding is looked up
across the registered projects, as `rupu findings export` does, and must be in
exactly one ledger (a project keeps one per coverage target). Two files for
the same finding fail both.

**What happens to the finding.** It becomes a full-profile finding. The report
goes through the same validation as `report_finding` and attaches whole or not
at all: a report that fails validation is listed with every problem and changes
nothing, and the other reports in the run are still imported. `summary`,
`severity` and `evidence` are re-derived from the report, as for any full
finding; the id, provenance (run, model, surface, declared-at), location and
every other field of the record are kept. A finding that already has a report
is skipped, never changed. A finding records no engagement asset, so an import
runs no [engagement profile](#engagement-profiles) completeness check and
stamps no asset: the report is held to the finding report contract only. The size limits (`report_max_bytes` and the
`artifact_max_*` keys) come from `[findings]` in the global config only;
unlike when an agent records a report, a project's `.rupu/config.toml` is not
layered in.

**Evidence and artifacts are as they are at import.** An imported report's
evidence claims are stored without a hash of their files (an agent's claims
get one when it records them): the claims were made against the code as it
was when the report was written, and hashing today's file would present them
as current. The control plane shows their evidence status as `unknown`.
Artifacts (and any file an evidence block names) are verified and copied from
the workspace as it is at import, not as it was when the report was written.

**Backups and the lock file.** Every real import that attaches anything first
copies the ledger byte for byte (and syncs the copy to disk) to
`findings.jsonl.pre-import-<UTC time>` beside it, for example
`findings.jsonl.pre-import-20260930T101500Z`, so each such run leaves one more
backup. The ledger is then replaced atomically under the ledger lock: every
other line is written back unchanged. The lock is `findings.jsonl.lock`, in the
same `.rupu/coverage/<target>/` directory; every writer of the ledger creates
it, and it is empty. On a filesystem that cannot lock at all (some network and
FUSE mounts), agents still record findings, without the lock and with a logged
warning, but an import cannot write there: it needs the lock, so every report
that would have attached to that ledger fails with `cannot update` (reports
rejected for other reasons still list their own problems). Once you have checked the imported
findings, delete the backups: each is a full copy of the ledger, and a
workspace sync or a commit of `.rupu/` would carry it along. A ledger that is a
symlink is never replaced (the rename would replace the link, not the file it
points to): its reports fail with the reason, on a dry run too. An import
interrupted (Ctrl-C, a crash) after taking its backup and before replacing the
ledger leaves the ledger as it was, plus that backup and
`findings.jsonl.import-tmp` beside it; delete both, or just import again (the
next run overwrites the temp file and takes a backup of its own).

**Dry run.** `--dry-run` parses and validates every report and prints `would
attach` for those that would go in. It writes nothing: no ledger change, no
backup, and it takes no lock, so it also works on a read-only ledger directory.
It checks that each artifact the report lists, and each evidence-block file,
exists inside the workspace and is within the artifact count and size limits, and that the report as it would
be stored (directories expanded, every artifact recorded with its hash, size,
kind and storage) is within `report_max_bytes`. It copies nothing: it reads
only the first 8 KiB of each artifact, to tell text from binary as a real
import does. Trouble that only appears while copying an artifact is found only
by a real import.

**Missing content.** Nothing is invented. A field or section the schema has no
sentinel for must be in the file, or the file fails: the title; Category,
Attack Vector, Impact, Likelihood, Risk Rating and Risk Factor; and the
Description, Impact, Location, Root Cause, Evidence, Remediation, Replication
Steps and References sections (Evidence and Replication Steps need at least one
entry). A rating that is not a level the schema allows fails too, and so does a
range (`High/Critical`, `Medium-High`, `Medium to High`). Where the
schema has a sentinel, it is used: `Unknown` for a missing Owner, Product,
Affected Component, Source Repository, CVSS or ticket-references field; `None`
for a missing Cross-References section; and `Not Provided — section missing from
the imported report` for a call chain, patch, CI/CD detection or regression test
section the file lacks. A part missing from a section that is present, such as
the stage of a CI check, is `Not stated in the imported report.` Text with no
field to hold it is kept at the end of References, under `Other imported text:`,
one `From <where>:` block per place it came from (text before `Step 1:`
included: it is not a step; in a plain list of steps, text before the first
item is the first step).

**Cross-references, evidence and artifacts.** The Cross-References text is kept
verbatim in References. Each `fnd_` id it names also becomes a cross-reference
link, but only to another finding in the same ledger: an id in a different
ledger, and the finding's own id, are not linked. Code blocks in a call chain
become evidence claims. A claim's location is the first `path:lines` in it
whose path has a directory (`src/routes/notes.rs:40-58`,
`node_modules/@types/node/index.d.ts:10`), else a bare file name with a range of
lines (`notes.test.ts:10-20`), or in a code span when it has one dot at most or
ends in a source file's extension (`` `notes.rs:40` ``, `` `.env.local:3` ``,
`` `user.service.ts:42` ``). A host and port that reads as one is
not a location: an address (`10.0.0.5:9229`), a user before an `@`
(`admin@db:5432`), a name ending in a common top-level domain
(`notebin.example.com:443`), or a name with several dots and no range
(`` `debug.notebin.de:9229` ``); a bare name such as `` `db.prod:5432` `` cannot be
told from a file and is read as one. A list right after a line that introduces
it (`The handler skips two checks:`) is part of that line's claim, unless an
item names a place of its own (in a report `rupu findings export` wrote, which
prints each claim as one paragraph, always); a claim of several paragraphs
comes back as one claim per paragraph. Artifacts the report lists (an Artifacts table or list)
must exist in the finding's workspace; the report is refused with the reason
when one does not. A list item's path is its leading code span (`` `poc/x.sh` ``),
else its first word; an item that says more than its path is also kept as
other text. With no CWE field, a report `rupu findings export` wrote has none
(it prints the field whenever there are ids); in any other report the CWE ids
are those in Category and on the
References lines that start with one (`CWE-639: …`, `- CWE-639 …`; every id on
such a line counts); one mentioned in passing ("unlike CWE-79 …") is not taken.
A Recommended Patch section with a diff block is a patch, whatever its text
says; the text becomes the patch's notes. A report `rupu findings export`
wrote reads back field for field, with these exceptions: its Classifications
come back without their vectors (not printed), and a classification whose id
holds a space or a comma does not read (the field is kept as other text); a
`text` evidence block comes back as an evidence claim (it is prose, like a
claim); and a claim of several paragraphs comes back as several claims.

**Evidence blocks.** The Evidence section's typed blocks are read back in the
shapes the exporter writes them, in any report: a code slice (`**Code**`, or
in an exported report `` **`file`** ``, then a code block; in any other report
a bold place with code after it is a claim at that place), `**Diff**`, `**Decompiled** (lang)`,
`**Disassembly** (arch)` (its listing must read back to exactly what was
printed, or the block stays text), `**Scan output** (tool)`, `**HTTP request**`
and `**HTTP response**` each followed by its code block; `**Hexdump** (base
0x…) — `` `path` ``, with the untagged code block after it as its rendered dump;
`` _Packet capture: summary — `path`_ ``; a table; and an image, as the
exporter's `` _caption — `path`_ `` (in an exported report only: elsewhere an
italic line naming a file is a claim) or as `![caption](path)` (`<path>` when
it has spaces; one with a title is not read). A block's file must be a
workspace-relative path — not absolute, no `..`, not a URL — or the block is
read as a claim; like an artifact, the file must exist in the finding's
workspace, where it is verified and stored when the report is attached
(see [Evidence-block files](#evidence-block-files)). The schema needs at least
one evidence claim, so an Evidence section that is all blocks is read as claims,
as it was before blocks were read. An export made by a rupu that predates
`rupu findings import`, whose exporter printed its command blocks without a
`Command:` line, is told apart by its Regression Test (which always has a
command) and reads back too: each section's last `sh` block is its command (so
a CI/CD check that had no command, whose text ends in an `sh` block, gets that
block as its command). With no regression test to tell, its commands stay in
their sections' text.

**Output.** One line per file: `attached` (`would attach` on a dry run),
`skipped` (not a report, or the finding already has one) or `failed` with the
reason and, for a report that fails validation, each problem beneath it. Then a
`backup` line for each ledger that was rewritten, and a totals line. The exit
status is non-zero when any file failed. A ledger that cannot be updated (it
cannot be locked or written, is a symlink, or changed while the import ran)
fails the reports that would have attached to it (`cannot update <ledger>:
<why>`) and nothing in it changes; a report in the same ledger that fails
validation still lists its problems, and the other ledgers are imported.

**Known limit.** The ledger lock only excludes writers that take it. An unlocked
append that changes the ledger's length before the import replaces it is almost
always detected, and that ledger is then left as it was; one that lands in the
instant between that check and the replacement, or through a file handle opened
before it, is lost. Writers that do not take the lock are placed and remote runs
that sync their ledger back, and long-running local processes started from a
rupu older than the import (a session worker, `cp serve`). Stop those before
importing. The import also holds the lock while it copies artifacts, so an agent
recording a finding on the same target waits until it finishes: run it when no
agents are recording.

## Tagging findings

A finding can carry free-form tags (`needs-poc`, `class:sqli`, `status:triaged`)
that agents, workflow steps and operators use to sort, filter and queue
findings. Tags are not part of the finding's report and never change its
severity or its id.

### Syntax

A tag is trimmed and lowercased, then checked: only `a-z`, `0-9` and
`. _ : / -` are allowed, the first character must be a letter or digit, and it
is at most 64 characters long. So `Needs-POC` is stored as `needs-poc`. A tag
that fails the check is rejected with an error naming it; it is never rewritten
into a valid one. `namespace:value` (`class:sqli`, `status:triaged`) is a
convention for grouping, not something rupu interprets.

A finding has at most 32 tags. A change that would take a finding past 32 is
refused. A finding already over the cap (two remote units' tags can add up past
it when they are ingested) can still lose tags or swap one for another, just
not gain any.

### Who can tag

- **Agents.** `report_finding` takes an optional `tags` array, so a finding is
  tagged as it is declared. Two more tools are explicit `tools:` grants, like
  `report_finding` (an agent without the grant is not offered them):

  ```yaml
  tools: [read_file, query_findings, tag_findings]
  ```

  `query_findings` lists the agent's own workspace's findings matching a query
  (`{q, limit, cursor}`; the language is under "Querying findings" below). It
  returns `{rows, next_cursor, total,
  tags_in_use}`: one page of slim rows, a cursor for the next, the number of
  matches, and every tag already used in the workspace with its count (so an
  agent reuses a tag instead of inventing a near-duplicate). `limit` defaults
  to 50 and is at most 500. `tag_findings` adds or removes tags on one or many
  findings and returns each finding's tags before and after. Both work only on
  the agent's own workspace, and `tag_findings` is allowed in `readonly` mode:
  it annotates the ledger and never touches the workspace's files.
- **Workflow `action:` steps.** The MCP catalog has `findings.query` and
  `findings.tag` (same inputs and results as the agent tools), and
  `findings.record` accepts `tags`:

  ```yaml
  - id: mark
    action: findings.tag
    with: { finding_ids: ["fnd_01J…"], add: ["needs-poc"] }
  ```

- **Operators**, through `rupu findings` (below).

### Where tags live

Tags a finding was declared with are on the finding record in its target's
`findings.jsonl`. Every later add or remove is an event in one file per
workspace, `<workspace>/.rupu/coverage/finding_tags.jsonl`: workspace-wide
(finding ids are globally unique, so no per-target routing is needed),
append-only, with its own `.lock`. A finding's effective tags are its declared
tags with the events applied in file order, and every reader (agent tools, MCP,
CLI) folds on read. Each event records who made it: the agent's attribution
(run, agent, model) or an operator (`$USER`, and whether through the CLI or the
control plane). A change that alters nothing (adding a tag the finding has)
writes no event. A batch with an unknown finding id or a finding that would pass
the cap is rejected whole by the agent tools and MCP; `rupu findings tag` is
atomic per workspace (a batch that spans workspaces applies to each independently,
so the others keep their changes when one is refused).

### CLI

```
rupu findings list [--limit N] [--ids-only] [QUERY]…
rupu findings tag <ID>… [--add TAG]… [--remove TAG]…
rupu findings tags [QUERY]…
```

`list` spans every project; narrow it with a query (see "Querying findings").
Flags go before the query words: a query word that starts with `--` is
refused with a hint, since it would otherwise read as a negated word.
`--limit N` shows the first N, and says "showing N of M findings" on stderr
when it left some out. `--ids-only` prints bare ids, one per line. `tags`
lists the tags in use with how many findings carry each, over the findings the
query selects (all of them with no query).

```bash
rupu findings tag fnd_01J… --add needs-poc --add class:sqli
rupu findings list 'severity>=high' tag:class:sqli,class:xss
rupu findings list --ids-only -has:tags
rupu findings tags

# Tag everything matching a query: a lone `-` reads ids from stdin
rupu findings list --ids-only tag:class:sqli | rupu findings tag - --add needs-poc

rupu --format json findings list tag:needs-poc
```

`tag` changes findings across projects, project by project. An unknown id, or a
workspace that could not be changed (a finding past the cap, an unwritable log),
makes the command exit non-zero after the other workspaces have kept their
changes; its output says which were not changed. `--format json` works with all
three commands (`--format table` is the default).

### Remote units

A placed unit (`host:` / `distribute:`) tags findings in its own workspace.
Those changes travel to the coordinator in the run's coverage stream, as `tags`
lines next to the `findings` lines, and `ingest_unit_stream` appends them to
the coordinator workspace's tag log (a replayed event is dropped as a
duplicate), so they reach the coordinator exactly as the unit's findings do.

### Limits

- Tags belong to a finding id. A vulnerability an agent reports again is a new
  finding and starts with no tags.
- Tagging a finding that exists only in a remote host's own ledgers, and was
  never ingested into the coordinator, is not supported.
- The control plane's findings page filters and shows tags (a read-only Tags
  column), but bulk tagging and the report page's tag editor are a later plan.
  Today tags are written through the agent tools, MCP and `rupu findings tag`.

## Querying findings

One single-line query language filters findings on every surface: `rupu
findings list|tags`, the agent tool `query_findings`, the MCP tool
`findings.query`, and the control plane's Findings page.

```
severity>=high tag:class:sqli -tag:false-positive -has:poc "sql injection"
```

### Grammar

- Tokens are separated by whitespace, and every token must match (AND).
  Repeating a key ANDs too: `tag:a tag:b` needs both tags.
- `key:value` filters a field. `key:a,b` matches any of the values (OR within
  one token). `-key:value` negates the token.
- Only `severity` takes `>=`, `>`, `<=` and `<`, with one value (`severity>=high`).
- A bare word or quoted phrase is free text, matched case-insensitively against
  title, summary, id and file path. `-word` negates it.
- A token shaped like letters plus an operator (`word:`, `word>=`) names a key,
  so an unknown key is an error, never text. To search for text containing a
  colon, quote it: `"http://host"`.
- Quotes (`"` or `'`) open only at the start of an item: the start of a token
  (after an optional `-`), right after a key's operator, or right after a `,`
  in a keyed value. `\` escapes the next character. A quote anywhere else is
  an ordinary character, so `a="c d"` is the two words `a="c` and `d"`. Text
  glued to a closing quote is an error.
- A tag value may itself contain `:` (`tag:class:sqli`): only the first `:`
  after the key splits.

An invalid query does not run; it is refused with an error naming the token and
a code: `unknown_key`, `empty_value`, `bad_value` (not a valid severity,
enum value, tag or CWE), `bad_operator` (a comparison on a key other than
`severity`, or with several values), `unclosed_quote`, `bad_quote`.

### Fields

| Key | Matches |
|---|---|
| `severity` (`sev`) | `info`, `low`, `medium`, `high`, `critical`; also `>=`, `>`, `<=`, `<` |
| `tag` | an effective tag, normalized like any tag (`Class:SQLi` is `class:sqli`) |
| `has` | `tags`, `report`, `poc` (the report has artifacts), `cwe` |
| `project` | workspace name or id (CLI and control plane only) |
| `cwe` | `79` or `CWE-79`, by number: the report's CWE or the one the concern names |
| `owner`, `product` | the report's ownership, case-insensitive |
| `verified` | `unverified`, `confirmed`, `disputed`, `inconclusive` (no verification counts as `unverified`) |
| `profile` | `full`, `summary` |
| `scope` | `line`, `file`, `repo`, `host`, `endpoint`, `resource` |
| `concern` | concern id, case-insensitive |
| `agent` | agent name or codename, case-insensitive |
| `workflow` | workflow name, case-insensitive (CLI and control plane only) |
| `file` | path prefix |
| `run` | run id; the CLI and control plane also match the run's sub-runs |
| `id` | finding id |

`project:` and `workflow:` need provenance, which only the CLI and control
plane have: the agent tool and MCP refuse a query that uses them. There
`run:` is an exact match on the run id.

### On each surface

```bash
# CLI: quote what your shell would treat as a redirect or split on spaces
rupu findings list 'severity>=high' tag:needs-poc -has:poc
rupu findings list 'owner:"payments team"' run:run_01J…
rupu findings tags severity:critical
```

`>` and `<` are shell redirects, so `severity>=high` unquoted writes a file
named `=high`; quote the query word (or the whole query). Put `--limit` and
`--ids-only` before the query words.

```yaml
# Agent tool and MCP: {q, limit, cursor}; limit defaults to 50, at most 500
- id: queue
  action: findings.query
  with: { q: "severity>=high tag:needs-poc -verified:confirmed", limit: 20 }
```

An agent calls `query_findings` the same way (`{"q": "has:poc tag:class:xss"}`);
a bad `q` comes back as a tool error with the parse message, and `cursor` is
the previous page's `next_cursor`.

Control plane: the Findings page has a query bar. Press `/` to focus it. Typing
offers fuzzy suggestions (keys, then values with counts); Enter or Tab accepts
one, Escape clears the draft, and Backspace on an empty draft turns the last
chip back into text. An invalid query shows its error on the page instead of
stale results. The query lives in the URL, so
`/findings?q=severity%3Ahigh%20tag%3Aneeds-poc` is a shareable link, and the
severity tiles toggle `severity:<x>` in it. The table has a Tags column, and a
banner names any workspace whose tag log could not be read (its findings still
show with their declared tags, so tag filters may miss them).

The same query reaches the API as `GET /api/findings?q=…`. The response adds
`facets` (per key, `[{value, count}]` over the scope-filtered but unqueried set;
they feed the suggestions and tiles) and `tags_unavailable` (the workspace ids
above). An invalid `q` is a 400 with `{error, token, code, start, end}`, offsets
in characters.

## CLI

All inspection commands take the global `--format table|json|csv` flag
(`table` is the default; structured commands support `table`/`json`, tabular
ones also `csv`).

```
rupu coverage list                          List targets under .rupu/coverage/
rupu coverage templates {list, show}        List bundled catalogs / print one's concerns
rupu coverage catalog <target>              Print the effective catalog snapshot
rupu coverage show <target>                 Derived view: files touched + assertions + findings
rupu coverage audit <target>                Full report: per-concern coverage, gaps, cross-model, serendipitous findings
rupu coverage gap <target>                  Just the gaps (in-scope files lacking an assertion)
rupu coverage runs <target>                 List the runs recorded against a target
rupu coverage diff <target> [base compare]  What changed between two runs (defaults: previous latest)
rupu coverage rerun <target> <run_id>       Replay an agent run, appending a new run to the same target
```

Find a target id with `rupu coverage list`, then inspect:

```bash
rupu coverage audit a1b2c3d4e5f6          # human report
rupu --format json coverage audit a1b2c3d4e5f6   # machine-readable
rupu --format csv  coverage runs  a1b2c3d4e5f6
```

## The rerun → diff loop

Model output varies run-to-run — sometimes usefully (different angles surface
different bugs), but you could never evaluate *the combination* for
completeness. Now you can:

```bash
rupu run security-assessor "assess this repo"   # run 1
rupu coverage runs <target>                      # grab run 1's id
rupu coverage rerun <target> <run_id>            # replay it → run 2, same target
rupu coverage diff <target> <run_id> latest      # what did run 2 do differently?
```

`diff` reports four dimensions: **cell-coverage delta** (newly / no-longer
asserted), **verdict flips** (with `clean → finding` flagged), **findings
appeared / disappeared**, and **file-touch delta**.

> v1 `rerun` dispatch covers the **agent** surface; session / workflow / autoflow
> runs are captured but `rerun` returns an explicit "not yet supported" error.
> A replay re-resolves provider/model/concerns from the agent's current
> frontmatter (it goes through `rupu run`), so those manifest fields are a record
> of the original run, not replay inputs.

## Determinism

Everything the harness controls about what the model sees — concern ordering,
the catalog snapshot, and the live file list — is byte-stable and independent of
the order catalog inputs are declared in. That makes the model the *only* source
of run-to-run variance, which is exactly what `diff` measures. The harness does
**not** claim byte-identical model output: prompt *construction* is
deterministic; sampling is not.

## Engagement profiles

A finding is about an **asset**, and the asset has a **kind**. Code
(`file`/`function`) is one kind; a binary `function@address`, a network
`host`/`service`, a cloud `resource`, a web `route` are others. An **engagement
profile** is a pure-data package (TOML) that declares a domain's asset kinds and
their locator coordinates, the evidence blocks and taxonomies it uses, a
completeness checklist, and a coverage depth ladder. Adding a new engagement
type is authoring a profile, not writing Rust.

This governs how a finding is **validated and routed** — it is not a sandbox and
does not run, gate, or scope your tools. Agents reach binaries, hosts and
services with bash and whatever tooling they want (nmap, curl, radare2, …); rupu
records and validates the resulting *findings*, not the traffic.

### Selecting a profile

```bash
rupu run --engagement-profile binary  my-agent "reverse this blob"
rupu run --engagement-profile network my-agent "assess 10.0.0.0/24"
rupu run --engagement-profiles pentest my-agent "..."   # a composite = network + web
```

An empty selection is the native `code` path — byte-identical to before. A
finding whose asset kind no active profile owns, or an unknown profile id, is a
loud error, never a silent default.

### Built-in catalog

`code` · `binary` · `firmware` · `network` · `web` · `api` · `cloud` · `sca` ·
`iac` · `secrets` · `container` · `redteam` · `threat-model`, plus the composites
`mobile` (= `binary` + `web` + MASVS) and `pentest` (= `network` + `web`).
Composites activate each member as its own routing target, so a `pentest` run
files a `network:service` finding and a `web:route` finding side by side, each
validated against its own profile — never a merged union.

Operators and projects override or add profiles by dropping a `*.toml` under
`~/.rupu/profiles/` (global) or `.rupu/profiles/` (project); a later source wins
by id (built-in < global < project). A profile that fails to parse fails the
launch rather than running under the wrong rules.

### Recording an asset

Under an active engagement, `report_finding` (and `findings.record`) take an
optional `asset { kind, coordinates }`: the kind routes the finding to its
owning profile, the profile's **required completeness checks** must pass (e.g.
`network` requires the service pinned to a host + port), and the asset is stamped
into the engagement asset graph (`assets.jsonl`). The `asset_mark` tool — offered
only under an active engagement — records how deeply an asset was examined along
its profile's depth ladder (`discovered → enumerated → tested → exploited` for
`network`); depth is **monotonic**, so a shallower mark after a deeper one keeps
the deeper rung.

Spec: `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md`.

## See also

- `docs/agent-format.md` — full agent frontmatter schema (incl. `concerns:` and `findingsProfile`)
- `docs/workflow-format.md` — workflow `findings_profile` (step and `defaults`)
- `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md` — the finding report design
- `docs/agent-authoring.md` — writing good agents
- Slice specs/plans under `docs/superpowers/{specs,plans}/` (search `coverage-harness`)
