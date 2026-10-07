# Finding Tags — Plan 2 (query language everywhere + Findings query bar) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Filter findings everywhere with one single-line query language, e.g. `severity>=high tag:class:sqli -tag:false-positive`. The same query works from:
- the CP API (`?q=`)
- the CLI (`rupu findings list|tags [QUERY…]`)
- the agent tool and MCP (`{"q": …}`)
- a Ghost-style spotlight query bar on the CP Findings page

**Architecture:**
- **Rust is the authority.** `rupu_coverage::ledger::query_lang` tokenizes, parses and validates the query against a field registry. `ledger::finding_filter` evaluates it against a finding plus its provenance. `ledger::query` pages for tools.
- **The web parses, it doesn't evaluate.** It has a TypeScript twin of the parser (`web/src/lib/findingQuery/`) that only drives chips, suggestions and inline errors; the server always evaluates.
- **Lockstep.** The two parsers are held in lockstep by shared JSON fixtures that both `cargo test` and vitest run.
- **One generic component.** `QueryBar`, ported from Ghost's `SpotlightBar`, is driven by a per-view field registry plus server-provided `facets`. Plan 2 wires it only into the global Findings page.

**Tech Stack:** Rust (serde, thiserror, axum) · React 18 + TypeScript + Tailwind · lucide-react · vitest + Testing Library.

**Spec:** `docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md`. Read the "Query language (Plans 2–3)" section first. Plan 1, which this builds on, is merged (#766).

## Global Constraints

- **Grammar** (spec, verbatim intent):
  - **Tokens and AND.** Tokens are separated by whitespace, and every token must match (AND).
  - **Values.** `key:value` filters. `key:a,b` matches any of the values (OR within one token). `-key:value` negates.
  - **Quoting.** Quotes (`"` or `'`) open only at the start of an item: the token start (after an optional `-`), right after an operator character, or right after a `,`. `\` escapes the next character.
  - **Severity comparisons.** Only `severity` takes `>=`, `>`, `<=`, `<`.
  - **Free text.** A bare word or quoted phrase is free text, matched case-insensitively against title, summary, id and file path. `-word` negates it.
  - **Keys.** A token shaped like `letters` + an operator names a key, so an unknown key is an error, never text.
- **Error codes** (snake_case, shared by Rust, TS and fixtures): `unknown_key`, `empty_value`, `bad_value`, `bad_operator`, `unclosed_quote`, `bad_quote`. An error carries the token index (0-based) and char offsets in Unicode scalar values (`chars()` in Rust, `Array.from` in TS).
- **Fields, in this order:**

  | Key | Kind | Values |
  |---|---|---|
  | `severity` (alias `sev`) | severity | info, low, medium, high, critical |
  | `tag` | tag | normalized like `Tag` |
  | `has` | enum | tags, report, poc, cwe |
  | `project` | text | |
  | `cwe` | cwe | normalized `CWE-<n>` |
  | `owner` | text | |
  | `product` | text | |
  | `verified` | enum | unverified, confirmed, disputed, inconclusive |
  | `profile` | enum | full, summary |
  | `scope` | enum | line, file, repo, host, endpoint, resource |
  | `concern` | text | |
  | `agent` | text | |
  | `workflow` | text | |
  | `file` | text | |
  | `run` | text | |
  | `id` | text | |

- **Provenance-only keys.** `project` and `workflow` need provenance, so they are available only in the CLI and CP. Agent tools and MCP refuse them with an "unavailable here" error.
- **`run`.** The CLI and CP expand `run` to the run plus its sub-runs (`resolve_run_scope`). Everywhere else `run` is an exact match.
- **Repeated keys AND.** `tag:a tag:b` means both tags. Use a comma for OR.
- **Rust is the only evaluator.** The web never filters findings itself; it sends `q` to `GET /api/findings`.
- **Plan 1's structured filters are removed, not kept alongside.** That covers `tags`, `tag_mode`, `untagged`, `min_severity`, `concern_id`, `file_prefix`, and the CLI's `--tag`, `--any-tag`, `--untagged`, `--severity`, `--project` and `--run`. They never shipped in a beta. Pagination stays: `limit`, `cursor`, `--limit`, `--ids-only`.
- **The UI matches Ghost's spotlight.**
  - Chips sit inside the input row.
  - A fuzzy spotlight dropdown carries lucide icons, per-value colors and counts.
  - ↑/↓ move, Enter/Tab accept, Escape clears, and Backspace on an empty draft pops the last chip back into the draft.
  - `/` focuses the bar. **⌘K stays the CP's global command palette:** do not bind it.
  - Styling uses rupu tokens: `panel`, `surface`, `surface-hover`, `border`, `ink`, `ink-dim`, `ink-mute`, `brand-*`, `text-meta`/`text-note`/`text-ui`.
- **Rust repo rules:**
  - Workspace deps only; this plan adds no dependencies in any crate or `package.json`.
  - One integration-test binary per crate (`tests/it/` modules).
  - `#![deny(clippy::all)]`.
  - Per-file `rustfmt` only, never `cargo fmt`. Revert reflows of lines you didn't write.
- **Running tests.** Run targeted tests only (`cargo test -p <crate> --lib <module>::`, `--test it <module>::`, `npx vitest run <file>`). Never run a cold full-workspace `cargo test`.
- **Git.** No `git stash`, no checkout, no push. End every commit with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Fixtures** are invented, never assessment data.

## File map

| File | Responsibility |
|---|---|
| `crates/rupu-coverage/src/report/cwe.rs` (new) | `parse_cwe`, `concern_cwe`, `finding_cwes` (moved from rupu-findings-report) |
| `crates/rupu-coverage/src/ledger/query_lang.rs` (new) | field registry, scanner, parser, `ParseError` |
| `crates/rupu-coverage/src/ledger/finding_filter.rs` (new) | `FindingView`, `RunScopes`, `matches`, `select`, `facets`, `check_available` |
| `crates/rupu-coverage/src/ledger/query.rs` | `FindingQuery {q, limit, cursor}`, paged `query`, `query_response`, schema (rewritten) |
| `crates/rupu-coverage/tests/fixtures/finding_query/{cases,fields}.json` (new) | the lockstep contract |
| `crates/rupu-coverage/tests/it/finding_query_lockstep.rs` (new) | Rust side of the lockstep |
| `crates/rupu-findings-report/src/select.rs` | re-export the moved CWE helpers |
| `crates/rupu-agent/src/coverage_tools.rs`, `crates/rupu-mcp/src/tools/findings.rs` | tool descriptions and tests for `q` |
| `crates/rupu-cli/src/cmd/findings.rs` | `list` / `tags` take `[QUERY…]` |
| `crates/rupu-cp/src/api/findings.rs` | `q`, `facets`, `tags_unavailable`, structured 400 |
| `crates/rupu-cp/web/src/lib/findingQuery/{grammar,fields}.ts` (new) | the TS parser twin and the findings field registry (icons) |
| `crates/rupu-cp/web/src/components/query/{suggest.ts,QueryBar.tsx}` (new) | generic suggestion engine and spotlight bar |
| `crates/rupu-cp/web/src/pages/Findings.tsx`, `components/findings/FindingsTable.tsx`, `lib/api.ts` | integration, Tags column, types |

---

### Task 1: Move the CWE helpers into rupu-coverage

**Files:**
- Create: `crates/rupu-coverage/src/report/cwe.rs`
- Modify: `crates/rupu-coverage/src/report/mod.rs` (add `pub mod cwe;`)
- Modify: `crates/rupu-findings-report/src/select.rs` (delete `parse_cwe` and `concern_cwe` bodies; re-export)

**Interfaces:**
- Produces:
  - `rupu_coverage::report::cwe::{parse_cwe(&str) -> Option<u32>, concern_cwe(&str) -> Option<u32>, finding_cwes(&FindingRecord) -> Vec<u32>}`
  - `rupu_findings_report::select::{parse_cwe, concern_cwe}` keep working as re-exports, so every existing caller compiles unchanged.

- [ ] **Step 1: Write the failing test.** Create `cwe.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, FindingEvidence, FindingRecord, FindingScope, Surface};

    #[test]
    fn parse_cwe_reads_every_spelling() {
        for s in ["CWE-79", "cwe-79", "cwe_79", "cwe79", " 79 ", "079"] {
            assert_eq!(parse_cwe(s), Some(79), "{s}");
        }
        for s in ["", "CWE-", "xss", "79a", "99999999999"] {
            assert_eq!(parse_cwe(s), None, "{s}");
        }
    }

    #[test]
    fn concern_cwe_reads_the_whole_digit_run() {
        assert_eq!(concern_cwe("cwe-top25-2023:cwe-798-hardcoded-credentials"), Some(798));
        assert_eq!(concern_cwe("cwe-79-xss"), Some(79));
        assert_eq!(concern_cwe("authz-idor"), None);
    }

    #[test]
    fn finding_cwes_unions_report_and_concern() {
        let mut r = FindingRecord {
            id: "fnd_x".into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: "s".into(),
            severity: crate::catalog::types::Severity::Low,
            concern_id: Some("cwe-89-sqli".into()),
            evidence: FindingEvidence { code_excerpt: None, rationale: "r".into(), references: vec![] },
            declared_by: Attribution {
                run_id: "r".into(),
                model: "m".into(),
                surface: Surface::Workflow,
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: chrono::Utc::now(),
            profile: crate::report::FindingProfile::Summary,
            report: None,
            tags: vec![],
        };
        assert_eq!(finding_cwes(&r), vec![89]);
        r.concern_id = None;
        assert!(finding_cwes(&r).is_empty());
    }
}
```

- [ ] **Step 2: Run the test and confirm it fails.**
  - Run: `cargo test -p rupu-coverage --lib report::cwe`
  - Expected: a compile error (`parse_cwe` is not defined).

- [ ] **Step 3: Implement.**
  - Move the exact bodies (and their doc comments) of `parse_cwe` and `concern_cwe` from `crates/rupu-findings-report/src/select.rs` into `cwe.rs`, above the tests, as `pub fn`.
  - Add `finding_cwes`:

```rust
/// Every CWE number a finding names: each `report.cwe` entry [`parse_cwe`]
/// can read, then the CWE its `concern_id` names. Deduplicated, in that order.
pub fn finding_cwes(r: &crate::ledger::events::FindingRecord) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let from_report = r
        .report
        .iter()
        .flat_map(|rep| rep.cwe.iter())
        .filter_map(|c| parse_cwe(c));
    for n in from_report.chain(r.concern_id.as_deref().and_then(concern_cwe)) {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}
```

  - Add `pub mod cwe;` to `crates/rupu-coverage/src/report/mod.rs`.
  - In `select.rs`, replace the two moved functions with `pub use rupu_coverage::report::cwe::{concern_cwe, parse_cwe};`.
  - If `select.rs`'s own `select` builds the finding's CWE set by hand from `report.cwe` + `concern_cwe`, switch it to `rupu_coverage::report::cwe::finding_cwes(r)` only when the behavior is identical. Otherwise leave it.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run `cargo test -p rupu-coverage --lib report::cwe`.
  - Run `cargo test -p rupu-findings-report --test it`.
  - Run `cargo test -p rupu-cli --test it findings_export::`.
  - Expected: all pass.

- [ ] **Step 5: Format, lint, commit.**
  - Run rustfmt on the touched files.
  - Run `cargo clippy -p rupu-coverage -p rupu-findings-report --all-targets -- -D warnings`.

```bash
git add crates/rupu-coverage/src/report crates/rupu-findings-report/src/select.rs
git commit -m "refactor(coverage): CWE helpers live in rupu_coverage::report::cwe

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The Rust query language — registry, scanner, parser, lockstep fixtures

**Files:**
- Create: `crates/rupu-coverage/src/ledger/query_lang.rs`
- Create: `crates/rupu-coverage/tests/fixtures/finding_query/cases.json`, `crates/rupu-coverage/tests/fixtures/finding_query/fields.json`
- Create: `crates/rupu-coverage/tests/it/finding_query_lockstep.rs`; modify `tests/it/main.rs` (`mod finding_query_lockstep;`)
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`, `crates/rupu-coverage/src/lib.rs` (exports)

**Interfaces:**
- Consumes: `Tag::parse` (`ledger::tags`), and `parse_cwe` (Task 1).
- Produces (all re-exported from `rupu_coverage`):
  - `Key`, `Op`, `Term { negated, key, op, values }`, `ParsedQuery { terms }`
  - `ErrorCode`, `ParseError { token, start, end, code, message }`
  - `FieldKind`, `FieldDef { key, aliases, kind, values, id }`, `FIELDS`
  - `field(&str) -> Option<&'static FieldDef>`
  - `parse_query(&str) -> Result<ParsedQuery, ParseError>`
- `Term` serializes canonically as `{"neg":bool,"key":"severity","op":"eq|gt|ge|lt|le","values":[…]}`, and `ParsedQuery` as `{"terms":[…]}`.

- [ ] **Step 1: Write the fixtures.**

`crates/rupu-coverage/tests/fixtures/finding_query/fields.json`:

```json
[
  {"key": "severity", "aliases": ["sev"], "kind": "severity", "values": ["info", "low", "medium", "high", "critical"]},
  {"key": "tag", "aliases": [], "kind": "tag", "values": []},
  {"key": "has", "aliases": [], "kind": "enum", "values": ["tags", "report", "poc", "cwe"]},
  {"key": "project", "aliases": [], "kind": "text", "values": []},
  {"key": "cwe", "aliases": [], "kind": "cwe", "values": []},
  {"key": "owner", "aliases": [], "kind": "text", "values": []},
  {"key": "product", "aliases": [], "kind": "text", "values": []},
  {"key": "verified", "aliases": [], "kind": "enum", "values": ["unverified", "confirmed", "disputed", "inconclusive"]},
  {"key": "profile", "aliases": [], "kind": "enum", "values": ["full", "summary"]},
  {"key": "scope", "aliases": [], "kind": "enum", "values": ["line", "file", "repo", "host", "endpoint", "resource"]},
  {"key": "concern", "aliases": [], "kind": "text", "values": []},
  {"key": "agent", "aliases": [], "kind": "text", "values": []},
  {"key": "workflow", "aliases": [], "kind": "text", "values": []},
  {"key": "file", "aliases": [], "kind": "text", "values": []},
  {"key": "run", "aliases": [], "kind": "text", "values": []},
  {"key": "id", "aliases": [], "kind": "text", "values": []}
]
```

`crates/rupu-coverage/tests/fixtures/finding_query/cases.json`. Each case has `q` and exactly one of `terms` (success) or `error` (`{token, code}`):

```json
[
  {"q": "", "terms": []},
  {"q": "   ", "terms": []},
  {"q": "severity:high", "terms": [{"neg": false, "key": "severity", "op": "eq", "values": ["high"]}]},
  {"q": "sev:HIGH", "terms": [{"neg": false, "key": "severity", "op": "eq", "values": ["high"]}]},
  {"q": "SEVERITY:low", "terms": [{"neg": false, "key": "severity", "op": "eq", "values": ["low"]}]},
  {"q": "severity>=high", "terms": [{"neg": false, "key": "severity", "op": "ge", "values": ["high"]}]},
  {"q": "severity>medium severity<=critical", "terms": [{"neg": false, "key": "severity", "op": "gt", "values": ["medium"]}, {"neg": false, "key": "severity", "op": "le", "values": ["critical"]}]},
  {"q": "severity<medium", "terms": [{"neg": false, "key": "severity", "op": "lt", "values": ["medium"]}]},
  {"q": "severity:high,critical", "terms": [{"neg": false, "key": "severity", "op": "eq", "values": ["high", "critical"]}]},
  {"q": "tag:class:sqli tag:needs-poc", "terms": [{"neg": false, "key": "tag", "op": "eq", "values": ["class:sqli"]}, {"neg": false, "key": "tag", "op": "eq", "values": ["needs-poc"]}]},
  {"q": "tag:a,b", "terms": [{"neg": false, "key": "tag", "op": "eq", "values": ["a", "b"]}]},
  {"q": "tag:Needs-POC", "terms": [{"neg": false, "key": "tag", "op": "eq", "values": ["needs-poc"]}]},
  {"q": "tag:\"class:sqli\"", "terms": [{"neg": false, "key": "tag", "op": "eq", "values": ["class:sqli"]}]},
  {"q": "-tag:false-positive", "terms": [{"neg": true, "key": "tag", "op": "eq", "values": ["false-positive"]}]},
  {"q": "-has:tags", "terms": [{"neg": true, "key": "has", "op": "eq", "values": ["tags"]}]},
  {"q": "has:poc,report", "terms": [{"neg": false, "key": "has", "op": "eq", "values": ["poc", "report"]}]},
  {"q": "cwe:79 cwe:CWE-89 cwe:cwe_022", "terms": [{"neg": false, "key": "cwe", "op": "eq", "values": ["CWE-79"]}, {"neg": false, "key": "cwe", "op": "eq", "values": ["CWE-89"]}, {"neg": false, "key": "cwe", "op": "eq", "values": ["CWE-22"]}]},
  {"q": "owner:\"Payments Team\"", "terms": [{"neg": false, "key": "owner", "op": "eq", "values": ["Payments Team"]}]},
  {"q": "owner:'a b',c", "terms": [{"neg": false, "key": "owner", "op": "eq", "values": ["a b", "c"]}]},
  {"q": "owner:a\\,b", "terms": [{"neg": false, "key": "owner", "op": "eq", "values": ["a,b"]}]},
  {"q": "owner:\"say \\\"hi\\\"\"", "terms": [{"neg": false, "key": "owner", "op": "eq", "values": ["say \"hi\""]}]},
  {"q": "file:src/api/ run:run_01J id:fnd_01J", "terms": [{"neg": false, "key": "file", "op": "eq", "values": ["src/api/"]}, {"neg": false, "key": "run", "op": "eq", "values": ["run_01J"]}, {"neg": false, "key": "id", "op": "eq", "values": ["fnd_01J"]}]},
  {"q": "verified:Confirmed profile:full scope:host", "terms": [{"neg": false, "key": "verified", "op": "eq", "values": ["confirmed"]}, {"neg": false, "key": "profile", "op": "eq", "values": ["full"]}, {"neg": false, "key": "scope", "op": "eq", "values": ["host"]}]},
  {"q": "agent:heron#3 workflow:review-flow concern:authz-idor project:shop-web", "terms": [{"neg": false, "key": "agent", "op": "eq", "values": ["heron#3"]}, {"neg": false, "key": "workflow", "op": "eq", "values": ["review-flow"]}, {"neg": false, "key": "concern", "op": "eq", "values": ["authz-idor"]}, {"neg": false, "key": "project", "op": "eq", "values": ["shop-web"]}]},
  {"q": "sql injection", "terms": [{"neg": false, "key": "text", "op": "eq", "values": ["sql"]}, {"neg": false, "key": "text", "op": "eq", "values": ["injection"]}]},
  {"q": "\"sql injection\"", "terms": [{"neg": false, "key": "text", "op": "eq", "values": ["sql injection"]}]},
  {"q": "-noise", "terms": [{"neg": true, "key": "text", "op": "eq", "values": ["noise"]}]},
  {"q": "-\"two words\"", "terms": [{"neg": true, "key": "text", "op": "eq", "values": ["two words"]}]},
  {"q": "-", "terms": [{"neg": false, "key": "text", "op": "eq", "values": ["-"]}]},
  {"q": "it's", "terms": [{"neg": false, "key": "text", "op": "eq", "values": ["it's"]}]},
  {"q": "a\\ b", "terms": [{"neg": false, "key": "text", "op": "eq", "values": ["a b"]}]},
  {"q": "a,b", "terms": [{"neg": false, "key": "text", "op": "eq", "values": ["a,b"]}]},
  {"q": "  severity:high \t tag:x  ", "terms": [{"neg": false, "key": "severity", "op": "eq", "values": ["high"]}, {"neg": false, "key": "tag", "op": "eq", "values": ["x"]}]},
  {"q": "sevrity:high", "error": {"token": 0, "code": "unknown_key"}},
  {"q": "http://example.test", "error": {"token": 0, "code": "unknown_key"}},
  {"q": "severity:urgent", "error": {"token": 0, "code": "bad_value"}},
  {"q": "severity:high cwe:abc", "error": {"token": 1, "code": "bad_value"}},
  {"q": "tag:a\\ b", "error": {"token": 0, "code": "bad_value"}},
  {"q": "tag:", "error": {"token": 0, "code": "empty_value"}},
  {"q": "tag:a,", "error": {"token": 0, "code": "empty_value"}},
  {"q": "tag:a,,b", "error": {"token": 0, "code": "empty_value"}},
  {"q": "\"\"", "error": {"token": 0, "code": "empty_value"}},
  {"q": "tag>=a", "error": {"token": 0, "code": "bad_operator"}},
  {"q": "severity>=high,critical", "error": {"token": 0, "code": "bad_operator"}},
  {"q": "x:1 owner:\"abc", "error": {"token": 1, "code": "unclosed_quote"}},
  {"q": "owner:\"a\"b", "error": {"token": 0, "code": "bad_quote"}},
  {"q": "\"phrase\"tail", "error": {"token": 0, "code": "bad_quote"}}
]
```

> **Note on case 45.** `x:1 owner:"abc` must report `unclosed_quote` at token 1, not `unknown_key` at token 0. The scanner runs over the whole string before any token is parsed, so scan errors win.

- [ ] **Step 2: Write the failing Rust lockstep test.** Create `crates/rupu-coverage/tests/it/finding_query_lockstep.rs`:

```rust
//! The Rust half of the finding-query lockstep. The web's
//! `src/lib/findingQuery/grammar.lockstep.test.ts` runs the same fixtures, so
//! the two parsers cannot drift apart.

use rupu_coverage::{parse_query, FIELDS};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/finding_query/");
    serde_json::from_str(&std::fs::read_to_string(format!("{path}{name}")).unwrap()).unwrap()
}

#[test]
fn every_case_parses_or_fails_exactly_as_the_fixture_says() {
    for case in fixture("cases.json").as_array().unwrap() {
        let q = case["q"].as_str().unwrap();
        match (parse_query(q), case.get("terms"), case.get("error")) {
            (Ok(parsed), Some(terms), None) => {
                assert_eq!(serde_json::to_value(&parsed).unwrap()["terms"], *terms, "q = {q:?}");
            }
            (Err(e), None, Some(want)) => {
                assert_eq!(e.token as u64, want["token"].as_u64().unwrap(), "q = {q:?}: {e}");
                assert_eq!(serde_json::to_value(e.code).unwrap(), want["code"], "q = {q:?}: {e}");
            }
            (got, _, _) => panic!("q = {q:?}: fixture and parser disagree: {got:?}"),
        }
    }
}

#[test]
fn the_registry_matches_the_fixture() {
    assert_eq!(serde_json::to_value(FIELDS).unwrap(), fixture("fields.json"));
}

#[test]
fn errors_carry_char_offsets_of_their_token() {
    let e = parse_query("é tag>=x").unwrap_err();
    assert_eq!((e.token, e.start, e.end), (1, 2, 8));
}
```

  - Add `mod finding_query_lockstep;` to `tests/it/main.rs`.

- [ ] **Step 3: Run the test and confirm it fails.**
  - Run: `cargo test -p rupu-coverage --test it finding_query_lockstep::`
  - Expected: a compile error (`parse_query` is undefined).

- [ ] **Step 4: Implement `query_lang.rs`.**

```rust
//! The findings query language (spec "Query language"). One line, e.g.
//! `severity>=high tag:class:sqli -tag:false-positive "sql injection"`.
//!
//! This module only parses and validates. `ledger::finding_filter`
//! evaluates. The web's `src/lib/findingQuery/grammar.ts` is a twin of this
//! file, held in lockstep by `tests/fixtures/finding_query/`. Change both,
//! and the fixtures, together.

use crate::ledger::tags::Tag;
use crate::report::cwe::parse_cwe;
use serde::Serialize;

/// A field a term filters on; `Text` is free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Key {
    Severity,
    Tag,
    Has,
    Project,
    Cwe,
    Owner,
    Product,
    Verified,
    Profile,
    Scope,
    Concern,
    Agent,
    Workflow,
    File,
    Run,
    Id,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

/// One token of a query. `values` are OR-ed. A `Term` matches when any
/// value matches, inverted when `negated`. Values are normalized: enum and
/// severity values are lowercase, tags pass `Tag::parse`, CWEs are `CWE-<n>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Term {
    #[serde(rename = "neg")]
    pub negated: bool,
    pub key: Key,
    pub op: Op,
    pub values: Vec<String>,
}

/// Every term must match (AND).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ParsedQuery {
    pub terms: Vec<Term>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnknownKey,
    EmptyValue,
    BadValue,
    BadOperator,
    UnclosedQuote,
    BadQuote,
}

