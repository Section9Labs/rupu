//! The renderer-neutral document model. `finding_blocks` / `project_blocks`
//! lay a finding out in the reporting standard's section order; each emitter
//! (Markdown, HTML, Typst) only decides how to draw a block, never which
//! blocks exist or in what order.

use crate::model::{Blobs, ExportFinding, ReportMeta};
use crate::number;
use crate::text::{escape_semicolons, longest_backtick_run, one_line, unescape_semicolons};
use chrono::{DateTime, SecondsFormat, Utc};
use rupu_coverage::report::{
    is_sha256_hex, raster_image_type, ArtifactKind, ArtifactRef, ArtifactStorage, ChainHop,
    EvidenceBlock, FindingReport, HopRole, Likelihood, OrSentinel, Relation, RiskLevel, Ticket,
    VerificationStatus, NOT_PROVIDED_PREFIX,
};
use rupu_coverage::FindingRecord;
use rupu_coverage::{FindingProfile, FindingScope, Severity, Surface};
use std::collections::HashMap;
use std::sync::Arc;

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
    /// A raster image (PNG / JPEG / GIF / WebP by its magic bytes) read from
    /// the local artifact store, at most [`MAX_EMBED_BYTES`]. HTML embeds it
    /// as a `data:` URI and PDF as the image itself; Markdown references
    /// `path` instead (no bytes inlined). `caption` is plain text: the alt
    /// text and the line under the image. A `Prose` block naming the file
    /// (path, sha256, size) always follows it.
    Image {
        caption: String,
        path: String,
        sha256: String,
        /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
        mime: &'static str,
        bytes: Arc<[u8]>,
    },
}

/// The largest image an export embeds; a bigger one is shown by reference
/// (path, sha256, size) with a note saying it was not embedded.
pub const MAX_EMBED_BYTES: u64 = 4 * 1024 * 1024;

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

/// A file an evidence block points at, as one Markdown line: its recorded
/// path, sha256 and size, and the host it lives on when it is not here.
fn file_facts(a: &ArtifactRef) -> String {
    let mut s = code_span(&a.path);
    if !a.sha256.trim().is_empty() {
        s.push_str(&format!(" · sha256 {}", code_span(&a.sha256)));
    }
    s.push_str(&format!(" · {} bytes", a.size));
    if let Some(h) = nonblank(&a.host) {
        s.push_str(&format!(" · on host {}", code_span(h)));
    }
    s
}

/// The bytes of `a` from the local artifact store, or why there are none.
/// Only a file recorded `stored: copied` with no host is looked up, and only
/// up to `max` bytes: a file on another host, or one the store never copied,
/// is never fetched from anywhere.
fn local_blob(a: &ArtifactRef, blobs: Blobs<'_>, max: u64) -> Result<Vec<u8>, String> {
    if let Some(h) = nonblank(&a.host) {
        return Err(format!("the file is on host {}", code_span(h)));
    }
    if a.stored != Some(ArtifactStorage::Copied) {
        return Err("the file was not copied into the artifact store".to_string());
    }
    if !is_sha256_hex(&a.sha256) {
        return Err("no valid sha256 was recorded for the file".to_string());
    }
    if a.size > max {
        return Err(format!(
            "{} bytes is over the {} MiB embed limit",
            a.size,
            max / (1024 * 1024)
        ));
    }
    if !blobs.is_available() {
        return Err("no artifact store was available to this export".to_string());
    }
    blobs
        .read(&a.sha256, max)
        .ok_or_else(|| "the file is not in this machine's artifact store".to_string())
}

/// An `image` block: the image itself when it is a raster image in the local
/// store within [`MAX_EMBED_BYTES`], else the file by reference with the
/// reason it was not embedded.
fn image_blocks(artifact: &ArtifactRef, caption: Option<&str>, blobs: Blobs<'_>) -> Vec<Block> {
    let caption = caption.map(one_line).filter(|c| !c.is_empty());
    let embedded = local_blob(artifact, blobs, MAX_EMBED_BYTES).and_then(|bytes| {
        match raster_image_type(&bytes) {
            Some(mime) => Ok((mime, bytes)),
            None => Err("the file is not a PNG, JPEG, GIF or WebP image".to_string()),
        }
    });
    match embedded {
        Ok((mime, bytes)) => vec![
            Block::Image {
                caption: caption.unwrap_or_else(|| "Image".to_string()),
                path: artifact.path.clone(),
                sha256: artifact.sha256.clone(),
                mime,
                bytes: bytes.into(),
            },
            prose(file_facts(artifact)),
        ],
        Err(why) => {
            let label = match caption {
                Some(c) => format!("**Image:** {c}"),
                None => "**Image**".to_string(),
            };
            vec![prose(format!(
                "{label} — {} — *not embedded: {why}*",
                file_facts(artifact)
            ))]
        }
    }
}

