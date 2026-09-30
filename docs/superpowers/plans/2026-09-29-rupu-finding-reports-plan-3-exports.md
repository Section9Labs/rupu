# Finding reports — Plan 3: Markdown / HTML / PDF exports

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** rupu can generate each finding as a standalone Markdown, HTML, or PDF document, and can generate a project report covering all findings or a chosen subset. Both are available from the CLI (`rupu findings export`) and from the control plane (download buttons on a finding plus an export dialog on the findings lists).

**Architecture:** A new pure crate, `rupu-findings-report`, does no I/O. It turns a finding into a list of typed `Block`s once, following the section order of the reporting standard. Three small emitters then turn those blocks into Markdown, HTML and Typst markup. The Typst markup is compiled to PDF in process, using an embedded `typst::World` with the fonts shipped in `typst-assets`. Markdown prose inside reports goes through one parser, `pulldown-cmark`, with two safe emitters: HTML with raw HTML neutralised, and Typst where all text is inserted as string literals. The CP and CLI share one selection-and-numbering function.

**Tech Stack:** Rust. New dependencies: `typst`, `typst-pdf`, `typst-layout`, `typst-assets` (feature `fonts`) 0.15.1, `pulldown-cmark` 0.13, `zip` (stable major). Also axum, React + TS.

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md` (§ Export generation, § CP API). Builds on Plans 1 and 2.

## Global Constraints

- **The macOS app is out of scope.** Write no Swift. Run `make macos-fixtures` only if a CP serde shape the fixtures cover changes.
- Workspace deps only. Add every new dependency to the root `Cargo.toml` `[workspace.dependencies]` with an exact version, and use `x.workspace = true` in crates. Typst crates are pinned to `0.15.1`, which is verified to build with the workspace toolchain (1.95; typst needs ≥ 1.92).
- `#![deny(clippy::all)]`; `unsafe_code` forbidden. `rupu-findings-report` must stay pure: no filesystem, network or clock access. The caller passes `generated_at`.
- **Output safety:**
  - HTML exports escape every value and neutralise raw HTML and `javascript:` links in Markdown prose.
  - Typst output inserts every dynamic string as a Typst string literal (`#"…"`) or `raw(…)` argument, never as raw markup.
  - The CP serves every export with `Content-Disposition: attachment` (HTML is never rendered inline on the CP origin) and `X-Content-Type-Options: nosniff`.
- **Numbering:** findings get display numbers `<prefix>-NNN` (3 digits, 1-based) per project. The order is severity (critical first), then `declared_at` ascending, then id. Numbers are assigned over all of the project's findings (full and summary), so the same finding gets the same number in a single export and in any project report. The prefix comes from `[findings].export_id_prefix` (default `SEC`).
- **Filename:** `<NUMBER> - <Short Title>.<ext>`. Strip `/\:*?"<>|` and control characters from the title, collapse whitespace, and truncate it to 80 characters.
- Only run rustfmt on leaf files you change; never on `lib.rs`/`mod.rs`/`main.rs`; never `cargo fmt`.
- Rebase on `origin/main` before each task. Use a feature branch plus PR.

---

### Task 1: Crate scaffold, model, numbering, filenames, prose converters

**Files:**
- Create: `crates/rupu-findings-report/Cargo.toml`, `src/lib.rs`, `src/model.rs`, `src/number.rs`, `src/prose.rs`
- Modify: root `Cargo.toml` (workspace member + deps `pulldown-cmark = "0.13.4"`, and `rupu-findings-report = { path = "crates/rupu-findings-report" }` if internal crates are listed there; follow how other internal crates are referenced)

**Interfaces:**
- Produces:
  - `model::ExportInput { pub ws_id: String, pub project: String, pub workflow_name: Option<String>, pub record: rupu_coverage::FindingRecord }`
  - `model::ExportFinding { pub number: String, pub input: ExportInput }`
  - `model::ReportMeta { pub title: String, pub generated_at: chrono::DateTime<chrono::Utc>, pub scope: String }`
  - `number::assign_numbers(all: Vec<ExportInput>, prefix: &str) -> Vec<ExportFinding>` (per-project numbering, sorted by severity then declared_at then id)
  - `number::number_map(findings: &[ExportFinding]) -> HashMap<String, String>` (fnd id → number)
  - `number::filename(f: &ExportFinding, ext: &str) -> String`
  - `prose::md_to_html(md: &str) -> String`
  - `prose::md_to_typst(md: &str) -> String`
  - `prose::typst_str(s: &str) -> String` (a quoted Typst string literal)

- [ ] **Step 1: Scaffold the crate.** `Cargo.toml`:

```toml
[package]
name = "rupu-findings-report"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
rupu-coverage = { path = "../rupu-coverage" }
serde_json.workspace = true
chrono.workspace = true
thiserror.workspace = true
pulldown-cmark.workspace = true

[dev-dependencies]
```

(The `typst*` and `zip` deps are added in Task 4.) Start `src/lib.rs` with:

```rust
//! Pure renderers for finding reports: Markdown, HTML, and PDF (via Typst).
//! No I/O — callers supply findings, metadata and the timestamp.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod model;
pub mod number;
pub mod prose;
```

Add `"crates/rupu-findings-report"` to the workspace `members` list.

- [ ] **Step 2: Write the failing tests.**

`src/number.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExportInput;
    use rupu_coverage::{Attribution, FindingEvidence, FindingProfile, FindingRecord, FindingScope, Severity, Surface};

    fn rec(id: &str, sev: Severity, at: &str) -> FindingRecord {
        FindingRecord {
            id: id.into(), file_path: None, line_range: None, target_ref: None, scope: FindingScope::Repo,
            summary: format!("Summary of {id}"), severity: sev, concern_id: None,
            evidence: FindingEvidence { code_excerpt: None, rationale: "r".into(), references: vec![] },
            declared_by: Attribution { run_id: "run_1".into(), model: "m".into(), surface: Surface::Workflow },
            declared_at: at.parse().unwrap(), profile: FindingProfile::Summary, report: None,
        }
    }
    fn input(ws: &str, r: FindingRecord) -> ExportInput {
        ExportInput { ws_id: ws.into(), project: ws.into(), workflow_name: None, record: r }
    }

    #[test]
    fn numbers_per_project_by_severity_then_time() {
        let out = assign_numbers(vec![
            input("a", rec("fnd_low", Severity::Low, "2026-01-01T00:00:00Z")),
            input("a", rec("fnd_crit_late", Severity::Critical, "2026-02-01T00:00:00Z")),
            input("a", rec("fnd_crit_early", Severity::Critical, "2026-01-15T00:00:00Z")),
            input("b", rec("fnd_b", Severity::High, "2026-01-01T00:00:00Z")),
        ], "SEC");
        let m = number_map(&out);
        assert_eq!(m["fnd_crit_early"], "SEC-001");
        assert_eq!(m["fnd_crit_late"], "SEC-002");
        assert_eq!(m["fnd_low"], "SEC-003");
        assert_eq!(m["fnd_b"], "SEC-001");
    }

    #[test]
    fn filename_sanitizes_title() {
        let mut r = rec("fnd_x", Severity::High, "2026-01-01T00:00:00Z");
        r.summary = "Path: a/b\\c *weird* \"quoted\"   title".into();
        let f = assign_numbers(vec![input("a", r)], "SEC").remove(0);
        assert_eq!(filename(&f, "pdf"), "SEC-001 - Path ab c weird quoted title.pdf");
    }

    #[test]
    fn filename_truncates_long_titles() {
        let mut r = rec("fnd_x", Severity::High, "2026-01-01T00:00:00Z");
        r.summary = "x".repeat(200);
        let f = assign_numbers(vec![input("a", r)], "SEC").remove(0);
        assert_eq!(filename(&f, "md"), format!("SEC-001 - {}.md", "x".repeat(80)));
    }
}
```