/// Why a query does not parse: the 0-based token and its char span
/// (Unicode scalar values, `[start, end)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct ParseError {
    pub token: usize,
    pub start: usize,
    pub end: usize,
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Severity,
    Tag,
    Cwe,
    Enum,
    Text,
}

/// A queryable field. Serializes to the `fields.json` lockstep shape.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FieldDef {
    pub key: &'static str,
    pub aliases: &'static [&'static str],
    pub kind: FieldKind,
    pub values: &'static [&'static str],
    #[serde(skip)]
    pub id: Key,
}

pub const SEVERITIES: &[&str] = &["info", "low", "medium", "high", "critical"];

const fn def(key: &'static str, id: Key, kind: FieldKind) -> FieldDef {
    FieldDef { key, aliases: &[], kind, values: &[], id }
}

/// The findings field registry, in suggestion order.
pub const FIELDS: &[FieldDef] = &[
    FieldDef { key: "severity", aliases: &["sev"], kind: FieldKind::Severity, values: SEVERITIES, id: Key::Severity },
    def("tag", Key::Tag, FieldKind::Tag),
    FieldDef { key: "has", aliases: &[], kind: FieldKind::Enum, values: &["tags", "report", "poc", "cwe"], id: Key::Has },
    def("project", Key::Project, FieldKind::Text),
    def("cwe", Key::Cwe, FieldKind::Cwe),
    def("owner", Key::Owner, FieldKind::Text),
    def("product", Key::Product, FieldKind::Text),
    FieldDef { key: "verified", aliases: &[], kind: FieldKind::Enum, values: &["unverified", "confirmed", "disputed", "inconclusive"], id: Key::Verified },
    FieldDef { key: "profile", aliases: &[], kind: FieldKind::Enum, values: &["full", "summary"], id: Key::Profile },
    FieldDef { key: "scope", aliases: &[], kind: FieldKind::Enum, values: &["line", "file", "repo", "host", "endpoint", "resource"], id: Key::Scope },
    def("concern", Key::Concern, FieldKind::Text),
    def("agent", Key::Agent, FieldKind::Text),
    def("workflow", Key::Workflow, FieldKind::Text),
    def("file", Key::File, FieldKind::Text),
    def("run", Key::Run, FieldKind::Text),
    def("id", Key::Id, FieldKind::Text),
];

/// The field named `name` (or one of its aliases), case-insensitively.
pub fn field(name: &str) -> Option<&'static FieldDef> {
    let n = name.to_ascii_lowercase();
    FIELDS
        .iter()
        .find(|f| f.key == n || f.aliases.contains(&n.as_str()))
}

struct RawToken {
    start: usize,
    end: usize,
    text: Vec<char>,
}

fn is_op_char(c: char) -> bool {
    matches!(c, ':' | '<' | '>' | '=')
}

fn err(token: usize, start: usize, end: usize, code: ErrorCode, message: impl Into<String>) -> ParseError {
    ParseError { token, start, end, code, message: message.into() }
}

/// Split into tokens at unquoted whitespace. A quote opens only at an item
/// start: the token start (after an optional `-`), right after an operator
/// character, or right after a `,`. `\` escapes the next char, in or out of
/// quotes.
fn scan(q: &str) -> Result<Vec<RawToken>, ParseError> {
    let chars: Vec<char> = q.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let mut item_start = true;
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            if let Some(qc) = quote {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == qc {
                    quote = None;
                    item_start = false;
                }
                i += 1;
                continue;
            }
            if c.is_whitespace() {
                break;
            }
            if c == '\\' {
                i += 2;
                item_start = false;
                continue;
            }
            if item_start && (c == '"' || c == '\'') {
                quote = Some(c);
                i += 1;
                continue;
            }
            item_start = is_op_char(c) || c == ',' || (c == '-' && i == start);
            i += 1;
        }
        let end = i.min(chars.len());
        if quote.is_some() {
            return Err(err(out.len(), start, end, ErrorCode::UnclosedQuote, "this quote is never closed"));
        }
        out.push(RawToken { start, end, text: chars[start..end].to_vec() });
        i = end;
    }
    Ok(out)
}

