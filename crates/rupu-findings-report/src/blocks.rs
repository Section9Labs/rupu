//! The renderer-neutral document model. `finding_blocks` / `project_blocks`
//! lay a finding out in the reporting standard's section order; each emitter
//! (Markdown, HTML, Typst) only decides how to draw a block, never which
//! blocks exist or in what order.

use crate::model::{ExportFinding, ReportMeta};
use crate::number;
use chrono::{DateTime, SecondsFormat, Utc};
use rupu_coverage::report::{
    ArtifactStorage, FindingReport, Likelihood, OrSentinel, Relation, RiskLevel, Ticket,
    NOT_PROVIDED_PREFIX,
};
use rupu_coverage::{FindingProfile, Severity};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// The suggested file name, shown first so a printed copy is traceable.
    Filename(String),
    Title(String),
    /// Label / value rows. Values are plain text.
    Fields(Vec<(String, String)>),
    Heading(String),
    /// Agent-written Markdown.
    Prose(String),
    Code {
        lang: Option<String>,
        text: String,
    },
    /// Numbered steps, each Markdown.
    Steps(Vec<String>),
    /// A plain-text aside (sentinels, "no full report recorded").
    Note(String),
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    PageBreak,
}

/// `Not Provided — why` reads `Not provided: why`; every other sentinel
/// (`None`, `Unknown`, `None Provided`) is shown verbatim.
fn sentinel_text(s: &str) -> String {
    match s.strip_prefix(NOT_PROVIDED_PREFIX) {
        Some(rest) => format!("Not provided: {rest}"),
        None => s.to_string(),
    }
}

fn kv(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

fn heading(h: &str) -> Block {
    Block::Heading(h.to_string())
}

fn prose(p: impl Into<String>) -> Block {
    Block::Prose(p.into())
}

/// A language tag safe to put after a code fence: agent-supplied, so reduced
/// to the characters real tags use (`c++`, `objective-c`, `c#`).
fn clean_lang(l: &str) -> Option<String> {
    let s: String = l
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '.' | '#'))
        .collect();
    (!s.is_empty()).then_some(s)
}

fn code(lang: Option<&str>, text: &str) -> Block {
    Block::Code {
        lang: lang.and_then(clean_lang),
        text: text.to_string(),
    }
}

fn nonblank(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.trim().is_empty())
}

fn oneline(s: &str) -> String {
    s.split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// An inline code span holding `s` verbatim: the delimiter is longer than
/// any backtick run inside, padded when the text starts or ends with one.
fn code_span(s: &str) -> String {
    let s = oneline(s);
    let mut longest = 0;
    let mut run = 0;
    for c in s.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let ticks = "`".repeat(longest + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{s}{pad}{ticks}")
}

/// Where in the target something is: `file:a-b`, or the binary VA for a
/// binary-only target; both when both were recorded.
fn location(file: Option<&str>, lines: Option<[u32; 2]>, va: Option<&str>) -> Option<String> {
    let mut s = match (file, lines) {
        (Some(f), Some([a, b])) => format!("{f}:{a}-{b}"),
        (Some(f), None) => f.to_string(),
        (None, _) => String::new(),
    };
    if let Some(va) = va {
        s = if s.is_empty() {
            va.to_string()
        } else {
            format!("{s} @ {va}")
        };
    }
    (!s.is_empty()).then_some(s)
}

fn risk(r: RiskLevel) -> &'static str {
    match r {
        RiskLevel::Low => "Low",
        RiskLevel::Medium => "Medium",
        RiskLevel::High => "High",
        RiskLevel::Critical => "Critical",
    }
}

fn likelihood(l: Likelihood) -> &'static str {
    match l {
        Likelihood::Low => "Low",
        Likelihood::Medium => "Medium",
        Likelihood::High => "High",
    }
}

fn relation(r: Relation) -> &'static str {
    match r {
        Relation::Duplicate => "duplicate",
        Relation::Sibling => "sibling",
        Relation::Prerequisite => "prerequisite",
        Relation::Supersedes => "supersedes",
    }
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "critical",
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
        Severity::Info => "info",
    }
}

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "Critical",
        Severity::High => "High",
        Severity::Medium => "Medium",
        Severity::Low => "Low",
        Severity::Info => "Info",
    }
}

fn profile_word(p: FindingProfile) -> &'static str {
    match p {
        FindingProfile::Full => "full",
        FindingProfile::Summary => "summary",
    }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn tickets_text(t: &OrSentinel<Vec<Ticket>>) -> String {
    match t {
        OrSentinel::Sentinel(s) => sentinel_text(s),
        OrSentinel::Value(v) if v.is_empty() => "None Provided".to_string(),
        OrSentinel::Value(v) => v
            .iter()
            .map(|t| match nonblank(&t.url) {
                Some(url) => format!("{} {} ({url})", t.kind, t.identifier),
                None => format!("{} {}", t.kind, t.identifier),
            })
            .collect::<Vec<_>>()
            .join("; "),
    }
}

