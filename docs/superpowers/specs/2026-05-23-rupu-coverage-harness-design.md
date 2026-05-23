# rupu agentic coverage harness — Slice A (exhaustive coverage)

**Status:** Draft
**Author:** matt
**Date:** 2026-05-23
**Surface:** workspace-level. Touches `rupu-tools`, `rupu-agent`, `rupu-orchestrator`, `rupu-cli`; introduces a new `rupu-coverage` crate.

## Problem

When one or more models perform survey tasks — "review this repo for bugs
or vulnerabilities," "find all places we talk to the database," "audit
this codebase for accessibility" — findings vary widely run-to-run.
Sometimes the findings overlap; sometimes they are completely disjoint.

The root cause is that the model has no persistent record of **what it
examined and for what criteria**. Specifically:

1. **Coverage is invisible.** There's no contract that says "this scope,
   for this concern, has been examined." A user reading the output cannot
   distinguish "no issues found" from "the model never looked."
2. **Cross-run accumulation is impossible.** A second pass — same model
   again, or a different model — has no way to build on the first.
   Knowledge is reset every run.
3. **Cross-model ensembles can't be merged honestly.** Run model A, then
   model B; the user gets two transcripts and has to manually reconcile
   them. There's no shared substrate where each model's contributions
   are attributable.
4. **Multi-pass is wasteful.** A second pass redoes the same work
   without knowing it's redoing it.

Concretely: matt has observed running the same security-review prompt
against the same repo with the same model multiple times and getting
substantially different findings each time. The variance is not
*entirely* bad — different angles surface different bugs — but the
*combination* across runs is currently impossible to evaluate for
completeness.

## Goals

- **Auditable record of (file × concern × verdict)** for every review-shaped task.
- **Cross-run accumulation** against a shared target: a second pass picks up where the first left off automatically.
- **Cross-model attribution** preserved so multi-model ensembles are merge-able.
- **Minimal token cost.** File-touch tracking is *free* (harness-instrumented). Concern marking is one tool call per (concern × file) verdict.
- **Industry-anchored catalogs.** Ship templates for OWASP Top 10, CWE Top 25, STRIDE, common code-smell sets, etc. Users include or extend; rare users author from scratch.
- **Surface-uniform.** Works the same way whether the agent loop is driven by a workflow, a one-off agent run, an autoflow cycle, or an interactive session.

## Non-goals (this slice)

- **Reproducibility / determinism** — that's [Slice B](#out-of-scope), a separate feature that *stacks* on top of this one (Slice A is the foundation; Slice B adds seed control, prompt templating, deterministic ordering, and run-comparison tooling).
- **Agent-generated catalogs.** Catalogs are human-curated. Agents inform catalog evolution via serendipitous findings, but never write to the catalog directly.
- **Concern revision API.** Ledgers are append-only. Within a run, a later `coverage_mark` for the same (concern, file) supersedes the earlier one in the derived view. Cross-run disagreement is *preserved* — that's signal.
- **Cross-target catalog aliasing.** Two catalogs that use different concern_ids for the same logical concern won't merge. v1 rule: catalogs that want to share must use the same IDs.
- **Concern excludes / partial template imports.** v1 supports `include` + `overrides`; `exclude: [id1, id2]` lands when a real need emerges.
- **GUI surface in rupu-app.** Coverage data is queried via CLI in v1.

## Design

### Three append-only JSONL ledgers, one effective-catalog snapshot

```
.rupu/coverage/<target_id>/
  ├── catalog.yaml      # effective catalog snapshot (flattened includes + overrides)
  ├── files.jsonl       # harness-populated: every file-touch event
  ├── concerns.jsonl    # agent-populated: (concern × file × verdict) assertions
  └── findings.jsonl    # agent-populated: discovered issues
```

`<target_id>` is a deterministic identifier derived from `(workspace, scope_name)`:

| Surface | scope_name |
| --- | --- |
| Workflow | workflow name (e.g. `security-review`) |
| Agent (one-off) | agent name (e.g. `security-reviewer`) |
| Autoflow | the workflow it drives — same target as that workflow's runs |
| Session | session_id by default; configurable to share with the agent's target |

The same target across surfaces shares all three ledgers. A workflow run on Monday, an autoflow cycle on Wednesday, and a session on Friday all append to the same files — assertions interleave, file touches accumulate.

### Ledger schemas

**files.jsonl** — one line per file-touch event. Append-only.