/// Decode items from `s` (escapes resolved, quotes removed). With `split`,
/// an unescaped, unquoted `,` separates items.
fn items(s: &[char], split: bool) -> Result<Vec<String>, (ErrorCode, &'static str)> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let mut cur = String::new();
        if i < s.len() && (s[i] == '"' || s[i] == '\'') {
            let qc = s[i];
            i += 1;
            loop {
                if i >= s.len() {
                    return Err((ErrorCode::UnclosedQuote, "this quote is never closed"));
                }
                let c = s[i];
                if c == '\\' && i + 1 < s.len() {
                    cur.push(s[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if c == qc {
                    break;
                }
                cur.push(c);
            }
            if i < s.len() && !(split && s[i] == ',') {
                return Err((ErrorCode::BadQuote, "put a space or a comma after a closing quote"));
            }
        } else {
            while i < s.len() {
                let c = s[i];
                if c == '\\' && i + 1 < s.len() {
                    cur.push(s[i + 1]);
                    i += 2;
                    continue;
                }
                if split && c == ',' {
                    break;
                }
                cur.push(c);
                i += 1;
            }
        }
        if cur.is_empty() {
            return Err((ErrorCode::EmptyValue, "empty value"));
        }
        out.push(cur);
        if split && i < s.len() && s[i] == ',' {
            i += 1;
            if i == s.len() {
                return Err((ErrorCode::EmptyValue, "empty value after a comma"));
            }
            continue;
        }
        return Ok(out);
    }
}

fn normalize(def: &FieldDef, v: &str) -> Result<String, String> {
    match def.kind {
        FieldKind::Severity | FieldKind::Enum => {
            let l = v.to_ascii_lowercase();
            if def.values.contains(&l.as_str()) {
                Ok(l)
            } else {
                Err(format!("`{v}` is not a {} (one of {})", def.key, def.values.join(", ")))
            }
        }
        FieldKind::Tag => Tag::parse(v).map(String::from).map_err(|e| e.to_string()),
        FieldKind::Cwe => parse_cwe(v)
            .map(|n| format!("CWE-{n}"))
            .ok_or_else(|| format!("`{v}` is not a CWE id (expected 79 or CWE-79)")),
        FieldKind::Text => Ok(v.to_string()),
    }
}

fn parse_token(t: &RawToken, idx: usize) -> Result<Term, ParseError> {
    let e = |code: ErrorCode, message: String| err(idx, t.start, t.end, code, message);
    let (negated, body) = if t.text.len() > 1 && t.text[0] == '-' {
        (true, &t.text[1..])
    } else {
        (false, &t.text[..])
    };
    let key_len = body
        .iter()
        .take_while(|c| c.is_ascii_alphabetic() || **c == '_')
        .count();
    let rest = &body[key_len..];
    let op = if key_len == 0 {
        None
    } else {
        match (rest.first(), rest.get(1)) {
            (Some('>'), Some('=')) => Some((Op::Ge, 2)),
            (Some('<'), Some('=')) => Some((Op::Le, 2)),
            (Some('>'), _) => Some((Op::Gt, 1)),
            (Some('<'), _) => Some((Op::Lt, 1)),
            (Some(':'), _) => Some((Op::Eq, 1)),
            _ => None,
        }
    };
    let Some((op, op_len)) = op else {
        let values = items(body, false).map_err(|(c, m)| e(c, m.to_string()))?;
        return Ok(Term { negated, key: Key::Text, op: Op::Eq, values });
    };
    let name: String = body[..key_len].iter().collect();
    let Some(def) = field(&name) else {
        return Err(e(
            ErrorCode::UnknownKey,
            format!("unknown key `{name}` (quote the text to search for it)"),
        ));
    };
    if op != Op::Eq && def.kind != FieldKind::Severity {
        return Err(e(ErrorCode::BadOperator, format!("`{}` only takes `:`", def.key)));
    }
    let raw = items(&rest[op_len..], true).map_err(|(c, m)| e(c, m.to_string()))?;
    if op != Op::Eq && raw.len() > 1 {
        return Err(e(ErrorCode::BadOperator, "a comparison takes one value".to_string()));
    }
    let values = raw
        .iter()
        .map(|v| normalize(def, v))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|m| e(ErrorCode::BadValue, m))?;
    Ok(Term { negated, key: def.id, op, values })
}

/// Parse a query. An empty or all-whitespace query has no terms and matches
/// everything.
pub fn parse_query(q: &str) -> Result<ParsedQuery, ParseError> {
    let tokens = scan(q)?;
    let terms = tokens
        .iter()
        .enumerate()
        .map(|(i, t)| parse_token(t, i))
        .collect::<Result<_, _>>()?;
    Ok(ParsedQuery { terms })
}
```

  - Exports:
    - `ledger/mod.rs`: `pub mod query_lang;` and `pub use query_lang::{field, parse_query, ErrorCode, FieldDef, FieldKind, Key, Op, ParseError, ParsedQuery, Term, FIELDS, SEVERITIES};`
    - Mirror the same names in `lib.rs`'s `pub use ledger::{…}`.

- [ ] **Step 5: Run the tests and confirm they pass.**
  - Run `cargo test -p rupu-coverage --test it finding_query_lockstep::`.
  - Expected: PASS. If a case fails, the code is wrong, not the fixture: the fixture encodes the spec. Two exceptions:
    - a case where the fixture contradicts the Global Constraints' grammar
    - a case where the Rust escape/quote semantics shown here produce something else for a reason you can justify

    In either situation, stop and report NEEDS_CONTEXT.

- [ ] **Step 6: Format, lint, commit.**

```bash
git add crates/rupu-coverage/src/ledger crates/rupu-coverage/src/lib.rs crates/rupu-coverage/tests
git commit -m "feat(coverage): the findings query language parser + lockstep fixtures

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The evaluator — `finding_filter` (match, select, facets)

**Files:**
- Create: `crates/rupu-coverage/src/ledger/finding_filter.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`, `crates/rupu-coverage/src/lib.rs`

**Interfaces:**
- Consumes:
  - `ParsedQuery`, `Term`, `Key` and `Op` (Task 2)
  - `finding_cwes` and `parse_cwe` (Task 1)
  - `ledger::query::severity_rank`, which exists from Plan 1
- Produces (all re-exported):
  - `FindingView<'a> { record: &'a FindingRecord, project: Option<&'a str>, ws_id: Option<&'a str>, workflow: Option<&'a str> }` and `FindingView::bare(&FindingRecord)`
  - `type RunScopes = HashMap<String, HashSet<String>>`
  - `run_values(&ParsedQuery) -> Vec<String>`
  - `check_available(&ParsedQuery, provenance: bool) -> Result<(), Unavailable>`
  - `Unavailable { key: &'static str }` (thiserror)
  - `matches(&ParsedQuery, &FindingView, &RunScopes) -> bool`
  - `select<'a, T>(items: &'a [T], view: impl Fn(&'a T) -> FindingView<'a>, q: &ParsedQuery, runs: &RunScopes) -> Vec<&'a T>`, sorted by severity, then newest, then id
  - `FacetValue { value: String, count: usize }`
  - `facets<'a>(views: impl IntoIterator<Item = FindingView<'a>>) -> BTreeMap<&'static str, Vec<FacetValue>>`

  The facet keys are `severity` (always all five, critical first), `tag`, `project`, `owner`, `product`, `cwe`, `agent`, `workflow`, `concern`, `verified`, `profile` and `scope`. Every key except `severity` is sorted by count descending, then by value.

- [ ] **Step 1: Write the failing tests.** In `finding_filter.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, FindingEvidence, FindingRecord, FindingScope, Surface};
    use crate::ledger::query_lang::parse_query;
    use chrono::{TimeZone, Utc};

    fn rec(id: &str, sev: Severity, minute: u32, tags: &[&str]) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: Some(format!("src/{id}.rs")),
            line_range: Some([3, 4]),
            target_ref: None,
            scope: FindingScope::Line,
            summary: format!("Order search builds SQL in {id}"),
            severity: sev,
            concern_id: Some("cwe-89-sqli".into()),
            evidence: FindingEvidence { code_excerpt: None, rationale: "r".into(), references: vec![] },
            declared_by: Attribution {
                run_id: format!("run_{id}"),
                model: "m".into(),
                surface: Surface::Workflow,
                codename: Some("jade-reef/heron#3".into()),
                agent: Some("sqli-hunter".into()),
                provider: None,
            },
            declared_at: Utc.with_ymd_and_hms(2026, 10, 6, 12, minute, 0).unwrap(),
            profile: crate::report::FindingProfile::Summary,
            report: None,
            tags: crate::ledger::tags::parse_tags(tags).unwrap(),
        }
    }

    fn fixture() -> Vec<FindingRecord> {
        vec![
            rec("fnd_low", Severity::Low, 1, &["class:sqli"]),
            rec("fnd_crit", Severity::Critical, 2, &["class:sqli", "needs-poc"]),
            rec("fnd_high", Severity::High, 3, &[]),
        ]
    }

    fn ids(q: &str) -> Vec<String> {
        let f = fixture();
        let parsed = parse_query(q).unwrap();
        select(&f, FindingView::bare, &parsed, &RunScopes::new())
            .into_iter()
            .map(|r| r.id.clone())
            .collect()
    }

    #[test]
    fn empty_query_selects_everything_in_order() {
        assert_eq!(ids(""), ["fnd_crit", "fnd_high", "fnd_low"]);
    }

    #[test]
    fn severity_compares_by_rank() {
        assert_eq!(ids("severity>=high"), ["fnd_crit", "fnd_high"]);
        assert_eq!(ids("severity<high"), ["fnd_low"]);
        assert_eq!(ids("severity:low,critical"), ["fnd_crit", "fnd_low"]);
        assert_eq!(ids("-severity:critical"), ["fnd_high", "fnd_low"]);
    }

    #[test]
    fn tags_and_across_tokens_or_within_one() {
        assert_eq!(ids("tag:class:sqli tag:needs-poc"), ["fnd_crit"]);
        assert_eq!(ids("tag:needs-poc,class:sqli"), ["fnd_crit", "fnd_low"]);
        assert_eq!(ids("-has:tags"), ["fnd_high"]);
        assert_eq!(ids("-tag:needs-poc has:tags"), ["fnd_low"]);
    }

    #[test]
    fn other_fields_match() {
        assert_eq!(ids("cwe:89").len(), 3);
        assert!(ids("cwe:79").is_empty());
        assert_eq!(ids("agent:sqli-hunter").len(), 3);
        assert_eq!(ids("agent:JADE-REEF/HERON#3").len(), 3);
        assert_eq!(ids("file:src/fnd_c"), ["fnd_crit"]);
        assert_eq!(ids("id:fnd_high"), ["fnd_high"]);
        assert_eq!(ids("verified:unverified").len(), 3);
        assert_eq!(ids("profile:summary scope:line").len(), 3);
        assert_eq!(ids("concern:CWE-89-SQLI").len(), 3);
        assert!(ids("has:report").is_empty());
    }

    #[test]
    fn free_text_is_case_insensitive_over_title_summary_id_and_file() {
        assert_eq!(ids("ORDER search").len(), 3);
        assert_eq!(ids("fnd_crit"), ["fnd_crit"]);
        assert_eq!(ids("-crit"), ["fnd_high", "fnd_low"]);
    }

    #[test]
    fn run_matches_exactly_or_through_its_scope() {
        let f = fixture();
        let q = parse_query("run:parent_run").unwrap();
        assert!(select(&f, FindingView::bare, &q, &RunScopes::new()).is_empty());
        let mut runs = RunScopes::new();
        runs.insert("parent_run".into(), ["run_fnd_low".to_string()].into());
        let got: Vec<_> = select(&f, FindingView::bare, &q, &runs).into_iter().map(|r| r.id.clone()).collect();
        assert_eq!(got, ["fnd_low"]);
        assert_eq!(run_values(&q), ["parent_run"]);
    }

    #[test]
    fn project_and_workflow_need_provenance() {
        let q = parse_query("project:shop-web workflow:review").unwrap();
        assert_eq!(check_available(&q, false).unwrap_err().key, "project");
        assert!(check_available(&q, true).is_ok());
        let f = fixture();
        let views: Vec<FindingView> = f
            .iter()
            .map(|r| FindingView { record: r, project: Some("shop-web"), ws_id: Some("ws1"), workflow: Some("review") })
            .collect();
        assert!(matches(&q, &views[0], &RunScopes::new()));
        assert!(matches(&parse_query("project:ws1").unwrap(), &views[0], &RunScopes::new()));
        assert!(!matches(&parse_query("project:other").unwrap(), &views[0], &RunScopes::new()));
    }

    #[test]
    fn facets_count_values_most_used_first() {
        let f = fixture();
        let fc = facets(f.iter().map(FindingView::bare));
        let sev: Vec<(&str, usize)> = fc["severity"].iter().map(|v| (v.value.as_str(), v.count)).collect();
        assert_eq!(sev, [("critical", 1), ("high", 1), ("medium", 0), ("low", 1), ("info", 0)]);
        let tags: Vec<(&str, usize)> = fc["tag"].iter().map(|v| (v.value.as_str(), v.count)).collect();
        assert_eq!(tags, [("class:sqli", 2), ("needs-poc", 1)]);
        assert_eq!(fc["cwe"][0].value, "CWE-89");
        assert_eq!(fc["verified"][0].value, "unverified");
        assert!(fc["project"].is_empty());
    }
}
```

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cargo test -p rupu-coverage --lib ledger::finding_filter`
  - Expected: a compile error.

- [ ] **Step 3: Implement** (above the tests):

```rust
//! Evaluate a parsed findings query (`ledger::query_lang`) against findings,
//! with whatever provenance the caller knows. The ONE evaluator: the CP API,
//! the CLI, the agent tool and MCP all filter through `select`.

use crate::catalog::types::Severity;
use crate::ledger::events::FindingRecord;
use crate::ledger::query::severity_rank;
use crate::ledger::query_lang::{Key, Op, ParsedQuery, Term};
use crate::report::cwe::{finding_cwes, parse_cwe};
use crate::report::{FindingProfile, VerificationStatus};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// A finding plus the provenance a surface knows about it.
#[derive(Debug, Clone, Copy)]
pub struct FindingView<'a> {
    pub record: &'a FindingRecord,
    /// Owning project name (workspace basename).
    pub project: Option<&'a str>,
    pub ws_id: Option<&'a str>,
    pub workflow: Option<&'a str>,
}

impl<'a> FindingView<'a> {
    /// A finding with no provenance (agent tool, MCP).
    pub fn bare(record: &'a FindingRecord) -> Self {
        Self { record, project: None, ws_id: None, workflow: None }
    }
}

/// `run:` value → that run plus its sub-runs. A value with no entry matches
/// `declared_by.run_id` exactly.
pub type RunScopes = HashMap<String, HashSet<String>>;

/// The query uses a key this surface has no data for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{key}:` isn't available here: findings are read without project or workflow provenance")]
pub struct Unavailable {
    pub key: &'static str,
}

/// Refuse `project:` / `workflow:` where findings carry no provenance.
pub fn check_available(q: &ParsedQuery, provenance: bool) -> Result<(), Unavailable> {
    if provenance {
        return Ok(());
    }
    for t in &q.terms {
        match t.key {
            Key::Project => return Err(Unavailable { key: "project" }),
            Key::Workflow => return Err(Unavailable { key: "workflow" }),
            _ => {}
        }
    }
    Ok(())
}

/// Every `run:` value in the query, for the caller to resolve into
/// [`RunScopes`].
pub fn run_values(q: &ParsedQuery) -> Vec<String> {
    q.terms
        .iter()
        .filter(|t| t.key == Key::Run)
        .flat_map(|t| t.values.iter().cloned())
        .collect()
}

fn ci(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn sev_of(s: &str) -> Severity {
    match s {
        "critical" => Severity::Critical,
        "high" => Severity::High,
        "medium" => Severity::Medium,
        "low" => Severity::Low,
        _ => Severity::Info,
    }
}

fn verified_of(r: &FindingRecord) -> &'static str {
    match r.report.as_ref().and_then(|rep| rep.verification.as_ref()).map(|v| v.status) {
        Some(VerificationStatus::Confirmed) => "confirmed",
        Some(VerificationStatus::Disputed) => "disputed",
        Some(VerificationStatus::Inconclusive) => "inconclusive",
        Some(VerificationStatus::Unverified) | None => "unverified",
    }
}

fn profile_of(r: &FindingRecord) -> &'static str {
    match r.profile {
        FindingProfile::Full => "full",
        FindingProfile::Summary => "summary",
    }
}

fn title_of(r: &FindingRecord) -> &str {
    r.report.as_ref().map(|rep| rep.title.as_str()).unwrap_or(&r.summary)
}

fn text_hit(r: &FindingRecord, needle: &str) -> bool {
    let n = needle.to_lowercase();
    [Some(title_of(r)), Some(r.summary.as_str()), Some(r.id.as_str()), r.file_path.as_deref()]
        .into_iter()
        .flatten()
        .any(|h| h.to_lowercase().contains(&n))
}

fn term_matches(t: &Term, v: &FindingView, runs: &RunScopes) -> bool {
    let r = v.record;
    let any = |f: &dyn Fn(&str) -> bool| t.values.iter().any(|x| f(x));
    let hit = match t.key {
        Key::Severity => {
            let have = severity_rank(r.severity);
            any(&|x| {
                let want = severity_rank(sev_of(x));
                match t.op {
                    Op::Eq => have == want,
                    Op::Gt => have > want,
                    Op::Ge => have >= want,
                    Op::Lt => have < want,
                    Op::Le => have <= want,
                }
            })
        }
        Key::Tag => any(&|x| r.tags.iter().any(|tag| tag.as_str() == x)),
        Key::Has => any(&|x| match x {
            "tags" => !r.tags.is_empty(),
            "report" => r.report.is_some(),
            "poc" => r.report.as_ref().is_some_and(|rep| !rep.artifacts.is_empty()),
            "cwe" => !finding_cwes(r).is_empty(),
            _ => false,
        }),
        Key::Project => any(&|x| v.project.is_some_and(|p| ci(p, x)) || v.ws_id == Some(x)),
        Key::Cwe => {
            let have = finding_cwes(r);
            any(&|x| parse_cwe(x).is_some_and(|n| have.contains(&n)))
        }
        Key::Owner => any(&|x| r.report.as_ref().is_some_and(|rep| ci(&rep.ownership.owner, x))),
        Key::Product => any(&|x| r.report.as_ref().is_some_and(|rep| ci(&rep.ownership.product, x))),
        Key::Verified => any(&|x| x == verified_of(r)),
        Key::Profile => any(&|x| x == profile_of(r)),
        Key::Scope => any(&|x| x == r.scope.as_str()),
        Key::Concern => any(&|x| r.concern_id.as_deref().is_some_and(|c| ci(c, x))),
        Key::Agent => any(&|x| {
            r.declared_by.agent.as_deref().is_some_and(|a| ci(a, x))
                || r.declared_by.codename.as_deref().is_some_and(|c| ci(c, x))
        }),
        Key::Workflow => any(&|x| v.workflow.is_some_and(|w| ci(w, x))),
        Key::File => any(&|x| r.file_path.as_deref().is_some_and(|f| f.starts_with(x))),
        Key::Run => any(&|x| {
            r.declared_by.run_id == x
                || runs.get(x).is_some_and(|s| s.contains(&r.declared_by.run_id))
        }),
        Key::Id => any(&|x| r.id == x),
        Key::Text => any(&|x| text_hit(r, x)),
    };
    hit != t.negated
}

