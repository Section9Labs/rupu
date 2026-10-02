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
as append-only JSONL plus a catalog snapshot. The one exception is `rupu findings
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
- One report's artifacts are bounded before anything is copied: at most
  `[findings].artifact_max_files` files (default 500), and the files that
  will be copied into the store may add up to at most
  `[findings].artifact_total_max_bytes` bytes (default 2 GiB). A file over
  `artifact_max_bytes` is recorded by reference and does not count toward
  that total, so it never rejects the finding. A larger set rejects the
  finding with the count or total named; list specific files instead of
  large directories.
- A path that escapes the workspace, names the workspace root itself (`.`),
  does not exist, or names something other than a regular file (a device,
  socket, or the like) rejects the finding, so a typo is not silently dropped.
- A remote workflow unit (`host:` / `distribute:`) runs `report_finding` on the
  host, so its artifacts go into **that host's** store and are recorded
  `stored: copied` with no `host`. Recording them as `stored: external` with
  `host` set, and pulling them into the coordinator's store on first view, is
  specified but not built yet.

Each evidence claim's `sha256` is taken only from a file that resolves inside
the workspace and is no larger than `artifact_max_bytes`; other claims are
stored without a hash.

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
  finding lists it; `:sha256` must be 64 lowercase hex characters. Text is
  served inline as `text/plain`; anything else is an attachment. Every response
  carries `X-Content-Type-Options: nosniff` and `Content-Security-Policy:
  sandbox`. A copied artifact is read from the content-addressed store. An
  external artifact on this machine is opened once and hashed from that same
  handle: `409` if it no longer matches the recorded hash, `404` if it is gone.
  Artifacts recorded by remote or placed units are not viewable in the control
  plane yet: such a unit ingests into that host's own store, and nothing records
  the host on the artifact today. Should an artifact ever carry a `host`, the
  endpoint answers `404` for it, because fetching from another host is not
  built (see `TODO.md`).

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
  and images are rendered as their alt text rather than loaded.
- **PDF** (`pdf`): generated in process with Typst, with no headless browser
  and no external tools. Fonts are bundled (Libertinus Serif, New Computer
  Modern, DejaVu Sans Mono), so output does not depend on the machine; the flip
  side is that the bundle has no CJK or emoji glyphs, so those characters do not
  render in a PDF (Markdown and HTML keep them). The PDF renderer cannot read
  files from disk, so a report cannot pull in a local file or image. PDF export
  is behind the `pdf` cargo feature, on by default and forwarded by `rupu-cp`
  and `rupu-cli`; it adds roughly 45-55 MB to a release binary. A build without
  it (`--no-default-features`) still exports Markdown and HTML, and asking for
  PDF fails with "compiled without PDF support" (a non-zero exit from the CLI,
  `501` from the control plane).

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
into the section before it.

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
Artifacts are copied from the workspace as it is at import, not as it was when
the report was written.

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
It checks that each artifact the report lists exists inside the workspace and
is within the artifact count and size limits, and that the report as it would
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
lines (`notes.test.ts:10-20`), or in a code span when it has one dot at most
(`` `notes.rs:40` ``, `` `.env.local:3` ``). A host and port that reads as one is
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
holds a space or a comma does not read (the field is kept as other text); its
typed evidence blocks (scan output, HTTP exchanges, disassembly, …) come back as
evidence claims, text and code kept, type not; and a claim of several
paragraphs comes back as several claims. An export made by a rupu that predates
`rupu findings import`, whose exporter printed its command blocks without a
`Command:` line, reads back too: each section's last `sh` block is its
command.

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