/// Emit the blocks for a value, or a `Note` for its sentinel.
fn value_or_note<T>(
    v: &OrSentinel<T>,
    out: &mut Vec<Block>,
    on_value: impl FnOnce(&T, &mut Vec<Block>),
) {
    match v {
        OrSentinel::Value(v) => on_value(v, out),
        OrSentinel::Sentinel(s) => out.push(Block::Note(sentinel_text(s))),
    }
}

fn provenance(f: &ExportFinding) -> Vec<Block> {
    let r = &f.input.record;
    vec![
        heading("Provenance"),
        Block::Fields(vec![
            kv("Finding ID", r.id.clone()),
            kv("Project", f.input.project.clone()),
            kv(
                "Workflow",
                f.input
                    .workflow_name
                    .clone()
                    .unwrap_or_else(|| "—".to_string()),
            ),
            kv("Run", r.declared_by.run_id.clone()),
            kv("Model", r.declared_by.model.clone()),
            kv("Declared", rfc3339(r.declared_at)),
        ]),
    ]
}

/// One finding, in the reporting standard's section order. A finding with no
/// recorded report (a summary record, or a full record whose report could
/// not be loaded) gets the short summary layout.
pub fn finding_blocks(f: &ExportFinding, numbers: &HashMap<String, String>) -> Vec<Block> {
    match &f.input.record.report {
        Some(report) => full_blocks(f, report, numbers),
        None => summary_blocks(f),
    }
}

fn hop_step(h: &rupu_coverage::report::ChainHop) -> String {
    let mut s = format!("**{}**", h.label);
    if let Some(l) = location(h.file.as_deref(), h.lines, h.binary_va.as_deref()) {
        s.push_str(&format!(" — {}", code_span(&l)));
    }
    match (nonblank(&h.gate), nonblank(&h.passes_because)) {
        (Some(g), Some(p)) => s.push_str(&format!(" — gate: {g} (passes because {p})")),
        (Some(g), None) => s.push_str(&format!(" — gate: {g}")),
        (None, Some(p)) => s.push_str(&format!(" — passes because {p}")),
        (None, None) => {}
    }
    s
}

fn artifact_row(a: &rupu_coverage::report::ArtifactRef) -> Vec<String> {
    let stored = match (a.stored, nonblank(&a.host)) {
        (Some(ArtifactStorage::Copied), _) => "Copied".to_string(),
        (Some(ArtifactStorage::External), Some(h)) => format!("External ({h})"),
        (Some(ArtifactStorage::External), None) => "External".to_string(),
        (None, _) => "—".to_string(),
    };
    let sha: String = a.sha256.chars().take(12).collect();
    vec![
        a.path.clone(),
        if sha.is_empty() {
            "—".to_string()
        } else {
            sha
        },
        format!("{} B", a.size),
        stored,
    ]
}

fn full_blocks(
    f: &ExportFinding,
    r: &FindingReport,
    numbers: &HashMap<String, String>,
) -> Vec<Block> {
    let mut b = vec![
        Block::Filename(number::filename(f, "pdf")),
        Block::Title(r.title.clone()),
        Block::Fields(vec![
            kv("Identifier", f.number.clone()),
            kv("Owner", r.ownership.owner.clone()),
            kv("Product", r.ownership.product.clone()),
            kv("Affected Component", r.ownership.affected_component.clone()),
            kv("Source Repository", r.ownership.source_repository.clone()),
            kv("Existing Ticket References", tickets_text(&r.tickets)),
            kv("Impact", risk(r.rating.impact)),
            kv("Category", r.category.clone()),
            kv("Attack Vector", r.attack_vector.clone()),
            kv("Likelihood", likelihood(r.rating.likelihood)),
            kv("Risk Rating", risk(r.rating.risk_rating)),
        ]),
        heading("Description"),
        prose(r.description.clone()),
        heading("Impact"),
        prose(r.impact.clone()),
        heading("Location"),
        Block::Fields(vec![
            kv("Input", r.location.input.clone()),
            kv("Output", r.location.output.clone()),
        ]),
        heading("Root Cause"),
        prose(r.root_cause.clone()),
        heading("Call Chain / Attack Flow"),
    ];

    value_or_note(&r.call_chain, &mut b, |hops, out| {
        out.push(Block::Steps(hops.iter().map(hop_step).collect()));
    });

    b.push(heading("Evidence"));
    if r.evidence.is_empty() {
        b.push(Block::Note("No evidence recorded.".to_string()));
    }
    for e in &r.evidence {
        let loc = location(e.file.as_deref(), e.lines, e.binary_va.as_deref());
        b.push(prose(match loc {
            Some(l) => format!("**{}** — {}", code_span(&l), e.claim),
            None => e.claim.clone(),
        }));
        if let Some(x) = nonblank(&e.excerpt) {
            b.push(code(e.lang.as_deref(), x));
        }
    }

    b.push(heading("Remediation"));
    b.push(prose(r.remediation.clone()));

    b.push(heading("Recommended Patch"));
    value_or_note(&r.recommended_patch, &mut b, |p, out| {
        out.push(code(Some("diff"), &p.diff));
        if let Some(n) = nonblank(&p.notes) {
            out.push(prose(n));
        }
    });

    b.push(heading("CI/CD Detection"));
    value_or_note(&r.ci_cd_detection, &mut b, |c, out| {
        out.push(prose(format!("**Stage:** {}", c.stage)));
        out.push(prose(c.body.clone()));
        if let Some(cmd) = nonblank(&c.command) {
            out.push(code(Some("sh"), cmd));
        }
        out.push(prose(format!("**Fails when:** {}", c.expect)));
    });

    b.push(heading("Regression Test"));
    value_or_note(&r.regression_test, &mut b, |t, out| {
        out.push(prose(t.body.clone()));
        out.push(code(Some("sh"), &t.command));
        out.push(Block::Fields(vec![
            kv("Vulnerable build", t.expect_vulnerable.clone()),
            kv("Patched build", t.expect_patched.clone()),
        ]));
    });

    b.push(heading("Cross-References"));
    value_or_note(&r.cross_references, &mut b, |refs, out| {
        if refs.is_empty() {
            out.push(Block::Note("None".to_string()));
            return;
        }
        let lines: Vec<String> = refs
            .iter()
            .map(|c| {
                let who = numbers.get(&c.finding_id).unwrap_or(&c.finding_id);
                match nonblank(&c.note) {
                    Some(n) => format!("- {who} ({}) — {}", relation(c.relation), oneline(n)),
                    None => format!("- {who} ({})", relation(c.relation)),
                }
            })
            .collect();
        out.push(prose(lines.join("\n")));
    });

    b.push(heading("References"));
    b.push(prose(r.references.clone()));

    b.push(Block::Fields(vec![
        kv("CVSS v3 Base Score", r.rating.cvss_v3.clone()),
        kv("Risk Factor", risk(r.rating.risk_factor)),
    ]));

    b.push(heading("Replication Steps"));
    b.push(Block::Steps(r.replication_steps.clone()));

    if !r.artifacts.is_empty() {
        b.push(heading("Artifacts"));
        b.push(Block::Table {
            headers: ["Path", "SHA-256 (first 12 chars)", "Size", "Stored"]
                .map(str::to_string)
                .to_vec(),
            rows: r.artifacts.iter().map(artifact_row).collect(),
        });
    }

    b.extend(provenance(f));
    b
}