```jsonc
{
  "path": "src/handlers/users.rs",
  "kind": "read",                         // read | grep | edit | glob | cmd
  "tool": "read_file",                    // the actual tool name (more specific than `kind`)
  "line_range": [1, 240],                 // for read/edit/grep events; absent for glob/cmd
  "pattern": "execute|query",             // for grep events
  "match_count": 3,                       // for grep events
  "matched_lines": [42, 108, 156],        // for grep events
  "lines_changed": 6,                     // for edit events
  "command": "cargo test",                // for cmd events
  "run_id": "run_01KS19A4MQXP",
  "model": "claude-sonnet-4-6",
  "surface": "workflow",                  // workflow | agent | autoflow | session
  "at": "2026-05-23T14:01:32Z"
}
```

The derived per-file view (computed at query time, not stored):

```jsonc
{
  "path": "src/handlers/users.rs",
  "touch_modes": ["glob", "read", "grep"],
  "strongest": "read",                    // ordering: glob < cmd < grep < read < edit
  "read_lines": [[1, 240]],               // merged from all read events
  "grep_matches": 3,
  "edits": 0,
  "first_at": "2026-05-23T14:00:12Z",
  "last_at": "2026-05-23T14:02:50Z",
  "touched_by": [
    { "run_id": "run_01KS19A4MQXP", "model": "claude-sonnet-4-6", "surface": "workflow" }
  ]
}
```

**concerns.jsonl** — one line per assertion. Append-only.

```jsonc
{
  "concern_id": "owasp-top10-2021:a03-injection",
  "file_path": "src/db/queries.rs",
  "status": "finding",                    // clean | finding | examined | not_applicable
  "evidence": {
    "summary": "Raw string interpolation on lines 87-92 builds SQL from req.username without parameterization.",
    "line_ranges": [[87, 92]],
    "finding_ids": ["fnd_01KS19A3"]       // required non-empty if status == "finding"
  },
  "declared_by": {
    "run_id": "run_01KS19A4MQXP",
    "model": "claude-sonnet-4-6",
    "surface": "workflow"
  },
  "declared_at": "2026-05-23T14:03:44Z"
}
```

**Status semantics** (four values, not three):

- `clean` — examined; no issue found.
- `finding` — examined; issue found. `finding_ids` must be non-empty.
- `examined` — examined; inconclusive. Needs follow-up or more context.
- `not_applicable` — concern does not apply to this file (e.g. examining a `.css` file for SQL-injection concern). Only status that does **not** require a `read`-strength file touch.

**findings.jsonl** — one line per discovered issue. Append-only.

```jsonc
{
  "id": "fnd_01KS19A3",
  "file_path": "src/db/queries.rs",       // nullable for repo-scope findings
  "line_range": [87, 92],                 // nullable for file-scope findings
  "scope": "line",                        // line | file | repo
  "summary": "SQL string concatenation with user-controlled username field.",
  "severity": "high",                     // info | low | medium | high | critical
  "concern_id": "owasp-top10-2021:a03-injection",  // nullable — null = serendipitous
  "evidence": {
    "code_excerpt": "let query = format!(\"SELECT * FROM users WHERE name = '{}'\", req.username);",
    "rationale": "User-supplied input flows directly into SQL string; use parameterized query via sqlx::query!.",
    "references": ["https://cwe.mitre.org/data/definitions/89.html"]
  },
  "declared_by": { "run_id": "run_01KS19A4MQXP", "model": "claude-sonnet-4-6", "surface": "workflow" },
  "declared_at": "2026-05-23T14:03:44Z"
}
```

Findings can carry `concern_id: null` for **serendipitous discoveries** — the model spotted something while looking for something else. These don't break coverage math (which is calculated over declared concerns only) but feed catalog evolution: repeated null-concern findings clustering around a theme are a signal to add that theme to the catalog.

### File-touch instrumentation (the "passive" half)

`rupu-tools::ToolContext` gains a new optional field:

```rust
pub struct ToolContext {
    pub workspace_path: PathBuf,
    pub coverage_writer: Option<Arc<CoverageWriter>>,
    // ...existing fields
}
```

Each built-in tool emits a file-touch event when the writer is present:

| Tool | Emits | Kind |
| --- | --- | --- |
| `read_file` | one event per call | `read` |
| `grep` | one event per matching file | `grep` |
| `glob` | one event per matched path | `glob` |
| `edit_file` | one event per edit | `edit` |
| `bash` / `command` | one event per *recognized* path argument | `cmd` |

**Unknown tools.** For tools the harness doesn't have a touch-mapping
for (any MCP-server-exposed tool, any custom tool), the harness *does
not* fabricate a file_path — it cannot reliably know which files such
a tool touched. Instead, it appends a `tool_call_observed` event to
`files.jsonl` with no `path` field, carrying the tool name and arg
hash so the audit can flag "this run used unrecognized tooling; some
file-touch evidence may be missing." Users who want full coverage
under custom MCP tools can register a touch-mapping (string-to-path-
arg) for their tool via project-level config (`.rupu/coverage/tool-
mappings.yaml`).

