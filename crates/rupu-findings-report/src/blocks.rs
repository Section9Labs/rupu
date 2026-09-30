//! The renderer-neutral document model. `finding_blocks` / `project_blocks`
//! lay a finding out in the reporting standard's section order; each emitter
//! (Markdown, HTML, Typst) only decides how to draw a block, never which
//! blocks exist or in what order.

use crate::model::{ExportFinding, ReportMeta};
use crate::number;
use crate::text::{longest_backtick_run, one_line};
use chrono::{DateTime, SecondsFormat, Utc};
use rupu_coverage::report::{
    ArtifactKind, ArtifactRef, ArtifactStorage, ChainHop, FindingReport, Likelihood, OrSentinel,
    Relation, RiskLevel, Ticket, VerificationStatus, NOT_PROVIDED_PREFIX,
};
use rupu_coverage::FindingRecord;
use rupu_coverage::{FindingProfile, FindingScope, Severity, Surface};
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

/// An inline code span holding `s` verbatim: the delimiter is longer than
/// any backtick run inside, padded when the text starts or ends with one.
fn code_span(s: &str) -> String {
    let s = one_line(s);
    let ticks = "`".repeat(longest_backtick_run(&s) + 1);
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

fn scope_word(s: FindingScope) -> &'static str {
    match s {
        FindingScope::Line => "line",
        FindingScope::File => "file",
        FindingScope::Repo => "repo",
        FindingScope::Host => "host",
        FindingScope::Endpoint => "endpoint",
        FindingScope::Resource => "resource",
    }
}

fn surface_word(s: Surface) -> &'static str {
    match s {
        Surface::Workflow => "workflow",
        Surface::Agent => "agent",
        Surface::Autoflow => "autoflow",
        Surface::Session => "session",
    }
}