`src/prose.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escapes_raw_html_and_drops_js_links() {
        let h = md_to_html("<script>alert(1)</script> and [x](javascript:alert(1)) and `a<b`");
        assert!(!h.contains("<script>"), "{h}");
        assert!(h.contains("&lt;script&gt;"), "{h}");
        assert!(!h.contains("javascript:"), "{h}");
        assert!(h.contains("<code>a&lt;b</code>"), "{h}");
    }

    #[test]
    fn typst_string_escapes() {
        assert_eq!(typst_str(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(typst_str("x\ny"), r#""x\ny""#);
    }

    #[test]
    fn typst_never_emits_raw_markup_from_text() {
        let t = md_to_typst("Use #set and *stars* and $math$ and <label> and @ref\n\n```sh\nrm -rf /\n```");
        // every prose run is a string literal; the fenced block is raw()
        assert!(t.contains(r##"#"Use #set and ""##), "{t}");
        assert!(t.contains("#emph["), "{t}");
        assert!(t.contains(r#"#raw(block: true, lang: "sh", "rm -rf /\n")"#), "{t}");
        assert!(!t.contains("\n#set"), "{t}");
    }

    #[test]
    fn typst_lists_and_code_spans() {
        let t = md_to_typst("- one `x`\n- two");
        assert!(t.contains("\n- #\"one \"#raw(\"x\")"), "{t}");
        assert!(t.contains("\n- #\"two\""), "{t}");
    }
}
```

- [ ] **Step 3: Run and check they fail**

Run: `cargo test -p rupu-findings-report`
Expected: compile errors (missing items).

- [ ] **Step 4: Implement.**

`src/model.rs`:

```rust
use chrono::{DateTime, Utc};
use rupu_coverage::FindingRecord;

/// One finding as the caller collected it.
#[derive(Debug, Clone)]
pub struct ExportInput {
    pub ws_id: String,
    pub project: String,
    pub workflow_name: Option<String>,
    pub record: FindingRecord,
}

/// A finding with its display number (e.g. `SEC-004`).
#[derive(Debug, Clone)]
pub struct ExportFinding {
    pub number: String,
    pub input: ExportInput,
}

#[derive(Debug, Clone)]
pub struct ReportMeta {
    pub title: String,
    pub generated_at: DateTime<Utc>,
    /// Human description of what was selected, e.g. "Project notebin · severity ≥ high".
    pub scope: String,
}
```

`src/number.rs` (above the tests):

```rust
use crate::model::{ExportFinding, ExportInput};
use rupu_coverage::Severity;
use std::collections::{BTreeMap, HashMap};

fn rank(s: Severity) -> u8 {
    match s {
        Severity::Critical => 0,
        Severity::High => 1,
        Severity::Medium => 2,
        Severity::Low => 3,
        Severity::Info => 4,
    }
}

/// Number findings per project: severity (critical first), then declared_at
/// ascending, then id. Output is grouped by project (ws_id order) and in
/// number order within each project.
pub fn assign_numbers(all: Vec<ExportInput>, prefix: &str) -> Vec<ExportFinding> {
    let mut by_ws: BTreeMap<String, Vec<ExportInput>> = BTreeMap::new();
    for i in all {
        by_ws.entry(i.ws_id.clone()).or_default().push(i);
    }
    let mut out = Vec::new();
    for (_, mut items) in by_ws {
        items.sort_by(|a, b| {
            rank(a.record.severity)
                .cmp(&rank(b.record.severity))
                .then(a.record.declared_at.cmp(&b.record.declared_at))
                .then(a.record.id.cmp(&b.record.id))
        });
        for (n, input) in items.into_iter().enumerate() {
            out.push(ExportFinding { number: format!("{prefix}-{:03}", n + 1), input });
        }
    }
    out
}

pub fn number_map(findings: &[ExportFinding]) -> HashMap<String, String> {
    findings.iter().map(|f| (f.input.record.id.clone(), f.number.clone())).collect()
}

/// The finding's display title: the report title for full findings, the
/// summary otherwise.
pub fn title(f: &ExportFinding) -> &str {
    f.input.record.report.as_ref().map(|r| r.title.as_str()).unwrap_or(&f.input.record.summary)
}

pub fn filename(f: &ExportFinding, ext: &str) -> String {
    let cleaned: String = title(f)
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = collapsed.chars().take(80).collect();
    format!("{} - {}.{ext}", f.number, short.trim_end())
}
```

(The Severity variant names must match `rupu_coverage::Severity`; check `catalog/types.rs`. If the `rec` helper's struct literal differs from the real `FindingRecord` fields, for example because Plan 1's final fix added a field, add the missing fields.)

`src/prose.rs` (above the tests):

