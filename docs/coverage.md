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
as append-only JSONL plus a catalog snapshot:

| File | Contents |
|------|----------|
| `files.jsonl` | every file touch (read / grep / glob / edit / cmd), with attribution |
| `concerns.jsonl` | every `(concern, file) → verdict` assertion |
| `findings.jsonl` | every reported issue |
| `assets.jsonl` | the asset graph (kind + typed locator + depth), written only under an [engagement profile](#engagement-profiles) |
| `catalog.yaml` | the effective concern catalog, snapshotted at run start |
| `runs.jsonl` | one manifest per run (its defining inputs, for replay) |

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

## Engagement profiles

By default a finding is about a source file, and the harness above is built
around files and concerns. An **engagement profile** lets an assessment be about
something else: the functions of a binary, the images in a firmware drop, the
hosts of a network. A profile declares the kinds of *asset* that kind of
engagement works with, the typed coordinates that identify them, what a
complete finding must contain, and the depth ladder used to record how
thoroughly each asset has been covered. Under an engagement a finding can name
an `asset`; rupu routes the finding to the profile that owns that asset's kind
and enforces that profile's rules. With no engagement (the default) nothing
changes: findings are validated exactly as described in
[Finding reports](#finding-reports).

Design: `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md`.

### Profiles that ship

Only two profiles are built in:

| Id | Asset kinds | Depth ladder | Notes |
|----|-------------|--------------|-------|
| `code` | `code:file` | `unreviewed` → `reviewed` | The native path. Selecting only `code`, or nothing, means no engagement is active. |
| `binary` | `binary:binary`, `binary:function` | `located` → `disassembled` → `analyzed` | Reverse engineering. Under the `full` findings profile, a finding on a `binary:*` asset needs a `disasm` or `hexdump` evidence block, a `root_cause`, and a CWE classification. |

There is no built-in `network` or `web` profile. A `network` profile is
planned but does not ship yet; until then, anything else is a profile you
author yourself ([Authoring a profile](#authoring-a-profile)). Selecting an id
that no profile defines fails the launch with `unknown engagement profile`.

A runnable sample ships in the repo: `.rupu/agents/binary-analyst.md` (an agent
that declares `engagementProfiles: [binary]`, grants itself `asset_mark`, and
records function-level findings) and `.rupu/workflows/binary-assessment.yaml`
(a workflow that runs it under `defaults.engagement_profiles: [binary]`).

### Selecting an engagement

A run's engagement is a list of profile ids, chosen at the first level below
that has a non-empty list. Lists are never merged across levels.

1. **A flag**, for a standalone run or a session: `rupu run <agent> --engagement-profile binary`
   (repeatable) or `--engagement-profiles code,binary` (comma-separated).
   `rupu session start` takes the same flag; the selection is snapshotted when
   the session starts and applies to every turn.
2. **The workflow**, for a workflow run: `defaults.engagement_profiles: [binary]`.
3. **The agent**: `engagementProfiles: [binary]` in its frontmatter.
4. **`code`**: the native path, no engagement.

What a list means:

- Omitted or `[]` selects nothing, so resolution falls through to the next
  level. On a workflow *step* it means "inherit the run's set".
- `[code]` alone is the **explicit native path**: no engagement is active. On a
  step it opts that one step out of the run's engagement. `[]` and `[code]` are
  not the same thing: `[]` inherits, `[code]` opts out.
- `[binary]` is an engagement with `binary`.
- `[code, binary]` is an engagement with both profiles. Here `code` is an
  ordinary id (it contributes the `code:file` kind, so findings that name no
  `asset` are accepted).

A workflow step can also set `engagement_profiles`, but a step never *selects*
an engagement. It only **narrows** the run's set: naming an id the run set does
not contain is an error at launch (the step fails, naming the step and the id),
and so is selecting any profile on a step whose run is on the native `code`
path. A composite counts as containing its members, so a step may narrow a
composite to one of the profiles it includes. `[code]` is always allowed on a
step.

```yaml
name: assess-firmware
defaults:
  engagement_profiles: [code, binary]    # the run's set
steps:
  - id: triage
    agent: binary-analyst
    prompt: "Triage the sample"
    engagement_profiles: [binary]        # narrowed: only binary kinds are valid here
  - id: review-update-client
    agent: code-reviewer
    prompt: "Review the update client"
    # inherits [code, binary]
  - id: summarize
    agent: writer
    prompt: "Summarize the findings so far"
    engagement_profiles: [code]          # native path: no engagement for this step
```

```bash
rupu run binary-analyst "Analyse ./sample.bin" --engagement-profiles binary
```

See [workflow-format.md](workflow-format.md#engagement_profiles) and
[agent-format.md](agent-format.md#engagementprofiles) for the placement rules,
and the remote-step restriction in [Known limits](#known-limits).

### Per-origin routing

Every asset kind is namespaced by the profile that declares it: `code:file`,
`binary:function`. A finding is validated against the profile that **owns** its
asset kind, with that profile's own checks, never a merged union of everything
active. A **composite** profile (one with `includes`) is only a selection
shorthand: selecting it activates each included profile as its own routing
target, so a `binary:function` finding is judged by `binary` whether `binary` or
a composite containing it was selected.

A composite and one of its own members cannot both be selected: the kinds
collide and the launch fails (`kind "..." is declared by more than one active
profile`). Listing the same id twice is harmless; it is collapsed.

### Assets and depth

Under an engagement, two things write to the project's asset store,
`<workspace>/.rupu/coverage/<target_id>/assets.jsonl` (next to `findings.jsonl`,
not in a run directory, so assets accumulate across runs):

- **`report_finding` / `action: findings.record` with an `asset`** registers the
  asset (an upsert: re-reporting the same asset adds no line) and stamps the
  finding with the asset it is about. A finding never resets an asset's depth.
- **`asset_mark`** registers an asset *and* records how deeply it has been
  covered. It is the only way to set depth. Depth is a field of the asset itself
  (`depth` on the record in `assets.jsonl`); there is no separate depth file.

An asset names its **kind** and a **locator**: a list of single-key typed
coordinates, never a string. The kind must be one an active profile declares.

```json
{
  "kind": "binary:function",
  "locator": [
    { "sha256": "<64 hex chars>" },
    { "address": 4198400 },
    { "symbol": "parse_header" }
  ],
  "label": "parse_header"
}
```

`parent` (the `asset_id` an earlier `asset_mark` returned) and `label` are
optional; without a label rupu fills the kind's `label` template from the
locator. The valid coordinate tags are:

| Tag | Value |
|-----|-------|
| `path` | string |
| `line_range` | `{ "start": 1, "end": 9 }` |
| `symbol` | string |
| `commit` | string |
| `sha256` | hex string |
| `offset` | number |
| `address` | number (decimal: `4198400` is `0x401000`) |
| `host` | string |
| `port` | `{ "number": 443, "proto": "tcp" }` (`tcp` or `udp`) |
| `url` | string |
| `http_route` | `{ "method": "GET", "path": "/v1/things" }` |
| `param` | string |
| `resource_id` | `{ "scheme": "arn", "id": "aws:s3:::bucket" }` |

An asset's id is a hash of its kind and its locator as an ordered list, so
give the same coordinates, in the same order, every time you mean the same
asset. A kind's `coordinates` list says which tags it uses (for its label and
for the agent's guidance); rupu does not otherwise require a locator to match
it. To require one, use a `locator_has_coordinate` completeness check.

**Setting depth with `asset_mark`.** The tool is never ambient. The agent must
list it in `tools:` (a `concerns:` block does not add it), and an engagement
must be active: without one the call fails with `asset_mark needs an active
engagement profile`. It takes `kind`, `locator` and `depth` (plus optional
`parent` and `label`), and `depth` must be a rung of the owning profile's
`depth_ladder`; the real ladder for `binary` is `located` → `disassembled` →
`analyzed`. An unknown kind or an unknown rung is rejected, naming what is
declared. Re-marking an asset updates its depth, and the last write wins: it
is not monotonic, so marking `analyzed` and later `located` leaves `located`.

```yaml
---
name: binary-analyst
tools: [report_finding, asset_mark]   # asset_mark is an explicit grant
engagementProfiles: [binary]
---
```

When an engagement is active and the agent records findings (`report_finding`
or `concerns:`) or has `asset_mark`, rupu appends an "Engagement profiles"
section to the agent's system prompt listing the active profiles' asset kinds
and their coordinates, evidence block kinds, classification systems and depth
ladder. It
does not list the completeness checks, so say in the agent's prompt what a
finding must carry (the sample `binary-analyst` does), or the agent learns it
from rejections. `asset` is advertised on `report_finding` only under an
engagement, and `findings.record` refuses an `asset` outright when no
engagement is active, rather than silently dropping it.

In a workflow, `action: findings.record` takes the same `asset` object:

```yaml
name: record-one-function
defaults:
  engagement_profiles: [binary]
inputs:
  sha256:
    type: string
    required: true
steps:
  - id: record
    action: findings.record
    findings_profile: summary
    with:
      scope: repo
      summary: "parse_header copies an attacker-controlled length into a 64-byte buffer"
      severity: high
      rationale: "The length byte reaches memcpy unchecked at 0x401012."
      asset:
        kind: binary:function
        locator:
          - sha256: "{{ inputs.sha256 }}"
          - address: 4198400
          - symbol: parse_header
```

String coordinates (`sha256`, `symbol`, `host`, `path`, `url`, ...) may be
filled from templates. Numeric ones (`address`, `offset`, `port.number`,
`line_range`) must be literals: a rendered template is a string, and these
coordinates are numbers.

### What an engagement enforces

Under an engagement, `report_finding` and `findings.record` check:

- **Always (both findings profiles): the asset kind.** The finding's kind must
  be declared by an active profile. An unknown kind, a kind owned by a profile
  that is not active, or an undeclared kind in an active profile (`binary:nope`)
  is rejected, naming the active set.
- **Always: the no-asset default.** A finding that names no `asset` is filed as
  `code:file` (its `file_path` becomes the locator). That is accepted only when
  `code` is active (for example `[code, binary]`); under `[binary]` alone it is
  rejected with ``this finding names no `asset`, so its scope maps to
  `code:file`, which belongs to no active engagement profile``.
- **Under the `full` findings profile only: the completeness checks.** Every
  `required` check of the owning profile must be satisfied, and every unmet
  check is listed at once. Under the `summary` findings profile only the two
  routing rules above are enforced, so a profile's completeness checks do not
  apply to summary findings.

A profile's `evidence_blocks` and `classification_systems` are **guidance, not
allow-lists**: they are shown to the agent, and an extra block or system is
harmless. A profile that truly needs a block or a classification says so with
a completeness check that references it (`has_block_kind`,
`has_classification_system`); that check is then enforced like any other. So
asset kinds and required completeness checks do reject findings, and the lists
of blocks and systems do not.

`asset_mark` separately enforces the kind and the depth ladder, as above.

### Authoring a profile

Profiles are TOML files in:

- `~/.rupu/profiles/*.toml` (global; `<RUPU_HOME>/profiles`)
- `<project>/.rupu/profiles/*.toml` (project)

Built-ins are overlaid first, then global, then project; a later source
replaces an earlier one **by `id`**, so a project profile shadows a global
profile of the same id, and either can replace a built-in. The profile's id is
the `id` inside the file, not the file name; name the file after the id so the
two cannot disagree.

Files are read only when a real engagement is selected (a run on the native
`code` path never loads them), and they are read all-or-nothing: **any `*.toml`
in those directories that fails to parse (TOML syntax, an unknown coordinate
tag, an unreadable file) fails the whole selection**, whether or not you
selected that profile. This is deliberate (fail-closed): a profile that silently
failed to load would leave an engagement running under the wrong rules.

A profile has the same shape as the built-in `binary` profile
(`crates/rupu-coverage/src/profile/builtin/binary.toml`):

```toml
id = "firmware"
name = "Firmware image review"
# Top-level keys come first. After a `[[asset_kinds]]` header they would belong
# to that kind and be silently ignored, leaving the profile with no
# evidence-block or classification guidance.
evidence_blocks = ["text", "hexdump", "disasm"]
classification_systems = ["CWE", "CVE"]

[[asset_kinds]]
id = "image"                          # registered as `firmware:image`
coordinates = ["sha256"]
label = "{sha256}"

[[asset_kinds]]
id = "partition"                      # registered as `firmware:partition`
parent = "image"                      # a bare id in this same profile
coordinates = ["sha256", "offset"]
label = "{offset} in {sha256}"

[[completeness]]
id = "has_root_cause"
label = "Root cause stated"
required = true
satisfied_when = { has_field = "root_cause" }

[[completeness]]
id = "classified"
label = "Weakness classified (CWE)"
required = true
satisfied_when = { has_classification_system = "CWE" }

[coverage]
enumerates = ["image", "partition"]
depth_ladder = ["located", "unpacked", "analyzed"]
```

Select it like any other id (`--engagement-profiles firmware`, or
`engagementProfiles: [firmware]`); its kinds are `firmware:image` and
`firmware:partition`.

| Key | Meaning |
|-----|---------|
| `id`, `name` | Required. `id` is the id you select by and the namespace of its kinds. |
| `includes` | Optional list of other profile ids; makes this a composite (below). |
| `evidence_blocks` | Optional. Block kinds the agent is told to use: `text`, `code_slice`, `diff`, `table`, `image`, `hexdump`, `disasm`, `decompile`, `http_exchange`, `scan_output`, `pcap_ref`. Guidance only. |
| `classification_systems` | Optional. Taxonomies the agent is told to use (`CWE`, `CVE`, ...). Guidance only. |
| `[[asset_kinds]]` | `id` (bare; the registry prefixes `<profile id>:`), `label` (a template over coordinate tags, e.g. `"{symbol} @ {address}"`), optional `parent` (a bare kind id in the same profile), optional `coordinates` (tags from the table above). |
| `[[completeness]]` | `id`, `label`, `required` (default `true`; an optional check never blocks), and `satisfied_when`, a predicate (below). |
| `[coverage]` | `depth_ladder`: the ordered rungs, shallowest to deepest, that `asset_mark` accepts. `enumerates`: the kinds the profile expects to cover (parsed, but nothing in the run path reads it yet). |
| `[bundle]` | `agents`, `tools`, `workflows`: names that travel with the profile (informational today). |

Required: `id` and `name`; for each kind, `id` and `label`; for each
completeness check, `id`, `label` and `satisfied_when`. Everything else may be
omitted.

**TOML ordering.** What must precede the first `[[asset_kinds]]` are the
top-level keys: `id`, `name`, `includes`, `evidence_blocks`,
`classification_systems`. `[[completeness]]`, `[coverage]` and `[bundle]` each
have their own header, so where they sit does not matter.

**Predicates.** `satisfied_when` is a single inline table:

| Predicate | True when |
|-----------|-----------|
| `{ has_field = "root_cause" }` | the report field is present and non-empty; one of `root_cause`, `remediation`, `impact`, `description`, `attack_vector`, `category`, `replication_steps`, `evidence` |
| `{ has_block_kind = "disasm" }` | an evidence claim carries a block of that kind |
| `{ has_classification_system = "CWE" }` | the report is classified under that system (case-insensitive; entries in `cwe` count) |
| `{ locator_has_coordinate = "address" }` | the asset's locator carries that coordinate tag |
| `{ min_severity = "High" }` | the risk rating is at least `Low`, `Medium`, `High` or `Critical` |
| `{ all = [ ... ] }`, `{ any = [ ... ] }` | every / at least one of the listed predicates |

An unknown field or tag in a predicate makes every finding of that profile fail
(`engagement profile ... has an invalid completeness check`), rather than
letting findings through. Completeness only runs under the `full` findings
profile, so this applies there; `summary` findings never evaluate the checks.

**Composites.** A profile with `includes` is a selection shorthand. It may
declare kinds of its own (namespaced by its own id) and a `[bundle]`:

```toml
id = "firmware-review"
name = "Firmware review (source and binary)"
includes = ["code", "binary"]        # members stay separate routing targets

[bundle]
agents = ["binary-analyst"]
```

Selecting `firmware-review` activates `code` and `binary`; a `code:file` finding
is judged by `code` and a `binary:function` finding by `binary`. Includes may
nest; a cycle or an include that names no profile fails the launch. Do not
select a composite together with one of its own members (see
[Per-origin routing](#per-origin-routing)).

### Known limits

- **Remote and placed steps.** A workflow step with `host:` or `distribute:`
  cannot be run under an engagement yet: the engagement set does not travel to
  the host. Such a unit is **refused** (the step fails with an error) rather
  than run under the native rules, when the step's `engagement_profiles`, else
  `defaults.engagement_profiles`, selects anything other than `code`. To run
  the step anyway, opt out explicitly with `engagement_profiles: [code]` on
  that step; the unit then runs on the host under the native `code` rules with
  no engagement. The refusal does not look
  at an agent's own `engagementProfiles`. A fan-out without `distribute:` runs
  locally and is unaffected.
- **Dispatched sub-agents.** An agent started by `dispatch_agent` or
  `dispatch_agents_parallel` does not consult its own `engagementProfiles`. It
  inherits the engagement of the run that dispatched it: for `rupu run`, the
  parent run's resolved set; for a workflow run, `defaults.engagement_profiles`
  only. A step's narrowing, and the parent agent's own frontmatter, are not
  carried down, so set `defaults.engagement_profiles` if dispatched children
  must record under the engagement.
- **Run manifest and `rerun`.** The run manifest (`runs.jsonl`) is written only
  for agents with a `concerns:` block, and it records the engagement profiles
  the run used. `rupu coverage rerun` replays agent runs and re-launches them with the
  recorded profiles. A findings-only engaged
  run (no `concerns:`) records findings and assets but no manifest, so it
  cannot be replayed.
- **Control-plane workflow editor.** The editor (`rupu cp serve`) does not
  expose `engagement_profiles` yet; author it in the YAML.
- **Numeric coordinates in workflows.** `address`, `offset`, `port.number` and
  `line_range` cannot be filled from `{{ ... }}` templates in a `findings.record`
  `with:`; use literals.
- **Depth.** Depth is last-write-wins, not monotonic (see
  [Assets and depth](#assets-and-depth)).
- **Unused declarations.** `[coverage].enumerates` and `[bundle]` are parsed and
  kept but nothing in the run path acts on them yet.

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

## See also

- `docs/agent-format.md` — full agent frontmatter schema (incl. `concerns:`, `findingsProfile`, `engagementProfiles`)
- `docs/workflow-format.md` — workflow `findings_profile` and `engagement_profiles` (step and `defaults`)
- `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md` — the finding report design
- `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md` — the engagement profiles design
- `docs/agent-authoring.md` — writing good agents
- Slice specs/plans under `docs/superpowers/{specs,plans}/` (search `coverage-harness` or `engagement`)
