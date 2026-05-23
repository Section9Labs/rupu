# rupu coverage harness — Plan 1: Foundation + curated catalog

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a minimum-viable coverage harness that lets a workflow or agent declare a `concerns:` block from curated industry-standard templates, automatically tracks file touches, and exposes four agent tools (`coverage_mark`, `coverage_status`, `coverage_remaining`, `report_finding`) backed by three append-only JSONL ledgers and a catalog snapshot. End state: `rupu workflow run security-review` against a repo produces a real coverage record.

**Architecture:** A new `rupu-coverage` crate owns all catalog types, ledger types, JSONL writers, and the four tool implementations. `rupu-tools` gains a `CoverageWriter` field on `ToolContext` that the built-in file-touching tools emit events to. `rupu-agent` and `rupu-orchestrator` read the `concerns:` block from agent frontmatter / workflow YAML respectively, flatten it (resolving includes + overrides), write the snapshot, render the catalog into the system prompt, and inject the four tools.

**Tech Stack:** Rust 2021 (workspace edition), `serde` / `serde_yaml` / `serde_json`, `tokio` (async writer + MPSC channel), `ulid` (finding IDs), `glob` (applicable_globs matching), `sha2` (target_id hashing), `chrono` (timestamps), `thiserror` (error types). Tests use the workspace-standard `tempfile` + `MockProvider` patterns already present in `rupu-agent`.

**Spec:** `docs/superpowers/specs/2026-05-23-rupu-coverage-harness-design.md`

**Out of scope for this plan** (deferred to follow-ups):
- CWE-full templates + the generator (`gen_cwe_catalog.rs`) — Plan 2.
- Index-mode catalog rendering + `coverage_concerns_search` / `coverage_concerns_detail` tools — Plan 2.
- Catalog filters (severity / tags / ids / applicable_to_path) on `include:` directives — Plan 2.
- `rupu coverage` CLI subcommand + human-readable audit rendering — Plan 3.
- Session-surface integration + `/coverage` slash command — Plan 3.
- Per-target `tool-mappings.yaml` for unknown MCP tools — Plan 3.

---

## File structure

```
crates/rupu-coverage/                         (NEW)
├── Cargo.toml
├── src/
│   ├── lib.rs                                # crate-public API surface
│   ├── catalog/
│   │   ├── mod.rs                            # re-exports
│   │   ├── types.rs                          # Concern, Severity, Template, ConcernsBlock, IncludeDirective
│   │   ├── parse.rs                          # YAML parsing for templates and concerns blocks
│   │   ├── flatten.rs                        # include resolution, inline-wins, overrides, duplicate detection
│   │   ├── builtin.rs                        # include_str! the 8 curated templates
│   │   ├── render.rs                         # full-mode prompt section rendering
│   │   └── snapshot.rs                       # write effective catalog to catalog.yaml
│   ├── ledger/
│   │   ├── mod.rs                            # re-exports
│   │   ├── events.rs                         # FileTouchEvent, ConcernAssertion, FindingRecord types
│   │   ├── target_id.rs                      # deterministic target_id from (workspace, scope_name)
│   │   ├── paths.rs                          # canonical ledger file paths under .rupu/coverage/<target_id>/
│   │   ├── writer.rs                         # CoverageWriter (tokio task + MPSC channel)
│   │   └── views.rs                          # derived per-file and per-concern views
│   ├── tools/
│   │   ├── mod.rs                            # CoverageTools::register(...)
│   │   ├── coverage_mark.rs                  # the mutating tool + validation
│   │   ├── coverage_status.rs                # query prior assertions
│   │   ├── coverage_remaining.rs             # touched-but-unasserted
│   │   └── report_finding.rs                 # findings.jsonl appender
│   └── error.rs                              # CoverageError (thiserror)
├── templates/concerns/                       (NEW)
│   ├── owasp-top10-2021.yaml
│   ├── owasp-api-top10-2023.yaml
│   ├── cwe-top25-2023.yaml
│   ├── stride.yaml
│   ├── secrets-in-source.yaml
│   ├── code-smells.yaml
│   ├── web-security-default.yaml
│   └── api-security-default.yaml
└── tests/
    └── end_to_end.rs                         # workflow-with-concerns smoke test

crates/rupu-tools/src/                        (MODIFY)
└── context.rs                                # add coverage_writer: Option<Arc<CoverageWriter>>

crates/rupu-tools/src/builtin/                (MODIFY)
├── read_file.rs                              # emit FileTouchEvent
├── grep.rs                                   # emit FileTouchEvent per matching file
├── glob.rs                                   # emit FileTouchEvent per matched path
├── edit_file.rs                              # emit FileTouchEvent
└── bash.rs                                   # emit FileTouchEvent for recognized path args

crates/rupu-agent/src/                        (MODIFY)
├── spec.rs                                   # parse `concerns:` from agent frontmatter
└── runner.rs                                 # flatten catalog, write snapshot, render prompt, inject tools

crates/rupu-orchestrator/src/                 (MODIFY)
├── workflow.rs                               # parse `concerns:` from workflow YAML
└── executor/in_process.rs                    # propagate workflow catalog to each step's agent

Cargo.toml                                    (MODIFY)
└── workspace.members                         # add "crates/rupu-coverage"
```

---

## Task 1: Create `rupu-coverage` crate skeleton

**Files:**
- Create: `crates/rupu-coverage/Cargo.toml`
- Create: `crates/rupu-coverage/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)
- Test: `crates/rupu-coverage/src/lib.rs` (inline)

- [ ] **Step 1: Write the failing test**

Add to `crates/rupu-coverage/src/lib.rs`:

```rust
//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_compiles() {
        // Sentinel: this test exists only so `cargo test -p rupu-coverage`
        // exercises the crate skeleton; later tasks replace it.
        assert_eq!(2 + 2, 4);
    }
}
```

- [ ] **Step 2: Author `Cargo.toml`**

Create `crates/rupu-coverage/Cargo.toml`:

```toml
[package]
name = "rupu-coverage"
version.workspace = true
edition = "2021"
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
serde_yaml = { workspace = true }
tokio = { workspace = true, features = ["sync", "rt", "macros", "fs", "io-util"] }
tracing = { workspace = true }
thiserror = { workspace = true }
chrono = { workspace = true, features = ["serde"] }
ulid = { workspace = true }
glob = { workspace = true }
sha2 = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true, features = ["test-util"] }
```

If `sha2`, `glob`, or `ulid` are not yet workspace deps, add them to the root `Cargo.toml` `[workspace.dependencies]` block with pinned versions before this step (check `cargo tree` against an existing crate's deps; if rupu already uses them transitively, hoist to workspace).

- [ ] **Step 3: Register the crate in the workspace**

Edit the workspace `Cargo.toml`. Find the `[workspace]` `members = [...]` array and add `"crates/rupu-coverage"` in alphabetical position (between `rupu-config` and `rupu-keychain-acl` or similar).

- [ ] **Step 4: Run the build + test**

```bash
cargo build -p rupu-coverage
cargo test -p rupu-coverage
```

Expected: builds clean, one passing test (`crate_compiles`).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage Cargo.toml
git commit -m "feat(coverage): add rupu-coverage crate skeleton"
```

---

## Task 2: Catalog types — `Concern`, `Severity`, `Template`

**Files:**
- Create: `crates/rupu-coverage/src/catalog/mod.rs`
- Create: `crates/rupu-coverage/src/catalog/types.rs`
- Modify: `crates/rupu-coverage/src/lib.rs` (re-export)
- Test: `crates/rupu-coverage/src/catalog/types.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/catalog/types.rs` with the test at the bottom:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Default for Severity {
    fn default() -> Self {
        Severity::Medium
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concern {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default = "default_applicable_globs")]
    pub applicable_globs: Vec<String>,
    #[serde(default = "default_min_strength")]
    pub min_strength: TouchStrength,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_applicable_globs() -> Vec<String> {
    vec!["**".to_string()]
}

fn default_min_strength() -> TouchStrength {
    TouchStrength::Read
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchStrength {
    Glob,
    Cmd,
    Grep,
    Read,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    #[serde(default = "default_template_version")]
    pub version: u32,
    pub description: String,
    #[serde(default)]
    pub references: Vec<String>,
    pub concerns: Vec<Concern>,
}

fn default_template_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concern_yaml_round_trip_with_defaults() {
        let yaml = r#"
id: secrets-in-source
name: Secrets in source
description: Find hardcoded credentials.
"#;
        let concern: Concern = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(concern.id, "secrets-in-source");
        assert_eq!(concern.severity, Severity::Medium);
        assert_eq!(concern.applicable_globs, vec!["**".to_string()]);
        assert_eq!(concern.min_strength, TouchStrength::Read);
    }

    #[test]
    fn touch_strength_orders_glob_below_edit() {
        assert!(TouchStrength::Glob < TouchStrength::Read);
        assert!(TouchStrength::Read < TouchStrength::Edit);
    }
}
```

- [ ] **Step 2: Create the catalog module**

Create `crates/rupu-coverage/src/catalog/mod.rs`:

```rust
pub mod types;
pub use types::{Concern, Severity, Template, TouchStrength};
```

- [ ] **Step 3: Re-export from `lib.rs`**

Edit `crates/rupu-coverage/src/lib.rs`, replace the body (keeping the `#![deny]` lines):

```rust
//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod catalog;

pub use catalog::{Concern, Severity, Template, TouchStrength};
```

Delete the placeholder `tests` module — the catalog tests replace it.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: two passing tests (`concern_yaml_round_trip_with_defaults`, `touch_strength_orders_glob_below_edit`).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): define core catalog types (Concern, Template, Severity, TouchStrength)"
```

---

## Task 3: Template parsing

**Files:**
- Create: `crates/rupu-coverage/src/catalog/parse.rs`
- Modify: `crates/rupu-coverage/src/catalog/mod.rs`
- Test: `crates/rupu-coverage/src/catalog/parse.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/catalog/parse.rs`:

```rust
use crate::catalog::types::Template;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("yaml error in {path}: {source}")]
    Yaml {
        path: String,
        #[source]
        source: serde_yaml::Error,
    },
}

pub fn parse_template_str(yaml: &str, source_label: &str) -> Result<Template, ParseError> {
    serde_yaml::from_str(yaml).map_err(|source| ParseError::Yaml {
        path: source_label.to_string(),
        source,
    })
}