```rust
//! Agent-written Markdown → safe HTML / safe Typst. One parser
//! (pulldown-cmark), two emitters.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

fn parser(md: &str) -> Parser<'_> {
    Parser::new_ext(md, Options::ENABLE_STRIKETHROUGH)
}

fn unsafe_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("javascript:") || u.starts_with("data:") || u.starts_with("vbscript:")
}

/// Markdown → HTML with raw HTML shown as text and script-ish links removed.
pub fn md_to_html(md: &str) -> String {
    let events = parser(md).map(|e| match e {
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        Event::Start(Tag::Link { link_type, dest_url, title, id }) if unsafe_url(&dest_url) => {
            Event::Start(Tag::Link { link_type, dest_url: "#".into(), title, id })
        }
        other => other,
    });
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

/// A Typst string literal (with quotes) for arbitrary text.
pub fn typst_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Markdown → Typst markup. All text becomes string literals, so nothing in
/// agent-written prose can execute as Typst code.
pub fn md_to_typst(md: &str) -> String {
    let mut out = String::new();
    let mut code: Option<(String, String)> = None; // (lang, text) while inside a fenced block
    let mut lists: Vec<bool> = Vec::new(); // true = ordered
    for e in parser(md) {
        if let Some((_, buf)) = code.as_mut() {
            match e {
                Event::Text(t) => { buf.push_str(&t); continue; }
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, text) = code.take().expect("inside code block");
                    if lang.is_empty() {
                        out.push_str(&format!("#raw(block: true, {})\n", typst_str(&text)));
                    } else {
                        out.push_str(&format!("#raw(block: true, lang: {}, {})\n", typst_str(&lang), typst_str(&text)));
                    }
                    continue;
                }
                _ => continue,
            }
        }
        match e {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => out.push_str(if lists.is_empty() { "\n\n" } else { "" }),
            Event::Start(Tag::Heading { level, .. }) => {
                let lvl = match level { HeadingLevel::H1 => 3, HeadingLevel::H2 => 4, _ => 5 };
                out.push_str(&format!("#heading(level: {lvl}, outlined: false)["));
            }
            Event::End(TagEnd::Heading(_)) => out.push_str("]\n"),
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind { CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(), CodeBlockKind::Indented => String::new() };
                code = Some((lang, String::new()));
            }
            Event::Start(Tag::List(start)) => lists.push(start.is_some()),
            Event::End(TagEnd::List(_)) => { lists.pop(); out.push_str("\n\n"); }
            Event::Start(Tag::Item) => out.push_str(if *lists.last().unwrap_or(&false) { "\n+ " } else { "\n- " }),
            Event::End(TagEnd::Item) => {}
            Event::Start(Tag::Emphasis) => out.push_str("#emph["),
            Event::Start(Tag::Strong) => out.push_str("#strong["),
            Event::Start(Tag::Strikethrough) => out.push_str("#strike["),
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => out.push(']'),
            Event::Start(Tag::BlockQuote(_)) => out.push_str("#quote(block: true)["),
            Event::End(TagEnd::BlockQuote(_)) => out.push_str("]\n"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                if unsafe_url(&dest_url) { out.push('['); } else { out.push_str(&format!("#link({})[", typst_str(&dest_url))); }
            }
            Event::End(TagEnd::Link) => out.push(']'),
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => { out.push('#'); out.push_str(&typst_str(&t)); }
            Event::Code(t) => out.push_str(&format!("#raw({})", typst_str(&t))),
            Event::SoftBreak => out.push_str("#\" \""),
            Event::HardBreak => out.push_str("#linebreak()"),
            Event::Rule => out.push_str("#line(length: 100%)\n"),
            _ => {}
        }
    }
    out.trim_end().to_string() + "\n"
}
```

Notes:
- The unsafe-link branch opens a bare `[` and the matching `End(TagEnd::Link)` closes it, so the text stays but no link is emitted. A bare `[...]` in markup mode is content, which is fine.
- If `BlockQuote`/`TagEnd` variants differ in the resolved pulldown-cmark 0.13 patch release, adjust to its enum; the tests pin the behaviour.

- [ ] **Step 5: Run and check they pass**

Run: `cargo test -p rupu-findings-report && cargo clippy -p rupu-findings-report --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/rupu-findings-report
git commit -m "feat(findings-report): crate scaffold, numbering, filenames, safe markdown→HTML/Typst

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Block model + Markdown emitter (the reference layout)

**Files:**
- Create: `crates/rupu-findings-report/src/blocks.rs`, `src/markdown.rs`, `tests/fixtures/` (copy `crates/rupu-coverage/tests/fixtures/finding_report/valid_full.json` or `include_str!` it via a relative path)
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `blocks::Block` enum:
    - `Filename(String)`
    - `Title(String)`
    - `Fields(Vec<(String, String)>)`
    - `Heading(String)`
    - `Prose(String)` (markdown)
    - `Code { lang: Option<String>, text: String }`
    - `Steps(Vec<String>)` (markdown each)
    - `Note(String)` (plain text)
    - `Table { headers: Vec<String>, rows: Vec<Vec<String>> }`
    - `PageBreak`
  - `blocks::finding_blocks(f: &ExportFinding, numbers: &HashMap<String, String>) -> Vec<Block>`
  - `blocks::project_blocks(meta: &ReportMeta, findings: &[ExportFinding]) -> Vec<Block>`
  - `markdown::render(blocks: &[Block]) -> String`

**Section order for a full finding** (the reporting standard's layout):
1. `Filename(filename(f,"pdf"))`
2. `Title(report.title)`
3. `Fields`: Identifier=`number`, Owner, Product, Affected Component, Source Repository, Existing Ticket References, Impact, Category, Attack Vector, Likelihood, Risk Rating.
4. Heading "Description" + Prose.
5. Heading "Impact" + Prose.
6. Heading "Location" + `Fields`: Input, Output.
7. Heading "Root Cause" + Prose.
8. Heading "Call Chain / Attack Flow" + `Steps`. One step per hop: `**label** — \`file:a-b\` — gate: G (passes because P)`, with absent parts omitted. A sentinel chain becomes `Note`.
9. Heading "Evidence" + per claim: Prose(`**\`file:a-b\`** — claim`), and `Code{lang, excerpt}` when an excerpt exists.
10. Heading "Remediation" + Prose.
11. Heading "Recommended Patch" + `Code{lang:"diff"}` + notes Prose, or a sentinel `Note`.
12. Heading "CI/CD Detection" + Prose(`**Stage:** …`) + Prose(body) + `Code{lang:"sh", command}` + Prose(`**Fails when:** …`), or `Note`.
13. Heading "Regression Test" + Prose(body) + `Code{lang:"sh", command}` + `Fields`(Vulnerable build, Patched build), or `Note`.
14. Heading "Cross-References" + Prose. Each ref is `- <number or id> (<relation>) — note`, using `numbers` to map ids to display numbers; the sentinel becomes a `Note`.
15. Heading "References" + Prose.
16. `Fields`: CVSS v3 Base Score, Risk Factor.
17. Heading "Replication Steps" + `Steps`.
18. If there are artifacts: Heading "Artifacts" + `Table`(Path, SHA-256 (first 12 chars), Size, Stored).
19. Heading "Provenance" + `Fields`: Finding ID, Project, Workflow, Run, Model, Declared.

Tickets: `None Provided`/`Unknown` shown verbatim, or `"<type> <identifier> (<url>)"` joined with `; `.