/// Whether `v` matches every term.
pub fn matches(q: &ParsedQuery, v: &FindingView, runs: &RunScopes) -> bool {
    q.terms.iter().all(|t| term_matches(t, v, runs))
}

/// The items whose view matches, ordered by severity (critical first), then
/// newest, then id.
pub fn select<'a, T>(
    items: &'a [T],
    view: impl Fn(&'a T) -> FindingView<'a>,
    q: &ParsedQuery,
    runs: &RunScopes,
) -> Vec<&'a T> {
    let mut out: Vec<&'a T> = items.iter().filter(|i| matches(q, &view(i), runs)).collect();
    out.sort_by_cached_key(|i| {
        let r = view(i).record;
        (std::cmp::Reverse(severity_rank(r.severity)), std::cmp::Reverse(r.declared_at), r.id.clone())
    });
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FacetValue {
    pub value: String,
    pub count: usize,
}

fn ranked(counts: HashMap<String, usize>) -> Vec<FacetValue> {
    let mut v: Vec<FacetValue> = counts.into_iter().map(|(value, count)| FacetValue { value, count }).collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    v
}

/// Values in use per key, for autocomplete and the severity tiles.
pub fn facets<'a>(views: impl IntoIterator<Item = FindingView<'a>>) -> BTreeMap<&'static str, Vec<FacetValue>> {
    let keys = ["tag", "project", "owner", "product", "cwe", "agent", "workflow", "concern", "verified", "profile", "scope"];
    let mut counts: HashMap<&'static str, HashMap<String, usize>> = keys.iter().map(|k| (*k, HashMap::new())).collect();
    let mut sev = [0usize; 5];
    let mut bump = |k: &'static str, v: &str| {
        *counts.get_mut(k).expect("known key").entry(v.to_string()).or_default() += 1;
    };
    for v in views {
        let r = v.record;
        sev[severity_rank(r.severity) as usize] += 1;
        for t in &r.tags {
            bump("tag", t.as_str());
        }
        if let Some(p) = v.project {
            bump("project", p);
        }
        if let Some(rep) = &r.report {
            bump("owner", &rep.ownership.owner);
            bump("product", &rep.ownership.product);
        }
        for n in finding_cwes(r) {
            bump("cwe", &format!("CWE-{n}"));
        }
        if let Some(a) = &r.declared_by.agent {
            bump("agent", a);
        }
        if let Some(w) = v.workflow {
            bump("workflow", w);
        }
        if let Some(c) = &r.concern_id {
            bump("concern", c);
        }
        bump("verified", verified_of(r));
        bump("profile", profile_of(r));
        bump("scope", r.scope.as_str());
    }
    let mut out: BTreeMap<&'static str, Vec<FacetValue>> =
        counts.into_iter().map(|(k, c)| (k, ranked(c))).collect();
    out.insert(
        "severity",
        ["critical", "high", "medium", "low", "info"]
            .iter()
            .map(|s| FacetValue { value: s.to_string(), count: sev[severity_rank(sev_of(s)) as usize] })
            .collect(),
    );
    out
}
```

Notes:
- `severity_rank` returns 0..=4 (info = 0, critical = 4), so the `sev` array indexes by rank.
- If `severity_rank` lives somewhere other than `ledger::query`, import it from where it is. Don't duplicate it.
- If clippy objects to the `bump` closure capturing `counts` mutably while `counts` is consumed afterwards, scope the loop in a block.
- Exports: add `pub mod finding_filter;` and `pub use finding_filter::{check_available, facets, run_values, FacetValue, FindingView, RunScopes, Unavailable};` to `ledger/mod.rs`. Do **not** export `select` or `matches` at the crate root, because `query::select` still exists until Task 4. In `lib.rs`, re-export the same names except `select`/`matches`; callers use `rupu_coverage::ledger::finding_filter::select`.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run `cargo test -p rupu-coverage --lib ledger::`.
  - Expected: PASS.

- [ ] **Step 5: Format, lint, commit.**

```bash
git add crates/rupu-coverage/src
git commit -m "feat(coverage): evaluate findings queries (finding_filter: match, select, facets)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Move every non-web surface to `q` and delete the Plan 1 structured filters

**Files:**
- Modify: `crates/rupu-coverage/src/ledger/query.rs` (rewrite `FindingQuery`, `query`, `query_input_schema`; delete `TagMode`, the old `matches`, the old `select`, and their tests; keep `severity_rank`, `FindingRow`, `Page`, the cursor fns, `TagCount`, `tags_in_use` and `query_response`)
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`, `crates/rupu-coverage/src/lib.rs` (drop the removed exports; export `ledger::finding_filter::select` at the root as `select`)
- Modify: `crates/rupu-agent/src/coverage_tools.rs` (`QueryFindingsTool::description` mentions `q`); `crates/rupu-agent/tests/it/findings_tags.rs`
- Modify: `crates/rupu-mcp/src/tools/findings.rs` (the `findings.query` description); `crates/rupu-mcp/tests/it/findings_tags.rs`; re-bless `tests/snapshots/tools_list.json`
- Modify: `crates/rupu-cli/src/cmd/findings.rs` (`list` and `tags` take `[QUERY…]`); `crates/rupu-cli/tests/it/findings_tags.rs`

**Interfaces:**
- Consumes:
  - `parse_query`, `ParseError` (Task 2)
  - `FindingView`, `RunScopes`, `run_values`, `check_available`, `Unavailable`, `finding_filter::select` (Task 3)
  - `cp_findings::{collect_all_findings, resolve_run_scope}` (existing)
- Produces:
  - `FindingQuery { q: String, limit: Option<usize>, cursor: Option<String> }` (`Deserialize`, `deny_unknown_fields`, `q` defaults to empty)
  - `QueryError::{Parse(ParseError), Unavailable(Unavailable), Limit(usize), Cursor(String)}`
  - `query(&[FindingRecord], &FindingQuery) -> Result<Page, QueryError>`, which parses `q`, refuses provenance keys, and pages
  - `query_response`, unchanged in shape
  - `query_input_schema()`, now `{q, limit, cursor}`
  - CLI: `rupu findings list [QUERY]… [--limit N] [--ids-only]` and `rupu findings tags [QUERY]…`. The words are joined with single spaces into one query.

- [ ] **Step 1: Update the failing tests first.**
  - In `query.rs`'s test module, delete the tests of the removed fields and keep the paging/cursor tests rewritten to use `q`. Use the existing fixture (the `rec` helper and the `fixture()` records with tags). Replace the filter tests with:

```rust
    #[test]
    fn query_parses_q_and_pages() {
        let f = fixture();
        let p = query(&f, &FindingQuery { q: "tag:needs-poc".into(), ..q() }).unwrap();
        assert_eq!(p.total, 2);
        let p = query(&f, &FindingQuery { q: "severity>=high".into(), limit: Some(1), ..q() }).unwrap();
        assert_eq!(p.rows.len(), 1);
        assert!(p.next_cursor.is_some());
    }

    #[test]
    fn query_refuses_bad_q_and_provenance_keys() {
        let f = fixture();
        assert!(matches!(query(&f, &FindingQuery { q: "sevrity:x".into(), ..q() }), Err(QueryError::Parse(_))));
        assert!(matches!(query(&f, &FindingQuery { q: "project:x".into(), ..q() }), Err(QueryError::Unavailable(_))));
    }

    #[test]
    fn query_input_is_q_limit_cursor_only() {
        let ok: FindingQuery = serde_json::from_value(serde_json::json!({"q": "tag:x", "limit": 5})).unwrap();
        assert_eq!(ok.q, "tag:x");
        assert!(serde_json::from_value::<FindingQuery>(serde_json::json!({"tags": ["x"]})).is_err());
    }
```

  The existing tests' names for the test-local fixture may differ; adapt to what is there. Keep the `pages_follow_the_cursor_and_stay_stable_under_appends` and `limits_and_cursors_are_validated` tests, building `FindingQuery { q: String::new(), limit, cursor }`.

  - In `crates/rupu-agent/tests/it/findings_tags.rs`, change every `query_findings` input:
    - `{"tags": ["needs-poc"]}` → `{"q": "tag:needs-poc"}`
    - `{"limit": 1}` stays
    - the bad-input case `{"tagz": …}` stays an error
  - Add:

```rust
#[tokio::test]
async fn a_bad_query_is_an_error_the_agent_can_read() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    let ctx = ToolContext { workspace_path: ws.clone(), ..Default::default() };
    let out = QueryFindingsTool::new(ws).invoke(serde_json::json!({"q": "project:x"}), &ctx).await.unwrap();
    assert!(out.error.unwrap().contains("isn't available here"));
}
```

  - In `crates/rupu-mcp/tests/it/findings_tags.rs`, change `{"tags": ["needs-poc"]}` → `{"q": "tag:needs-poc"}`.
  - In `crates/rupu-cli/tests/it/findings_tags.rs`, rewrite the list invocations with **flags before the query**, because a var-arg positional that allows leading hyphens may swallow later flags:
    - `["findings", "list", "--tag", "class:sqli", "--ids-only"]` → `["findings", "list", "--ids-only", "tag:class:sqli"]`
    - `["findings", "list", "--untagged", "--ids-only"]` → `["findings", "list", "--ids-only", "-has:tags"]`
    - every `--tag x` → `tag:x`
    - `--limit` / `--ids-only` always come before the query words
  - Add:

```rust
#[test]
fn list_takes_a_query_with_severity_comparison_and_negation() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    ok(&rupu(tmp.path()).args(["findings", "tag", "fnd_b", "--add", "noise"]).output().unwrap());
    let out = rupu(tmp.path()).args(["findings", "list", "--ids-only", "severity>=low -tag:noise"]).output().unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
    let out = rupu(tmp.path()).args(["findings", "list", "--ids-only", "project:repo", "severity:high"]).output().unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
}

#[test]
fn a_bad_query_is_a_clear_error() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path()).args(["findings", "list", "sevrity:high"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown key `sevrity`"));
}
```

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run `cargo test -p rupu-coverage --lib ledger::query`.
  - Expected: a compile error or failures.

- [ ] **Step 3: Implement.**
  - **`query.rs`:**

```rust
/// Agent tool / MCP input: a findings query string plus paging. Unknown
/// fields are refused.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("{0}")]
    Parse(#[from] crate::ledger::query_lang::ParseError),
    #[error("{0}")]
    Unavailable(#[from] crate::ledger::finding_filter::Unavailable),
    #[error("`limit` must be between 1 and {MAX_LIMIT}, got {0}")]
    Limit(usize),
    #[error("`cursor` is not one this query returned: {0}")]
    Cursor(String),
}
```

  - `query()`:
    - Validate `limit` as before.
    - `let parsed = crate::ledger::query_lang::parse_query(&q.q)?; crate::ledger::finding_filter::check_available(&parsed, false)?;`
    - `let all = crate::ledger::finding_filter::select(records, FindingView::bare, &parsed, &RunScopes::new());`
    - Then the same cursor and paging code as today.
  - Delete `TagMode`, the old `matches`, the old `select` and `UntaggedWithTags`.
  - `query_input_schema()` becomes:

```rust
pub fn query_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "q": { "type": "string", "description": "Findings query, e.g. `severity>=high tag:class:sqli -tag:false-positive -has:poc \"sql injection\"`. Tokens AND; `key:a,b` is any-of; `-` negates; keys: severity (>=,>,<=,<), tag, has (tags|report|poc|cwe), cwe, owner, product, verified, profile, scope, concern, agent, file (path prefix), run, id; bare words search title/summary/id/file. Empty matches everything." },
            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Rows per page (default 50)." },
            "cursor": { "type": "string", "description": "`next_cursor` from the previous page." }
        }
    })
}
```

  - **Exports:**
    - Remove `TagMode` from `ledger/mod.rs` and `lib.rs`.
    - The root `select` now points at `ledger::finding_filter::select`.
    - Fix any compile errors this causes in other crates. The CLI is handled below; the agent and MCP use only `FindingQuery`/`query_response`/`query_input_schema`.
  - **Agent** (`coverage_tools.rs`), the `QueryFindingsTool::description` text:

    "List findings recorded in this project with a one-line query in `q` (e.g. `severity>=high tag:needs-poc -has:poc`). Returns one page of slim rows (id, title, severity, location, tags), `next_cursor`, `total`, and `tags_in_use` — reuse an existing tag where it fits."
  - **MCP** (`findings.rs`), the `findings.query` description: "List this project's findings matching a one-line query `q` (e.g. `severity>=high tag:needs-poc`): one page of slim rows, `next_cursor`, `total`, and `tags_in_use`."
  - **CLI** (`cmd/findings.rs`):
    - Replace `ListArgs`'s `scope`, `tags`, `any_tag`, `untagged` and `severity` with:

```rust
    /// Findings query, e.g. `severity>=high tag:class:sqli -tag:false-positive`.
    /// Several words are joined with spaces; quote anything with `>`, `<` or
    /// spaces for your shell. Keys: severity (>=,>,<=,<), tag, has, project,
    /// cwe, owner, product, verified, profile, scope, concern, agent,
    /// workflow, file, run, id; bare words search title/summary/id/file.
    #[arg(value_name = "QUERY", allow_hyphen_values = true, num_args = 0..)]
    query: Vec<String>,