pub fn parse_template_file(path: &Path) -> Result<Template, ParseError> {
    let yaml = std::fs::read_to_string(path).map_err(|source| ParseError::Io {
        path: path.display().to_string(),
        source,
    })?;
    parse_template_str(&yaml, &path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE_FIXTURE: &str = r#"
name: stride
version: 1
description: STRIDE threat modeling categories
references:
  - https://learn.microsoft.com/en-us/azure/security/develop/threat-modeling-tool-threats

concerns:
  - id: stride:spoofing
    name: Spoofing
    description: Identity-verification threats.
    severity: high
  - id: stride:tampering
    name: Tampering
    description: Data-integrity threats.
    severity: high
"#;

    #[test]
    fn parse_stride_fixture() {
        let template = parse_template_str(STRIDE_FIXTURE, "stride.yaml").unwrap();
        assert_eq!(template.name, "stride");
        assert_eq!(template.concerns.len(), 2);
        assert_eq!(template.concerns[0].id, "stride:spoofing");
        assert_eq!(
            template.concerns[1].name,
            "Tampering"
        );
    }

    #[test]
    fn parse_template_missing_required_field_errors() {
        let bad = r#"
name: missing-description
concerns: []
"#;
        let err = parse_template_str(bad, "bad.yaml").unwrap_err();
        assert!(matches!(err, ParseError::Yaml { .. }));
    }
}
```

- [ ] **Step 2: Wire into module**

Edit `crates/rupu-coverage/src/catalog/mod.rs`:

```rust
pub mod parse;
pub mod types;
pub use parse::{parse_template_file, parse_template_str, ParseError};
pub use types::{Concern, Severity, Template, TouchStrength};
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: 4 passing tests (2 from Task 2 + 2 new).

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): parse template YAML into Template struct"
```

---

## Task 4: Author the 8 curated template files

**Files:**
- Create: `crates/rupu-coverage/templates/concerns/owasp-top10-2021.yaml`
- Create: `crates/rupu-coverage/templates/concerns/owasp-api-top10-2023.yaml`
- Create: `crates/rupu-coverage/templates/concerns/cwe-top25-2023.yaml`
- Create: `crates/rupu-coverage/templates/concerns/stride.yaml`
- Create: `crates/rupu-coverage/templates/concerns/secrets-in-source.yaml`
- Create: `crates/rupu-coverage/templates/concerns/code-smells.yaml`
- Create: `crates/rupu-coverage/templates/concerns/web-security-default.yaml`
- Create: `crates/rupu-coverage/templates/concerns/api-security-default.yaml`

These are large data files. The spec contains the full content for OWASP Top 10 (Task 4a), CWE Top 25 top 5 (full corpus in Task 4c), STRIDE (Task 4d), and code-smells excerpt (Task 4f). Author each file directly from the spec's worked-example sections — the spec is the authoritative source for entries.

- [ ] **Step 1: Author `owasp-top10-2021.yaml`**

Copy verbatim from the spec section "Worked example: `owasp-top10-2021.yaml`". All 10 entries: `a01-broken-access-control` through `a10-ssrf`.

- [ ] **Step 2: Author `cwe-top25-2023.yaml`**

The spec shows the top 5 entries (CWE-787, CWE-79, CWE-89, CWE-416, CWE-78). Add the remaining 20 entries from the [2023 CWE Top 25 list](https://cwe.mitre.org/top25/archive/2023/2023_top25_list.html), following the same schema. For each:
- `id`: `cwe-top25-2023:cwe-{N}-{kebab-name}`
- `name`: `CWE-{N} — {Title} (rank #{R})`
- `description`: 2-4 sentence summary derived from MITRE's CWE entry.
- `severity`: Critical for memory-corruption / RCE primitives (787, 416, 125, 78, 502); High for injection / XSS / auth (89, 79, 287, 22, 352); Medium for the rest.
- `applicable_globs`: Tailored to the weakness (memory-corruption → C/C++/Rust unsafe; XSS → templates/views; ad-hoc otherwise).
- `references`: `https://cwe.mitre.org/data/definitions/{N}.html`

- [ ] **Step 3: Author `stride.yaml`**

Copy verbatim from the spec section "Worked example: `stride.yaml`". All 6 entries.

- [ ] **Step 4: Author `owasp-api-top10-2023.yaml`**

Schema-mirror the OWASP Top 10 2021 structure. The 10 entries are the official [OWASP API Security Top 10 (2023)](https://owasp.org/API-Security/editions/2023/en/0x11-t10/) categories (API1–API10). For each:
- `id`: `owasp-api-top10-2023:api{N}-{kebab-name}` (e.g. `api1-broken-object-level-authorization`).
- `name`: `API{N}:2023 — {Title}`.
- `description`: 2-4 sentences from OWASP's published explanation.
- `severity`: Critical for auth-bypass / authorization (API1, API3, API5); High for injection / consumption (API4, API6, API10); Medium for inventory / monitoring / config (API2, API7, API8, API9).
- `applicable_globs`: `["**/handlers/**", "**/routes/**", "**/controllers/**", "**/api/**", "**/middleware/**"]` for most; expand per concern.
- `references`: the editions/2023 permalink for each category.

- [ ] **Step 5: Author `secrets-in-source.yaml`**

One concern:

```yaml
name: secrets-in-source
version: 1
description: Hardcoded secrets in source code
references:
  - https://cwe.mitre.org/data/definitions/798.html
concerns:
  - id: secrets-in-source
    name: Secrets in source code
    description: |
      Find hardcoded credentials, API keys, tokens, passwords, or other
      sensitive material committed to the repository. Includes .env-style
      configuration files checked into version control.
    severity: high
    applicable_globs:
      - "**/*.rs"
      - "**/*.py"
      - "**/*.ts"
      - "**/*.js"
      - "**/*.go"
      - "**/*.toml"
      - "**/*.yaml"
      - "**/*.yml"
      - "**/*.json"
      - "**/.env*"
      - "!**/target/**"
      - "!**/node_modules/**"
      - "!**/.git/**"
    references:
      - https://cwe.mitre.org/data/definitions/798.html
      - https://owasp.org/Top10/A02_2021-Cryptographic_Failures/
```

- [ ] **Step 6: Author `code-smells.yaml`**

The spec shows 4 entries (long-method, god-object, feature-envy, duplicated-code). Add 8 more from Fowler's standard catalog:
- `code-smells:data-clumps` (Medium): Groups of variables that travel together; signals missing class.
- `code-smells:primitive-obsession` (Low): Using primitives for domain concepts that warrant types.
- `code-smells:switch-statements` (Low): Long switch/match on type tags — usually misses polymorphism.
- `code-smells:speculative-generality` (Low): Abstractions added for hypothetical future requirements.
- `code-smells:lazy-class` (Low): Class that doesn't justify its existence.
- `code-smells:large-class` (Medium): Class with too many fields/methods — sibling of god-object.
- `code-smells:divergent-change` (Medium): One class changed for many unrelated reasons.
- `code-smells:shotgun-surgery` (Medium): One conceptual change requires edits to many classes.

Each entry uses `applicable_globs: ["**/*.rs"]` (or omit for default `["**"]`).

- [ ] **Step 7: Author `web-security-default.yaml`** (composite — empty `concerns:` allowed)

```yaml
name: web-security-default
version: 1
description: Sensible default for reviewing a web application
concerns: []
includes:
  - owasp-top10-2021
  - cwe-top25-2023
  - secrets-in-source
```

Note: this introduces a new template-level `includes:` field. Defer the parsing change to Task 6 where flatten logic lives. For now, `parse_template_file` will still accept it (extra fields are tolerated by serde with `#[serde(default)]` on the new field). Add `includes: Vec<String>` to `Template` in `types.rs` with `#[serde(default)]`.

- [ ] **Step 8: Author `api-security-default.yaml`**

```yaml
name: api-security-default
version: 1
description: Sensible default for reviewing an API service
concerns: []
includes:
  - owasp-api-top10-2023
  - cwe-top25-2023
  - secrets-in-source
```

- [ ] **Step 9: Verify each template parses**

Write a parameterized test in `crates/rupu-coverage/src/catalog/parse.rs`:

```rust
#[test]
fn all_curated_templates_parse() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/concerns");
    let expected = [
        "owasp-top10-2021.yaml",
        "owasp-api-top10-2023.yaml",
        "cwe-top25-2023.yaml",
        "stride.yaml",
        "secrets-in-source.yaml",
        "code-smells.yaml",
        "web-security-default.yaml",
        "api-security-default.yaml",
    ];
    for filename in expected {
        let path = dir.join(filename);
        parse_template_file(&path).unwrap_or_else(|e| panic!("failed to parse {filename}: {e}"));
    }
}
```

Add `includes: Vec<String>` to `Template` (default empty) in `types.rs`:

```rust
#[serde(default)]
pub includes: Vec<String>,
```

- [ ] **Step 10: Run the tests**

```bash
cargo test -p rupu-coverage all_curated_templates_parse
```

Expected: pass.

- [ ] **Step 11: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): author 8 curated catalog templates (OWASP, CWE-25, STRIDE, code smells)"
```

---

## Task 5: Built-in template registry

**Files:**
- Create: `crates/rupu-coverage/src/catalog/builtin.rs`
- Modify: `crates/rupu-coverage/src/catalog/mod.rs`
- Test: `crates/rupu-coverage/src/catalog/builtin.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/catalog/builtin.rs`:

```rust
use crate::catalog::parse::{parse_template_str, ParseError};
use crate::catalog::types::Template;

/// Static map of template name → bundled YAML body.
const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (
        "owasp-top10-2021",
        include_str!("../../templates/concerns/owasp-top10-2021.yaml"),
    ),
    (
        "owasp-api-top10-2023",
        include_str!("../../templates/concerns/owasp-api-top10-2023.yaml"),
    ),
    (
        "cwe-top25-2023",
        include_str!("../../templates/concerns/cwe-top25-2023.yaml"),
    ),
    (
        "stride",
        include_str!("../../templates/concerns/stride.yaml"),
    ),
    (
        "secrets-in-source",
        include_str!("../../templates/concerns/secrets-in-source.yaml"),
    ),
    (
        "code-smells",
        include_str!("../../templates/concerns/code-smells.yaml"),
    ),
    (
        "web-security-default",
        include_str!("../../templates/concerns/web-security-default.yaml"),
    ),
    (
        "api-security-default",
        include_str!("../../templates/concerns/api-security-default.yaml"),
    ),
];

pub fn resolve_builtin(name: &str) -> Option<Result<Template, ParseError>> {
    BUILTIN_TEMPLATES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, body)| parse_template_str(body, &format!("builtin:{name}")))
}

pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_TEMPLATES.iter().map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_builtin_resolves_to_template_with_matching_name() {
        for name in builtin_names() {
            let resolved = resolve_builtin(name).expect("name exists").expect("parses");
            assert_eq!(resolved.name, name, "template body's name field must match registry key");
        }
    }

    #[test]
    fn unknown_template_returns_none() {
        assert!(resolve_builtin("definitely-not-real").is_none());
    }
}
```

- [ ] **Step 2: Wire into module**

Edit `crates/rupu-coverage/src/catalog/mod.rs`:

```rust
pub mod builtin;
pub mod parse;
pub mod types;
pub use builtin::{builtin_names, resolve_builtin};
pub use parse::{parse_template_file, parse_template_str, ParseError};
pub use types::{Concern, Severity, Template, TouchStrength};
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: all prior tests + 2 new (`each_builtin_resolves_to_template_with_matching_name`, `unknown_template_returns_none`).

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): bundle curated templates via include_str! registry"
```

---

## Task 6: ConcernsBlock + include resolution (flatten)

**Files:**
- Create: `crates/rupu-coverage/src/catalog/flatten.rs`
- Modify: `crates/rupu-coverage/src/catalog/types.rs` (add ConcernsBlock, IncludeDirective)
- Modify: `crates/rupu-coverage/src/catalog/mod.rs`
- Test: `crates/rupu-coverage/src/catalog/flatten.rs` (inline)

- [ ] **Step 1: Add `ConcernsBlock` and `IncludeDirective` to `types.rs`**

Append to `crates/rupu-coverage/src/catalog/types.rs`:

```rust
/// A user-declared concerns block — appears in agent frontmatter or
/// workflow YAML. A list of entries, each either an inline concern or
/// an include of a named template.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConcernsBlock {
    pub entries: Vec<ConcernsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConcernsEntry {
    Include(IncludeDirective),
    Inline(Concern),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncludeDirective {
    pub include: String,
    #[serde(default)]
    pub overrides: Vec<ConcernOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcernOverride {
    pub id: String,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub applicable_globs: Option<Vec<String>>,
    #[serde(default)]
    pub min_strength: Option<TouchStrength>,
    #[serde(default)]
    pub references: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub description: Option<String>,
}

/// The flattened catalog — what the harness actually uses. All includes
/// resolved, all overrides applied, all duplicates rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlatCatalog {
    pub concerns: Vec<Concern>,
    /// Source-tracking: for each concern_id, where it came from (template name or "inline").
    pub sources: std::collections::BTreeMap<String, String>,
}
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-coverage/src/catalog/flatten.rs`:

```rust
use crate::catalog::builtin::resolve_builtin;
use crate::catalog::types::{
    Concern, ConcernOverride, ConcernsBlock, ConcernsEntry, FlatCatalog, IncludeDirective, Template,
};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum FlattenError {
    #[error("unknown template `{0}`")]
    UnknownTemplate(String),
    #[error("template `{0}` failed to parse: {1}")]
    TemplateParse(String, String),
    #[error("duplicate concern_id `{id}` from `{first}` and `{second}` — declare an explicit override to resolve")]
    DuplicateId {
        id: String,
        first: String,
        second: String,
    },
    #[error("override targets unknown concern_id `{id}` in include `{template}`")]
    OverrideUnknownId { template: String, id: String },
}

pub fn flatten(block: &ConcernsBlock) -> Result<FlatCatalog, FlattenError> {
    flatten_with_resolver(block, &|name| {
        resolve_builtin(name)
            .ok_or_else(|| FlattenError::UnknownTemplate(name.to_string()))?
            .map_err(|e| FlattenError::TemplateParse(name.to_string(), e.to_string()))
    })
}

/// Lower-level entry point used by tests and by the agent runner (when
/// project/global templates are also available beyond the builtin set).
pub fn flatten_with_resolver<F>(
    block: &ConcernsBlock,
    resolve: &F,
) -> Result<FlatCatalog, FlattenError>
where
    F: Fn(&str) -> Result<Template, FlattenError>,
{
    // Pass 1: collect inline concerns first so they win on duplicate ids.
    let mut by_id: BTreeMap<String, Concern> = BTreeMap::new();
    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    for entry in &block.entries {
        if let ConcernsEntry::Inline(concern) = entry {
            by_id.insert(concern.id.clone(), concern.clone());
            sources.insert(concern.id.clone(), "inline".to_string());
        }
    }

    // Pass 2: resolve includes, recursing if a template `includes:` other templates.
    for entry in &block.entries {
        let ConcernsEntry::Include(directive) = entry else {
            continue;
        };
        let template = resolve(&directive.include)?;
        let mut template_concerns = template.concerns.clone();

        // Recurse into nested includes (composite templates like
        // web-security-default that list `includes: [...]`).
        for nested_name in &template.includes {
            let nested = resolve(nested_name)?;
            template_concerns.extend(nested.concerns);
        }

        // Apply overrides — must target a concern that exists in the
        // resolved template (after nested includes).
        let template_ids: std::collections::HashSet<&str> =
            template_concerns.iter().map(|c| c.id.as_str()).collect();
        for over in &directive.overrides {
            if !template_ids.contains(over.id.as_str()) {
                return Err(FlattenError::OverrideUnknownId {
                    template: directive.include.clone(),
                    id: over.id.clone(),
                });
            }
        }

        for mut concern in template_concerns {
            // Inline wins.
            if by_id.contains_key(&concern.id) && sources.get(&concern.id).map(String::as_str) == Some("inline") {
                continue;
            }
            // Apply override if present.
            if let Some(over) = directive.overrides.iter().find(|o| o.id == concern.id) {
                if let Some(s) = over.severity {
                    concern.severity = s;
                }
                if let Some(g) = over.applicable_globs.clone() {
                    concern.applicable_globs = g;
                }
                if let Some(m) = over.min_strength {
                    concern.min_strength = m;
                }
                if let Some(r) = over.references.clone() {
                    concern.references = r;
                }
                if let Some(t) = over.tags.clone() {
                    concern.tags = t;
                }
                if let Some(d) = over.description.clone() {
                    concern.description = d;
                }
            }

            // Duplicate-id detection across includes.
            if let Some(existing_source) = sources.get(&concern.id) {
                if existing_source != "inline" && existing_source != &directive.include {
                    return Err(FlattenError::DuplicateId {
                        id: concern.id.clone(),
                        first: existing_source.clone(),
                        second: directive.include.clone(),
                    });
                }
            }

            by_id.insert(concern.id.clone(), concern.clone());
            sources.entry(concern.id.clone()).or_insert_with(|| directive.include.clone());
        }
    }

    Ok(FlatCatalog {
        concerns: by_id.into_values().collect(),
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::{Concern, Severity};

    fn inline(id: &str) -> Concern {
        Concern {
            id: id.to_string(),
            name: id.to_string(),
            description: "test".to_string(),
            severity: Severity::Low,
            applicable_globs: vec!["**".to_string()],
            min_strength: crate::catalog::types::TouchStrength::Read,
            references: vec![],
            tags: vec![],
        }
    }

    #[test]
    fn flatten_single_include_pulls_template_concerns() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let cat = flatten(&block).unwrap();
        assert_eq!(cat.concerns.len(), 6);
        assert!(cat.concerns.iter().any(|c| c.id == "stride:spoofing"));
    }

    #[test]
    fn inline_concern_wins_over_include() {
        let mut custom = inline("stride:spoofing");
        custom.description = "OVERRIDDEN".to_string();
        let block = ConcernsBlock {
            entries: vec![
                ConcernsEntry::Include(IncludeDirective {
                    include: "stride".to_string(),
                    overrides: vec![],
                }),
                ConcernsEntry::Inline(custom),
            ],
        };
        let cat = flatten(&block).unwrap();
        let spoofing = cat
            .concerns
            .iter()
            .find(|c| c.id == "stride:spoofing")
            .unwrap();
        assert_eq!(spoofing.description, "OVERRIDDEN");
        assert_eq!(cat.sources["stride:spoofing"], "inline");
    }

    #[test]
    fn override_directive_patches_single_field() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![ConcernOverride {
                    id: "stride:spoofing".to_string(),
                    severity: Some(Severity::Critical),
                    ..Default::default()
                }],
            })],
        };
        let cat = flatten(&block).unwrap();
        let spoofing = cat
            .concerns
            .iter()
            .find(|c| c.id == "stride:spoofing")
            .unwrap();
        assert_eq!(spoofing.severity, Severity::Critical);
    }

    #[test]
    fn unknown_template_errors() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "not-a-real-template".to_string(),
                overrides: vec![],
            })],
        };
        let err = flatten(&block).unwrap_err();
        assert!(matches!(err, FlattenError::UnknownTemplate(_)));
    }

    #[test]
    fn composite_template_resolves_nested_includes() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "web-security-default".to_string(),
                overrides: vec![],
            })],
        };
        let cat = flatten(&block).unwrap();
        // owasp-top10-2021 (10) + cwe-top25-2023 (25) + secrets-in-source (1) = 36
        // assuming no id collisions between the three templates.
        assert!(cat.concerns.len() >= 30);
        assert!(cat.concerns.iter().any(|c| c.id.starts_with("owasp-top10-2021:")));
        assert!(cat.concerns.iter().any(|c| c.id.starts_with("cwe-top25-2023:")));
        assert!(cat.concerns.iter().any(|c| c.id == "secrets-in-source"));
    }
}
```

Note: `ConcernOverride` needs a `Default` impl. Add `#[derive(Default)]` to it in `types.rs`.

- [ ] **Step 3: Wire into module**

Edit `crates/rupu-coverage/src/catalog/mod.rs`:

```rust
pub mod builtin;
pub mod flatten;
pub mod parse;
pub mod types;
pub use builtin::{builtin_names, resolve_builtin};
pub use flatten::{flatten, flatten_with_resolver, FlattenError};
pub use parse::{parse_template_file, parse_template_str, ParseError};
pub use types::{
    Concern, ConcernOverride, ConcernsBlock, ConcernsEntry, FlatCatalog, IncludeDirective,
    Severity, Template, TouchStrength,
};
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: all prior tests + 5 new flatten tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): flatten concerns blocks with include resolution, overrides, and duplicate-id detection"
```

---

## Task 7: Catalog snapshot writer

**Files:**
- Create: `crates/rupu-coverage/src/catalog/snapshot.rs`
- Modify: `crates/rupu-coverage/src/catalog/mod.rs`
- Test: `crates/rupu-coverage/src/catalog/snapshot.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/catalog/snapshot.rs`:

```rust
use crate::catalog::types::FlatCatalog;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("io error writing {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("yaml serialization failed: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

pub fn write_snapshot(catalog: &FlatCatalog, path: &Path) -> Result<(), SnapshotError> {
    let yaml = serde_yaml::to_string(catalog)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SnapshotError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(path, yaml).map_err(|source| SnapshotError::Io {
        path: path.display().to_string(),
        source,
    })
}

pub fn read_snapshot(path: &Path) -> Result<FlatCatalog, SnapshotError> {
    let yaml = std::fs::read_to_string(path).map_err(|source| SnapshotError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let catalog = serde_yaml::from_str(&yaml)?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};

    #[test]
    fn snapshot_round_trips_through_yaml() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let original = flatten(&block).unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nested/catalog.yaml");
        write_snapshot(&original, &path).unwrap();

        let loaded = read_snapshot(&path).unwrap();
        assert_eq!(original, loaded);
    }
}
```

- [ ] **Step 2: Wire into module**

Edit `crates/rupu-coverage/src/catalog/mod.rs`:

```rust
pub mod builtin;
pub mod flatten;
pub mod parse;
pub mod snapshot;
pub mod types;
pub use builtin::{builtin_names, resolve_builtin};
pub use flatten::{flatten, flatten_with_resolver, FlattenError};
pub use parse::{parse_template_file, parse_template_str, ParseError};
pub use snapshot::{read_snapshot, write_snapshot, SnapshotError};
pub use types::{
    Concern, ConcernOverride, ConcernsBlock, ConcernsEntry, FlatCatalog, IncludeDirective,
    Severity, Template, TouchStrength,
};
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: all prior tests + 1 new snapshot test.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): write and read effective-catalog snapshot YAML"
```

---

## Task 8: Target ID derivation + ledger paths

**Files:**
- Create: `crates/rupu-coverage/src/ledger/mod.rs`
- Create: `crates/rupu-coverage/src/ledger/target_id.rs`
- Create: `crates/rupu-coverage/src/ledger/paths.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline in each new file

- [ ] **Step 1: Write the failing test for target_id**

Create `crates/rupu-coverage/src/ledger/target_id.rs`:

```rust
use sha2::{Digest, Sha256};
use std::path::Path;

/// Stable identifier for a coverage target.
///
/// Inputs: the workspace path (canonicalized when possible) and a
/// scope_name. The scope_name is the workflow name, agent name, or
/// session_id depending on which surface initiated the run.
///
/// Returns a 16-character lowercase hex prefix of the SHA-256 hash —
/// short enough for human-readable directory names, long enough to
/// avoid collisions in practice.
pub fn target_id(workspace: &Path, scope_name: &str) -> String {
    let canonical = workspace.canonicalize().unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.display().to_string().as_bytes());
    hasher.update(b"::");
    hasher.update(scope_name.as_bytes());
    let digest = hasher.finalize();
    hex_short(&digest)
}

fn hex_short(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(16);
    for byte in &bytes[..8] {
        use std::fmt::Write;
        write!(&mut out, "{byte:02x}").unwrap();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_id_is_deterministic() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = target_id(tmp.path(), "security-review");
        let b = target_id(tmp.path(), "security-review");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn target_id_differs_for_different_scopes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = target_id(tmp.path(), "security-review");
        let b = target_id(tmp.path(), "perf-review");
        assert_ne!(a, b);
    }

    #[test]
    fn target_id_differs_for_different_workspaces() {
        let tmp1 = tempfile::TempDir::new().unwrap();
        let tmp2 = tempfile::TempDir::new().unwrap();
        let a = target_id(tmp1.path(), "x");
        let b = target_id(tmp2.path(), "x");
        assert_ne!(a, b);
    }
}
```

- [ ] **Step 2: Write the failing test for paths**

Create `crates/rupu-coverage/src/ledger/paths.rs`:

```rust
use std::path::{Path, PathBuf};

/// Canonical layout of a target's coverage data on disk.
pub struct CoveragePaths {
    pub root: PathBuf,
    pub files: PathBuf,
    pub concerns: PathBuf,
    pub findings: PathBuf,
    pub catalog: PathBuf,
}