**Summary-profile finding:**
- `Filename`
- `Title(summary)`
- `Fields`: Identifier, Severity, Location (file:lines or target_ref, or "—"), Concern.
- `Note("Summary finding — no full report was recorded.")`
- Heading "Rationale" + Prose.
- `Code` with the excerpt, if any.
- Heading "References" + Prose (a list).
- Provenance as above.

**Project:**
- `Title(meta.title)`
- `Fields`: Generated (RFC 3339), Scope, Findings (`N — critical C, high H, …`).
- Heading "Index" + `Table`(Number, Severity, Title, Project, Finding ID, Profile).
- Then, for each finding: `PageBreak` followed by its blocks.

- [ ] **Step 1: Write the failing tests** — `tests/markdown.rs`:
  - The full fixture finding renders `Filename: SEC-001 - Notes API returns another user's note by id.pdf` on its first line, then `# Notes API returns…`, and contains:
    - `**Owner:** Unknown`
    - `## Root Cause`
    - `## Call Chain / Attack Flow`
    - `` ```diff ``
    - `## Replication Steps`
    - `Step 1: Sign up as user A`
    - `**CVSS v3 Base Score:** Unknown`
  - The section headings appear in exactly the order listed above. Assert on the index positions of the `## ` lines.
  - A cross-reference to a finding in `numbers` renders its number, not the fnd id.
  - A summary finding renders the note and no "Root Cause" heading.
  - A project with two findings renders the index table with both numbers and a `---` page separator between findings (the Markdown emitter renders `PageBreak` as a horizontal rule).

  Build fixtures by deserializing `valid_full.json` into `FindingReport` and wrapping it in a `FindingRecord` with `profile: Full`.

- [ ] **Step 2: Run and check they fail.** `cargo test -p rupu-findings-report` → compile errors.

- [ ] **Step 3: Implement** `blocks.rs` following the order above exactly, using `number::title`/`number::filename`, `rupu_coverage::report::{OrSentinel, …}` and `sentinel` rendering (`Not Provided — x` → `Not provided: x`). Then `markdown.rs`:

```rust
use crate::blocks::Block;

pub fn render(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        match b {
            Block::Filename(f) => out.push_str(&format!("Filename: {f}\n\n")),
            Block::Title(t) => out.push_str(&format!("# {t}\n\n")),
            Block::Fields(rows) => {
                for (k, v) in rows {
                    out.push_str(&format!("**{k}:** {v}  \n"));
                }
                out.push('\n');
            }
            Block::Heading(h) => out.push_str(&format!("## {h}\n\n")),
            Block::Prose(p) => out.push_str(&format!("{}\n\n", p.trim_end())),
            Block::Code { lang, text } => {
                let fence = if text.contains("```") { "````" } else { "```" };
                out.push_str(&format!("{fence}{}\n{}\n{fence}\n\n", lang.as_deref().unwrap_or(""), text.trim_end_matches('\n')));
            }
            Block::Steps(steps) => {
                for (i, s) in steps.iter().enumerate() {
                    out.push_str(&format!("Step {}: {}\n", i + 1, s.trim()));
                }
                out.push('\n');
            }
            Block::Note(n) => out.push_str(&format!("_{n}_\n\n")),
            Block::Table { headers, rows } => {
                let esc = |c: &str| c.replace('|', "\\|").replace('\n', " ");
                out.push_str(&format!("| {} |\n", headers.iter().map(|h| esc(h)).collect::<Vec<_>>().join(" | ")));
                out.push_str(&format!("|{}\n", " --- |".repeat(headers.len())));
                for r in rows {
                    out.push_str(&format!("| {} |\n", r.iter().map(|c| esc(c)).collect::<Vec<_>>().join(" | ")));
                }
                out.push('\n');
            }
            Block::PageBreak => out.push_str("---\n\n"),
        }
    }
    out.trim_end().to_string() + "\n"
}
```

Add `pub mod blocks; pub mod markdown;` to `lib.rs`.

- [ ] **Step 4: Run and check they pass.** `cargo test -p rupu-findings-report && cargo clippy -p rupu-findings-report --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-findings-report
git commit -m "feat(findings-report): block model in the standard section order + Markdown emitter

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: HTML and Typst emitters

**Files:**
- Create: `crates/rupu-findings-report/src/html.rs`, `src/typst_doc.rs`, `tests/html_typst.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `html::render(title: &str, blocks: &[Block]) -> String`: a complete self-contained document with inline CSS and no external loads. `PageBreak` becomes `<div class="pb"></div>` with `page-break-before: always`.
  - `typst_doc::render(blocks: &[Block]) -> String`: a complete Typst document (preamble + body).

- [ ] **Step 1: Write the failing tests** — `tests/html_typst.rs`:
  - **HTML.** A field value containing `<img src=x onerror=alert(1)>` is escaped as `&lt;img`. The output contains no `<script`, no `http://` or `https://` stylesheet or script references (a prose link `href` is fine), and no `javascript:`. It contains `<pre><code class="language-diff">` for the patch and `class="pb"` between two findings.
  - **Typst.** Every `Fields` label and value, headings, titles and notes go through `typst_str`. A field value `#set page(width: 1cm)` appears only inside a string literal, so the output does not contain the line `#set page(width: 1cm)` outside quotes. Assert that `"#set page(width: 1cm)"` appears quoted. The preamble contains `#set text(font: "Libertinus Serif"` and `#show raw: set text(font: "DejaVu Sans Mono"`. `PageBreak` becomes `#pagebreak()`.

- [ ] **Step 2: Run and check they fail.**

- [ ] **Step 3: Implement.**

`html.rs`:

```rust
use crate::blocks::Block;
use crate::prose::md_to_html;

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

const CSS: &str = "body{font:14px/1.55 -apple-system,system-ui,sans-serif;color:#1b1924;max-width:860px;margin:2rem auto;padding:0 1rem}\
h1{font-size:22px;margin:.2em 0 .6em}h2{font-size:15px;text-transform:uppercase;letter-spacing:.06em;color:#4a4659;margin:1.6em 0 .5em}\
.fn{font:12px ui-monospace,Menlo,monospace;color:#807b92}dl{display:grid;grid-template-columns:12rem 1fr;gap:.2rem 1rem;margin:.5rem 0}\
dt{color:#807b92}dd{margin:0}pre{background:#f4f3f8;padding:.6rem .8rem;border-radius:4px;overflow-x:auto;font:12px/1.5 ui-monospace,Menlo,monospace}\
code{font-family:ui-monospace,Menlo,monospace}ol.steps li{margin:.25rem 0}.note{color:#807b92;font-style:italic}\
table{border-collapse:collapse;width:100%;font-size:13px}th,td{border-bottom:1px solid #dedbe7;text-align:left;padding:.3rem .5rem}\
.pb{page-break-before:always;border-top:1px dashed #dedbe7;margin:2rem 0}@media print{.pb{border:0;margin:0}}";

pub fn render(title: &str, blocks: &[Block]) -> String {
    let mut body = String::new();
    for b in blocks {
        match b {
            Block::Filename(f) => body.push_str(&format!("<div class=\"fn\">Filename: {}</div>\n", esc(f))),
            Block::Title(t) => body.push_str(&format!("<h1>{}</h1>\n", esc(t))),
            Block::Fields(rows) => {
                body.push_str("<dl>");
                for (k, v) in rows {
                    body.push_str(&format!("<dt>{}</dt><dd>{}</dd>", esc(k), esc(v)));
                }
                body.push_str("</dl>\n");
            }
            Block::Heading(h) => body.push_str(&format!("<h2>{}</h2>\n", esc(h))),
            Block::Prose(p) => body.push_str(&md_to_html(p)),
            Block::Code { lang, text } => body.push_str(&format!(
                "<pre><code{}>{}</code></pre>\n",
                lang.as_deref().map(|l| format!(" class=\"language-{}\"", esc(l))).unwrap_or_default(),
                esc(text)
            )),
            Block::Steps(steps) => {
                body.push_str("<ol class=\"steps\">");
                for s in steps {
                    body.push_str(&format!("<li>{}</li>", md_to_html(s)));
                }
                body.push_str("</ol>\n");
            }
            Block::Note(n) => body.push_str(&format!("<p class=\"note\">{}</p>\n", esc(n))),
            Block::Table { headers, rows } => {
                body.push_str("<table><thead><tr>");
                for h in headers {
                    body.push_str(&format!("<th>{}</th>", esc(h)));
                }
                body.push_str("</tr></thead><tbody>");
                for r in rows {
                    body.push_str("<tr>");
                    for c in r {
                        body.push_str(&format!("<td>{}</td>", esc(c)));
                    }
                    body.push_str("</tr>");
                }
                body.push_str("</tbody></table>\n");
            }
            Block::PageBreak => body.push_str("<div class=\"pb\"></div>\n"),
        }
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:\">\
<title>{}</title><style>{CSS}</style></head><body>\n{body}</body></html>\n",
        esc(title)
    )
}
```

(The CSP meta blocks scripts even if escaping had a bug, which gives defence in depth.)

`typst_doc.rs`:

```rust
use crate::blocks::Block;
use crate::prose::{md_to_typst, typst_str};

const PREAMBLE: &str = "#set page(paper: \"a4\", margin: (x: 2cm, y: 2cm), numbering: \"1\")\n\
#set text(font: \"Libertinus Serif\", size: 10pt)\n\
#set par(justify: false)\n\
#show raw: set text(font: \"DejaVu Sans Mono\", size: 8pt)\n\
#show raw.where(block: true): block.with(fill: luma(245), inset: 6pt, radius: 3pt, width: 100%)\n\
#show heading.where(level: 2): set text(size: 11pt)\n\n";

fn content(s: &str) -> String {
    format!("[#{}]", typst_str(s))
}

pub fn render(blocks: &[Block]) -> String {
    let mut out = String::from(PREAMBLE);
    for b in blocks {
        match b {
            Block::Filename(f) => out.push_str(&format!("#text(size: 8pt, fill: gray)[#{}]\n\n", typst_str(&format!("Filename: {f}")))),
            Block::Title(t) => out.push_str(&format!("#heading(level: 1)[#{}]\n\n", typst_str(t))),
            Block::Fields(rows) => {
                out.push_str("#table(columns: (auto, 1fr), stroke: none, inset: (x: 0pt, y: 2pt), column-gutter: 1em,\n");
                for (k, v) in rows {
                    out.push_str(&format!("  [#strong[#{}]], {},\n", typst_str(k), content(v)));
                }
                out.push_str(")\n\n");
            }
            Block::Heading(h) => out.push_str(&format!("#heading(level: 2)[#{}]\n\n", typst_str(h))),
            Block::Prose(p) => { out.push_str(&md_to_typst(p)); out.push('\n'); }
            Block::Code { lang, text } => match lang {
                Some(l) => out.push_str(&format!("#raw(block: true, lang: {}, {})\n\n", typst_str(l), typst_str(text))),
                None => out.push_str(&format!("#raw(block: true, {})\n\n", typst_str(text))),
            },
            Block::Steps(steps) => {
                out.push_str("#enum(numbering: \"Step 1:\",\n");
                for s in steps {
                    out.push_str(&format!("  [{}],\n", md_to_typst(s).trim_end()));
                }
                out.push_str(")\n\n");
            }
            Block::Note(n) => out.push_str(&format!("#emph[#{}]\n\n", typst_str(n))),
            Block::Table { headers, rows } => {
                out.push_str(&format!("#table(columns: {}, stroke: 0.5pt + luma(200), inset: 4pt,\n  table.header(", headers.len()));
                out.push_str(&headers.iter().map(|h| format!("[#strong[#{}]]", typst_str(h))).collect::<Vec<_>>().join(", "));
                out.push_str("),\n");
                for r in rows {
                    out.push_str(&format!("  {},\n", r.iter().map(|c| content(c)).collect::<Vec<_>>().join(", ")));
                }
                out.push_str(")\n\n");
            }
            Block::PageBreak => out.push_str("#pagebreak()\n\n"),
        }
    }
    out
}
```

Add `pub mod html; pub mod typst_doc;` to `lib.rs`.

- [ ] **Step 4: Run and check they pass.** `cargo test -p rupu-findings-report && cargo clippy -p rupu-findings-report --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-findings-report
git commit -m "feat(findings-report): self-contained HTML and Typst emitters with escaping

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: PDF via embedded Typst, render API, split zip

**Files:**
- Create: `crates/rupu-findings-report/src/pdf.rs`, `src/render.rs`, `tests/pdf.rs`
- Modify: root `Cargo.toml` (`typst = "0.15.1"`, `typst-pdf = "0.15.1"`, `typst-layout = "0.15.1"`, `typst-assets = { version = "0.15.1", features = ["fonts"] }`, and `zip`: use the newest **non-prerelease** version listed by `cargo search zip --limit 20`, with `default-features = false, features = ["deflate"]`), the crate's `Cargo.toml`, `src/lib.rs`

**Interfaces:**
- Produces:
  - `pdf::render_pdf(markup: String) -> Result<Vec<u8>, ExportError>`
  - `render::Format { Markdown, Html, Pdf }` with `ext()` (`md`, `html`, `pdf`) and `content_type()` (`text/markdown; charset=utf-8`, `text/html; charset=utf-8`, `application/pdf`)
  - `render::render_finding(f: &ExportFinding, numbers: &HashMap<String,String>, fmt: Format) -> Result<Vec<u8>, ExportError>`
  - `render::render_project(meta: &ReportMeta, findings: &[ExportFinding], fmt: Format) -> Result<Vec<u8>, ExportError>`
  - `render::render_split_zip(meta: &ReportMeta, findings: &[ExportFinding], fmt: Format) -> Result<Vec<u8>, ExportError>`: one file per finding named `number::filename(f, ext)`, plus `index.md`, the Markdown project index (title + index table only)
  - `#[derive(thiserror::Error)] ExportError { Typst(String), Zip(String) }`