```

    - Keep `limit` and `ids_only`. `TagsArgs` gets the same `query` field instead of `scope`. Delete `ScopeArgs` if nothing else uses it.
    - The `list`/`tags` help says flags go before the query words. The rewritten tests prove `findings list --ids-only -has:tags` reads `-has:tags` as the query, not as a flag.
    - Replace `scoped_findings` with:

```rust
/// Every finding the query selects, with provenance. `run:` values expand to
/// the run plus its sub-runs.
fn queried_findings(query: &[String]) -> anyhow::Result<Vec<cp_findings::FindingOut>> {
    let global = crate::paths::global_dir()?;
    let parsed = rupu_coverage::parse_query(&query.join(" "))?;
    let store = RunStore::new(global.join("runs"));
    let runs: rupu_coverage::RunScopes = rupu_coverage::run_values(&parsed)
        .into_iter()
        .map(|r| {
            let scope = cp_findings::resolve_run_scope(&store, &r);
            (r, scope)
        })
        .collect();
    let mut all = cp_findings::collect_all_findings(&global);
    let wf = cp_findings::workflow_names_for(&store, &all);
    for f in &mut all {
        f.workflow_name = wf.get(&f.record.declared_by.run_id).cloned();
    }
    let keep: std::collections::HashSet<String> = rupu_coverage::select(
        &all,
        |f| rupu_coverage::FindingView {
            record: &f.record,
            project: Some(&f.project),
            ws_id: Some(&f.ws_id),
            workflow: f.workflow_name.as_deref(),
        },
        &parsed,
        &runs,
    )
    .into_iter()
    .map(|f| format!("{}/{}/{}", f.ws_id, f.target_id, f.record.id))
    .collect();
    let mut out: Vec<cp_findings::FindingOut> = all
        .into_iter()
        .filter(|f| keep.contains(&format!("{}/{}/{}", f.ws_id, f.target_id, f.record.id)))
        .collect();
    out.sort_by(|a, b| {
        rupu_coverage::severity_rank(b.record.severity)
            .cmp(&rupu_coverage::severity_rank(a.record.severity))
            .then_with(|| b.record.declared_at.cmp(&a.record.declared_at))
            .then_with(|| a.record.id.cmp(&b.record.id))
    });
    Ok(out)
}
```

      This needs `cp_findings::workflow_names_for(&RunStore, &[FindingOut]) -> HashMap<String, String>`. Add it to `rupu-cp/src/api/findings.rs` as a thin `pub` wrapper over the existing private `workflow_names_by_run(store, findings.iter().map(|f| f.record.declared_by.run_id.as_str()))`. `list_findings` keeps calling the private fn.
    - `list_cmd`: `let rows = queried_findings(&args.query)?;`, then the existing limit/ids-only/table/JSON code over `rows`. The `select` call and the `FindingQuery` construction go away.
    - `tags_cmd`: `rupu_coverage::tags_in_use(queried_findings(&args.query)?.iter().map(|f| &f.record))`.
    - A `ParseError`'s `Display` is its message (e.g. "unknown key `sevrity` …"). `anyhow` surfaces it through `diag::fail`, which the test asserts.
  - **Re-bless the MCP snapshot:** `BLESS=1 cargo test -p rupu-mcp --test it schema_snapshot::`. Check the diff touches only the `findings.query` schema and description.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - `cargo test -p rupu-coverage --lib ledger::`
  - `cargo test -p rupu-coverage --test it`
  - `cargo test -p rupu-agent --test it findings_`
  - `cargo test -p rupu-mcp --test it`
  - `cargo test -p rupu-cli --test it findings_`
  - `cargo check --workspace --all-targets`
  - Run clippy with `-D warnings` on rupu-coverage, rupu-agent, rupu-mcp, rupu-cp and rupu-cli.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-coverage crates/rupu-agent crates/rupu-mcp crates/rupu-cli crates/rupu-cp/src/api/findings.rs
git commit -m "feat(findings): one query string for the CLI, agent tool and MCP

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: CP API — `q`, `facets`, `tags_unavailable`, structured 400

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs`: `FindingsQuery`, `FindingsResponse`, `build_response`, `each_ledger`, `collect_all_findings`, `list_findings`
- Test: the existing in-file test module in `findings.rs` (it has `app_for` + `tower::ServiceExt::oneshot`, around line 2695), or `crates/rupu-cp/tests/it/finding_artifacts.rs`'s server helpers in a new module `tests/it/findings_query.rs`. Use whichever builds a registered-workspace fixture more simply; the in-file module already has helpers.

**Interfaces:**
- Consumes:
  - `parse_query`, `ParseError`, `ErrorCode` (Task 2)
  - `finding_filter::{select, facets, FindingView, RunScopes, run_values, FacetValue}` (Task 3)
- Produces:
  - `GET /api/findings?q=<query>`, alongside the existing `ws_id`, `workflow` and `run_id`.
  - `FindingsResponse` gains:
    - `facets: BTreeMap<&'static str, Vec<FacetValue>>`, over the ws/workflow/run-scoped set before `q` is applied
    - `tags_unavailable: Vec<String>`, the ws ids whose tag log could not be read. Always present, possibly empty.
  - `summary` stays computed over the returned (q-filtered) findings.
  - A bad `q` returns 400 with `{"error": message, "token": n, "code": "unknown_key", "start": s, "end": e}`.
  - `pub fn collect_all_findings_reporting(global) -> (Vec<FindingOut>, Vec<String>)`. `collect_all_findings` delegates to it and drops the list.

- [ ] **Step 1: Write the failing tests.** Seed one registered workspace with three findings: critical tagged `needs-poc`, high, and low. Assert:
  1. `GET /api/findings?q=severity%3E%3Dhigh` returns 2 findings, `summary.total == 2`, and `facets.severity` shows the unfiltered counts (critical 1, high 1, low 1).
  2. `GET /api/findings?q=tag%3Aneeds-poc` returns 1 finding, and that row's JSON carries `"tags": ["needs-poc"]`.
  3. `GET /api/findings?q=sevrity%3Ahigh` is a 400 with body `code == "unknown_key"`, `token == 0`, and `error` containing "unknown key".
  4. With the workspace's `.rupu/coverage/finding_tags.jsonl` replaced by a **directory**, `GET /api/findings` is a 200 whose `tags_unavailable == ["<ws id>"]`, and the findings are still listed.
  5. `GET /api/findings` with no `q` returns all 3 findings and `tags_unavailable == []`.

  Write each as a separate `#[tokio::test]` using the module's existing request helpers. Build the seed with `rupu_coverage::append_record` plus the existing workspace-registration helper. To tag `fnd_crit`, call `rupu_coverage::apply(&TagLog::for_workspace(ws), &TagChange{…}, &TagActor::operator(OperatorSurface::Cli))`.

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cargo test -p rupu-cp --lib api::findings` (or `--test it findings_query::`).
  - Expected: FAIL. `q` is ignored, and there are no `facets` or `tags_unavailable`.

- [ ] **Step 3: Implement.**
  - **`FindingsQuery`:** add `pub q: Option<String>,`.
  - **`FindingsResponse`:**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct FindingsResponse {
    pub findings: Vec<FindingOut>,
    pub summary: FindingsSummary,
    /// Values in use per query key over the scope (ws/workflow/run), before
    /// `q` — autocomplete and the severity tiles.
    pub facets: std::collections::BTreeMap<&'static str, Vec<rupu_coverage::FacetValue>>,
    /// Workspaces whose finding-tag log could not be read: their findings are
    /// served with declared tags only, and tag filters may miss them.
    pub tags_unavailable: Vec<String>,
}
```

    `build_response` sets both to empty (`Default::default()`), and `list_findings` fills them.
  - **`each_ledger`:** add a final parameter `tags_unavailable: &mut Vec<String>`. In the existing tag-log read `Err` arm, after the `warn!`, `tags_unavailable.push(w.id.clone());`. Update every caller:
    - `collect_all_findings_reporting` passes its own vec.
    - `collect_all_findings` calls it and discards the list.
    - `finding_ledgers` and `tag_findings_across` pass a throwaway `&mut Vec::new()`.
  - **`list_findings`** becomes:

```rust
async fn list_findings(
    State(s): State<AppState>,
    Query(q): Query<FindingsQuery>,
) -> Result<Json<FindingsResponse>, axum::response::Response> {
    use axum::response::IntoResponse;
    let parsed = rupu_coverage::parse_query(q.q.as_deref().unwrap_or("")).map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": e.message, "token": e.token, "code": e.code, "start": e.start, "end": e.end
            })),
        )
            .into_response()
    })?;
    let (mut out, tags_unavailable) = collect_all_findings_reporting(&s.global_dir);
    let wf_by_run = workflow_names_by_run(
        &s.run_store,
        out.iter().map(|f| f.record.declared_by.run_id.as_str()),
    );
    for f in &mut out {
        f.workflow_name = wf_by_run.get(&f.record.declared_by.run_id).cloned();
    }
    let run_ids: Option<HashSet<String>> = q
        .run_id
        .as_ref()
        .map(|parent| resolve_run_scope(&s.run_store, parent));
    let scoped = scope_by_run_set(out, &run_ids, &q.ws_id, &q.workflow).findings;
    let view = |f: &FindingOut| rupu_coverage::FindingView {
        record: &f.record,
        project: Some(&f.project),
        ws_id: Some(&f.ws_id),
        workflow: f.workflow_name.as_deref(),
    };
    let facets = rupu_coverage::facets(scoped.iter().map(view));
    let runs: rupu_coverage::RunScopes = rupu_coverage::run_values(&parsed)
        .into_iter()
        .map(|r| {
            let scope = resolve_run_scope(&s.run_store, &r);
            (r, scope)
        })
        .collect();
    let key = |f: &FindingOut| format!("{}/{}/{}", f.ws_id, f.target_id, f.record.id);
    let keep: HashSet<String> = rupu_coverage::select(&scoped, view, &parsed, &runs)
        .into_iter()
        .map(key)
        .collect();
    let filtered: Vec<FindingOut> = scoped.into_iter().filter(|f| keep.contains(&key(f))).collect();
    let mut resp = build_response(filtered);
    resp.findings = resp.findings.into_iter().map(FindingOut::into_list_row).collect();
    resp.facets = facets;
    resp.tags_unavailable = tags_unavailable;
    Ok(Json(resp))
}
```

    `keep` is keyed by `ws_id/target_id/id`, the same identity the CLI uses (Task 4) and the table's `rowKey`.
  - `ws_id`/`workflow`/`run_id` scoping keeps working exactly as before, because `scope_by_run_set` is reused. Only `facets`, `tags_unavailable` and the `q` filter are new.
  - The project/run findings tabs that call `getFindings({wsId})` / `({runId})` are unaffected; they just receive two extra fields.

- [ ] **Step 4: Run the tests and confirm they pass.** Run the new tests plus:
  - `cargo test -p rupu-cp --test it finding_artifacts::`
  - `cargo test -p rupu-cp --lib api::findings`
  - `cargo test -p rupu-cp --lib host::local`, because `dashboard_summary` uses `build_response`
  - clippy on rupu-cp

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/src crates/rupu-cp/tests
git commit -m "feat(cp): GET /api/findings?q=, facets, tags_unavailable

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: TypeScript parser twin + findings field registry (lockstep with Rust)

**Files:**
- Create: `crates/rupu-cp/web/src/lib/findingQuery/grammar.ts` (generic over a field registry)
- Create: `crates/rupu-cp/web/src/lib/findingQuery/fields.ts` (the findings registry plus icons)
- Create: `crates/rupu-cp/web/src/lib/findingQuery/grammar.lockstep.test.ts`, `crates/rupu-cp/web/src/lib/findingQuery/grammar.test.ts`

**Interfaces:**
- Produces, from `grammar.ts`:
  - `FieldKind = 'severity' | 'tag' | 'cwe' | 'enum' | 'text'`
  - `FieldSpec { key: string; aliases: string[]; kind: FieldKind; values: string[] }`
  - `Op = 'eq' | 'gt' | 'ge' | 'lt' | 'le'`
  - `Term { neg: boolean; key: string; op: Op; values: string[] }`, where `key` is a canonical field key or `'text'`
  - `ErrorCode`
  - `QueryError { token: number; start: number; end: number; code: ErrorCode; message: string }`
  - `ParseResult = { ok: true; terms: Term[] } | { ok: false; error: QueryError }`
  - `RawToken { start: number; end: number; text: string }`
  - `tokenize(q: string): { ok: true; tokens: RawToken[] } | { ok: false; error: QueryError }`
  - `parseQuery(q: string, fields: readonly FieldSpec[]): ParseResult`
  - `parseToken(raw: string, fields: readonly FieldSpec[]): ParseResult`, which parses one chip
  - `quoteValue(v: string): string`
- Produces, from `fields.ts`:
  - `QueryField extends FieldSpec { label: string; description: string; icon: LucideIcon }`
  - `FINDING_FIELDS: QueryField[]`, in the same order as `fields.json`

- [ ] **Step 1: Write the failing lockstep test.** `grammar.lockstep.test.ts`:

```ts
// The TypeScript half of the finding-query lockstep: the same fixtures the
// Rust `finding_query_lockstep` test runs. If this fails, the two parsers
// drifted — fix the parser, never the fixture, unless the spec changed.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { parseQuery } from './grammar';
import { FINDING_FIELDS } from './fields';

const dir = new URL('../../../../../rupu-coverage/tests/fixtures/finding_query/', import.meta.url);
const cases = JSON.parse(readFileSync(new URL('cases.json', dir), 'utf8')) as Array<{
  q: string;
  terms?: unknown[];
  error?: { token: number; code: string };
}>;
const fields = JSON.parse(readFileSync(new URL('fields.json', dir), 'utf8')) as unknown[];

describe('finding query lockstep', () => {
  it.each(cases.map((c) => [c.q, c] as const))('%j', (_q, c) => {
    const r = parseQuery(c.q, FINDING_FIELDS);
    if (c.terms) {
      expect(r).toEqual({ ok: true, terms: c.terms });
    } else {
      expect(r.ok).toBe(false);
      if (!r.ok) expect({ token: r.error.token, code: r.error.code }).toEqual(c.error);
    }
  });

  it('registry matches fields.json', () => {
    expect(FINDING_FIELDS.map(({ key, aliases, kind, values }) => ({ key, aliases, kind, values }))).toEqual(fields);
  });
});
```

  `grammar.test.ts` covers the parts the fixtures don't:

```ts
import { describe, expect, it } from 'vitest';
import { parseQuery, parseToken, quoteValue, tokenize } from './grammar';
import { FINDING_FIELDS } from './fields';

describe('grammar extras', () => {
  it('tokenize keeps raw token text and code-point offsets', () => {
    const r = tokenize('é tag:"a b"  -x');
    expect(r.ok && r.tokens.map((t) => [t.text, t.start, t.end])).toEqual([
      ['é', 0, 1],
      ['tag:"a b"', 2, 11],
      ['-x', 13, 15],
    ]);
  });
  it('errors carry the token span', () => {
    const r = parseQuery('é tag>=x', FINDING_FIELDS);
    expect(!r.ok && [r.error.token, r.error.start, r.error.end, r.error.code]).toEqual([1, 2, 8, 'bad_operator']);
  });
  it('parseToken parses one chip', () => {
    expect(parseToken('-tag:needs-poc', FINDING_FIELDS)).toEqual({
      ok: true,
      terms: [{ neg: true, key: 'tag', op: 'eq', values: ['needs-poc'] }],
    });
  });
  it('quoteValue quotes only when it must, and round-trips', () => {
    expect(quoteValue('needs-poc')).toBe('needs-poc');
    expect(quoteValue('Payments Team')).toBe('"Payments Team"');
    expect(quoteValue('say "hi"')).toBe('"say \\"hi\\""');
    const r = parseQuery(`owner:${quoteValue('a, "b"')}`, FINDING_FIELDS);
    expect(r.ok && r.terms[0].values).toEqual(['a, "b"']);
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cd crates/rupu-cp/web && npx vitest run src/lib/findingQuery`
  - Expected: FAIL. The module isn't found.

- [ ] **Step 3: Implement `grammar.ts`.** It is a line-for-line twin of `query_lang.rs`: same scanner rules, same item decoder, same op detection, same normalization. Characters are code points (`Array.from`); whitespace is Rust's `char::is_whitespace` set.