fn summary_blocks(f: &ExportFinding) -> Vec<Block> {
    let r = &f.input.record;
    let loc = location(r.file_path.as_deref(), r.line_range, None)
        .or_else(|| nonblank(&r.target_ref).map(str::to_string))
        .unwrap_or_else(|| "—".to_string());
    let mut b = vec![
        Block::Filename(number::filename(f, "pdf")),
        Block::Title(r.summary.clone()),
        Block::Fields(vec![
            kv("Identifier", f.number.clone()),
            kv("Severity", severity_label(r.severity)),
            kv("Location", loc),
            kv("Concern", nonblank(&r.concern_id).unwrap_or("—")),
        ]),
        Block::Note("Summary finding — no full report was recorded.".to_string()),
        heading("Rationale"),
        prose(r.evidence.rationale.clone()),
    ];
    if let Some(x) = nonblank(&r.evidence.code_excerpt) {
        b.push(code(None, x));
    }
    if !r.evidence.references.is_empty() {
        b.push(heading("References"));
        b.push(prose(
            r.evidence
                .references
                .iter()
                .map(|x| format!("- {}", oneline(x)))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    b.extend(provenance(f));
    b
}

/// A whole-project report: title, a summary of what it covers, an index, then
/// every finding on its own page.
pub fn project_blocks(meta: &ReportMeta, findings: &[ExportFinding]) -> Vec<Block> {
    let numbers = number::number_map(findings);

    let counts: Vec<String> = [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
        Severity::Info,
    ]
    .into_iter()
    .filter_map(|s| {
        let n = findings
            .iter()
            .filter(|f| f.input.record.severity == s)
            .count();
        (n > 0).then(|| format!("{} {n}", severity_word(s)))
    })
    .collect();
    let total = if counts.is_empty() {
        findings.len().to_string()
    } else {
        format!("{} — {}", findings.len(), counts.join(", "))
    };

    let mut b = vec![
        Block::Title(meta.title.clone()),
        Block::Fields(vec![
            kv("Generated", rfc3339(meta.generated_at)),
            kv("Scope", meta.scope.clone()),
            kv("Findings", total),
        ]),
        heading("Index"),
        Block::Table {
            headers: [
                "Number",
                "Severity",
                "Title",
                "Project",
                "Finding ID",
                "Profile",
            ]
            .map(str::to_string)
            .to_vec(),
            rows: findings
                .iter()
                .map(|f| {
                    vec![
                        f.number.clone(),
                        severity_label(f.input.record.severity).to_string(),
                        number::title(f).to_string(),
                        f.input.project.clone(),
                        f.input.record.id.clone(),
                        profile_word(f.input.record.profile).to_string(),
                    ]
                })
                .collect(),
        },
    ];
    for f in findings {
        b.push(Block::PageBreak);
        b.extend(finding_blocks(f, &numbers));
    }
    b
}