- [ ] **Step 1: Write the failing tests** — `tests/pdf.rs`:
  - `render_finding(full_fixture, Pdf)` returns bytes starting with `%PDF`.
  - A project with 2 findings renders a PDF, and it is larger than a single finding.
  - A field containing Typst-looking text (`#panic("x")`) still compiles. It must not error, which proves escaping.
  - `render_split_zip` with Markdown returns a zip. Read it back with `zip::ZipArchive` and assert the entry names are `index.md` and the two `SEC-00N - …md` files.
  - `Format::Pdf.content_type() == "application/pdf"`.

- [ ] **Step 2: Run and check they fail.**

- [ ] **Step 3: Implement.** This `World` is verified to compile against typst 0.15.1 and to produce a PDF:

```rust
//! PDF rendering: compiles our own Typst markup in-process with the fonts
//! bundled by `typst-assets`. The only source is the generated document;
//! every other file access is refused.

use std::sync::LazyLock;
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

use crate::render::ExportError;

struct Fonts {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    let fonts: Vec<Font> = typst_assets::fonts().flat_map(|data| Font::iter(Bytes::new(data))).collect();
    Fonts { book: LazyHash::new(FontBook::from_fonts(&fonts)), fonts }
});

static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| LazyHash::new(Library::default()));

struct ReportWorld {
    main: Source,
}

impl World for ReportWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }
    fn book(&self) -> &LazyHash<FontBook> {
        &FONTS.book
    }
    fn main(&self) -> FileId {
        self.main.id()
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }
    fn file(&self, _id: FileId) -> FileResult<Bytes> {
        Err(FileError::AccessDenied)
    }
    fn font(&self, index: usize) -> Option<Font> {
        FONTS.fonts.get(index).cloned()
    }
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        // Reports never print "today"; a fixed date keeps output deterministic.
        Datetime::from_ymd(2000, 1, 1)
    }
}

pub fn render_pdf(markup: String) -> Result<Vec<u8>, ExportError> {
    let world = ReportWorld { main: Source::detached(markup) };
    let doc: PagedDocument = typst::compile(&world)
        .output
        .map_err(|diags| ExportError::Typst(diags.iter().map(|d| d.message.to_string()).collect::<Vec<_>>().join("; ")))?;
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default())
        .map_err(|diags| ExportError::Typst(diags.iter().map(|d| d.message.to_string()).collect::<Vec<_>>().join("; ")))
}
```

(If `static LazyLock<LazyHash<…>>` fails a `Sync` bound, keep the library per world as the verified probe did: store `library: LazyHash<Library>` in `ReportWorld`, built with `LazyHash::new(Library::default())`. The fonts `LazyLock` is the part that matters for speed.)

`render.rs`:

```rust
use crate::blocks::{finding_blocks, project_blocks};
use crate::model::{ExportFinding, ReportMeta};
use crate::number::{filename, number_map, title};
use crate::{html, markdown, pdf, typst_doc};
use std::collections::HashMap;
use std::io::Write;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("PDF rendering failed: {0}")]
    Typst(String),
    #[error("zip failed: {0}")]
    Zip(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    Pdf,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self { Format::Markdown => "md", Format::Html => "html", Format::Pdf => "pdf" }
    }
    pub fn content_type(self) -> &'static str {
        match self {
            Format::Markdown => "text/markdown; charset=utf-8",
            Format::Html => "text/html; charset=utf-8",
            Format::Pdf => "application/pdf",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s { "md" | "markdown" => Some(Format::Markdown), "html" => Some(Format::Html), "pdf" => Some(Format::Pdf), _ => None }
    }
}

fn emit(doc_title: &str, blocks: &[crate::blocks::Block], fmt: Format) -> Result<Vec<u8>, ExportError> {
    Ok(match fmt {
        Format::Markdown => markdown::render(blocks).into_bytes(),
        Format::Html => html::render(doc_title, blocks).into_bytes(),
        Format::Pdf => pdf::render_pdf(typst_doc::render(blocks))?,
    })
}

pub fn render_finding(f: &ExportFinding, numbers: &HashMap<String, String>, fmt: Format) -> Result<Vec<u8>, ExportError> {
    emit(&format!("{} - {}", f.number, title(f)), &finding_blocks(f, numbers), fmt)
}

pub fn render_project(meta: &ReportMeta, findings: &[ExportFinding], fmt: Format) -> Result<Vec<u8>, ExportError> {
    emit(&meta.title, &project_blocks(meta, findings), fmt)
}

pub fn render_split_zip(meta: &ReportMeta, findings: &[ExportFinding], fmt: Format) -> Result<Vec<u8>, ExportError> {
    let numbers = number_map(findings);
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let z = |e: zip::result::ZipError| ExportError::Zip(e.to_string());
    let index = crate::blocks::index_blocks(meta, findings);
    zip.start_file("index.md", opts).map_err(z)?;
    zip.write_all(markdown::render(&index).as_bytes()).map_err(|e| ExportError::Zip(e.to_string()))?;
    for f in findings {
        zip.start_file(filename(f, fmt.ext()), opts).map_err(z)?;
        zip.write_all(&render_finding(f, &numbers, fmt)?).map_err(|e| ExportError::Zip(e.to_string()))?;
    }
    Ok(zip.finish().map_err(z)?.into_inner())
}
```

Add `pub fn index_blocks(meta, findings) -> Vec<Block>` to `blocks.rs`: the project title, fields and index table, without the per-finding pages. Refactor `project_blocks` to start with it. Add `pub mod pdf; pub mod render;` and `pub use render::{render_finding, render_project, render_split_zip, ExportError, Format};` to `lib.rs`. If the resolved `zip` major has a different options type or `start_file` signature, adapt to that version; the tests pin the behaviour.

- [ ] **Step 4: Run and check they pass.**

Run: `cargo test -p rupu-findings-report && cargo clippy -p rupu-findings-report --all-targets -- -D warnings && cargo build --workspace`
Expected: PASS. Note in your report the compile time and how much the release binary grows (`ls -l target/release/rupu` before and after a `cargo build --release -p rupu-cli`, if time allows).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/rupu-findings-report
git commit -m "feat(findings-report): in-process PDF via Typst, render API, split zip

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Selection, config prefix, CP export endpoints