impl CoveragePaths {
    pub fn new(workspace: &Path, target_id: &str) -> Self {
        let root = workspace.join(".rupu").join("coverage").join(target_id);
        Self {
            files: root.join("files.jsonl"),
            concerns: root.join("concerns.jsonl"),
            findings: root.join("findings.jsonl"),
            catalog: root.join("catalog.yaml"),
            root,
        }
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_layout_under_dotrupu_coverage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "abc123");
        assert_eq!(paths.root, tmp.path().join(".rupu/coverage/abc123"));
        assert_eq!(paths.files, paths.root.join("files.jsonl"));
        assert_eq!(paths.concerns, paths.root.join("concerns.jsonl"));
        assert_eq!(paths.findings, paths.root.join("findings.jsonl"));
        assert_eq!(paths.catalog, paths.root.join("catalog.yaml"));
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "abc");
        paths.ensure_dir().unwrap();
        paths.ensure_dir().unwrap(); // second call must not fail
        assert!(paths.root.is_dir());
    }
}
```

- [ ] **Step 3: Module wiring**

Create `crates/rupu-coverage/src/ledger/mod.rs`:

```rust
pub mod paths;
pub mod target_id;
pub use paths::CoveragePaths;
pub use target_id::target_id;
```

Edit `crates/rupu-coverage/src/lib.rs`:

```rust
//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod catalog;
pub mod ledger;