/// Map one typed engagement [`EvidenceBlock`] onto the renderer-neutral
/// [`Block`] model. A code block always follows a line saying what it is, so
/// it never reads as the excerpt of the evidence claim printed before it. A
/// block's file is named by path, sha256 and size; only an `image` block's
/// file is ever read, through `blobs` (see [`image_blocks`]).
fn render_evidence_block(blk: &EvidenceBlock, blobs: Blobs<'_>, out: &mut Vec<Block>) {
    match blk {
        EvidenceBlock::Text { text } => out.push(prose(text.clone())),
        EvidenceBlock::CodeSlice {
            file,
            excerpt,
            lang,
        } => {
            match nonblank(file) {
                Some(f) => out.push(prose(format!("**{}**", code_span(f)))),
                None => out.push(prose("**Code**".to_string())),
            }
            out.push(code(lang.as_deref(), excerpt));
        }
        EvidenceBlock::Diff { diff } => {
            out.push(prose("**Diff**".to_string()));
            out.push(code(Some("diff"), diff));
        }
        EvidenceBlock::Table { headers, rows } => out.push(Block::Table {
            headers: headers.clone(),
            rows: rows.clone(),
        }),
        EvidenceBlock::Image { artifact, caption } => {
            out.extend(image_blocks(artifact, caption.as_deref(), blobs));
        }
        EvidenceBlock::Hexdump {
            base,
            artifact,
            rendered,
        } => {
            // `{:#x}` formats the u64 itself: exact for every 64-bit base.
            out.push(prose(format!(
                "**Hexdump** (base {base:#x}) — {}",
                file_facts(artifact)
            )));
            match nonblank(rendered) {
                Some(r) => out.push(code(None, r)),
                None => out.push(Block::Note(
                    "No rendered dump was recorded; the bytes are in the file above.".to_string(),
                )),
            }
        }
        EvidenceBlock::Disasm { arch, listing } => {
            out.push(prose(format!("**Disassembly** ({arch})")));
            let text = listing
                .iter()
                .map(|l| {
                    format!(
                        "{:#010x}  {:<12} {} {}",
                        l.address, l.bytes, l.mnemonic, l.ops
                    )
                    .trim_end()
                    .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            out.push(code(None, &text));
        }
        EvidenceBlock::Decompile { lang, listing } => {
            out.push(prose(format!("**Decompiled** ({lang})")));
            out.push(code(Some(lang), listing));
        }
        EvidenceBlock::HttpExchange { request, response } => {
            out.push(prose("**HTTP request**".to_string()));
            out.push(code(Some("http"), request));
            out.push(prose("**HTTP response**".to_string()));
            out.push(code(Some("http"), response));
        }
        EvidenceBlock::ScanOutput { tool, output } => {
            out.push(prose(format!("**Scan output** ({tool})")));
            out.push(code(None, output));
        }
        EvidenceBlock::PcapRef { artifact, summary } => {
            out.push(prose(format!(
                "**Packet capture** — {}",
                file_facts(artifact)
            )));
            if !summary.trim().is_empty() {
                out.push(prose(summary.clone()));
            }
        }
    }
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

/// The field holding the ticket references. Its value joins the tickets with
/// `; `, a `;` in a ticket's own text escaped as `\;` (Markdown shows `;`;
/// the HTML and Typst emitters unescape it for this field), so `import` can
/// split it back into the same tickets.
pub(crate) const TICKETS_LABEL: &str = "Existing Ticket References";

/// A `Fields` value as plain text, for the emitters that do not read it as
/// Markdown: the ticket references field's `\;` escapes undone.
pub(crate) fn field_text<'a>(label: &str, value: &'a str) -> std::borrow::Cow<'a, str> {
    if label == TICKETS_LABEL {
        unescape_semicolons(value).into()
    } else {
        value.into()
    }
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
                escape_semicolons(&s)
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
///
/// `blobs` is the local artifact store: an `image` evidence block's file is
/// embedded from it when it can be (see [`MAX_EMBED_BYTES`]).
pub fn finding_blocks(
    f: &ExportFinding,
    numbers: &HashMap<String, String>,
    blobs: Blobs<'_>,
) -> Vec<Block> {
    let r = &f.input.record;
    match (&r.report, r.profile) {
        (Some(report), _) => full_blocks(f, report, numbers, blobs),
        (None, FindingProfile::Full) => summary_blocks(
            f,
            "Full report could not be loaded by this build — summary record shown.",
        ),
        (None, FindingProfile::Summary) => {
            summary_blocks(f, "Summary finding — no full report was recorded.")
        }
    }
}

fn hop_role(r: HopRole) -> &'static str {
    match r {
        HopRole::Source => "source",
        HopRole::Hop => "hop",
        HopRole::Sink => "sink",
    }
}

/// One call-chain hop as a step, ending in its role (`source` / `hop` /
/// `sink`: the schema does not require hops in that order, so the order alone
/// does not say which is which). Every agent-supplied part is collapsed to
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
    s.push_str(&format!(" — role: {}", hop_role(h.role)));
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
    blobs: Blobs<'_>,
) -> Vec<Block> {
    let mut identity = vec![
        kv("Identifier", f.number.clone()),
        kv("Owner", r.ownership.owner.clone()),
        kv("Product", r.ownership.product.clone()),
        kv("Affected Component", r.ownership.affected_component.clone()),
        kv("Source Repository", r.ownership.source_repository.clone()),
        kv(TICKETS_LABEL, tickets_text(&r.tickets)),
        kv("Impact", risk(r.rating.impact)),
        kv("Category", r.category.clone()),
    ];
    if !r.cwe.is_empty() {
        identity.push(kv("CWE", r.cwe.join(", ")));
    }
    if !r.classifications.is_empty() {
        let joined = r
            .classifications
            .iter()
            .map(|c| format!("{} {}", c.system, c.id))
            .collect::<Vec<_>>()
            .join(", ");
        identity.push(kv("Classifications", joined));
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
        // Markdown per the report schema (both are usually code spans), so
        // prose, not plain-text fields.
        prose(format!(
            "**Input:** {}\n\n**Output:** {}",
            r.location.input.trim(),
            r.location.output.trim()
        )),
        heading("Root Cause"),
        prose(r.root_cause.clone()),
        heading("Call Chain / Attack Flow"),
    ];

    value_or_note(&r.call_chain, &mut b, |hops, out| {
        out.push(Block::Steps(hops.iter().map(hop_step).collect()));
    });

    b.push(heading("Evidence"));
    if r.evidence.is_empty() && r.blocks.is_empty() {
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
    for blk in &r.blocks {
        render_evidence_block(blk, blobs, &mut b);
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
            out.push(prose("**Command:**".to_string()));
            out.push(code(Some("sh"), cmd));
        }
        out.push(prose(format!("**Fails when:** {}", c.expect)));
    });

    b.push(heading("Regression Test"));
    value_or_note(&r.regression_test, &mut b, |t, out| {
        out.push(prose(t.body.clone()));
        out.push(prose("**Command:**".to_string()));
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

/// The head of a project report: title, a summary of what it covers, and the
/// index table. `project_blocks` continues with the per-finding pages; the
/// split archive's `index.md` is exactly this and nothing more.
pub fn index_blocks(meta: &ReportMeta, findings: &[ExportFinding]) -> Vec<Block> {
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

    vec![
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
    ]
}

/// `numbers` plus the display number of every finding in `findings`, so a
/// cross-reference between two findings of the report always resolves even
/// when the caller's map is partial (or empty).
pub(crate) fn with_own_numbers(
    numbers: &HashMap<String, String>,
    findings: &[ExportFinding],
) -> HashMap<String, String> {
    let mut all = numbers.clone();
    all.extend(number::number_map(findings));
    all
}

/// A whole-project report: title, a summary of what it covers, an index, then
/// every finding on its own page. `numbers` resolves cross-references: the
/// caller passes the numbers of every finding the selection was made from (a
/// finding the selection left out is still cited by its `SEC-00N`, as its own
/// single-finding export would cite it), and the findings' own numbers are
/// added to it. `blobs` is as for [`finding_blocks`].
pub fn project_blocks(
    meta: &ReportMeta,
    findings: &[ExportFinding],
    numbers: &HashMap<String, String>,
    blobs: Blobs<'_>,
) -> Vec<Block> {
    let numbers = with_own_numbers(numbers, findings);
    let mut b = index_blocks(meta, findings);
    for f in findings {
        b.push(Block::PageBreak);
        b.extend(finding_blocks(f, &numbers, blobs));
    }
    b
}