**Files:**
- Create: `crates/rupu-findings-report/src/select.rs`
- Modify: `crates/rupu-config/src/findings_config.rs` (`export_id_prefix: Option<String>`), `crates/rupu-cp/Cargo.toml` (+ `rupu-findings-report`), `crates/rupu-cp/src/api/findings.rs`

**Interfaces:**
- Produces:
  - `select::Selection { pub ids: Vec<String>, pub ws_id: Option<String>, pub run_ids: Option<HashSet<String>>, pub min_severity: Option<Severity>, pub owner: Option<String>, pub cwe: Option<String>, pub include_summaries: bool }`
  - `select::select(numbered: Vec<ExportFinding>, sel: &Selection) -> Vec<ExportFinding>`. Numbering happens BEFORE selection, so numbers stay stable. `ids` non-empty means only those ids. `include_summaries: false` drops summary-profile findings **unless** they were explicitly listed in `ids`.
  - CP routes:
    - `GET /api/findings/:id/export?format=md|html|pdf` returns an attachment named `filename(f, ext)`. The finding is numbered within its own project.
    - `POST /api/findings/export` with the JSON body `{ "format": "md"|"html"|"pdf", "title"?: string, "ids"?: [..], "ws_id"?: .., "run_id"?: .., "min_severity"?: "critical"|"high"|"medium"|"low"|"info", "owner"?: .., "cwe"?: .., "include_summaries"?: bool, "split"?: bool }`. It returns an attachment: `"<title>.<ext>"`, or `"<title>.zip"` when `split`. The request is 400 on an unknown format, and 404 when the selection is empty (`{"error":"no findings match this selection"}`).
  - Both routes run the render in `spawn_blocking` and set `X-Content-Type-Options: nosniff`.

- [ ] **Step 1: Write the failing tests.**
  - `select.rs` unit tests:
    - `min_severity: High` keeps critical and high only.
    - `ids` overrides `include_summaries`.
    - `owner` matches `report.ownership.owner` exactly.
    - `cwe` matches `report.cwe` or `concern_id` containing it.
    - `run_ids` matches `declared_by.run_id`.
  - `rupu-config` test: `export_id_prefix = "VULN"` parses.
  - CP HTTP tests (same harness as the other findings tests):
    - `GET …/export?format=md` returns 200, `content-disposition` starting with `attachment; filename="SEC-001 - `, and a body starting with `Filename:`.
    - `format=pdf` returns `application/pdf` with a body starting with `%PDF`.
    - `format=exe` returns 400.
    - The `POST` with `ids` of two findings and `split: true` returns `application/zip`.
    - The `POST` with a selection matching nothing returns 404.
    - HTML export is an attachment, never inline.
    - With `[findings].export_id_prefix = "VULN"` in the state's config, the filename starts with `VULN-001`.

- [ ] **Step 2: Run and check they fail.**

- [ ] **Step 3: Implement.**
  - `select.rs` follows the interface above. Compare severities with the same rank as `number.rs`.
  - In `rupu-cp`, convert `FindingOut` into `ExportInput { ws_id, project, workflow_name, record }`, where `workflow_name` is joined via `run_store` as `list_findings` does.
  - The prefix comes from `s.config.read()…findings.export_id_prefix.clone().unwrap_or_else(|| "SEC".into())`. Match the real `AppState.config` lock type.
  - `run_id` goes through the existing `resolve_run_scope`, the same scope the list endpoint uses for a run.
  - For the GET:
    1. collect all findings;
    2. keep the finding's `ws_id`;
    3. call `assign_numbers(project_findings, prefix)`;
    4. find the id;
    5. `render_finding(f, &number_map(&numbered), fmt)`.
  - For the POST:
    1. collect all findings;
    2. call `assign_numbers(all, prefix)`;
    3. call `select`;
    4. `ReportMeta { title: body.title.unwrap_or("Findings report"), generated_at: Utc::now(), scope: <describe the selection in words> }`;
    5. `render_project` or `render_split_zip`.
  - The attachment filename for a project report is the title with the same sanitising as `number::filename`. Expose a `pub fn sanitize_title(&str) -> String` from `number.rs` and reuse it.
  - Add `export_id_prefix` to `FindingsConfig` with doc comment "Display-number prefix for exported reports (default `SEC`)."

- [ ] **Step 4: Run and check they pass.** `cargo test -p rupu-findings-report -p rupu-config -p rupu-cp && cargo clippy -p rupu-cp --all-targets -- -D warnings`. Run `make macos-fixtures` and commit any drift (expected: none).

- [ ] **Step 5: Commit**

```bash
git add crates Cargo.lock apps/rupu-macos/Fixtures
git commit -m "feat(cp): finding and project report exports (md/html/pdf, split zip) + export_id_prefix

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: `rupu findings export`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/findings.rs`, `crates/rupu-cli/Cargo.toml` (+ `rupu-findings-report`), `crates/rupu-cp/src/api/findings.rs` (make `collect_all_findings` and `resolve_run_scope` `pub`, and re-export them if the module is private)
- Create: `crates/rupu-cli/tests/findings_export.rs`

**Interfaces:**
- Produces:

```
rupu findings export [--id <fnd_…>]… [--project <ws_id|path>] [--run <run_id>]
                     [--severity <critical|high|medium|low|info>] [--owner <s>] [--cwe <CWE-n>]
                     [--include-summaries] [--format md|html|pdf] [--split] [--title <s>] -o <path>
```

  - A single `--id` with no filters writes one finding document to `-o` (a file path).
  - Otherwise it writes a project report, or a zip with `--split`.
  - It errors (exit code 1, with a message) when nothing matches.
  - `--project` accepts a workspace id or a path; a path is resolved to the workspace whose path matches.
  - The default format is `md`.

  The logic lives in libraries: selection and rendering in `rupu-findings-report`, collection in `rupu-cp`. The CLI parses arguments and writes the bytes.

- [ ] **Step 1: Write the failing test** (`assert_cmd` + a temp `RUPU_HOME`). Seed one workspace with two full-profile findings: write `workspaces/<id>.toml` and the ledger JSONL, following the CP test setup. Then:
  - `rupu findings export --format md -o out.md` succeeds, and the file contains both `SEC-001` and `SEC-002`.
  - `--id <fnd> --format pdf -o one.pdf` writes a `%PDF` file.
  - `--severity critical` with only high findings exits non-zero, with "no findings match".
  - `--split -o out.zip` writes a zip.

  Set `RUPU_HOME` to the temp dir the way other CLI tests isolate their home. Check `tests/cli_*.rs` for the env var name used.