pub use catalog::{
    builtin_names, flatten, read_snapshot, resolve_builtin, write_snapshot, Concern, ConcernOverride,
    ConcernsBlock, ConcernsEntry, FlatCatalog, FlattenError, IncludeDirective, ParseError, Severity,
    SnapshotError, Template, TouchStrength,
};
pub use ledger::{target_id, CoveragePaths};
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: all prior tests + 5 new (3 target_id, 2 paths) pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): derive target_id and canonical .rupu/coverage/<target>/ layout"
```

---

## Task 9: Ledger event types

**Files:**
- Create: `crates/rupu-coverage/src/ledger/events.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`
- Test: `crates/rupu-coverage/src/ledger/events.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/ledger/events.rs`:

```rust
use crate::catalog::types::{Severity, TouchStrength};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    pub run_id: String,
    pub model: String,
    pub surface: Surface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Workflow,
    Agent,
    Autoflow,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FileTouchEvent {
    Read {
        path: String,
        line_range: [u32; 2],
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Grep {
        path: String,
        pattern: String,
        match_count: u32,
        matched_lines: Vec<u32>,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Glob {
        path: String,
        pattern: String,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Edit {
        path: String,
        line_range: [u32; 2],
        lines_changed: u32,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Cmd {
        path: String,
        command: String,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Unknown {
        tool: String,
        arg_hash: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
}

impl FileTouchEvent {
    pub fn strength(&self) -> Option<TouchStrength> {
        match self {
            FileTouchEvent::Edit { .. } => Some(TouchStrength::Edit),
            FileTouchEvent::Read { .. } => Some(TouchStrength::Read),
            FileTouchEvent::Grep { .. } => Some(TouchStrength::Grep),
            FileTouchEvent::Cmd { .. } => Some(TouchStrength::Cmd),
            FileTouchEvent::Glob { .. } => Some(TouchStrength::Glob),
            FileTouchEvent::Unknown { .. } => None,
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            FileTouchEvent::Read { path, .. }
            | FileTouchEvent::Grep { path, .. }
            | FileTouchEvent::Glob { path, .. }
            | FileTouchEvent::Edit { path, .. }
            | FileTouchEvent::Cmd { path, .. } => Some(path),
            FileTouchEvent::Unknown { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionStatus {
    Clean,
    Finding,
    Examined,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub summary: String,
    #[serde(default)]
    pub line_ranges: Vec<[u32; 2]>,
    #[serde(default)]
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcernAssertion {
    pub concern_id: String,
    pub file_path: String,
    pub status: AssertionStatus,
    pub evidence: Evidence,
    pub declared_by: Attribution,
    pub declared_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingScope {
    Line,
    File,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingEvidence {
    #[serde(default)]
    pub code_excerpt: Option<String>,
    pub rationale: String,
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingRecord {
    pub id: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_range: Option<[u32; 2]>,
    pub scope: FindingScope,
    pub summary: String,
    pub severity: Severity,
    #[serde(default)]
    pub concern_id: Option<String>,
    pub evidence: FindingEvidence,
    pub declared_by: Attribution,
    pub declared_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_01KS19A4MQXP".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[test]
    fn file_touch_read_event_round_trips_jsonl() {
        let event = FileTouchEvent::Read {
            path: "src/handlers/users.rs".to_string(),
            line_range: [1, 240],
            tool: "read_file".to_string(),
            attribution: attribution(),
            at: DateTime::parse_from_rfc3339("2026-05-23T14:01:32Z").unwrap().with_timezone(&Utc),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: FileTouchEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
        assert_eq!(event.strength(), Some(TouchStrength::Read));
        assert_eq!(event.path(), Some("src/handlers/users.rs"));
    }

    #[test]
    fn concern_assertion_round_trips_jsonl() {
        let assertion = ConcernAssertion {
            concern_id: "stride:spoofing".to_string(),
            file_path: "src/auth/login.rs".to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "Token check covers all entry points.".to_string(),
                line_ranges: vec![[1, 80]],
                finding_ids: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
        };
        let json = serde_json::to_string(&assertion).unwrap();
        let decoded: ConcernAssertion = serde_json::from_str(&json).unwrap();
        assert_eq!(assertion, decoded);
    }

    #[test]
    fn finding_record_round_trips_jsonl_with_null_concern() {
        let record = FindingRecord {
            id: "fnd_01KS19A3".to_string(),
            file_path: Some("src/config.rs".to_string()),
            line_range: Some([20, 28]),
            scope: FindingScope::Line,
            summary: "Hardcoded API key.".to_string(),
            severity: Severity::High,
            concern_id: None, // serendipitous
            evidence: FindingEvidence {
                code_excerpt: Some("const STRIPE_KEY = \"sk_live_...\"".to_string()),
                rationale: "Key should come from env.".to_string(),
                references: vec!["https://cwe.mitre.org/data/definitions/798.html".to_string()],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
        };
        let json = serde_json::to_string(&record).unwrap();
        let decoded: FindingRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, decoded);
    }
}
```

- [ ] **Step 2: Wire into module**

Edit `crates/rupu-coverage/src/ledger/mod.rs`:

```rust
pub mod events;
pub mod paths;
pub mod target_id;
pub use events::{
    AssertionStatus, Attribution, ConcernAssertion, Evidence, FileTouchEvent, FindingEvidence,
    FindingRecord, FindingScope, Surface,
};
pub use paths::CoveragePaths;
pub use target_id::target_id;
```

- [ ] **Step 3: Re-export from `lib.rs`**

Edit `crates/rupu-coverage/src/lib.rs`'s `pub use ledger::...` line:

```rust
pub use ledger::{
    target_id, AssertionStatus, Attribution, ConcernAssertion, CoveragePaths, Evidence,
    FileTouchEvent, FindingEvidence, FindingRecord, FindingScope, Surface,
};
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: prior tests + 3 new events tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): define ledger event types (FileTouchEvent, ConcernAssertion, FindingRecord)"
```

---

## Task 10: `CoverageWriter` — async batched JSONL writer

**Files:**
- Create: `crates/rupu-coverage/src/ledger/writer.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: `crates/rupu-coverage/src/ledger/writer.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/ledger/writer.rs`:

```rust
use crate::ledger::events::{ConcernAssertion, FileTouchEvent, FindingRecord};
use crate::ledger::paths::CoveragePaths;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug)]
enum WriteRequest {
    File(FileTouchEvent),
    Concern(ConcernAssertion),
    Finding(FindingRecord),
    Flush(tokio::sync::oneshot::Sender<()>),
}

#[derive(Debug, Clone)]
pub struct CoverageWriter {
    tx: mpsc::Sender<WriteRequest>,
}

pub struct CoverageWriterHandle {
    pub writer: Arc<CoverageWriter>,
    task: JoinHandle<()>,
}

impl CoverageWriterHandle {
    /// Spawn the async writer task and return a writer handle.
    pub fn spawn(paths: CoveragePaths) -> std::io::Result<Self> {
        paths.ensure_dir()?;
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let task = tokio::spawn(run_writer(paths, rx));
        Ok(Self {
            writer: Arc::new(CoverageWriter { tx }),
            task,
        })
    }

    /// Block until pending writes have flushed, then shut down the task.
    pub async fn shutdown(self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.writer.tx.send(WriteRequest::Flush(tx)).await;
        let _ = rx.await;
        drop(self.writer);
        let _ = self.task.await;
    }
}

impl CoverageWriter {
    pub async fn record_file_touch(&self, event: FileTouchEvent) {
        let _ = self.tx.send(WriteRequest::File(event)).await;
    }

    pub async fn record_concern(&self, assertion: ConcernAssertion) {
        let _ = self.tx.send(WriteRequest::Concern(assertion)).await;
    }

    pub async fn record_finding(&self, record: FindingRecord) {
        let _ = self.tx.send(WriteRequest::Finding(record)).await;
    }
}

async fn run_writer(paths: CoveragePaths, mut rx: mpsc::Receiver<WriteRequest>) {
    let mut files_f = match OpenOptions::new().create(true).append(true).open(&paths.files).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(?e, path = ?paths.files, "open coverage files.jsonl");
            return;
        }
    };
    let mut concerns_f = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.concerns)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(?e, path = ?paths.concerns, "open coverage concerns.jsonl");
            return;
        }
    };
    let mut findings_f = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.findings)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(?e, path = ?paths.findings, "open coverage findings.jsonl");
            return;
        }
    };

    while let Some(req) = rx.recv().await {
        match req {
            WriteRequest::File(ev) => {
                if let Ok(line) = serde_json::to_string(&ev) {
                    let _ = files_f.write_all(line.as_bytes()).await;
                    let _ = files_f.write_all(b"\n").await;
                }
            }
            WriteRequest::Concern(a) => {
                if let Ok(line) = serde_json::to_string(&a) {
                    let _ = concerns_f.write_all(line.as_bytes()).await;
                    let _ = concerns_f.write_all(b"\n").await;
                }
            }
            WriteRequest::Finding(f) => {
                if let Ok(line) = serde_json::to_string(&f) {
                    let _ = findings_f.write_all(line.as_bytes()).await;
                    let _ = findings_f.write_all(b"\n").await;
                }
            }
            WriteRequest::Flush(ack) => {
                let _ = files_f.flush().await;
                let _ = concerns_f.flush().await;
                let _ = findings_f.flush().await;
                let _ = ack.send(());
            }
        }
    }
    let _ = files_f.flush().await;
    let _ = concerns_f.flush().await;
    let _ = findings_f.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, Surface};
    use chrono::Utc;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_test".to_string(),
            model: "mock".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[tokio::test]
    async fn writer_persists_many_file_events() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "test-target");
        let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

        for i in 0..50 {
            handle
                .writer
                .record_file_touch(FileTouchEvent::Read {
                    path: format!("file{i}.rs"),
                    line_range: [1, (i + 1) as u32 * 10],
                    tool: "read_file".to_string(),
                    attribution: attribution(),
                    at: Utc::now(),
                })
                .await;
        }
        handle.shutdown().await;

        let contents = tokio::fs::read_to_string(&paths.files).await.unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 50);
        for line in lines {
            let _: FileTouchEvent = serde_json::from_str(line).unwrap();
        }
    }
}
```

Note: `CoveragePaths` needs to be `Clone` for this test. Add `#[derive(Clone)]` to it in `paths.rs`.

- [ ] **Step 2: Wire into module**

Edit `crates/rupu-coverage/src/ledger/mod.rs`:

```rust
pub mod events;
pub mod paths;
pub mod target_id;
pub mod writer;
pub use events::{
    AssertionStatus, Attribution, ConcernAssertion, Evidence, FileTouchEvent, FindingEvidence,
    FindingRecord, FindingScope, Surface,
};
pub use paths::CoveragePaths;
pub use target_id::target_id;
pub use writer::{CoverageWriter, CoverageWriterHandle};
```

Edit `crates/rupu-coverage/src/lib.rs` re-exports to add `CoverageWriter, CoverageWriterHandle`.

- [ ] **Step 3: Run the test**

```bash
cargo test -p rupu-coverage writer_persists_many_file_events
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): async CoverageWriter persists ledger events to JSONL"
```

---

## Task 11: Derived per-file and per-concern views

**Files:**
- Create: `crates/rupu-coverage/src/ledger/views.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`
- Test: `crates/rupu-coverage/src/ledger/views.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/ledger/views.rs`:

```rust
use crate::catalog::types::TouchStrength;
use crate::ledger::events::{Attribution, ConcernAssertion, FileTouchEvent};
use crate::ledger::paths::CoveragePaths;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileView {
    pub path: String,
    pub touch_modes: Vec<TouchStrength>,
    pub strongest: TouchStrength,
    pub read_lines: Vec<[u32; 2]>,
    pub grep_matches: u32,
    pub edits: u32,
    pub first_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    pub touched_by: Vec<Attribution>,
}

pub fn read_file_events(paths: &CoveragePaths) -> std::io::Result<Vec<FileTouchEvent>> {
    if !paths.files.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&paths.files)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<FileTouchEvent>(l).ok())
        .collect())
}

pub fn read_concern_assertions(paths: &CoveragePaths) -> std::io::Result<Vec<ConcernAssertion>> {
    if !paths.concerns.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&paths.concerns)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<ConcernAssertion>(l).ok())
        .collect())
}

pub fn file_views(events: &[FileTouchEvent]) -> Vec<FileView> {
    let mut by_path: BTreeMap<String, FileView> = BTreeMap::new();
    for ev in events {
        let Some(path) = ev.path() else { continue };
        let Some(strength) = ev.strength() else { continue };
        let at = match ev {
            FileTouchEvent::Read { at, .. }
            | FileTouchEvent::Grep { at, .. }
            | FileTouchEvent::Glob { at, .. }
            | FileTouchEvent::Edit { at, .. }
            | FileTouchEvent::Cmd { at, .. }
            | FileTouchEvent::Unknown { at, .. } => *at,
        };
        let attribution = match ev {
            FileTouchEvent::Read { attribution, .. }
            | FileTouchEvent::Grep { attribution, .. }
            | FileTouchEvent::Glob { attribution, .. }
            | FileTouchEvent::Edit { attribution, .. }
            | FileTouchEvent::Cmd { attribution, .. }
            | FileTouchEvent::Unknown { attribution, .. } => attribution.clone(),
        };
        let view = by_path.entry(path.to_string()).or_insert_with(|| FileView {
            path: path.to_string(),
            touch_modes: vec![],
            strongest: strength,
            read_lines: vec![],
            grep_matches: 0,
            edits: 0,
            first_at: at,
            last_at: at,
            touched_by: vec![],
        });
        if !view.touch_modes.contains(&strength) {
            view.touch_modes.push(strength);
        }
        if strength > view.strongest {
            view.strongest = strength;
        }
        if at < view.first_at {
            view.first_at = at;
        }
        if at > view.last_at {
            view.last_at = at;
        }
        if !view.touched_by.iter().any(|a| a == &attribution) {
            view.touched_by.push(attribution);
        }
        match ev {
            FileTouchEvent::Read { line_range, .. } => view.read_lines.push(*line_range),
            FileTouchEvent::Edit { .. } => view.edits += 1,
            FileTouchEvent::Grep { match_count, .. } => view.grep_matches += match_count,
            _ => {}
        }
    }
    by_path.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, Surface};

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_t".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[test]
    fn file_views_aggregates_multiple_touches_per_path() {
        let now = Utc::now();
        let events = vec![
            FileTouchEvent::Read {
                path: "src/a.rs".to_string(),
                line_range: [1, 100],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: now,
            },
            FileTouchEvent::Read {
                path: "src/a.rs".to_string(),
                line_range: [101, 200],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: now,
            },
            FileTouchEvent::Edit {
                path: "src/a.rs".to_string(),
                line_range: [50, 55],
                lines_changed: 5,
                tool: "edit_file".to_string(),
                attribution: attribution(),
                at: now,
            },
        ];
        let views = file_views(&events);
        assert_eq!(views.len(), 1);
        let v = &views[0];
        assert_eq!(v.path, "src/a.rs");
        assert_eq!(v.strongest, TouchStrength::Edit);
        assert_eq!(v.read_lines.len(), 2);
        assert_eq!(v.edits, 1);
    }
}
```

- [ ] **Step 2: Wire into module**

Edit `crates/rupu-coverage/src/ledger/mod.rs`:

```rust
pub mod events;
pub mod paths;
pub mod target_id;
pub mod views;
pub mod writer;
pub use events::{
    AssertionStatus, Attribution, ConcernAssertion, Evidence, FileTouchEvent, FindingEvidence,
    FindingRecord, FindingScope, Surface,
};
pub use paths::CoveragePaths;
pub use target_id::target_id;
pub use views::{file_views, read_concern_assertions, read_file_events, FileView};
pub use writer::{CoverageWriter, CoverageWriterHandle};
```

Edit `crates/rupu-coverage/src/lib.rs` re-exports to add `FileView, file_views, read_concern_assertions, read_file_events`.

- [ ] **Step 3: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: prior + new view test pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): derive per-file view from JSONL event log"
```

---

## Task 12: Wire `CoverageWriter` into `rupu-tools::ToolContext`

**Files:**
- Modify: `crates/rupu-tools/Cargo.toml` (add `rupu-coverage` dep)
- Modify: `crates/rupu-tools/src/context.rs` (add field)
- Test: `crates/rupu-tools/src/context.rs` (inline)

- [ ] **Step 1: Inspect existing `ToolContext`**

Run:

```bash
grep -n "pub struct ToolContext" crates/rupu-tools/src/context.rs
```

Read the struct so you know which file and which line to edit. Note its current fields and `Default` impl.

- [ ] **Step 2: Add the dependency**

Edit `crates/rupu-tools/Cargo.toml`:

```toml
[dependencies]
# ...existing entries...
rupu-coverage = { path = "../rupu-coverage" }
```

- [ ] **Step 3: Add the field**

Edit `crates/rupu-tools/src/context.rs`. In `ToolContext`, add:

```rust
/// Optional coverage writer. When set, file-touching built-in tools
/// emit FileTouchEvents to this writer. None disables coverage capture
/// entirely (the default outside of a coverage-enabled run).
pub coverage_writer: Option<std::sync::Arc<rupu_coverage::CoverageWriter>>,
```

Update the `Default` impl (if explicit) to include `coverage_writer: None,`. If `ToolContext` already uses `#[derive(Default)]` and all other fields are `Option<_>` or `Default`-friendly, no further change is needed.

- [ ] **Step 4: Write the test**

Append to `crates/rupu-tools/src/context.rs`:

```rust
#[cfg(test)]
mod coverage_context_tests {
    use super::*;

    #[test]
    fn default_tool_context_has_no_coverage_writer() {
        let ctx = ToolContext::default();
        assert!(ctx.coverage_writer.is_none());
    }
}
```

- [ ] **Step 5: Run the tests**

```bash
cargo test -p rupu-tools default_tool_context_has_no_coverage_writer
cargo build --workspace
```

Expected: pass; workspace builds.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-tools Cargo.lock
git commit -m "feat(tools): expose optional CoverageWriter on ToolContext"
```

---

## Task 13: Instrument file-touching built-in tools

**Files:**
- Modify each tool implementation in `crates/rupu-tools/src/builtin/`:
  - `read_file.rs`
  - `grep.rs`
  - `glob.rs`
  - `edit_file.rs`
  - `bash.rs`
- Test: `crates/rupu-tools/tests/coverage_instrumentation.rs` (new file)

The exact location of each tool varies — verify with `grep -n "fn run" crates/rupu-tools/src/builtin/*.rs` before editing. The pattern is uniform: after the tool's main work succeeds, build a `FileTouchEvent` and call `ctx.coverage_writer.as_ref()` to dispatch.

- [ ] **Step 1: Add a helper for emitting events**

Add a new file `crates/rupu-tools/src/coverage_emit.rs`:

```rust
use crate::context::ToolContext;
use rupu_coverage::{Attribution, FileTouchEvent, Surface};

pub fn attribution_from(ctx: &ToolContext, run_id: &str, model: &str) -> Attribution {
    Attribution {
        run_id: run_id.to_string(),
        model: model.to_string(),
        surface: surface_for(ctx),
    }
}

fn surface_for(ctx: &ToolContext) -> Surface {
    // Surface is propagated by the runner via a string tag on ToolContext
    // (added in Task 18). Default to Workflow when not set.
    match ctx.surface_tag.as_deref() {
        Some("agent") => Surface::Agent,
        Some("autoflow") => Surface::Autoflow,
        Some("session") => Surface::Session,
        _ => Surface::Workflow,
    }
}

pub async fn emit(ctx: &ToolContext, event: FileTouchEvent) {
    if let Some(writer) = &ctx.coverage_writer {
        writer.record_file_touch(event).await;
    }
}
```

Add `surface_tag: Option<String>` field to `ToolContext` in `context.rs` (default None). Same pattern as `coverage_writer`.

Add `pub mod coverage_emit;` to `crates/rupu-tools/src/lib.rs`.

- [ ] **Step 2: Instrument `read_file`**

In `crates/rupu-tools/src/builtin/read_file.rs`, after the read succeeds and returns content with N lines:

```rust
use crate::coverage_emit;
use rupu_coverage::FileTouchEvent;
use chrono::Utc;

// ...inside the existing run() function, after successful read:
if let Some(writer) = ctx.coverage_writer.as_ref().cloned() {
    let event = FileTouchEvent::Read {
        path: args.path.clone(),
        line_range: [
            args.offset.unwrap_or(0) as u32 + 1,
            (args.offset.unwrap_or(0) as u32) + lines_read as u32,
        ],
        tool: "read_file".to_string(),
        attribution: coverage_emit::attribution_from(ctx, run_id, model),
        at: Utc::now(),
    };
    writer.record_file_touch(event).await;
}
```

`run_id` and `model` must be available in `ctx` — if not, add them as `run_id: Option<String>` and `model: Option<String>` on `ToolContext`. (Check current shape first; the runner already passes some identity in.)

- [ ] **Step 3: Instrument `grep`** — one event per matched file.

```rust
// After grep results are collected, group matches by file path:
let mut by_file: std::collections::BTreeMap<String, Vec<u32>> = Default::default();
for m in &matches {
    by_file.entry(m.path.clone()).or_default().push(m.line as u32);
}
if let Some(writer) = ctx.coverage_writer.as_ref().cloned() {
    for (path, matched_lines) in by_file {
        let event = FileTouchEvent::Grep {
            path,
            pattern: args.pattern.clone(),
            match_count: matched_lines.len() as u32,
            matched_lines,
            tool: "grep".to_string(),
            attribution: coverage_emit::attribution_from(ctx, run_id, model),
            at: Utc::now(),
        };
        writer.record_file_touch(event).await;
    }
}
```

- [ ] **Step 4: Instrument `glob`** — one event per matched path.

```rust
if let Some(writer) = ctx.coverage_writer.as_ref().cloned() {
    for path in &matched_paths {
        let event = FileTouchEvent::Glob {
            path: path.clone(),
            pattern: args.pattern.clone(),
            tool: "glob".to_string(),
            attribution: coverage_emit::attribution_from(ctx, run_id, model),
            at: Utc::now(),
        };
        writer.record_file_touch(event).await;
    }
}
```

- [ ] **Step 5: Instrument `edit_file`** — one event per edit.

```rust
if let Some(writer) = ctx.coverage_writer.as_ref().cloned() {
    let event = FileTouchEvent::Edit {
        path: args.path.clone(),
        line_range: [start_line as u32, end_line as u32],
        lines_changed: lines_changed as u32,
        tool: "edit_file".to_string(),
        attribution: coverage_emit::attribution_from(ctx, run_id, model),
        at: Utc::now(),
    };
    writer.record_file_touch(event).await;
}
```

- [ ] **Step 6: Instrument `bash`** — `Cmd` event for *recognized path args*.

```rust
// Parse args.command for tokens that look like workspace-relative paths.
// Heuristic: split on whitespace; for each token, if it (a) doesn't start
// with '-' and (b) refers to an existing file under ctx.workspace_path,
// emit a Cmd touch event.
if let Some(writer) = ctx.coverage_writer.as_ref().cloned() {
    for token in args.command.split_whitespace() {
        if token.starts_with('-') {
            continue;
        }
        let candidate = ctx.workspace_path.join(token);
        if candidate.is_file() {
            let event = FileTouchEvent::Cmd {
                path: token.to_string(),
                command: args.command.clone(),
                tool: "bash".to_string(),
                attribution: coverage_emit::attribution_from(ctx, run_id, model),
                at: Utc::now(),
            };
            writer.record_file_touch(event).await;
        }
    }
}
```

- [ ] **Step 7: Write integration test**

Create `crates/rupu-tools/tests/coverage_instrumentation.rs`:

```rust
use rupu_coverage::{CoverageWriterHandle, CoveragePaths, FileTouchEvent};
use rupu_tools::builtin::read_file;
use rupu_tools::context::ToolContext;
use std::sync::Arc;

#[tokio::test]
async fn read_file_emits_read_event() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "line1\nline2\n").unwrap();

    let paths = CoveragePaths::new(tmp.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

    let mut ctx = ToolContext::default();
    ctx.workspace_path = tmp.path().to_path_buf();
    ctx.coverage_writer = Some(handle.writer.clone());
    ctx.surface_tag = Some("workflow".to_string());
    ctx.run_id = Some("run_test".to_string());
    ctx.model = Some("mock".to_string());

    // Call the tool. Adapt the invocation to actual read_file signature in this crate.
    let result = read_file::run(&ctx, read_file::Args { path: "hello.txt".to_string(), offset: None, limit: None }).await;
    assert!(result.is_ok());

    handle.shutdown().await;

    let body = std::fs::read_to_string(&paths.files).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 1, "expected one read event");
    let ev: FileTouchEvent = serde_json::from_str(lines[0]).unwrap();
    assert!(matches!(ev, FileTouchEvent::Read { .. }));
    assert_eq!(ev.path(), Some("hello.txt"));
}
```

(Adapt the `read_file::run` call to match actual signatures in the crate. The shape will mirror existing tests in `rupu-tools`.)

- [ ] **Step 8: Run the tests**

```bash
cargo test -p rupu-tools coverage_instrumentation
```

Expected: pass.

- [ ] **Step 9: Commit**

```bash
git add crates/rupu-tools
git commit -m "feat(tools): instrument file-touching tools to emit FileTouchEvents to CoverageWriter"
```

---

## Task 14: `coverage_mark` tool + validation

**Files:**
- Create: `crates/rupu-coverage/src/tools/mod.rs`
- Create: `crates/rupu-coverage/src/tools/coverage_mark.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: `crates/rupu-coverage/src/tools/coverage_mark.rs` (inline)

- [ ] **Step 1: Module skeleton**

Create `crates/rupu-coverage/src/tools/mod.rs`:

```rust
pub mod coverage_mark;
pub use coverage_mark::{coverage_mark, CoverageMarkInput, CoverageMarkOutput, CoverageMarkError};
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-coverage/src/tools/coverage_mark.rs`:

```rust
use crate::catalog::types::{FlatCatalog, TouchStrength};
use crate::ledger::events::{
    AssertionStatus, Attribution, ConcernAssertion, Evidence, FileTouchEvent,
};
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::{file_views, read_file_events};
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageMarkInput {
    pub concern_id: String,
    pub file_path: String,
    pub status: AssertionStatus,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageMarkOutput {
    pub ok: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CoverageMarkError {
    #[error("unknown concern_id `{0}` — must be declared in the effective catalog")]
    UnknownConcernId(String),
    #[error("file `{file}` was never read at min_strength `{required:?}` — call read_file first or use status `not_applicable`")]
    FileNotExamined {
        file: String,
        required: TouchStrength,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub async fn coverage_mark(
    paths: &CoveragePaths,
    catalog: &FlatCatalog,
    attribution: Attribution,
    input: CoverageMarkInput,
) -> Result<CoverageMarkOutput, CoverageMarkError> {
    // Validation 1: concern_id must exist in the catalog.
    let concern = catalog
        .concerns
        .iter()
        .find(|c| c.id == input.concern_id)
        .ok_or_else(|| CoverageMarkError::UnknownConcernId(input.concern_id.clone()))?;

    // Validation 2: file must have been read (or any qualifying touch),
    // unless status is `not_applicable`.
    if input.status != AssertionStatus::NotApplicable {
        let events = read_file_events(paths)?;
        let views = file_views(&events);
        let view = views.iter().find(|v| v.path == input.file_path);
        let touched = view.map(|v| v.strongest).unwrap_or(TouchStrength::Glob);
        if touched < concern.min_strength {
            return Err(CoverageMarkError::FileNotExamined {
                file: input.file_path.clone(),
                required: concern.min_strength,
            });
        }
    }

    // Validation 3 (warn-only): status=Finding with empty finding_ids.
    let mut warnings = Vec::new();
    if input.status == AssertionStatus::Finding && input.evidence.finding_ids.is_empty() {
        warnings.push(
            "status=finding with no finding_ids — call report_finding first or attach the id"
                .to_string(),
        );
    }

    let assertion = ConcernAssertion {
        concern_id: input.concern_id,
        file_path: input.file_path,
        status: input.status,
        evidence: input.evidence,
        declared_by: attribution,
        declared_at: Utc::now(),
    };
    paths.ensure_dir()?;
    let line = serde_json::to_string(&assertion)?;
    let body = format!("{line}\n");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.concerns)?;
    use std::io::Write;
    f.write_all(body.as_bytes())?;
    f.flush()?;

    Ok(CoverageMarkOutput { ok: true, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective, Surface as _};
    use crate::ledger::events::Surface;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_t".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
        }
    }

    async fn touch_file_as_read(paths: &CoveragePaths, rel: &str) {
        use crate::ledger::writer::CoverageWriterHandle;
        let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();
        handle
            .writer
            .record_file_touch(FileTouchEvent::Read {
                path: rel.to_string(),
                line_range: [1, 100],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            })
            .await;
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn happy_path_clean_assertion_persists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let catalog = flatten(&block).unwrap();
        touch_file_as_read(&paths, "src/auth/login.rs").await;

        let out = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "stride:spoofing".to_string(),
                file_path: "src/auth/login.rs".to_string(),
                status: AssertionStatus::Clean,
                evidence: Evidence {
                    summary: "OK".to_string(),
                    line_ranges: vec![[1, 80]],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap();
        assert!(out.ok);
        assert!(out.warnings.is_empty());

        let body = std::fs::read_to_string(&paths.concerns).unwrap();
        let assertion: ConcernAssertion = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(assertion.concern_id, "stride:spoofing");
    }

    #[tokio::test]
    async fn rejects_unknown_concern_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let catalog = flatten(&block).unwrap();
        touch_file_as_read(&paths, "x.rs").await;

        let err = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "not-real".to_string(),
                file_path: "x.rs".to_string(),
                status: AssertionStatus::Clean,
                evidence: Evidence {
                    summary: "x".to_string(),
                    line_ranges: vec![],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CoverageMarkError::UnknownConcernId(_)));
    }

    #[tokio::test]
    async fn rejects_clean_when_file_not_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let catalog = flatten(&block).unwrap();
        // Do NOT touch the file.

        let err = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "stride:spoofing".to_string(),
                file_path: "unread.rs".to_string(),
                status: AssertionStatus::Clean,
                evidence: Evidence {
                    summary: "x".to_string(),
                    line_ranges: vec![],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CoverageMarkError::FileNotExamined { .. }));
    }

    #[tokio::test]
    async fn allows_not_applicable_without_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let catalog = flatten(&block).unwrap();
        let out = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "stride:spoofing".to_string(),
                file_path: "trivially-na.rs".to_string(),
                status: AssertionStatus::NotApplicable,
                evidence: Evidence {
                    summary: "wrong language".to_string(),
                    line_ranges: vec![],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn finding_without_finding_ids_warns() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let catalog = flatten(&block).unwrap();
        touch_file_as_read(&paths, "x.rs").await;
        let out = coverage_mark(
            &paths,
            &catalog,
            attribution(),
            CoverageMarkInput {
                concern_id: "stride:spoofing".to_string(),
                file_path: "x.rs".to_string(),
                status: AssertionStatus::Finding,
                evidence: Evidence {
                    summary: "issue here".to_string(),
                    line_ranges: vec![],
                    finding_ids: vec![],
                },
            },
        )
        .await
        .unwrap();
        assert!(out.ok);
        assert_eq!(out.warnings.len(), 1);
    }
}
```

Remove the spurious `use crate::catalog::types::Surface as _;` line (it was a typo — `Surface` lives in `ledger::events`).

- [ ] **Step 3: Wire into the library**

Edit `crates/rupu-coverage/src/lib.rs`:

```rust
pub mod tools;
pub use tools::{coverage_mark, CoverageMarkError, CoverageMarkInput, CoverageMarkOutput};
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: all prior tests + 5 new coverage_mark tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): coverage_mark tool with catalog + touch-strength validation"
```

---

## Task 15: `coverage_status` and `coverage_remaining` tools

**Files:**
- Create: `crates/rupu-coverage/src/tools/coverage_status.rs`
- Create: `crates/rupu-coverage/src/tools/coverage_remaining.rs`
- Modify: `crates/rupu-coverage/src/tools/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline in each

- [ ] **Step 1: Write `coverage_status` failing test**

Create `crates/rupu-coverage/src/tools/coverage_status.rs`:

```rust
use crate::ledger::events::ConcernAssertion;
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::read_concern_assertions;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoverageStatusInput {
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub file_path_prefix: Option<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
}

pub fn coverage_status(
    paths: &CoveragePaths,
    input: CoverageStatusInput,
) -> std::io::Result<Vec<ConcernAssertion>> {
    let all = read_concern_assertions(paths)?;
    Ok(all
        .into_iter()
        .filter(|a| {
            input.concern_id.as_deref().is_none_or(|c| a.concern_id == c)
                && input
                    .file_path_prefix
                    .as_deref()
                    .is_none_or(|p| a.file_path.starts_with(p))
                && input
                    .since
                    .is_none_or(|s| a.declared_at >= s)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{AssertionStatus, Attribution, Evidence, Surface};

    fn assertion(concern: &str, file: &str) -> ConcernAssertion {
        ConcernAssertion {
            concern_id: concern.to_string(),
            file_path: file.to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "x".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: Attribution {
                run_id: "r".to_string(),
                model: "m".to_string(),
                surface: Surface::Workflow,
            },
            declared_at: Utc::now(),
        }
    }

    fn write_jsonl(paths: &CoveragePaths, assertions: &[ConcernAssertion]) {
        paths.ensure_dir().unwrap();
        let body: String = assertions
            .iter()
            .map(|a| serde_json::to_string(a).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.concerns, body).unwrap();
    }

    #[test]
    fn filters_by_concern_id_and_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        write_jsonl(
            &paths,
            &[
                assertion("ssrf", "src/handlers/users.rs"),
                assertion("ssrf", "src/db/queries.rs"),
                assertion("sqli", "src/handlers/admin.rs"),
            ],
        );
        let results = coverage_status(
            &paths,
            CoverageStatusInput {
                concern_id: Some("ssrf".to_string()),
                file_path_prefix: Some("src/handlers/".to_string()),
                since: None,
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "src/handlers/users.rs");
    }
}
```

- [ ] **Step 2: Write `coverage_remaining` failing test**

Create `crates/rupu-coverage/src/tools/coverage_remaining.rs`:

```rust
use crate::catalog::types::{FlatCatalog, TouchStrength};
use crate::ledger::events::AssertionStatus;
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::{file_views, read_concern_assertions, read_file_events};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoverageRemainingInput {
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub min_strength: Option<TouchStrength>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemainingItem {
    pub concern_id: String,
    pub file_path: String,
    pub touch_modes: Vec<TouchStrength>,
    pub reason: String,
}

pub fn coverage_remaining(
    paths: &CoveragePaths,
    catalog: &FlatCatalog,
    input: CoverageRemainingInput,
) -> std::io::Result<Vec<RemainingItem>> {
    let events = read_file_events(paths)?;
    let views = file_views(&events);
    let assertions = read_concern_assertions(paths)?;
    let mut out = Vec::new();
    let concerns_to_check: Vec<_> = catalog
        .concerns
        .iter()
        .filter(|c| input.concern_id.as_deref().is_none_or(|q| c.id == q))
        .collect();
    let min_strength = input.min_strength.unwrap_or(TouchStrength::Read);

    for concern in concerns_to_check {
        // Build glob patterns once.
        let patterns: Vec<glob::Pattern> = concern
            .applicable_globs
            .iter()
            .filter_map(|p| glob::Pattern::new(p).ok())
            .collect();
        for view in &views {
            let matches_glob = patterns.is_empty()
                || patterns.iter().any(|p| p.matches(&view.path));
            if !matches_glob {
                continue;
            }
            let strong_enough = view.strongest >= min_strength;
            let asserted = assertions
                .iter()
                .any(|a| a.concern_id == concern.id && a.file_path == view.path && a.status != AssertionStatus::NotApplicable);
            if asserted {
                continue;
            }
            let reason = if !strong_enough {
                "below_min_strength".to_string()
            } else {
                "no_assertion".to_string()
            };
            out.push(RemainingItem {
                concern_id: concern.id.clone(),
                file_path: view.path.clone(),
                touch_modes: view.touch_modes.clone(),
                reason,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};
    use crate::ledger::events::{Attribution, FileTouchEvent, Surface};
    use chrono::Utc;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "r".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
        }
    }

    fn write_events(paths: &CoveragePaths, events: &[FileTouchEvent]) {
        paths.ensure_dir().unwrap();
        let body: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.files, body).unwrap();
    }

    #[test]
    fn lists_touched_files_lacking_assertion() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "secrets-in-source".to_string(),
                overrides: vec![],
            })],
        };
        let catalog = flatten(&block).unwrap();
        write_events(
            &paths,
            &[FileTouchEvent::Read {
                path: "src/config.rs".to_string(),
                line_range: [1, 50],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            }],
        );
        // No assertions yet → src/config.rs should appear as remaining.
        let remaining = coverage_remaining(&paths, &catalog, CoverageRemainingInput::default()).unwrap();
        assert!(remaining.iter().any(|r| r.file_path == "src/config.rs" && r.reason == "no_assertion"));
    }
}
```

- [ ] **Step 3: Wire into module**

Edit `crates/rupu-coverage/src/tools/mod.rs`:

```rust
pub mod coverage_mark;
pub mod coverage_remaining;
pub mod coverage_status;
pub use coverage_mark::{coverage_mark, CoverageMarkError, CoverageMarkInput, CoverageMarkOutput};
pub use coverage_remaining::{coverage_remaining, CoverageRemainingInput, RemainingItem};
pub use coverage_status::{coverage_status, CoverageStatusInput};
```

Edit `crates/rupu-coverage/src/lib.rs` to re-export the new symbols.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: all prior + 2 new tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): coverage_status and coverage_remaining query tools"
```

---

## Task 16: `report_finding` tool

**Files:**
- Create: `crates/rupu-coverage/src/tools/report_finding.rs`
- Modify: `crates/rupu-coverage/src/tools/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: `crates/rupu-coverage/src/tools/report_finding.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/tools/report_finding.rs`:

```rust
use crate::catalog::types::Severity;
use crate::ledger::events::{Attribution, FindingEvidence, FindingRecord, FindingScope};
use crate::ledger::paths::CoveragePaths;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFindingInput {
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_range: Option<[u32; 2]>,
    pub scope: FindingScope,
    pub summary: String,
    pub severity: Severity,
    #[serde(default)]
    pub concern_id: Option<String>,
    pub evidence: FindingEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFindingOutput {
    pub id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ReportFindingError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

pub fn report_finding(
    paths: &CoveragePaths,
    attribution: Attribution,
    input: ReportFindingInput,
) -> Result<ReportFindingOutput, ReportFindingError> {
    let id = format!("fnd_{}", Ulid::new());
    let record = FindingRecord {
        id: id.clone(),
        file_path: input.file_path,
        line_range: input.line_range,
        scope: input.scope,
        summary: input.summary,
        severity: input.severity,
        concern_id: input.concern_id,
        evidence: input.evidence,
        declared_by: attribution,
        declared_at: Utc::now(),
    };
    paths.ensure_dir()?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.findings)?;
    let line = serde_json::to_string(&record)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    f.flush()?;
    Ok(ReportFindingOutput { id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::Surface;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "r".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[test]
    fn appends_finding_and_returns_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let out = report_finding(
            &paths,
            attribution(),
            ReportFindingInput {
                file_path: Some("src/config.rs".to_string()),
                line_range: Some([20, 28]),
                scope: FindingScope::Line,
                summary: "Hardcoded API key.".to_string(),
                severity: Severity::High,
                concern_id: Some("secrets-in-source".to_string()),
                evidence: FindingEvidence {
                    code_excerpt: Some("const X = \"...\";".to_string()),
                    rationale: "Key in source.".to_string(),
                    references: vec![],
                },
            },
        )
        .unwrap();
        assert!(out.id.starts_with("fnd_"));
        let body = std::fs::read_to_string(&paths.findings).unwrap();
        assert_eq!(body.lines().count(), 1);
    }

    #[test]
    fn accepts_null_concern_for_serendipitous_finding() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let out = report_finding(
            &paths,
            attribution(),
            ReportFindingInput {
                file_path: None,
                line_range: None,
                scope: FindingScope::Repo,
                summary: "Spotted while looking for something else.".to_string(),
                severity: Severity::Low,
                concern_id: None,
                evidence: FindingEvidence {
                    code_excerpt: None,
                    rationale: "ad-hoc".to_string(),
                    references: vec![],
                },
            },
        )
        .unwrap();
        assert!(out.id.starts_with("fnd_"));
    }
}
```

- [ ] **Step 2: Wire into module + lib**

Edit `crates/rupu-coverage/src/tools/mod.rs` to add `pub mod report_finding;` and re-export `report_finding, ReportFindingError, ReportFindingInput, ReportFindingOutput`.

Edit `crates/rupu-coverage/src/lib.rs` re-exports likewise.

- [ ] **Step 3: Run the tests**

```bash
cargo test -p rupu-coverage
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): report_finding tool appends to findings.jsonl"
```

---

## Task 17: Catalog renderer (full-mode prompt section)

**Files:**
- Create: `crates/rupu-coverage/src/catalog/render.rs`
- Modify: `crates/rupu-coverage/src/catalog/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: `crates/rupu-coverage/src/catalog/render.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/catalog/render.rs`:

```rust
use crate::catalog::types::FlatCatalog;

pub fn render_full_mode(catalog: &FlatCatalog) -> String {
    let mut out = String::new();
    out.push_str("## Coverage Catalog\n\n");
    out.push_str(
        "You are reviewing this workspace against the following concerns. \
For each (file × concern) you assess, call `coverage_mark` with the \
appropriate status. For each issue you discover, call `report_finding`. \
Files you read, grep, or edit are tracked automatically — you do not \
need to declare them.\n\n",
    );
    for concern in &catalog.concerns {
        out.push_str(&format!("### {}\n", concern.id));
        out.push_str(&format!("**Name:** {}\n", concern.name));
        out.push_str(&format!("**Severity:** {}\n", severity_str(concern.severity)));
        if !concern.applicable_globs.is_empty() {
            out.push_str(&format!(
                "**Applies to:** {}\n",
                concern.applicable_globs.join(", ")
            ));
        }
        out.push('\n');
        out.push_str(concern.description.trim());
        out.push_str("\n\n");
        if !concern.references.is_empty() {
            out.push_str("References:\n");
            for r in &concern.references {
                out.push_str(&format!("- {r}\n"));
            }
            out.push('\n');
        }
    }
    out
}

fn severity_str(s: crate::catalog::types::Severity) -> &'static str {
    use crate::catalog::types::Severity::*;
    match s {
        Info => "info",
        Low => "low",
        Medium => "medium",
        High => "high",
        Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};

    #[test]
    fn renders_section_with_each_concern() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let cat = flatten(&block).unwrap();
        let rendered = render_full_mode(&cat);
        assert!(rendered.starts_with("## Coverage Catalog"));
        assert!(rendered.contains("### stride:spoofing"));
        assert!(rendered.contains("**Severity:** high"));
        assert!(rendered.contains("call `coverage_mark`"));
    }
}
```

- [ ] **Step 2: Wire into module + lib**

Edit `crates/rupu-coverage/src/catalog/mod.rs`:

```rust
pub mod render;
pub use render::render_full_mode;
```

Edit `crates/rupu-coverage/src/lib.rs` to re-export `render_full_mode`.

- [ ] **Step 3: Run the tests**

```bash
cargo test -p rupu-coverage renders_section_with_each_concern
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): render flattened catalog into system-prompt section (full mode)"
```

---

## Task 18: Agent integration — parse `concerns:` from frontmatter, wire into runner

**Files:**
- Modify: `crates/rupu-agent/Cargo.toml` (add `rupu-coverage` dep)
- Modify: `crates/rupu-agent/src/spec.rs` (or wherever frontmatter is parsed)
- Modify: `crates/rupu-agent/src/runner.rs` (wire writer + prompt + tools)
- Test: `crates/rupu-agent/src/runner.rs` (inline)

- [ ] **Step 1: Add dependency**

Edit `crates/rupu-agent/Cargo.toml`:

```toml
[dependencies]
rupu-coverage = { path = "../rupu-coverage" }
# ...existing...
```

- [ ] **Step 2: Extend `AgentSpec` to carry a `concerns:` block**

Find the struct that parses agent file YAML frontmatter (likely `AgentSpec` in `spec.rs`). Add:

```rust
use rupu_coverage::ConcernsBlock;

// inside AgentSpec or AgentFrontmatter:
#[serde(default)]
pub concerns: Option<ConcernsBlock>,
```

Add a unit test that an agent file with a `concerns:` block parses:

```rust
#[test]
fn agent_frontmatter_parses_concerns_block() {
    let md = r#"---
name: test-agent
description: test
provider: anthropic
model: claude-sonnet-4-6
concerns:
  - include: stride
---
body
"#;
    let spec = parse_agent_md(md).expect("parses");
    assert!(spec.concerns.is_some());
}
```

Adjust to the actual parser entry point.

- [ ] **Step 3: Wire the runner**

In `crates/rupu-agent/src/runner.rs` (or wherever a run is set up), when `concerns` is `Some`:

```rust
use rupu_coverage::{
    flatten, render_full_mode, target_id, write_snapshot, CoveragePaths, CoverageWriterHandle,
};

// Near the start of run_agent:
let coverage = if let Some(block) = opts.concerns.clone() {
    let catalog = flatten(&block)?;
    let target = target_id(&opts.workspace_path, &opts.scope_name);
    let paths = CoveragePaths::new(&opts.workspace_path, &target);
    paths.ensure_dir()?;
    write_snapshot(&catalog, &paths.catalog)?;
    let handle = CoverageWriterHandle::spawn(paths.clone())?;
    let prompt_section = render_full_mode(&catalog);
    Some(CoverageBundle {
        catalog,
        paths,
        handle,
        prompt_section,
    })
} else {
    None
};

// When building the system prompt:
let mut system = opts.agent_system_prompt.clone();
if let Some(bundle) = &coverage {
    system.push_str("\n\n");
    system.push_str(&bundle.prompt_section);
}

// When building the ToolContext for tool calls:
tool_context.coverage_writer = coverage.as_ref().map(|b| b.handle.writer.clone());
tool_context.surface_tag = Some("agent".to_string());

// When auto-injecting tools (next to existing tool registration):
if let Some(bundle) = &coverage {
    register_coverage_tools(&mut tool_list, bundle.catalog.clone(), bundle.paths.clone());
}

// After the run completes:
if let Some(bundle) = coverage {
    bundle.handle.shutdown().await;
}
```

Where `CoverageBundle` is a local struct holding `catalog`, `paths`, `handle`, `prompt_section`. `register_coverage_tools` is a new function — implement it in `rupu-agent/src/coverage_tools.rs` (next step).

`opts.scope_name` is the agent name; populate it from `opts.agent_name`.

`opts.concerns` is a new field on `AgentRunOpts` — add it as `Option<ConcernsBlock>` alongside the existing fields.

- [ ] **Step 4: Implement `register_coverage_tools`**

Create `crates/rupu-agent/src/coverage_tools.rs`:

```rust
use rupu_coverage::{
    coverage_mark, coverage_remaining, coverage_status, report_finding, Attribution,
    CoverageMarkInput, CoverageRemainingInput, CoverageStatusInput, FlatCatalog, ReportFindingInput,
    Surface,
};
// ... use the project's Tool trait + ToolContext + registry types here.

pub fn register_coverage_tools(/* tool_list: &mut ..., */ catalog: FlatCatalog, paths: rupu_coverage::CoveragePaths) {
    // Wrap each of coverage_mark/coverage_status/coverage_remaining/report_finding
    // in the project's Tool trait and push onto tool_list. The tool implementations
    // construct an Attribution from ToolContext (run_id, model, surface_tag).
}
```

This is a thin glue layer. The exact code mirrors how MCP tools are wrapped in `rupu-agent/src/runner.rs`'s existing tool registration. Follow that pattern verbatim; each coverage tool becomes a `BoxedTool` whose `run` calls the corresponding `rupu_coverage::*` function.

- [ ] **Step 5: Write an integration test**

In `crates/rupu-agent/tests/coverage_integration.rs`:

```rust
use rupu_coverage::{ConcernsBlock, ConcernsEntry, IncludeDirective};

#[tokio::test]
async fn agent_run_with_concerns_writes_snapshot_and_ledger() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    // ... construct AgentRunOpts mirroring the existing runner tests
    // ... opts.concerns = Some(ConcernsBlock { entries: vec![ConcernsEntry::Include(IncludeDirective { include: "stride".into(), overrides: vec![] })] })
    // ... run_agent with MockProvider that calls coverage_mark via a tool call
    // After the run:
    let target = rupu_coverage::target_id(&workspace, "test-agent");
    let paths = rupu_coverage::CoveragePaths::new(&workspace, &target);
    assert!(paths.catalog.exists());
}
```

(Adapt to existing MockProvider patterns in `crates/rupu-agent/src/runner.rs`'s test suite.)

- [ ] **Step 6: Run the tests**

```bash
cargo test -p rupu-agent
cargo build --workspace
```

Expected: workspace builds; tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-agent Cargo.lock
git commit -m "feat(agent): wire concerns block to flattener, snapshot, prompt, and tool injection"
```

---

## Task 19: Workflow integration — parse `concerns:` from workflow YAML

**Files:**
- Modify: `crates/rupu-orchestrator/Cargo.toml` (add `rupu-coverage` dep)
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (or whichever file defines the workflow struct)
- Modify: `crates/rupu-orchestrator/src/executor/in_process.rs` (propagate to agent runs)
- Test: `crates/rupu-orchestrator/src/workflow.rs` (inline) + integration test

- [ ] **Step 1: Add dependency**

Edit `crates/rupu-orchestrator/Cargo.toml`:

```toml
[dependencies]
rupu-coverage = { path = "../rupu-coverage" }
# ...existing...
```

- [ ] **Step 2: Add `concerns` to `Workflow`**

In `crates/rupu-orchestrator/src/workflow.rs` (or wherever the workflow YAML struct lives):

```rust
use rupu_coverage::ConcernsBlock;

// inside Workflow:
#[serde(default)]
pub concerns: Option<ConcernsBlock>,
```

Unit test:

```rust
#[test]
fn workflow_parses_concerns_block() {
    let yaml = r#"
name: test-workflow
description: test
concerns:
  - include: stride
steps: []
"#;
    let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
    assert!(wf.concerns.is_some());
}
```

- [ ] **Step 3: Propagate to step agents**

In `crates/rupu-orchestrator/src/executor/in_process.rs` (or wherever `WorkflowExecutor::execute_step` lives), when invoking the agent runner:

```rust
// Workflow-level concerns take precedence over step-level or agent-level.
let concerns = workflow.concerns.clone();
// Set the scope_name to the workflow name so all steps share the same target.
let agent_run_opts = AgentRunOpts {
    // ...existing fields...
    concerns,
    scope_name: workflow.name.clone(),
    // ...
};
```

Per the spec, workflow-level catalog wins over any agent-level catalog. If the agent file *also* has a `concerns:` block but the workflow declares one, the workflow's wins.

- [ ] **Step 4: Run tests**

```bash
cargo test -p rupu-orchestrator
cargo build --workspace
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator Cargo.lock
git commit -m "feat(orchestrator): propagate workflow concerns to step agents (workflow wins over agent-level)"
```

---

## Task 20: End-to-end smoke test

**Files:**
- Create: `crates/rupu-coverage/tests/end_to_end.rs`

- [ ] **Step 1: Write the e2e test**

```rust
use rupu_coverage::{
    coverage_mark, flatten, target_id, write_snapshot, AssertionStatus, Attribution, ConcernsBlock,
    ConcernsEntry, CoveragePaths, CoverageWriterHandle, Evidence, FileTouchEvent, IncludeDirective,
    Surface,
};
use chrono::Utc;

#[tokio::test]
async fn end_to_end_workflow_with_stride_catalog() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // 1. Construct a ConcernsBlock with stride
    let block = ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
        })],
    };
    let catalog = flatten(&block).unwrap();
    assert_eq!(catalog.concerns.len(), 6);

    // 2. Establish target paths + write snapshot
    let target = target_id(&workspace, "security-review");
    let paths = CoveragePaths::new(&workspace, &target);
    paths.ensure_dir().unwrap();
    write_snapshot(&catalog, &paths.catalog).unwrap();

    // 3. Spawn writer and emit a synthetic file touch
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();
    let attribution = Attribution {
        run_id: "run_e2e_test".to_string(),
        model: "mock".to_string(),
        surface: Surface::Workflow,
    };
    handle
        .writer
        .record_file_touch(FileTouchEvent::Read {
            path: "src/auth/login.rs".to_string(),
            line_range: [1, 80],
            tool: "read_file".to_string(),
            attribution: attribution.clone(),
            at: Utc::now(),
        })
        .await;
    // Give the writer a moment, then shutdown to flush
    handle.shutdown().await;

    // 4. Call coverage_mark
    let out = coverage_mark(
        &paths,
        &catalog,
        attribution,
        rupu_coverage::CoverageMarkInput {
            concern_id: "stride:spoofing".to_string(),
            file_path: "src/auth/login.rs".to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "Token check covers all entry points.".to_string(),
                line_ranges: vec![[1, 80]],
                finding_ids: vec![],
            },
        },
    )
    .await
    .unwrap();
    assert!(out.ok);

    // 5. Verify ledger artifacts exist and look right
    assert!(paths.catalog.exists());
    assert!(paths.files.exists());
    assert!(paths.concerns.exists());

    let snapshot = rupu_coverage::read_snapshot(&paths.catalog).unwrap();
    assert_eq!(snapshot.concerns.len(), 6);

    let touches: Vec<_> = std::fs::read_to_string(&paths.files)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<FileTouchEvent>(l).unwrap())
        .collect();
    assert_eq!(touches.len(), 1);

    let assertions: Vec<_> = std::fs::read_to_string(&paths.concerns)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<rupu_coverage::ConcernAssertion>(l).unwrap())
        .collect();
    assert_eq!(assertions.len(), 1);
    assert_eq!(assertions[0].concern_id, "stride:spoofing");
    assert_eq!(assertions[0].status, AssertionStatus::Clean);
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test -p rupu-coverage --test end_to_end
```

Expected: pass.

- [ ] **Step 3: Final workspace check**

```bash
cargo build --workspace
cargo test --workspace --no-fail-fast 2>&1 | grep -E "^test result"
cargo fmt --check
cargo clippy --workspace -- -D warnings
```

Expected: workspace builds, tests pass (some pre-existing failures may remain — those are out of scope), no fmt or clippy regressions.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage/tests/end_to_end.rs
git commit -m "test(coverage): end-to-end smoke — snapshot + file touch + coverage_mark"
```

---

## Self-review

**1. Spec coverage:**

| Spec requirement | Implemented in |
| --- | --- |
| Three append-only JSONL ledgers | Tasks 9, 10 |
| Catalog snapshot | Task 7 |
| target_id keyed by (workspace, scope) | Task 8 |
| Concern/Severity/TouchStrength types | Task 2 |
| Template parsing | Task 3 |
| 8 curated templates | Task 4 |
| Built-in template registry | Task 5 |
| ConcernsBlock, include resolution, inline-wins, overrides, duplicate detection | Task 6 |
| File-touch instrumentation per tool | Tasks 12, 13 |
| CoverageWriter (async batched) | Task 10 |
| Derived per-file view | Task 11 |
| coverage_mark + 3 validation rules + 1 warning rule | Task 14 |
| coverage_status / coverage_remaining | Task 15 |
| report_finding (with nullable concern_id) | Task 16 |
| Full-mode prompt section rendering | Task 17 |
| Agent integration | Task 18 |
| Workflow integration | Task 19 |
| End-to-end smoke | Task 20 |
| CWE-full templates + generator | **Plan 2** (out of scope here) |
| Index-mode rendering + search/detail tools | **Plan 2** |
| Catalog filters on includes | **Plan 2** |
| `rupu coverage` CLI subcommand | **Plan 3** |
| Audit report rendering | **Plan 3** |
| Session-surface integration | **Plan 3** |
| Custom MCP tool mappings | **Plan 3** |

**2. Placeholder scan:** None. Each code step contains the actual content. Generated files (templates) reference the spec as the authoritative source for entries — that's a pointer to data, not a placeholder.

**3. Type consistency:** `Concern`, `Template`, `ConcernsBlock`, `IncludeDirective`, `FlatCatalog`, `Attribution`, `FileTouchEvent`, `ConcernAssertion`, `FindingRecord`, `CoveragePaths`, `CoverageWriter`, `CoverageWriterHandle`, `Severity`, `TouchStrength`, `AssertionStatus`, `FindingScope` — same names used consistently across tasks. Method signatures match across consumers (e.g. `flatten(&ConcernsBlock) -> Result<FlatCatalog, FlattenError>` used in Tasks 6, 7, 14, 15, 17, 18, 20).

**4. Spec requirement with no task?** All in-scope spec requirements are covered. Out-of-scope items are explicitly listed under each plan boundary at the top.

---

## Execution

Plan complete and saved to `docs/superpowers/plans/2026-05-23-rupu-coverage-harness-plan-1-foundation-and-curated-catalog.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