```ts
// The findings query language — TypeScript twin of
// `crates/rupu-coverage/src/ledger/query_lang.rs`, generic over a field
// registry. The web only PARSES (chips, suggestions, inline errors); the
// server evaluates. Held in lockstep by the shared fixtures in
// `rupu-coverage/tests/fixtures/finding_query/` (grammar.lockstep.test.ts).
// Change both parsers and the fixtures together.

export type FieldKind = 'severity' | 'tag' | 'cwe' | 'enum' | 'text';
export interface FieldSpec {
  key: string;
  aliases: string[];
  kind: FieldKind;
  values: string[];
}
export type Op = 'eq' | 'gt' | 'ge' | 'lt' | 'le';
export interface Term {
  neg: boolean;
  key: string;
  op: Op;
  values: string[];
}
export type ErrorCode = 'unknown_key' | 'empty_value' | 'bad_value' | 'bad_operator' | 'unclosed_quote' | 'bad_quote';
export interface QueryError {
  token: number;
  start: number;
  end: number;
  code: ErrorCode;
  message: string;
}
export type ParseResult = { ok: true; terms: Term[] } | { ok: false; error: QueryError };
export interface RawToken {
  start: number;
  end: number;
  text: string;
}

// Rust's `char::is_whitespace` (Unicode White_Space).
const WS = /[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/u;
const isWs = (c: string) => WS.test(c);
const isOpChar = (c: string) => c === ':' || c === '<' || c === '>' || c === '=';
const isKeyChar = (c: string) => /^[A-Za-z_]$/.test(c);

function trimWs(s: string): string {
  const cs = Array.from(s);
  let a = 0;
  let b = cs.length;
  while (a < b && isWs(cs[a])) a++;
  while (b > a && isWs(cs[b - 1])) b--;
  return cs.slice(a, b).join('');
}

type ScanOut = { ok: true; tokens: { start: number; end: number; chars: string[] }[] } | { ok: false; error: QueryError };

function scan(q: string): ScanOut {
  const chars = Array.from(q);
  const tokens: { start: number; end: number; chars: string[] }[] = [];
  let i = 0;
  while (i < chars.length) {
    if (isWs(chars[i])) {
      i++;
      continue;
    }
    const start = i;
    let itemStart = true;
    let quote: string | null = null;
    while (i < chars.length) {
      const c = chars[i];
      if (quote !== null) {
        if (c === '\\') {
          i += 2;
          continue;
        }
        if (c === quote) {
          quote = null;
          itemStart = false;
        }
        i++;
        continue;
      }
      if (isWs(c)) break;
      if (c === '\\') {
        i += 2;
        itemStart = false;
        continue;
      }
      if (itemStart && (c === '"' || c === "'")) {
        quote = c;
        i++;
        continue;
      }
      itemStart = isOpChar(c) || c === ',' || (c === '-' && i === start);
      i++;
    }
    const end = Math.min(i, chars.length);
    if (quote !== null) {
      return {
        ok: false,
        error: { token: tokens.length, start, end, code: 'unclosed_quote', message: 'this quote is never closed' },
      };
    }
    tokens.push({ start, end, chars: chars.slice(start, end) });
    i = end;
  }
  return { ok: true, tokens };
}

export function tokenize(q: string): { ok: true; tokens: RawToken[] } | { ok: false; error: QueryError } {
  const s = scan(q);
  if (!s.ok) return s;
  return { ok: true, tokens: s.tokens.map((t) => ({ start: t.start, end: t.end, text: t.chars.join('') })) };
}

type ItemsOut = { ok: true; values: string[] } | { ok: false; code: ErrorCode; message: string };

function items(s: string[], split: boolean): ItemsOut {
  const out: string[] = [];
  let i = 0;
  for (;;) {
    let cur = '';
    if (i < s.length && (s[i] === '"' || s[i] === "'")) {
      const qc = s[i];
      i++;
      for (;;) {
        if (i >= s.length) return { ok: false, code: 'unclosed_quote', message: 'this quote is never closed' };
        const c = s[i];
        if (c === '\\' && i + 1 < s.length) {
          cur += s[i + 1];
          i += 2;
          continue;
        }
        i++;
        if (c === qc) break;
        cur += c;
      }
      if (i < s.length && !(split && s[i] === ',')) {
        return { ok: false, code: 'bad_quote', message: 'put a space or a comma after a closing quote' };
      }
    } else {
      while (i < s.length) {
        const c = s[i];
        if (c === '\\' && i + 1 < s.length) {
          cur += s[i + 1];
          i += 2;
          continue;
        }
        if (split && c === ',') break;
        cur += c;
        i++;
      }
    }
    if (cur === '') return { ok: false, code: 'empty_value', message: 'empty value' };
    out.push(cur);
    if (split && i < s.length && s[i] === ',') {
      i++;
      if (i === s.length) return { ok: false, code: 'empty_value', message: 'empty value after a comma' };
      continue;
    }
    return { ok: true, values: out };
  }
}

const asciiLower = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());

/** `Tag::parse` (rupu-coverage `ledger/tags.rs`). */
export function parseTag(raw: string): { ok: true; tag: string } | { ok: false; message: string } {
  const bad = (reason: string) => ({ ok: false as const, message: `invalid tag \`${raw}\`: ${reason}` });
  const t = trimWs(raw).toLowerCase();
  if (t === '') return bad('empty');
  if (!/^[a-z0-9._:/-]*$/.test(t)) return bad('only a-z, 0-9 and . _ : / - are allowed');
  if (!/^[a-z0-9]/.test(t)) return bad('must start with a letter or digit');
  if (t.length > 64) return bad('longer than 64 characters');
  return { ok: true, tag: t };
}

/** `parse_cwe` (rupu-coverage `report/cwe.rs`). */
export function parseCwe(raw: string): number | null {
  const s = trimWs(raw);
  let digits = s;
  if (asciiLower(s.slice(0, 3)) === 'cwe') {
    const rest = s.slice(3);
    digits = rest.startsWith('-') || rest.startsWith('_') ? rest.slice(1) : rest;
  }
  if (!/^[0-9]+$/.test(digits)) return null;
  const n = Number(digits);
  return Number.isSafeInteger(n) && n <= 4294967295 ? n : null;
}

function normalize(f: FieldSpec, v: string): { ok: true; value: string } | { ok: false; message: string } {
  switch (f.kind) {
    case 'severity':
    case 'enum': {
      const l = asciiLower(v);
      return f.values.includes(l)
        ? { ok: true, value: l }
        : { ok: false, message: `\`${v}\` is not a ${f.key} (one of ${f.values.join(', ')})` };
    }
    case 'tag': {
      const t = parseTag(v);
      return t.ok ? { ok: true, value: t.tag } : { ok: false, message: t.message };
    }
    case 'cwe': {
      const n = parseCwe(v);
      return n === null
        ? { ok: false, message: `\`${v}\` is not a CWE id (expected 79 or CWE-79)` }
        : { ok: true, value: `CWE-${n}` };
    }
    default:
      return { ok: true, value: v };
  }
}

export function findField(name: string, fields: readonly FieldSpec[]): FieldSpec | undefined {
  const n = asciiLower(name);
  return fields.find((f) => f.key === n || f.aliases.includes(n));
}

function parseRaw(t: { start: number; end: number; chars: string[] }, idx: number, fields: readonly FieldSpec[]): ParseResult {
  const fail = (code: ErrorCode, message: string): ParseResult => ({
    ok: false,
    error: { token: idx, start: t.start, end: t.end, code, message },
  });
  const neg = t.chars.length > 1 && t.chars[0] === '-';
  const body = neg ? t.chars.slice(1) : t.chars;
  let keyLen = 0;
  while (keyLen < body.length && isKeyChar(body[keyLen])) keyLen++;
  const rest = body.slice(keyLen);
  let op: Op | null = null;
  let opLen = 0;
  if (keyLen > 0) {
    if (rest[0] === '>' && rest[1] === '=') [op, opLen] = ['ge', 2];
    else if (rest[0] === '<' && rest[1] === '=') [op, opLen] = ['le', 2];
    else if (rest[0] === '>') [op, opLen] = ['gt', 1];
    else if (rest[0] === '<') [op, opLen] = ['lt', 1];
    else if (rest[0] === ':') [op, opLen] = ['eq', 1];
  }
  if (op === null) {
    const it = items(body, false);
    if (!it.ok) return fail(it.code, it.message);
    return { ok: true, terms: [{ neg, key: 'text', op: 'eq', values: it.values }] };
  }
  const name = body.slice(0, keyLen).join('');
  const f = findField(name, fields);
  if (!f) return fail('unknown_key', `unknown key \`${name}\` (quote the text to search for it)`);
  if (op !== 'eq' && f.kind !== 'severity') return fail('bad_operator', `\`${f.key}\` only takes \`:\``);
  const it = items(rest.slice(opLen), true);
  if (!it.ok) return fail(it.code, it.message);
  if (op !== 'eq' && it.values.length > 1) return fail('bad_operator', 'a comparison takes one value');
  const values: string[] = [];
  for (const v of it.values) {
    const n = normalize(f, v);
    if (!n.ok) return fail('bad_value', n.message);
    values.push(n.value);
  }
  return { ok: true, terms: [{ neg, key: f.key, op, values }] };
}

export function parseQuery(q: string, fields: readonly FieldSpec[]): ParseResult {
  const s = scan(q);
  if (!s.ok) return s;
  const terms: Term[] = [];
  for (let i = 0; i < s.tokens.length; i++) {
    const r = parseRaw(s.tokens[i], i, fields);
    if (!r.ok) return r;
    terms.push(...r.terms);
  }
  return { ok: true, terms };
}

/** Parse one chip's raw text as a one-token query. */
export function parseToken(raw: string, fields: readonly FieldSpec[]): ParseResult {
  return parseQuery(raw, fields);
}