fn verification_word(s: VerificationStatus) -> &'static str {
    match s {
        VerificationStatus::Unverified => "Unverified",
        VerificationStatus::Confirmed => "Confirmed",
        VerificationStatus::Disputed => "Disputed",
        VerificationStatus::Inconclusive => "Inconclusive",
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
            .map(|t| {
                let mut s = format!("{} {}", t.kind, t.identifier);
                if let Some(url) = nonblank(&t.url) {
                    s.push_str(&format!(" ({url})"));
                }
                if let Some(n) = nonblank(&t.notes) {
                    s.push_str(&format!(" — {}", one_line(n)));
                }
                s
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

/// Where the record says the finding is: `file:a-b`, else the target ref.
fn record_location(r: &FindingRecord) -> Option<String> {
    location(r.file_path.as_deref(), r.line_range, None)
        .or_else(|| nonblank(&r.target_ref).map(str::to_string))
}

/// Who declared the finding, and (for a full report) the record-level
/// facts the report itself has no field for. `report` is `Some` only for the
/// full layout: the summary layout already shows location and concern up top.
fn provenance(f: &ExportFinding, report: Option<&FindingReport>) -> Vec<Block> {
    let r = &f.input.record;
    let mut rows = vec![
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
        kv("Surface", surface_word(r.declared_by.surface)),
        kv("Model", r.declared_by.model.clone()),
        kv("Declared", rfc3339(r.declared_at)),
    ];
    if let Some(report) = report {
        if let Some(c) = nonblank(&r.concern_id) {
            rows.push(kv("Concern", c));
        }
        rows.push(kv("Scope", scope_word(r.scope)));
        if let Some(l) = record_location(r) {
            rows.push(kv("Location", l));
        }
        if let Some(v) = &report.verification {
            let mut s = verification_word(v.status).to_string();
            if let Some(by) = nonblank(&v.by_run) {
                s.push_str(&format!(" by {by}"));
            }
            if let Some(n) = nonblank(&v.notes) {
                s.push_str(&format!(" — {}", one_line(n)));
            }
            rows.push(kv("Verification", s));
        }
    }
    vec![heading("Provenance"), Block::Fields(rows)]
}

/// One finding, in the reporting standard's section order. A finding with no
/// recorded report gets the short summary layout: a summary record says so,
/// and a full record whose report this build could not load says that.
pub fn finding_blocks(f: &ExportFinding, numbers: &HashMap<String, String>) -> Vec<Block> {
    let r = &f.input.record;
    match (&r.report, r.profile) {
        (Some(report), _) => full_blocks(f, report, numbers),
        (None, FindingProfile::Full) => summary_blocks(
            f,
            "Full report could not be loaded by this build — summary record shown.",
        ),
        (None, FindingProfile::Summary) => {
            summary_blocks(f, "Summary finding — no full report was recorded.")
        }
    }
}

/// One call-chain hop as a step. Every agent-supplied part is collapsed to
/// one line: a label with a newline in it must not start a Markdown block.
fn hop_step(h: &ChainHop) -> String {
    let mut s = format!("**{}**", one_line(&h.label));
    if let Some(l) = location(h.file.as_deref(), h.lines, h.binary_va.as_deref()) {
        s.push_str(&format!(" — {}", code_span(&l)));
    }
    match (nonblank(&h.gate), nonblank(&h.passes_because)) {
        (Some(g), Some(p)) => s.push_str(&format!(
            " — gate: {} (passes because {})",
            one_line(g),
            one_line(p)
        )),
        (Some(g), None) => s.push_str(&format!(" — gate: {}", one_line(g))),
        (None, Some(p)) => s.push_str(&format!(" — passes because {}", one_line(p))),
        (None, None) => {}
    }
    s
}

fn sha_prefix(sha: &str) -> Option<String> {
    let p: String = sha.chars().take(12).collect();
    (!p.is_empty()).then_some(p)
}

fn artifact_row(a: &ArtifactRef) -> Vec<String> {
    let dash = || "—".to_string();
    vec![
        a.path.clone(),
        sha_prefix(&a.sha256).unwrap_or_else(dash),
        format!("{} B", a.size),
        a.kind
            .map(|k| match k {
                ArtifactKind::Text => "Text",
                ArtifactKind::Binary => "Binary",
            })
            .map_or_else(dash, str::to_string),
        a.stored
            .map(|s| match s {
                ArtifactStorage::Copied => "Copied",
                ArtifactStorage::External => "External",
            })
            .map_or_else(dash, str::to_string),
        nonblank(&a.host).map_or_else(dash, str::to_string),
    ]
}

fn full_blocks(
    f: &ExportFinding,
    r: &FindingReport,
    numbers: &HashMap<String, String>,
) -> Vec<Block> {
    let mut identity = vec![
        kv("Identifier", f.number.clone()),
        kv("Owner", r.ownership.owner.clone()),
        kv("Product", r.ownership.product.clone()),
        kv("Affected Component", r.ownership.affected_component.clone()),
        kv("Source Repository", r.ownership.source_repository.clone()),
        kv("Existing Ticket References", tickets_text(&r.tickets)),
        kv("Impact", risk(r.rating.impact)),
        kv("Category", r.category.clone()),
    ];
    if !r.cwe.is_empty() {
        identity.push(kv("CWE", r.cwe.join(", ")));
    }
    identity.extend([
        kv("Attack Vector", r.attack_vector.clone()),
        kv("Likelihood", likelihood(r.rating.likelihood)),
        kv("Risk Rating", risk(r.rating.risk_rating)),
    ]);
    let mut b = vec![
        Block::Filename(number::filename(f, "pdf")),
        Block::Title(r.title.clone()),
        Block::Fields(identity),
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
        let mut claim = match loc {
            Some(l) => format!("**{}** — {}", code_span(&l), e.claim.trim_end()),
            None => e.claim.trim_end().to_string(),
        };
        if let Some(a) = nonblank(&e.artifact) {
            claim.push_str(&format!(" — proven by {}", code_span(a)));
        }
        if let Some(sha) = e.sha256.as_deref().and_then(sha_prefix) {
            claim.push_str(&format!(" (sha256 {sha})"));
        }
        b.push(prose(claim));
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
                    Some(n) => format!("- {who} ({}) — {}", relation(c.relation), one_line(n)),
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
            headers: [
                "Path",
                "SHA-256 (first 12 chars)",
                "Size",
                "Kind",
                "Stored",
                "Host",
            ]
            .map(str::to_string)
            .to_vec(),
            rows: r.artifacts.iter().map(artifact_row).collect(),
        });
    }

    b.extend(provenance(f, Some(r)));
    b
}

fn summary_blocks(f: &ExportFinding, note: &str) -> Vec<Block> {
    let r = &f.input.record;
    let loc = record_location(r).unwrap_or_else(|| "—".to_string());
    let mut b = vec![
        Block::Filename(number::filename(f, "pdf")),
        Block::Title(r.summary.clone()),
        Block::Fields(vec![
            kv("Identifier", f.number.clone()),
            kv("Severity", severity_label(r.severity)),
            kv("Location", loc),
            kv("Concern", nonblank(&r.concern_id).unwrap_or("—")),
        ]),
        Block::Note(note.to_string()),
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
                .map(|x| format!("- {}", one_line(x)))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    b.extend(provenance(f, None));
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