- [ ] **Step 2: Run and check it fails.**

- [ ] **Step 3: Implement** an `Export { … }` variant on `cmd::findings::Action`, with clap args matching the interface. Its handler:
  1. loads config (`findings.export_id_prefix`);
  2. calls `rupu_cp::api::findings::collect_all_findings(&global)`;
  3. resolves `--run` via `resolve_run_scope` with a `RunStore` at `global/runs`;
  4. builds `ExportInput`s, then numbers, selects and renders;
  5. writes the file.

  Replace the `Cmd::Findings` format gate in `lib.rs` with a per-action `cmd::findings::ensure_output_format` that allows only Table (both actions write files or stdout JSON), following `cmd::coverage::ensure_output_format`.

- [ ] **Step 4: Run and check it passes.** `cargo test -p rupu-cli --test findings_export --test findings_schema && cargo clippy -p rupu-cli --all-targets -- -D warnings -A clippy::question_mark`

- [ ] **Step 5: Commit**

```bash
git add crates Cargo.lock
git commit -m "feat(cli): rupu findings export (single finding or project report; md/html/pdf; split zip)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Web export controls

**Files:**
- Create: `crates/rupu-cp/web/src/components/findings/ExportDialog.tsx` (+ test)
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (`findingExportUrl(id, format)`, `api.exportFindings(body): Promise<Blob>`), `crates/rupu-cp/web/src/pages/FindingDetail.tsx` (export links), `crates/rupu-cp/web/src/pages/Findings.tsx` and `crates/rupu-cp/web/src/components/project/ProjectFindingsTab.tsx` ("Export report" button that opens the dialog with the currently filtered rows)

**Interfaces:**
- Produces:
  - `ExportDialog({ open, onClose, findings, defaultTitle, wsId? })`, where `findings` is the currently filtered rows. Its controls:
    - a format radio (Markdown / HTML / PDF);
    - "One file per finding (zip)";
    - "Include summary findings" (default off; the label counts how many summary rows are in the set);
    - a title input;
    - an Export button that POSTs `{format, title, ids: findings.map(f => f.id), include_summaries, split}`, turns the Blob into an object URL, clicks a hidden `<a download>`, then revokes the URL.
  - Errors show inline.
  - FindingDetail gets three links in its header, "Markdown", "HTML" and "PDF", each an `<a href={findingExportUrl(id, fmt)} download>`.

- [ ] **Step 1: Write the failing tests.**
  - `ExportDialog.test.tsx`: mock `vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']))`, stub `URL.createObjectURL`/`revokeObjectURL`, then:
    - Choosing PDF and "zip" calls `exportFindings` with `{format:'pdf', split:true, ids:[…]}`.
    - A rejected promise shows the error text.
    - The summary-count label reflects the rows.
  - `api` test: `exportFindings` POSTs JSON to `/api/findings/export` and returns `res.blob()`. It throws `ApiError` with the server's error text on non-2xx.
  - FindingDetail test: the three links have `href`s `/api/findings/fnd_1/export?format=md|html|pdf` and a `download` attribute.

- [ ] **Step 2: Run and check they fail.**

- [ ] **Step 3: Implement.** `api.exportFindings` can't use `request<T>`, because that parses JSON:

```ts
async exportFindings(body: {
  format: 'md' | 'html' | 'pdf'; title?: string; ids?: string[]; include_summaries?: boolean; split?: boolean;
}): Promise<Blob> {
  const res = await fetch('/api/findings/export', {
    method: 'POST', credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new ApiError(res.status, text || res.statusText, text);
  }
  return res.blob();
},
```

`export function findingExportUrl(id: string, format: 'md' | 'html' | 'pdf'): string { return `/api/findings/${encodeURIComponent(id)}/export?format=${format}`; }`

Build the dialog with the existing `ui/Button`, the form-field classes used in `StepForm.tsx` (`fieldCls`), and a fixed overlay (`fixed inset-0 bg-black/30` + a centered `bg-panel border border-border rounded-xl shadow-card p-5`). If a shared `Modal` component already exists, use it instead: search `components/ui` for one first. Close it on Escape and on overlay click.

- [ ] **Step 4: Run and check they pass.** `cd crates/rupu-cp/web && npx vitest run src/components/findings src/pages src/lib && npx tsc --noEmit -p .`

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): finding export links + project report export dialog

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Restore the generation promise, docs, gate

**Files:**
- Modify: `crates/rupu-coverage/src/report/guidance.rs` (`FULL_GUIDANCE`), `crates/rupu-coverage/schema/finding_report.schema.json` (top-level `description`), `examples/agents/security-assessor.md`, `docs/coverage.md`, `docs/agent-format.md`, `docs/configuration.md` (`export_id_prefix`), `CLAUDE.md` (a `rupu-findings-report` crate entry; add Plan 3 to "Read first")

- [ ] **Step 1: Update the wording now that exports exist.**
  - **`FULL_GUIDANCE`:** replace "The report is stored as structured data and is the source of truth for this finding." with "rupu generates the Markdown, HTML and PDF reports from it, so do not also write a report file." Keep the heading and everything else.
  - **Schema `description`:** say that presentations and exports are generated from the report.
  - **Docs:** remove the "not built yet" note for exports and document:
    - `rupu findings export` (flags and examples);
    - the CP download links and export dialog;
    - numbering (per project, by severity then time; prefix from `[findings].export_id_prefix`);
    - the file naming;
    - that HTML exports are self-contained, with no external loads and a strict CSP.
  - **Example agent:** tell it not to write a separate report file.
  - **`CLAUDE.md`:** add a `rupu-findings-report` entry, "pure renderers (blocks → Markdown / HTML / Typst→PDF in-process with bundled fonts), numbering and selection shared by the CP and `rupu findings export`", and add Plan 3 to "Read first".
  - If the MCP advertised schema text changes, re-bless the snapshot: `BLESS=1 cargo test -p rupu-mcp --test schema_snapshot`.

- [ ] **Step 2: Gate** (run each command separately and record the results):

```bash
cargo test --workspace
```
```bash
cargo clippy --workspace --all-targets -- -D warnings -A clippy::question_mark
```
```bash
cd crates/rupu-cp/web && npx vitest run && npx tsc --noEmit -p .
```
```bash
make cp-web
```
Expected: green, except the known `AutoflowRuns.columnOrder` web failure.

- [ ] **Step 3: Commit**

```bash
git add crates docs examples CLAUDE.md
git commit -m "docs: finding report exports; guidance now points agents at generated reports

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 4: Handoff for a manual check.** matt exports:
  - one finding as PDF from the report page;
  - a filtered project report as PDF;
  - a split zip.

  He then opens them and checks the layout.