/** A value as the grammar must spell it: bare when it can be, else quoted. */
export function quoteValue(v: string): string {
  if (v !== '' && !/[\s,"'\\]/u.test(v)) return v;
  return `"${v.replace(/["\\]/g, (c) => `\\${c}`)}"`;
}
```

  `fields.ts`:

```ts
// The findings query field registry: the grammar spec (key/aliases/kind/
// values, in lockstep with rupu-coverage `FIELDS` via fields.json) plus the
// presentation the QueryBar shows (label, description, lucide icon).
import {
  Bot, Bug, CircleCheck, Crosshair, FileCode, FileText, FolderGit2, Hash, ListChecks, Package,
  Play, ShieldAlert, ShieldCheck, Tag, User, Workflow, type LucideIcon,
} from 'lucide-react';
import type { FieldSpec } from './grammar';

export interface QueryField extends FieldSpec {
  label: string;
  description: string;
  icon: LucideIcon;
}

export const FINDING_FIELDS: QueryField[] = [
  { key: 'severity', aliases: ['sev'], kind: 'severity', values: ['info', 'low', 'medium', 'high', 'critical'], label: 'Severity', description: 'severity:high · severity>=high', icon: ShieldAlert },
  { key: 'tag', aliases: [], kind: 'tag', values: [], label: 'Tag', description: 'tag:needs-poc · -tag:false-positive', icon: Tag },
  { key: 'has', aliases: [], kind: 'enum', values: ['tags', 'report', 'poc', 'cwe'], label: 'Has', description: 'has:poc · -has:tags (untagged)', icon: CircleCheck },
  { key: 'project', aliases: [], kind: 'text', values: [], label: 'Project', description: 'project:shop-web', icon: FolderGit2 },
  { key: 'cwe', aliases: [], kind: 'cwe', values: [], label: 'CWE', description: 'cwe:79 · cwe:CWE-89', icon: Bug },
  { key: 'owner', aliases: [], kind: 'text', values: [], label: 'Owner', description: 'owner:"Payments Team"', icon: User },
  { key: 'product', aliases: [], kind: 'text', values: [], label: 'Product', description: 'product:checkout', icon: Package },
  { key: 'verified', aliases: [], kind: 'enum', values: ['unverified', 'confirmed', 'disputed', 'inconclusive'], label: 'Verified', description: 'verified:confirmed', icon: ShieldCheck },
  { key: 'profile', aliases: [], kind: 'enum', values: ['full', 'summary'], label: 'Profile', description: 'profile:full', icon: FileText },
  { key: 'scope', aliases: [], kind: 'enum', values: ['line', 'file', 'repo', 'host', 'endpoint', 'resource'], label: 'Scope', description: 'scope:host', icon: Crosshair },
  { key: 'concern', aliases: [], kind: 'text', values: [], label: 'Concern', description: 'concern:authz-idor', icon: ListChecks },
  { key: 'agent', aliases: [], kind: 'text', values: [], label: 'Agent', description: 'agent name or codename', icon: Bot },
  { key: 'workflow', aliases: [], kind: 'text', values: [], label: 'Workflow', description: 'workflow:review-flow', icon: Workflow },
  { key: 'file', aliases: [], kind: 'text', values: [], label: 'File', description: 'file:src/api/ (path prefix)', icon: FileCode },
  { key: 'run', aliases: [], kind: 'text', values: [], label: 'Run', description: 'run:<id> (includes its sub-runs)', icon: Play },
  { key: 'id', aliases: [], kind: 'text', values: [], label: 'Finding id', description: 'id:fnd_…', icon: Hash },
];
```

  If any of those lucide icon names is missing in `lucide-react` ^0.468, substitute the nearest existing icon and say so in the report. Check with `grep -o '"<Name>"' node_modules/lucide-react/dist/lucide-react.d.ts` or by importing.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run `cd crates/rupu-cp/web && npx vitest run src/lib/findingQuery`.
  - Expected: PASS, with every fixture case green.
  - If a case differs, compare against the Rust parser. Fixtures win.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/web/src/lib/findingQuery
git commit -m "feat(cp-web): findings query parser twin, lockstep with the Rust fixtures

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Generic suggestion engine (pure TS)

**Files:**
- Create: `crates/rupu-cp/web/src/components/query/suggest.ts`, `crates/rupu-cp/web/src/components/query/suggest.test.ts`

**Interfaces:**
- Consumes:
  - `FieldSpec`, `findField`, `quoteValue` (Task 6)
  - `QueryField` (Task 6), used only as a type with `label`, `description` and `icon`
  - `fuzzyScore` (`src/lib/fuzzy.ts`)
- Produces:
  - `FacetValue { value: string; count: number }`
  - `Suggestion { id: string; kind: 'key' | 'value' | 'text'; label: string; detail?: string; insert: string; commit: boolean; field?: QueryField; value?: string; matched: number[] }`
  - `suggest(draft: string, fields: readonly QueryField[], facets: Record<string, FacetValue[]> | undefined, max = 8): Suggestion[]`

- [ ] **Step 1: Write the failing tests.** `suggest.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { suggest } from './suggest';
import { FINDING_FIELDS } from '../../lib/findingQuery/fields';

const facets = {
  tag: [
    { value: 'class:sqli', count: 5 },
    { value: 'needs-poc', count: 3 },
    { value: 'triaged', count: 1 },
  ],
  severity: [
    { value: 'critical', count: 1 },
    { value: 'high', count: 4 },
    { value: 'medium', count: 0 },
    { value: 'low', count: 2 },
    { value: 'info', count: 0 },
  ],
  owner: [{ value: 'Payments Team', count: 2 }],
};

describe('suggest', () => {
  it('lists keys for an empty draft, completing to key:', () => {
    const s = suggest('', FINDING_FIELDS, facets);
    expect(s[0]).toMatchObject({ kind: 'key', label: 'severity', insert: 'severity:', commit: false });
    expect(s.length).toBe(8);
  });
  it('fuzzy-matches keys and offers a text search', () => {
    const s = suggest('ta', FINDING_FIELDS, facets);
    expect(s[0]).toMatchObject({ kind: 'key', label: 'tag', insert: 'tag:' });
    expect(s.at(-1)).toMatchObject({ kind: 'text', insert: 'ta', commit: true });
  });
  it('suggests values in use for a key, most used first, with counts', () => {
    const s = suggest('tag:', FINDING_FIELDS, facets);
    expect(s.map((x) => x.label)).toEqual(['class:sqli', 'needs-poc', 'triaged']);
    expect(s[0]).toMatchObject({ kind: 'value', detail: '5', insert: 'tag:class:sqli', commit: true });
  });
  it('filters values by the partial after the last comma and keeps negation and op', () => {
    const s = suggest('-tag:class:sqli,ne', FINDING_FIELDS, facets);
    expect(s[0]).toMatchObject({ label: 'needs-poc', insert: '-tag:class:sqli,needs-poc' });
    expect(suggest('severity>=hi', FINDING_FIELDS, facets)[0]).toMatchObject({ insert: 'severity>=high' });
  });
  it('enum fields offer their fixed values even when unused', () => {
    expect(suggest('has:', FINDING_FIELDS, facets).map((x) => x.label)).toEqual(['tags', 'report', 'poc', 'cwe']);
  });
  it('quotes values that need it', () => {
    expect(suggest('owner:pay', FINDING_FIELDS, facets)[0].insert).toBe('owner:"Payments Team"');
  });
  it('an unknown key suggests nothing', () => {
    expect(suggest('nope:', FINDING_FIELDS, facets)).toEqual([]);
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `npx vitest run src/components/query/suggest.test.ts`
  - Expected: FAIL.

- [ ] **Step 3: Implement** `suggest.ts`:

```ts
// What the spotlight dropdown offers for the token being typed (Ghost's
// `graph3d/query.ts` `suggest`, adapted to a registry + server facets):
//   ''            → every key (`key:`)
//   'ta'          → fuzzy-matched keys, then "search for 'ta'" as text
//   'tag:ne'      → values for `tag` in use (facets) or the key's fixed
//                   values, fuzzy-matched on the part after the last comma
import { fuzzyScore } from '../../lib/fuzzy';
import { findField, quoteValue } from '../../lib/findingQuery/grammar';
import type { QueryField } from '../../lib/findingQuery/fields';

export interface FacetValue {
  value: string;
  count: number;
}

export interface Suggestion {
  id: string;
  kind: 'key' | 'value' | 'text';
  label: string;
  detail?: string;
  /** The draft text after accepting this suggestion. */
  insert: string;
  /** Accepting commits `insert` as a chip (else it only replaces the draft). */
  commit: boolean;
  field?: QueryField;
  value?: string;
  /** Matched char indices in `label`, for highlighting. */
  matched: number[];
}

const KEY_OP = /^([A-Za-z_]+)(>=|<=|:|>|<)(.*)$/s;

export function suggest(
  draft: string,
  fields: readonly QueryField[],
  facets: Record<string, FacetValue[]> | undefined,
  max = 8,
): Suggestion[] {
  const neg = draft.length > 1 && draft.startsWith('-');
  const body = neg ? draft.slice(1) : draft;
  const sign = neg ? '-' : '';
  const m = KEY_OP.exec(body);
  if (m) {
    const [, name, op, rest] = m;
    const field = findField(name, fields) as QueryField | undefined;
    if (!field) return [];
    const cut = rest.lastIndexOf(',');
    const prefix = cut >= 0 ? rest.slice(0, cut + 1) : '';
    const partial = cut >= 0 ? rest.slice(cut + 1) : rest;
    const counts = new Map((facets?.[field.key] ?? []).map((f) => [f.value, f.count]));
    const pool = field.values.length > 0 ? field.values : [...counts.keys()];
    return pool
      .map((value) => ({ value, hit: fuzzyScore(partial, value), count: counts.get(value) ?? 0 }))
      .filter((x) => x.hit !== null)
      .sort((a, b) =>
        field.values.length > 0 && partial === ''
          ? 0
          : b.hit!.score - a.hit!.score || b.count - a.count || a.value.localeCompare(b.value),
      )
      .slice(0, max)
      .map((x) => ({
        id: `v:${x.value}`,
        kind: 'value' as const,
        label: x.value,
        detail: counts.has(x.value) ? String(x.count) : undefined,
        insert: `${sign}${field.key}${op}${prefix}${quoteValue(x.value)}`,
        commit: true,
        field,
        value: x.value,
        matched: x.hit!.matched,
      }));
  }
  const keys = fields
    .map((field) => {
      const hits = [field.key, ...field.aliases].map((k) => fuzzyScore(body, k)).filter((h) => h !== null);
      const best = hits.sort((a, b) => b!.score - a!.score)[0];
      return best ? { field, hit: best } : null;
    })
    .filter((x): x is NonNullable<typeof x> => x !== null)
    .sort((a, b) => (body === '' ? 0 : b.hit!.score - a.hit!.score))
    .slice(0, body === '' ? max : max - 1)
    .map(({ field, hit }) => ({
      id: `k:${field.key}`,
      kind: 'key' as const,
      label: field.key,
      detail: field.description,
      insert: `${sign}${field.key}:`,
      commit: false,
      field,
      matched: body === '' ? [] : hit!.matched,
    }));
  if (body === '') return keys;
  return [
    ...keys,
    { id: 'text', kind: 'text', label: `Search “${body}”`, insert: draft, commit: true, matched: [] },
  ];
}
```

  Notes:
  - With an empty draft and an empty partial, the sort comparator returns 0 so the registry/enum order holds. `Array.prototype.sort` is stable.
  - The test expects severity first for `''`; `FINDING_FIELDS` order guarantees it.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run `npx vitest run src/components/query/suggest.test.ts`.
  - Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/web/src/components/query/suggest.ts crates/rupu-cp/web/src/components/query/suggest.test.ts
git commit -m "feat(cp-web): spotlight suggestion engine for query bars

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `QueryBar` — the spotlight bar (generic component)

**Files:**
- Create: `crates/rupu-cp/web/src/components/query/QueryBar.tsx`, `crates/rupu-cp/web/src/components/query/QueryBar.test.tsx`

**Reference:** Ghost's `SpotlightBar` holds the behaviour and polish to copy:
- chip row inside the input
- active-row handling
- `onMouseDown` + `preventDefault` so the input keeps focus
- ARIA combobox/listbox with `aria-activedescendant`
- match highlighting

Read it with `git -C /Users/matt/Security/Ghost show origin/main:crates/ghost-cp/web/src/graph3d/SpotlightBar.tsx` (and `spotlightResults.ts` for chip colors). Port the behaviour; don't copy its graph-specific results list, flow handling or context panel.

**Interfaces:**
- Consumes:
  - `tokenize`, `parseToken`, `FieldSpec` (Task 6)
  - `QueryField` (Task 6)
  - `suggest`, `Suggestion`, `FacetValue` (Task 7)
  - `cn` (`src/lib/cn.ts`)
- Produces:

```ts
export interface QueryBarProps {
  /** The committed query (source of truth, e.g. the `?q=` param). */
  value: string;
  /** Fired with the new committed query when chips change. */
  onChange: (q: string) => void;
  fields: readonly QueryField[];
  facets?: Record<string, FacetValue[]>;
  placeholder?: string;
  /** Tailwind classes for a single-value chip of `key:value` (e.g. severity colors); null = neutral brand chip. */
  valueTone?: (key: string, value: string) => string | null;
  /** Accessible label. */
  label?: string;
}
export function QueryBar(props: QueryBarProps): JSX.Element;
```

- [ ] **Step 1: Write the failing tests.** `QueryBar.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { QueryBar } from './QueryBar';
import { FINDING_FIELDS } from '../../lib/findingQuery/fields';

const facets = { tag: [{ value: 'needs-poc', count: 3 }, { value: 'class:sqli', count: 2 }] };

function Harness({ initial = '', spy = vi.fn() }: { initial?: string; spy?: (q: string) => void }) {
  const [q, setQ] = useState(initial);
  return (
    <QueryBar
      value={q}
      onChange={(next) => {
        spy(next);
        setQ(next);
      }}
      fields={FINDING_FIELDS}
      facets={facets}
      label="Filter findings"
    />
  );
}

const input = () => screen.getByRole('combobox', { name: 'Filter findings' });

describe('QueryBar', () => {
  it('renders committed tokens as chips', () => {
    render(<Harness initial="severity>=high -tag:noise" />);
    expect(screen.getByText('severity ≥ high')).toBeInTheDocument();
    expect(screen.getByText('not tag: noise')).toBeInTheDocument();
  });

  it('accepts a key then a value from the spotlight with the keyboard', () => {
    const spy = vi.fn();
    render(<Harness spy={spy} />);
    fireEvent.focus(input());
    fireEvent.change(input(), { target: { value: 'ta' } });
    fireEvent.keyDown(input(), { key: 'ArrowDown' });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(input()).toHaveValue('tag:');
    fireEvent.keyDown(input(), { key: 'ArrowDown' });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(spy).toHaveBeenLastCalledWith('tag:needs-poc');
    expect(input()).toHaveValue('');
  });

  it('ArrowUp from no selection wraps to the last option', () => {
    render(<Harness />);
    fireEvent.change(input(), { target: { value: 'tag:' } });
    fireEvent.keyDown(input(), { key: 'ArrowUp' });
    const opts = screen.getAllByRole('option');
    expect(opts[opts.length - 1]).toHaveAttribute('aria-selected', 'true');
  });

  it('commits typed free text on Enter', () => {
    const spy = vi.fn();
    render(<Harness spy={spy} />);
    fireEvent.change(input(), { target: { value: 'sql' } });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(spy).toHaveBeenLastCalledWith('sql');
  });

  it('refuses an invalid draft and shows why', () => {
    const spy = vi.fn();
    render(<Harness spy={spy} />);
    fireEvent.change(input(), { target: { value: 'sevrity:high' } });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(spy).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('unknown key `sevrity`');
  });

  it('Backspace on an empty draft pops the last chip back into the draft', () => {
    const spy = vi.fn();
    render(<Harness initial="tag:a tag:b" spy={spy} />);
    fireEvent.keyDown(input(), { key: 'Backspace' });
    expect(spy).toHaveBeenLastCalledWith('tag:a');
    expect(input()).toHaveValue('tag:b');
  });

  it('removes a chip with its ✕ button', () => {
    const spy = vi.fn();
    render(<Harness initial="tag:a tag:b" spy={spy} />);
    fireEvent.click(screen.getByRole('button', { name: 'Remove tag: a' }));
    expect(spy).toHaveBeenLastCalledWith('tag:b');
  });

  it('marks an invalid chip from the URL in red with its reason', () => {
    render(<Harness initial="sevrity:high" />);
    expect(screen.getByTitle(/unknown key/)).toHaveClass('ring-red-500/40');
  });

  it('/ focuses the bar from anywhere outside an editable field', () => {
    render(<Harness />);
    fireEvent.keyDown(document.body, { key: '/' });
    expect(input()).toHaveFocus();
  });
});
```

  **Active-row rule (binding):** no option is active until ↑/↓ is pressed, including right after a key suggestion is accepted. So Enter on a fresh draft commits the draft itself, and Enter after ↑/↓ accepts the highlighted option. `tag:` values are ordered by facet count descending, so `needs-poc` (3) comes before `class:sqli` (2).

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `npx vitest run src/components/query/QueryBar.test.tsx`
  - Expected: FAIL. The module isn't found.

- [ ] **Step 3: Implement `QueryBar.tsx`.**

```tsx
// Single-line spotlight query bar (ported from Ghost's graph3d/SpotlightBar):
// committed tokens render as chips inside the input row; the draft token gets
// a fuzzy spotlight dropdown of keys and in-use values (icons, colors,
// counts). Generic over a field registry — the view supplies `fields`,
// `facets` (values in use) and optional per-value chip tones.
//
// Keys: ↑/↓ move · Enter/Tab accept (Enter with no active row commits the
// draft) · Escape clears the draft, then closes · Backspace on an empty draft
// pops the last chip back into the draft · `/` focuses from anywhere.
import { Search, X } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { cn } from '../../lib/cn';
import { parseToken, tokenize, type Term } from '../../lib/findingQuery/grammar';
import type { QueryField } from '../../lib/findingQuery/fields';
import { suggest, type FacetValue, type Suggestion } from './suggest';

export interface QueryBarProps {
  value: string;
  onChange: (q: string) => void;
  fields: readonly QueryField[];
  facets?: Record<string, FacetValue[]>;
  placeholder?: string;
  valueTone?: (key: string, value: string) => string | null;
  label?: string;
}

const OP_LABEL: Record<Term['op'], string> = { eq: ':', gt: ' >', ge: ' ≥', lt: ' <', le: ' ≤' };

function chipText(t: Term): string {
  const head = t.key === 'text' ? '' : `${t.key}${OP_LABEL[t.op]} `;
  return `${t.neg ? 'not ' : ''}${head}${t.values.join(', ')}`.trim();
}

function Highlight({ text, matched }: { text: string; matched: number[] }) {
  if (matched.length === 0) return <>{text}</>;
  const set = new Set(matched);
  return (
    <>
      {Array.from(text).map((c, i) =>
        set.has(i) ? (
          <span key={i} className="font-semibold text-brand-600">
            {c}
          </span>
        ) : (
          <span key={i}>{c}</span>
        ),
      )}
    </>
  );
}

export function QueryBar({ value, onChange, fields, facets, placeholder, valueTone, label = 'Query' }: QueryBarProps) {
  const listId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState('');
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [error, setError] = useState<string | null>(null);

  const tokens = useMemo(() => {
    const t = tokenize(value);
    return t.ok ? t.tokens.map((x) => x.text) : value.split(/\s+/).filter(Boolean);
  }, [value]);
  const chips = useMemo(
    () => tokens.map((raw) => ({ raw, parsed: parseToken(raw, fields) })),
    [tokens, fields],
  );
  const options = useMemo(() => (open ? suggest(draft, fields, facets) : []), [open, draft, fields, facets]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== '/' || e.metaKey || e.ctrlKey || e.altKey) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName))) return;
      e.preventDefault();
      inputRef.current?.focus();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  const commit = (next: string[]) => {
    onChange(next.join(' '));
  };

  const commitDraft = (text: string) => {
    const raw = text.trim();
    if (raw === '') return;
    const p = parseToken(raw, fields);
    if (!p.ok) {
      setError(p.error.message);
      return;
    }
    const t = tokenize(raw);
    commit([...tokens, ...(t.ok ? t.tokens.map((x) => x.text) : [raw])]);
    setDraft('');
    setError(null);
    setActive(-1);
  };

  const accept = (s: Suggestion) => {
    if (s.commit) commitDraft(s.insert);
    else {
      setDraft(s.insert);
      setActive(-1);
      setError(null);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      setOpen(true);
      if (options.length === 0) return;
      const step = e.key === 'ArrowDown' ? 1 : -1;
      setActive((i) => (i + step + options.length + (i < 0 && step < 0 ? 1 : 0)) % options.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (active >= 0 && options[active]) accept(options[active]);
      else commitDraft(draft);
    } else if (e.key === 'Tab' && open && active >= 0 && options[active]) {
      e.preventDefault();
      accept(options[active]);
    } else if (e.key === 'Escape') {
      if (draft) {
        setDraft('');
        setError(null);
      } else {
        setOpen(false);
        inputRef.current?.blur();
      }
      setActive(-1);
    } else if (e.key === 'Backspace' && draft === '' && tokens.length > 0) {
      e.preventDefault();
      setDraft(tokens[tokens.length - 1]);
      commit(tokens.slice(0, -1));
    }
  };

  return (
    <div className="relative" onBlur={(e) => !e.currentTarget.contains(e.relatedTarget) && setOpen(false)}>
      <div
        className={cn(
          'flex flex-wrap items-center gap-1.5 rounded-xl border border-border bg-panel px-2.5 py-1.5 shadow-card',
          'focus-within:ring-2 focus-within:ring-brand-500/50',
        )}
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) {
            e.preventDefault();
            inputRef.current?.focus();
          }
        }}
      >
        <Search size={14} className="shrink-0 text-ink-mute" aria-hidden />
        {chips.map(({ raw, parsed }, i) => {
          const term = parsed.ok ? parsed.terms[0] : null;
          const field = term ? fields.find((f) => f.key === term.key) : undefined;
          const Icon = field?.icon ?? Search;
          const tone = term && term.values.length === 1 && !term.neg ? valueTone?.(term.key, term.values[0]) : null;
          const text = term ? chipText(term) : raw;
          return (
            <span
              key={`${i}:${raw}`}
              title={parsed.ok ? raw : parsed.error.message}
              className={cn(
                'inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 font-mono text-note ring-1',
                !parsed.ok
                  ? 'bg-red-500/10 text-red-700 ring-red-500/40'
                  : tone ?? 'bg-brand-500/10 text-brand-700 ring-brand-500/30',
              )}
            >
              <Icon size={12} aria-hidden />
              <span>{text}</span>
              <button
                type="button"
                aria-label={`Remove ${term ? chipText(term).replace(/^not /, '') : raw}`}
                className="rounded text-ink-mute hover:text-ink"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => commit(tokens.filter((_, j) => j !== i))}
              >
                <X size={11} />
              </button>
            </span>
          );
        })}
        <input
          ref={inputRef}
          role="combobox"
          aria-label={label}
          aria-expanded={open && options.length > 0}
          aria-controls={listId}
          aria-activedescendant={active >= 0 ? `${listId}-${active}` : undefined}
          aria-invalid={error ? true : undefined}
          value={draft}
          placeholder={tokens.length === 0 ? (placeholder ?? 'Filter… e.g. severity>=high tag:needs-poc') : ''}
          onFocus={() => setOpen(true)}
          onChange={(e) => {
            setDraft(e.target.value);
            setOpen(true);
            setActive(-1);
            setError(null);
          }}
          onKeyDown={onKeyDown}
          className="min-w-[12rem] flex-1 bg-transparent py-0.5 text-ui text-ink outline-none placeholder:text-ink-mute"
        />
      </div>
      {error && (
        <p role="alert" className="mt-1 text-note text-red-700">
          {error}
        </p>
      )}
      {open && options.length > 0 && (
        <ul
          id={listId}
          role="listbox"
          className="absolute z-30 mt-1 max-h-80 w-full overflow-auto rounded-xl border border-border bg-panel py-1 shadow-card"
        >
          {options.map((s, i) => {
            const Icon = s.field?.icon ?? Search;
            const tone = s.kind === 'value' && s.field && s.value ? valueTone?.(s.field.key, s.value) : null;
            return (
              <li
                key={s.id}
                id={`${listId}-${i}`}
                role="option"
                aria-selected={i === active}
                onMouseDown={(e) => e.preventDefault()}
                onMouseEnter={() => setActive(i)}
                onClick={() => accept(s)}
                className={cn(
                  'flex cursor-pointer items-center gap-2 px-3 py-1.5 text-ui',
                  i === active ? 'bg-surface-active text-ink' : 'text-ink-dim hover:bg-surface-hover',
                )}
              >
                <span className={cn('flex h-5 w-5 items-center justify-center rounded', tone ?? 'text-ink-mute')}>
                  <Icon size={13} aria-hidden />
                </span>
                <span className="font-mono">
                  <Highlight text={s.label} matched={s.matched} />
                </span>
                {s.detail && <span className="ml-auto truncate text-note text-ink-mute">{s.detail}</span>}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

export default QueryBar;
```

  - If `bg-surface-active` is not a token in rupu's `tailwind.config.ts`, use `bg-surface`. Check with `grep -n "surface" crates/rupu-cp/web/tailwind.config.ts`.
  - If `red-*` isn't used elsewhere, use the existing error color tokens that `ErrorBanner` uses. Keep the test's class assertion consistent with your choice.
  - The `ArrowUp` wrap from −1 lands on the last row; the test `ArrowUp from no selection wraps to the last option` covers it.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run `npx vitest run src/components/query`.
  - Then run `npx tsc -b --noEmit`, or the project's `tsc -b` through `npm run build` if `--noEmit` isn't supported with build mode. Expected: no type errors.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/web/src/components/query
git commit -m "feat(cp-web): QueryBar — spotlight single-line query with chips

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Findings page on the query bar + Tags column + unreadable-tags banner

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`:
  - `FindingRecord` gets `tags?: string[]`.
  - `FindingsResponse` gets `facets: Record<string, { value: string; count: number }[]>` and `tags_unavailable: string[]`.
  - `getFindings` opts get `q?: string`.
- Modify: `crates/rupu-cp/web/src/pages/Findings.tsx`, a rewrite of its filter section.
- Modify: `crates/rupu-cp/web/src/components/findings/FindingsTable.tsx`, which gets the Tags column.
- Test: `crates/rupu-cp/web/src/pages/Findings.test.tsx` (update), `crates/rupu-cp/web/src/components/findings/FindingsTable.tags.test.tsx` (new).

**Interfaces:**
- Consumes:
  - `QueryBar` (Task 8)
  - `FINDING_FIELDS`, `tokenize`, `parseQuery` (Task 6)
  - the `facets` and `tags_unavailable` API fields (Task 5)
  - `SEVERITY_STYLE` (`lib/severity.ts`)
- Produces: the Findings page shows:
  - the query bar, with the query in `?q=` (other search params preserved)
  - severity tiles driven by `facets.severity` that toggle `severity:<x>`
  - a `tags_unavailable` warning banner
  - a Tags column (read-only)

- [ ] **Step 1: Write the failing tests.**

`FindingsTable.tags.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { FindingsTable } from './FindingsTable';
import type { FindingOut } from '../../lib/api';

function row(id: string, tags: string[]): FindingOut {
  return {
    id,
    scope: 'repo',
    summary: `summary ${id}`,
    severity: 'high',
    evidence: { rationale: 'r', references: [] },
    declared_by: { run_id: 'r', model: 'm', surface: 'workflow' },
    declared_at: '2026-10-06T00:00:00Z',
    tags,
    ws_id: 'ws1',
    project: 'shop-web',
    target_id: 't',
    codename: 'jade-reef',
    codename_derived: true,
  } as unknown as FindingOut;
}

describe('FindingsTable tags column', () => {
  it('shows up to two tags then +N with the full list as a title', () => {
    render(
      <MemoryRouter>
        <FindingsTable findings={[row('fnd_a', ['class:sqli', 'needs-poc', 'triaged']), row('fnd_b', [])]} showProvenance />
      </MemoryRouter>,
    );
    expect(screen.getByText('class:sqli')).toBeInTheDocument();
    expect(screen.getByText('needs-poc')).toBeInTheDocument();
    expect(screen.queryByText('triaged')).not.toBeInTheDocument();
    expect(screen.getByText('+1')).toHaveAttribute('title', 'class:sqli, needs-poc, triaged');
  });
});
```

  Look at the existing `FindingsTable` test files for the exact `FindingOut` fixture shape and copy it instead of the cast if the cast fails type-checking.

  Update `Findings.test.tsx`, which already spies `api.getFindings`. Replace the profile/owner/CWE filter tests with:
  - **q reaches the server.** Render at route `/security?tab=findings&q=tag%3Aneeds-poc` (use `MemoryRouter initialEntries`). Assert `getFindings` was called with `{ q: 'tag:needs-poc' }`.
  - **Severity tile toggles a token.** Click the "High" severity tile. Assert the next `getFindings` call has `q` containing `severity:high`. Clicking it again removes it.
  - **Banner.** Mock a response with `tags_unavailable: ['ws1']` and a finding from `ws1`, project `billing-api`. Assert a banner containing "billing-api" and "couldn't be read".
  - **Local parse errors.** With `?q=sevrity%3Ax`, `getFindings` is NOT called and the bar shows the chip error. (The local parse fails, so the page doesn't fetch.)
  - **Empty result.** When the response has zero findings and `q` is non-empty, the page shows "No findings match" with the query.

  Write these with the file's existing spy and render helpers. Each mocked response must now include `facets` and `tags_unavailable`.

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `npx vitest run src/pages/Findings.test.tsx src/components/findings/FindingsTable.tags.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**
  - **`api.ts`:** the field additions above. `getFindings(opts?: { wsId?: string; workflow?: string; runId?: string; q?: string })` sets `q` when non-empty.
  - **`FindingsTable.tsx`:** add a column after the Summary column:

```tsx
    {
      key: 'tags',
      header: 'Tags',
      fit: true,
      render: (f) => {
        const tags = f.tags ?? [];
        if (tags.length === 0) return null;
        return (
          <span className="flex items-center gap-1">
            {tags.slice(0, 2).map((t) => (
              <span key={t} className="rounded bg-surface px-1.5 py-0.5 font-mono text-note text-ink ring-1 ring-border">
                {t}
              </span>
            ))}
            {tags.length > 2 && (
              <span title={tags.join(', ')} className="text-note text-ink-mute">
                +{tags.length - 2}
              </span>
            )}
          </span>
        );
      },
    },
```

  - **`Findings.tsx`:** rewrite the state and filter section. Keep the header, spinner, empty state, `FindingMetrics`, `ExportReportButton` and `FindingsTable`.
    - Remove the `profile`, `owner`, `cwe` and `activeSev` state, the `FilterBar`/`FilterPills`/`Select` imports, and the client-side `rows` filter.
    - Add:

```tsx
import { useSearchParams } from 'react-router-dom';
import { QueryBar } from '../components/query/QueryBar';
import { FINDING_FIELDS } from '../lib/findingQuery/fields';
import { parseQuery, tokenize } from '../lib/findingQuery/grammar';
import { apiErrorMessage } from '../lib/api';
import { SEVERITY_STYLE, type Severity } from '../lib/severity';

const SEVS: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

function sevTone(key: string, value: string): string | null {
  return key === 'severity' && (SEVS as string[]).includes(value) ? SEVERITY_STYLE[value as Severity].pill : null;
}

/** The severity a lone, positive `severity:<x>` token selects (the active tile). */
function activeSeverity(q: string): Severity | null {
  const p = parseQuery(q, FINDING_FIELDS);
  if (!p.ok) return null;
  const sev = p.terms.filter((t) => t.key === 'severity');
  return sev.length === 1 && !sev[0].neg && sev[0].op === 'eq' && sev[0].values.length === 1
    ? (sev[0].values[0] as Severity)
    : null;
}

/** `q` with every severity token removed, then `severity:<sev>` added (null = none). */
function withSeverity(q: string, sev: Severity | null): string {
  const t = tokenize(q);
  const kept = (t.ok ? t.tokens.map((x) => x.text) : [])
    .filter((raw) => {
      const p = parseQuery(raw, FINDING_FIELDS);
      return !(p.ok && p.terms[0]?.key === 'severity');
    });
  return [...kept, ...(sev ? [`severity:${sev}`] : [])].join(' ');
}
```

    - Inside the component:

```tsx
  const [params, setParams] = useSearchParams();
  const q = params.get('q') ?? '';
  const setQ = (next: string) =>
    setParams(
      (prev) => {
        const p = new URLSearchParams(prev);
        if (next.trim()) p.set('q', next);
        else p.delete('q');
        return p;
      },
      { replace: true },
    );
  const localError = useMemo(() => {
    const p = parseQuery(q, FINDING_FIELDS);
    return p.ok ? null : p.error.message;
  }, [q]);
  const [data, setData] = useState<FindingsResponse | null>(null);

  useEffect(() => {
    if (localError) return;
    let cancelled = false;
    setError(null);
    api
      .getFindings(q ? { q } : undefined)
      .then((d) => !cancelled && setData(d))
      .catch((e: unknown) => !cancelled && setError(apiErrorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [q, localError]);
```

      A `getFindings` call with no `q` must stay `getFindings()` with no args, so the existing tests that assert `toHaveBeenCalledWith()` still hold. If the existing tests assert differently, follow them.
    - Derive the tiles' summary from `data.facets.severity`:

```tsx
  const tileSummary: FindingsSummary = useMemo(() => {
    const counts = Object.fromEntries((data?.facets.severity ?? []).map((v) => [v.value, v.count]));
    const by = (s: Severity) => counts[s] ?? 0;
    return { total: SEVS.reduce((n, s) => n + by(s), 0), critical: by('critical'), high: by('high'), medium: by('medium'), low: by('low'), info: by('info') };
  }, [data]);
```

    - **Render:**
      - `FindingMetrics summary={tileSummary} active={activeSeverity(q)} onSelect={(sev) => setQ(withSeverity(q, sev === activeSeverity(q) ? null : sev))}`
      - then a row holding `<QueryBar value={q} onChange={setQ} fields={FINDING_FIELDS} facets={data?.facets} valueTone={sevTone} label="Filter findings" />` (`flex-1`) and `ExportReportButton findings={data.findings} …`
      - then the banner, when `data.tags_unavailable.length > 0`:

```tsx
        <div role="status" className="rounded-lg bg-amber-500/10 px-3 py-2 text-note text-amber-800 ring-1 ring-amber-500/30">
          Tags for {names.join(', ')} couldn't be read, so tag filters may miss their findings.
        </div>
```

        Here `names` maps each ws id to the `project` of a row with that `ws_id`, falling back to the ws id. Use the amber/warning tokens the CP already uses (grep for an existing warning banner, e.g. `ErrorBanner`'s sibling or "amber" in `components/ui`) instead of raw `amber-*` if the repo has them.
      - then the table: `findings.length === 0 && q ? <EmptyState title="No matches" hint={`No findings match \`${q}\`.`} /> : <FindingsTable findings={data.findings} showProvenance />`.
    - The page's "no findings at all" empty state shows only when `q` is empty and `facets.severity` totals 0.
    - Update the header blurb: "Every finding raised across all registered projects. Filter with the query bar, e.g. `severity>=high tag:needs-poc` — press / to focus."

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run `npx vitest run src/pages/Findings.test.tsx src/components/findings src/components/query src/lib/findingQuery`.
  - Then run `npm run build`. Expected: `tsc -b` and `vite build` succeed.
  - Then run the existing findings-adjacent suites that render `FindingsTable` (`npx vitest run src/components/findings`), since the new column must not break them.

- [ ] **Step 5: Commit.** Do NOT commit `web/dist` unless the repo tracks it. Check `git status`; if `dist` shows as modified and is tracked, ask before committing it.

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): findings query bar, Tags column, unreadable-tags banner

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Docs

**Files:**
- Modify: `docs/coverage.md`. Replace the Plan 1 CLI filter examples in "Tagging findings" and add a "Querying findings" section with:
  - the grammar
  - the field table
  - examples per surface (CLI, agent `q`, MCP, the CP bar with `/`, `?q=` links)
  - the "quote `>`/`<` for your shell" note
- Modify: `CLAUDE.md`. In the `rupu-coverage` entry, add one sentence on `ledger::query_lang` (parser) plus `ledger::finding_filter` (the one evaluator), with the TS twin `web/src/lib/findingQuery/` kept in lockstep by `tests/fixtures/finding_query/`. In the `rupu-cp` entry, add one clause: `GET /api/findings?q=` with `facets` and `tags_unavailable`, and a 400 `{error, token, code, start, end}`.
- Modify: the spec. Mark Plan 2 complete in the "Query language" section's Plans list, with this plan's path.

- [ ] **Step 1: Write the docs.** Every flag, key and example must match the code; verify against `cmd/findings.rs`, `query_lang.rs` and `fields.ts`. Use invented examples only.
- [ ] **Step 2: Commit.**

```bash
git add docs/coverage.md CLAUDE.md docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md
git commit -m "docs: the findings query language

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Final verification (after Task 10)

- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] clippy `-D warnings` is clean on rupu-coverage, rupu-findings-report, rupu-agent, rupu-mcp, rupu-cp and rupu-cli.
- [ ] Targeted Rust suites pass:
  - `cargo test -p rupu-coverage --lib`
  - `cargo test -p rupu-coverage --test it`
  - `cargo test -p rupu-findings-report --test it`
  - `cargo test -p rupu-agent --test it findings_`
  - `cargo test -p rupu-mcp --test it`
  - `cargo test -p rupu-cli --test it findings_`
  - `cargo test -p rupu-cp --test it finding_artifacts::`
  - `cargo test -p rupu-cp --lib api::findings`
- [ ] Web: `cd crates/rupu-cp/web && npx vitest run src/lib/findingQuery src/components/query src/components/findings src/pages/Findings.test.tsx && npm run build`.
- [ ] Visual check for matt (the GUI validation rule):
  - Run a local `rupu cp serve` built from this branch against a scratch `RUPU_HOME` seeded with invented findings.
  - Open Security → Findings in the browser pane.
  - Screenshot the bar with chips, the open spotlight dropdown, and an error chip, and show them to matt before the PR merges.