Writer implementation: a tokio task owns the JSONL file handle and
serves an MPSC channel. Each tool sends a `FileTouchEvent` to the channel
and continues; the writer task fsyncs in batches every ~50ms or 16
events, whichever first. Failure to write is logged but does not fail
the tool call.

### Agent tool surface (the "active" half)

Three new tools, auto-injected into the agent's tool list when the
agent's resolved `concerns:` block is non-empty:

**`coverage_mark`** — write a (concern × file) assertion.

```jsonc
// Input
{
  "concern_id": "owasp-top10-2021:a03-injection",
  "file_path": "src/handlers/users.rs",
  "status": "clean",
  "evidence": {
    "summary": "All queries delegate to db::queries module with prepared statements.",
    "line_ranges": [[1, 240]],
    "finding_ids": []
  }
}

// Output
{ "ok": true, "warnings": [] }
```

Harness-enforced validation:

- `concern_id` must exist in the effective catalog → reject with `unknown_concern_id`.
- `file_path` must have at least one `read`-strength event in `files.jsonl` for statuses `clean`, `finding`, `examined` → reject with `file_not_examined`.
- `not_applicable` does **not** require a `read` touch.
- `status: "finding"` with empty `finding_ids` → warn (don't reject); the agent may file the finding immediately after.
- Re-asserting the same (concern_id, file_path, run_id) → silently supersede earlier within run; cross-run entries are both preserved.

**`coverage_status`** — read prior assertions. Any combination of filters; returns intersection.

```jsonc
// Input
{ "concern_id": "owasp-top10-2021:a03-injection", "file_path_prefix": "src/handlers/" }

// Output: array of concerns.jsonl records matching the filter
[ ... ]
```

This is the multi-pass coordinator's primary read tool. Second-pass model calls it on startup to see what was concluded.

**`coverage_remaining`** — list files that have been touched but lack an assertion for the requested concern(s).

```jsonc
// Input
{ "concern_id": "owasp-top10-2021:a03-injection", "min_strength": "read" }

// Output
[
  { "file_path": "src/handlers/admin.rs", "touch_modes": ["read"], "reason": "no_assertion" },
  { "file_path": "src/handlers/upload.rs", "touch_modes": ["glob"], "reason": "below_min_strength" }
]
```

`min_strength` defaults to `read`. Omit `concern_id` to query against the full catalog at once (returns a per-concern breakdown).

**`report_finding`** — file a discovery. Independent of `coverage_mark`; an agent can find things without (yet) marking coverage, and can mark coverage without finding things.

```jsonc
// Input
{
  "file_path": "src/db/queries.rs",
  "line_range": [87, 92],
  "summary": "Raw string interpolation builds SQL from user input.",
  "severity": "high",
  "concern_id": "owasp-top10-2021:a03-injection",   // nullable
  "evidence": { "code_excerpt": "...", "rationale": "...", "references": [...] }
}

// Output
{ "id": "fnd_01KS19A3" }
```

Returns the finding ID so the agent can reference it in a subsequent
`coverage_mark` call with `status: "finding"`.

### Catalog declaration

#### Concern definition (the unit)

```yaml
- id: owasp-top10-2021:a03-injection
  name: A03:2021 — Injection
  description: |
    Code that constructs interpreter-bound strings from user input
    without parameterization or escaping. Includes SQL, NoSQL,
    command, LDAP, ORM expression injection.
  severity: high                       # default severity for findings under this concern
  applicable_globs:                    # optional; constrains the file scope
    - "**/*.rs"
    - "**/*.sql"
    - "!**/target/**"
    - "!**/node_modules/**"
  min_strength: read                   # touch-strength required for clean/finding/examined
  references:
    - https://owasp.org/Top10/A03_2021-Injection/
    - https://cwe.mitre.org/data/definitions/89.html
```

Required: `id`, `name`, `description`. Everything else has defaults
(`severity: medium`, `applicable_globs: ["**"]`, `min_strength: read`,
`references: []`).

#### Inline vs. include vs. override

```yaml
concerns:
  - include: owasp-top10-2021                  # pull in template wholesale

  - include: cwe-top25-2023                    # multiple includes allowed
    overrides:
      - id: cwe-top25-2023:cwe-787             # tweak one field on an included concern
        severity: critical

  - id: secrets-in-source                      # inline definition
    name: Secrets in source code
    description: |
      Find hardcoded credentials, API keys, tokens, or passwords
      committed to the repository.
    severity: high
    applicable_globs:
      - "**/*.rs"
      - "**/*.toml"
      - "**/.env*"
      - "!**/target/**"
    references:
      - https://cwe.mitre.org/data/definitions/798.html
```

**Merge rules:**

- Within `concerns:`, mix `include:` and inline declarations freely.
- If a `concern_id` appears both inline and via include, **inline wins**. No ordering surprises.
- `overrides:` on an `include:` patches specific fields without restating the concern.
- Duplicate concern_ids across two includes → error at catalog flatten time; user must override one explicitly.

#### Where the catalog is declared

| Surface | Location |
| --- | --- |
| Workflow | `concerns:` block at workflow YAML top level |
| Agent | `concerns:` block in agent file YAML frontmatter |
| Autoflow | inherited from the workflow it drives |
| Session | inherited from the agent; runtime-extendable via `/coverage add <concern_or_include>` |

When the run starts, the harness:

1. Flattens the catalog (resolves all includes, applies all overrides).
2. Detects duplicate-id conflicts; errors with a clear message if found.
3. Writes the **effective catalog snapshot** to `.rupu/coverage/<target_id>/catalog.yaml`.
4. Renders a section of the system prompt listing the concerns (id, name, description, applicable_globs) so the agent always knows what to work toward.

The snapshot is read by the audit step and by later runs targeting the
same target. Even if upstream `owasp-top10-2021` is bumped to v2 next
month, every prior assertion is still interpretable in terms of the
catalog as it was at the moment the assertion was written.

### Industry-standard catalog templates shipped in v1

All templates live under `crates/rupu-coverage/templates/concerns/` and
are referenced by name (without `.yaml` extension):

| Template | Concerns | Description |
| --- | :-: | --- |
| `owasp-top10-2021` | 10 | OWASP Top 10 web-app security risks (2021 edition) |
| `owasp-api-top10-2023` | 10 | OWASP API Security Top 10 (2023 edition) |
| `cwe-top25-2023` | 25 | CWE Top 25 Most Dangerous Software Weaknesses (2023) |
| `stride` | 6 | STRIDE threat-modeling categories |
| `secrets-in-source` | 1 | Single-purpose: hardcoded credentials, keys, tokens |
| `code-smells` | 12 | Fowler-style code smells (long method, god object, etc.) |
| `web-security-default` | composite | `include`s the three OWASP templates + secrets-in-source |
| `api-security-default` | composite | `include`s OWASP API Top 10 + CWE Top 25 + secrets |

Reasonable starting catalog (≈58 unique concerns across the templates,
post-deduplication). Additional templates (PCI-DSS, NIST SSDF, MASVS,
ASVS chapter-by-chapter, language-specific best-practice sets) can be
added in follow-up slices without changing the architecture.

#### Catalog ID namespacing

To avoid collisions across templates, each concern's ID is prefixed
with the template's namespace:

- `owasp-top10-2021:a01-broken-access-control`
- `cwe-top25-2023:cwe-787-out-of-bounds-write`
- `stride:tampering`
- `code-smells:long-method`

Inline (non-template) concerns may use unprefixed IDs (`secrets-in-source`).
Users may alias to their own short IDs via `overrides` if they prefer
ergonomics over namespacing.

#### Worked example: `owasp-top10-2021.yaml`

```yaml
name: owasp-top10-2021
version: 1
description: OWASP Top 10 web application security risks (2021 edition)
references:
  - https://owasp.org/Top10/

concerns:
  - id: owasp-top10-2021:a01-broken-access-control
    name: A01:2021 — Broken Access Control
    description: |
      Restrictions on what authenticated users are allowed to do are
      not enforced — vertical and horizontal privilege escalation,
      missing function-level authorization, CORS misconfiguration,
      forced browsing of authenticated pages as anonymous, IDOR.
    severity: critical
    applicable_globs:
      - "**/handlers/**"
      - "**/controllers/**"
      - "**/middleware/**"
      - "**/routes/**"
      - "**/api/**"
    references:
      - https://owasp.org/Top10/A01_2021-Broken_Access_Control/

  - id: owasp-top10-2021:a02-cryptographic-failures
    name: A02:2021 — Cryptographic Failures
    description: |
      Failures related to cryptography, often leading to sensitive data
      exposure — weak algorithms (MD5, SHA-1, DES), hardcoded keys,
      missing TLS, predictable randomness for security-sensitive use.
    severity: high
    applicable_globs: ["**"]
    references:
      - https://owasp.org/Top10/A02_2021-Cryptographic_Failures/

  - id: owasp-top10-2021:a03-injection
    name: A03:2021 — Injection
    description: |
      User-controllable data interpolated into interpreters without
      parameterization or escaping — SQL, NoSQL, OS command, LDAP, XPath,
      ORM expression injection.
    severity: high
    applicable_globs: ["**/*.rs", "**/*.sql", "**/*.py", "**/*.ts", "**/*.js"]
    references:
      - https://owasp.org/Top10/A03_2021-Injection/
      - https://cwe.mitre.org/data/definitions/89.html

  - id: owasp-top10-2021:a04-insecure-design
    name: A04:2021 — Insecure Design
    description: |
      Missing or ineffective control design — absent rate limiting,
      missing threat modeling, business logic flaws, insecure trust
      boundaries.
    severity: medium
    applicable_globs: ["**"]
    references:
      - https://owasp.org/Top10/A04_2021-Insecure_Design/

  - id: owasp-top10-2021:a05-security-misconfiguration
    name: A05:2021 — Security Misconfiguration
    description: |
      Insecure default configurations, incomplete configurations,
      verbose error messages exposing internals, unnecessary features
      enabled, missing security headers, outdated dependencies.
    severity: medium
    applicable_globs:
      - "**/*.toml"
      - "**/*.yaml"
      - "**/*.yml"
      - "**/Dockerfile*"
      - "**/docker-compose*.yml"
      - "**/config/**"
      - "**/*.env*"
    references:
      - https://owasp.org/Top10/A05_2021-Security_Misconfiguration/

  - id: owasp-top10-2021:a06-vulnerable-components
    name: A06:2021 — Vulnerable and Outdated Components
    description: |
      Dependencies with known vulnerabilities, end-of-life components,
      unmaintained transitive dependencies.
    severity: medium
    applicable_globs:
      - "**/Cargo.toml"
      - "**/Cargo.lock"
      - "**/package.json"
      - "**/package-lock.json"
      - "**/go.mod"
      - "**/go.sum"
      - "**/requirements*.txt"
      - "**/poetry.lock"
      - "**/Pipfile*"
    references:
      - https://owasp.org/Top10/A06_2021-Vulnerable_and_Outdated_Components/

  - id: owasp-top10-2021:a07-identification-and-authentication-failures
    name: A07:2021 — Identification and Authentication Failures
    description: |
      Weak credential management, missing or weak MFA, predictable
      session identifiers, missing brute-force protection, plaintext or
      reversible password storage.
    severity: high
    applicable_globs:
      - "**/auth/**"
      - "**/login/**"
      - "**/session/**"
      - "**/middleware/**"
    references:
      - https://owasp.org/Top10/A07_2021-Identification_and_Authentication_Failures/

  - id: owasp-top10-2021:a08-software-and-data-integrity-failures
    name: A08:2021 — Software and Data Integrity Failures
    description: |
      Insecure deserialization, missing integrity verification on
      software updates and CI/CD artifacts, unsigned dependencies.
    severity: medium
    applicable_globs:
      - "**/*.rs"
      - "**/*.py"
      - "**/*.ts"
      - "**/*.js"
      - "**/.github/workflows/**"
    references:
      - https://owasp.org/Top10/A08_2021-Software_and_Data_Integrity_Failures/

  - id: owasp-top10-2021:a09-security-logging-and-monitoring-failures
    name: A09:2021 — Security Logging and Monitoring Failures
    description: |
      Missing audit logs for security-relevant events, logs lacking
      sufficient context, plaintext sensitive data in logs, no alerting.
    severity: medium
    applicable_globs: ["**"]
    references:
      - https://owasp.org/Top10/A09_2021-Security_Logging_and_Monitoring_Failures/

  - id: owasp-top10-2021:a10-ssrf
    name: A10:2021 — Server-Side Request Forgery (SSRF)
    description: |
      Code fetches a URL based on user input without validating the
      destination — allows requests to internal services, cloud metadata
      endpoints (169.254.169.254), and SSRF-amplified attacks.
    severity: high
    applicable_globs:
      - "**/*.rs"
      - "**/*.py"
      - "**/*.ts"
      - "**/*.js"
      - "**/*.go"
    references:
      - https://owasp.org/Top10/A10_2021-Server-Side_Request_Forgery_%28SSRF%29/
      - https://cwe.mitre.org/data/definitions/918.html
```

#### Worked example: `cwe-top25-2023.yaml` (excerpt — top 5 by 2023 rank)

```yaml
name: cwe-top25-2023
version: 1
description: 2023 CWE Top 25 Most Dangerous Software Weaknesses
references:
  - https://cwe.mitre.org/top25/archive/2023/2023_top25_list.html

concerns:
  - id: cwe-top25-2023:cwe-787-out-of-bounds-write
    name: CWE-787 — Out-of-bounds Write (rank #1)
    description: |
      Code writes data past the end, or before the beginning, of the
      intended buffer. Memory-safety bug; common in C/C++ unsafe code
      and Rust `unsafe` blocks.
    severity: critical
    applicable_globs:
      - "**/*.c"
      - "**/*.cpp"
      - "**/*.h"
      - "**/*.hpp"
      - "**/*.rs"     # only `unsafe` blocks matter here
    references:
      - https://cwe.mitre.org/data/definitions/787.html

  - id: cwe-top25-2023:cwe-79-xss
    name: CWE-79 — Cross-site Scripting (rank #2)
    description: |
      Code includes user-controllable input in HTML/JS output without
      proper escaping or sanitization, enabling injection of script
      executed in other users' browsers.
    severity: high
    applicable_globs:
      - "**/*.html"
      - "**/*.tsx"
      - "**/*.jsx"
      - "**/templates/**"
      - "**/views/**"
    references:
      - https://cwe.mitre.org/data/definitions/79.html

  - id: cwe-top25-2023:cwe-89-sql-injection
    name: CWE-89 — SQL Injection (rank #3)
    description: |
      Unsanitized user input flows into SQL query construction.
      Overlaps with owasp-top10-2021:a03-injection but is included
      separately so CWE-anchored catalogs are self-contained.
    severity: high
    applicable_globs: ["**/*.rs", "**/*.sql", "**/*.py", "**/*.ts", "**/*.js"]
    references:
      - https://cwe.mitre.org/data/definitions/89.html

  - id: cwe-top25-2023:cwe-416-use-after-free
    name: CWE-416 — Use After Free (rank #4)
    description: |
      Referencing memory after it has been freed. Memory-safety bug;
      relevant in C/C++ and Rust `unsafe`.
    severity: critical
    applicable_globs:
      - "**/*.c"
      - "**/*.cpp"
      - "**/*.rs"
    references:
      - https://cwe.mitre.org/data/definitions/416.html

  - id: cwe-top25-2023:cwe-78-os-command-injection
    name: CWE-78 — OS Command Injection (rank #5)
    description: |
      User input flows into a shell command without proper escaping —
      enables arbitrary command execution.
    severity: critical
    applicable_globs: ["**/*.rs", "**/*.py", "**/*.ts", "**/*.js", "**/*.go", "**/*.sh"]
    references:
      - https://cwe.mitre.org/data/definitions/78.html

  # ... ranks 6-25 in the full file
```

Full CWE Top 25 catalog file lists all 25 entries. Brief structure shown
here; the full list will be populated from the canonical MITRE source.

#### Worked example: `stride.yaml`

```yaml
name: stride
version: 1
description: STRIDE threat modeling categories
references:
  - https://learn.microsoft.com/en-us/azure/security/develop/threat-modeling-tool-threats

concerns:
  - id: stride:spoofing
    name: Spoofing — identity verification
    description: |
      Threats to authentication — fake identities, credential theft,
      impersonation of users / services / tokens.
    severity: high
    applicable_globs: ["**/auth/**", "**/login/**", "**/middleware/**"]

  - id: stride:tampering
    name: Tampering — data integrity
    description: |
      Threats to data integrity in transit or at rest — modification of
      messages, files, or stored state without authorization.
    severity: high

  - id: stride:repudiation
    name: Repudiation — non-repudiation / audit
    description: |
      Threats arising from missing or insufficient logging — actions
      cannot be later proven to have happened.
    severity: medium

  - id: stride:information-disclosure
    name: Information Disclosure — confidentiality
    description: |
      Unauthorized exposure of data — error messages leaking internals,
      sensitive data in logs, missing encryption.
    severity: high

  - id: stride:denial-of-service
    name: Denial of Service — availability
    description: |
      Resource exhaustion, missing rate limits, unbounded recursion,
      algorithmic-complexity attacks.
    severity: medium

  - id: stride:elevation-of-privilege
    name: Elevation of Privilege — authorization
    description: |
      Privilege escalation — vertical (user → admin), horizontal
      (user A → user B), or via missing authorization checks.
    severity: critical
    applicable_globs: ["**/auth/**", "**/middleware/**", "**/handlers/**"]
```

#### Worked example: `code-smells.yaml` (excerpt)

```yaml
name: code-smells
version: 1
description: Fowler-style code smells — design issues that suggest deeper problems
references:
  - https://martinfowler.com/bliki/CodeSmell.html

concerns:
  - id: code-smells:long-method
    name: Long method
    description: |
      Functions / methods that exceed reasonable length (rough heuristic:
      >100 lines or >40 lines of significant logic). Often a sign of
      mixed responsibilities.
    severity: low

  - id: code-smells:god-object
    name: God object
    description: |
      A class or module that has too many responsibilities — knows too
      much, does too much. Brittle, hard to test.
    severity: medium

  - id: code-smells:feature-envy
    name: Feature envy
    description: |
      A method that uses methods of another class more than its own —
      suggests the method belongs in the other class.
    severity: low

  - id: code-smells:duplicated-code
    name: Duplicated code
    description: |
      Same or similar code in multiple places — bug fixes have to be
      applied in N places, drift over time.
    severity: low

  # ... ~8 more smells (data clumps, primitive obsession, switch statements,
  # parallel inheritance, lazy class, speculative generality, etc.)
```

#### Composite: `web-security-default.yaml`

```yaml
name: web-security-default
version: 1
description: Sensible default for reviewing a web application
concerns:
  - include: owasp-top10-2021
  - include: cwe-top25-2023
  - include: secrets-in-source
```

### Audit / report generation

After a run (or on demand via `rupu coverage audit <target>`), the
harness produces a report by joining the three ledgers:

1. **Per concern**: for each concern in the effective catalog:
   - Files in scope (touched ∩ matching `applicable_globs`)
   - Files asserted (any status)
   - Files asserted with `status` ∈ {clean, finding, examined}
   - Gap = (in scope) − (asserted with non-N/A status)
2. **Per file**: for each file with at least one touch:
   - Strongest touch mode
   - Concerns asserted against it
   - Concerns expected but missing (catalog concerns matching applicable_globs but no assertion)
3. **Cross-model**: for each (concern, file) pair touched by multiple
   models, surface agreement vs. disagreement on status.
4. **Serendipitous findings**: findings with `concern_id: null` clustered
   by file path and summary text for human review.

Report format: structured JSON suitable for further tooling, plus a
human-readable rendering via the existing `rupu-cli` output framework
(reusing the printer / palette / table primitives from `rupu-cli/src/output/`).

### Tool-prompt section injected by the harness

When `concerns:` is non-empty, the harness appends a section to the
agent's system prompt:

```
## Coverage Catalog

You are reviewing this workspace against the following concerns. For each
(file × concern) you assess, call `coverage_mark` with the appropriate
status. For each issue you discover, call `report_finding`. Files you
read, grep, or edit are tracked automatically — you do not need to
declare them.

### owasp-top10-2021:a01-broken-access-control
**Name:** A01:2021 — Broken Access Control
**Severity:** critical
**Applies to:** **/handlers/**, **/controllers/**, **/middleware/**, **/routes/**, **/api/**

Restrictions on what authenticated users are allowed to do are not
enforced — vertical and horizontal privilege escalation, missing
function-level authorization, CORS misconfiguration, forced browsing,
IDOR.

References:
- https://owasp.org/Top10/A01_2021-Broken_Access_Control/

### owasp-top10-2021:a02-cryptographic-failures
...
```

This is the model's source of truth for "what to do." The catalog
section is rendered from the *effective* catalog snapshot, so any
overrides and inline concerns are visible to the model.

## Components

### New crate: `rupu-coverage`

Owns:

- Catalog types (`Concern`, `Catalog`, `Template`) and flatten / merge logic.
- Ledger types (`FileTouchEvent`, `ConcernAssertion`, `FindingRecord`) and JSONL writers / readers.
- Tool implementations (`coverage_mark`, `coverage_status`, `coverage_remaining`, `report_finding`) — exposed via the standard `Tool` trait from `rupu-tools`.
- Catalog template files under `templates/concerns/`.
- Audit / report generator.

Cargo dependencies: `serde`, `serde_json`, `serde_yaml`, `tokio`, `ulid`,
`thiserror`, `glob`. No new transitive dependencies the workspace
doesn't already use.

### Changes to `rupu-tools`

- `ToolContext` gains `coverage_writer: Option<Arc<CoverageWriter>>`.
- Built-in tools (`read_file`, `grep`, `glob`, `edit_file`, `bash`) emit `FileTouchEvent`s when the writer is present.
- Unrecognized tool calls emit a `tool_call_observed` event (no `path`) so the audit can warn about untracked file access.
- Users may register a touch-mapping for custom tools via `.rupu/coverage/tool-mappings.yaml`.

### Changes to `rupu-agent`

- `AgentSpec` gains a `concerns: Option<ConcernsBlock>` field parsed from agent file frontmatter.
- When `concerns` is non-empty, the runtime:
  1. Flattens the catalog and writes the snapshot.
  2. Instantiates the `CoverageWriter` and attaches it to `ToolContext`.
  3. Auto-injects the four coverage tools into the agent's tool list.
  4. Appends the catalog section to the agent's system prompt.

### Changes to `rupu-orchestrator`

- `Workflow` gains a `concerns: Option<ConcernsBlock>` field at the workflow YAML top level.
- Workflow runs flatten the catalog at start and propagate it to each step's agent.
- Steps cannot override the workflow's catalog — workflow-level catalog wins (single source of truth per workflow run).

### Changes to `rupu-cli`

New subcommand `rupu coverage` with subcommands:

- `rupu coverage show <target> [--concern <id>] [--file <path>]` — print derived view from the ledgers.
- `rupu coverage audit <target>` — generate the audit report.
- `rupu coverage gap <target>` — short alias for "what's left."
- `rupu coverage catalog <target>` — print the effective catalog snapshot.
- `rupu coverage templates list` — list shipped templates.
- `rupu coverage templates show <name>` — print a template's concerns.

The session UI (Slice C) gains a footer-row indicator: `coverage 12/15
concerns` when active.

### Catalog templates location

```
crates/rupu-coverage/templates/concerns/
  ├── owasp-top10-2021.yaml
  ├── owasp-api-top10-2023.yaml
  ├── cwe-top25-2023.yaml
  ├── stride.yaml
  ├── secrets-in-source.yaml
  ├── code-smells.yaml
  ├── web-security-default.yaml
  └── api-security-default.yaml
```

Templates are bundled into the binary via `include_str!` at compile
time so the CLI is self-sufficient. User-defined templates may live in
`.rupu/concerns/` (project) or `~/.rupu/concerns/` (global) and are
discovered by name with project overriding global overriding builtin.

## Alternatives considered

1. **Single combined table (file × concern × verdict).** Rejected: collapses two concerns into one ledger, can't track file touches that aren't tied to a concern, doubles row count, makes "what files did the agent touch" a more expensive query.

2. **Coverage as SQLite.** Rejected: JSONL is the rupu convention; append-only is replay-friendly; humans can `tail` and `jq` the ledger directly; no SQLite dependency to add.

3. **Agent-generated catalog (planning pass).** Rejected: the catalog itself becomes non-deterministic, breaking the auditability story. Catalog evolution happens via human review of clustering serendipitous findings instead — the slower-but-trustworthy loop.

4. **Background-color highlight on agent-mark rows in session view.** Out of scope here; covered separately if needed.

5. **Coverage marking via prompt convention** (agent emits `<coverage>...</coverage>` blocks the harness parses). Rejected: tool-call-based marking is structured, validatable, and gives the agent feedback (rejection messages) it can act on.

## Risks

- **Tool-runtime overhead.** Every tool call emits an event. Mitigated by batched async writes through an MPSC channel; per-call CPU overhead is one Arc clone + one channel send.
- **Catalog size in the system prompt.** Including all 58 concerns from the default web template adds ≈3-4 kB to the system prompt. Acceptable; if catalogs grow much larger we'd compact via dropping verbose descriptions for less-likely concerns.
- **Agent gaming.** Agent might mark concerns "clean" without doing the work. Validation against `files.jsonl` catches the trivial cases (claimed-but-not-read). Cross-model audit catches subtler cases (model A says clean, model B finds an issue — surface the disagreement).
- **Catalog drift across templates.** OWASP / CWE update their lists periodically. Templates are versioned (`version: 1`); when MITRE publishes CWE Top 25 (2024), we ship `cwe-top25-2024.yaml` alongside `cwe-top25-2023.yaml` without breaking existing snapshots.
- **Multi-target identity ambiguity.** Workflow + agent with the same name in different workspaces should have distinct target_ids. Mitigated by including the workspace's stable identifier in the hash.

## Acceptance

- Running `rupu workflow run security-review --target ./some-repo` against a workflow with `concerns: [include: web-security-default]` produces all three JSONL files plus the snapshot under `.rupu/coverage/<target_id>/`.
- The four coverage tools are present in the agent's available tools and the catalog is visible in the system prompt.
- `coverage_mark` with `status: clean` on a file the agent never read is rejected.
- A second run on the same target without re-running succeeds, and the second-pass agent's `coverage_remaining` call shows only the gaps left by the first run.
- `rupu coverage audit <target>` produces a structured report identifying gaps, per-concern coverage, and serendipitous findings.
- All shipped templates (`owasp-top10-2021`, `cwe-top25-2023`, `stride`, etc.) parse cleanly and round-trip through the catalog flattener.

## Out of scope (this slice)

- **Slice B — reproducibility.** A follow-up feature, designed against this one. Adds seeded prompting, deterministic concern + file ordering, two-run diff tooling, and a "what changed between run X and run Y" report. Stackable; users can run only A, only B, or both.
- **Cross-target catalog aliases.** "ssrf" in one catalog = "owasp-top10-2021:a10-ssrf" in another — needs an alias registry; punted to v2.
- **Concern excludes** (`exclude: [id1, id2]`) — usable already via `overrides` setting `description: skipped`, but a first-class exclude lands when there's clear demand.
- **rupu-app coverage view** — graphical surface for the audit report. Slice C-adjacent.
- **Coverage-driven cost estimates.** "Reviewing this repo for these concerns will cost ~$X." Useful but out of scope.
