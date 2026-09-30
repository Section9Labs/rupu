//! Best-effort parse of a finding report written as Markdown in the report
//! layout, back into a [`FindingReport`]. This is the migration aid behind
//! `rupu findings import` (spec: "Backfill"), not a supported input path.
//!
//! Two spellings of the layout are read:
//! - the one `markdown.rs` emits (`## Heading`, `**Label:** value`, hops as
//!   `**label** — `file:a-b` — role: sink`, claims as `**`file:a-b`** — claim`);
//! - the plain-text one used by reports written before the full profile
//!   existed (a heading is a line of its own; fields are `Label: value`).
//!
//! Nothing is invented:
//! - A section or field the schema has no sentinel for fails the parse
//!   when it is missing.
//! - A missing section or field that allows a sentinel gets it (`Unknown`,
//!   `None`, or `Not Provided — section missing from the imported report`).
//! - A missing *part* of a section that is present (a CI check with no
//!   stated stage) is [`NOT_STATED`].
//! - Text with no typed home is kept as text. Cross-references are also
//!   appended to `references` verbatim, and code blocks in a call chain
//!   become evidence claims.
//!
//! What the exporter changes on the way out is undone where it can be: the
//! `\` it puts before a line-leading `<` is removed. Prose headings it
//! shifted two levels down are kept as they are: an author's own deeper
//! heading looks the same. Values the exporter collapses to one line (fields,
//! hop labels) stay collapsed. Claim hashes it shortens are dropped: the
//! prefix cannot be checked against anything.
//!
//! The report's own finding id is read only from a labelled id line
//! ([`ID_LABELS`]: `Finding ID: fnd_…`, `Native Finding: fnd_…`, …), in the
//! header or any section but Cross-References, outside code. An id merely
//! mentioned in prose is never taken for it.
//!
//! The result must still pass `validate_report`; the caller runs it.

use crate::markdown::starts_autolink;
use rupu_coverage::report::{
    ArtifactRef, ChainHop, CiDetection, CrossRef, EvidenceClaim, FindingReport, HopRole,
    Likelihood, OrSentinel, Ownership, Patch, Rating, RegressionTest, Relation, ReportLocation,
    RiskLevel, Ticket, NOT_PROVIDED_PREFIX,
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Filler for a required part of a present section that the imported
/// report does not state.
pub const NOT_STATED: &str = "Not stated in the imported report.";
const SECTION_MISSING: &str = "section missing from the imported report";
const EXCERPT_ONLY: &str = "Code excerpt; the imported report states no claim for it.";
/// The note the exporter prints for a report with no evidence claims.
const NO_EVIDENCE: &str = "No evidence recorded.";
/// Heading of the block appended to `references` that holds the imported
/// text no field has a place for.
pub const OTHER_TEXT: &str = "Other imported text:";
/// Fields never kept as other text: the file name is derived from the
/// identifier and title. (An id line is dropped only when it says nothing
/// but the id; see [`ID_LABELS`].)
const NEVER_KEPT: &[&str] = &["filename"];

/// Labels (lower-case; `**Label:**`, `**Label**:` and `Label:` all read)
/// of a line that states the report's own finding id. The exporter writes
/// `**Finding ID:** fnd_…` in Provenance.
pub const ID_LABELS: &[&str] = &[
    "finding id",
    "native finding",
    "native finding id",
    "rupu finding",
    "rupu finding id",
    "native rupu finding",
];

// `Report` dwarfs `NotAReport`, but one value is built per file and moved
// straight out, and the variant's shape is the module's contract.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// A finding report, and the distinct finding ids its labelled id lines
    /// state ([`ID_LABELS`]), in the order they appear: its own id, when
    /// there is exactly one. Ids cited anywhere else are not in it.
    Report {
        report: FindingReport,
        own_ids: Vec<String>,
    },
    /// Fewer than three of the layout's section headings and no labelled
    /// finding id: not a finding report (an index, a README). Skipped, not an
    /// error.
    NotAReport,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("missing section `{0}`")]
    MissingSection(&'static str),
    #[error("missing field `{0}`")]
    MissingField(&'static str),
    #[error("`{field}` is `{value}`, not one of {allowed}")]
    BadValue {
        field: &'static str,
        value: String,
        allowed: &'static str,
    },
    #[error("the file holds {0} findings; split it into one file per finding first")]
    SeveralFindings(usize),
    /// A labelled finding id, but fewer than three of the layout's section
    /// headings: meant as a report, so not skipped.
    #[error("has a Finding ID line but is missing the report sections")]
    MissingReportSections,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Sec {
    Description,
    Impact,
    Location,
    RootCause,
    CallChain,
    Evidence,
    Remediation,
    Patch,
    CiCd,
    Regression,
    CrossRefs,
    References,
    Replication,
    Artifacts,
    Provenance,
}

impl Sec {
    fn name(self) -> &'static str {
        match self {
            Sec::Description => "Description",
            Sec::Impact => "Impact",
            Sec::Location => "Location",
            Sec::RootCause => "Root Cause",
            Sec::CallChain => "Call Chain / Attack Flow",
            Sec::Evidence => "Evidence",
            Sec::Remediation => "Remediation",
            Sec::Patch => "Recommended Patch",
            Sec::CiCd => "CI/CD Detection",
            Sec::Regression => "Regression Test",
            Sec::CrossRefs => "Cross-References",
            Sec::References => "References",
            Sec::Replication => "Replication Steps",
            Sec::Artifacts => "Artifacts",
            Sec::Provenance => "Provenance",
        }
    }
}

/// Heading text (lower-case, `#`/`**`/trailing `:` removed) → section.
const SECTIONS: &[(&str, Sec)] = &[
    ("description", Sec::Description),
    ("impact", Sec::Impact),
    ("location", Sec::Location),
    ("root cause", Sec::RootCause),
    ("call chain / attack flow", Sec::CallChain),
    ("call chain/attack flow", Sec::CallChain),
    ("call chain", Sec::CallChain),
    ("attack flow", Sec::CallChain),
    ("evidence", Sec::Evidence),
    ("remediation", Sec::Remediation),
    ("recommended patch", Sec::Patch),
    ("ci/cd detection", Sec::CiCd),
    ("ci cd detection", Sec::CiCd),
    ("regression test", Sec::Regression),
    ("cross-references", Sec::CrossRefs),
    ("cross references", Sec::CrossRefs),
    ("references", Sec::References),
    ("replication steps", Sec::Replication),
    ("artifacts", Sec::Artifacts),
    ("provenance", Sec::Provenance),
];

/// Field labels of the layout's header block (lower-case). The id labels
/// are fields too, so an id line is never taken for the title.
const FIELDS: &[&str] = &[
    "filename",
    "identifier",
    "finding id",
    "native finding",
    "native finding id",
    "rupu finding",
    "rupu finding id",
    "native rupu finding",
    "owner",
    "product",
    "affected component",
    "source repository",
    "existing ticket references",
    "impact",
    "category",
    "cwe",
    "attack vector",
    "likelihood",
    "risk rating",
    "cvss v3 base score",
    "cvss v3",
    "cvss",
    "risk factor",
    "severity",
];

/// The rating fields the layout places after References' text, outside any
/// heading of their own.
const TRAILING_FIELDS: &[&str] = &["cvss v3 base score", "cvss v3", "cvss", "risk factor"];

pub fn parse_report(md: &str) -> Result<Parsed, ImportError> {
    // A byte-order mark (Windows editors write one) would hide the first
    // line's label.
    let md = md.strip_prefix('\u{feff}').unwrap_or(md);
    // CommonMark also ends a line at a lone `\r`.
    let md = md.replace("\r\n", "\n").replace('\r', "\n");
    let mut doc = split(&md);
    let kinds: HashSet<Sec> = doc.sections.iter().map(|(s, _)| *s).collect();
    if kinds.len() < 3 {
        // A labelled id says the file is meant as a report: say so rather
        // than skip it.
        let labelled =
            !header_id_lines(&doc.header).is_empty() || !take_id_lines(&mut doc).0.is_empty();
        return if labelled {
            Err(ImportError::MissingReportSections)
        } else {
            Ok(Parsed::NotAReport)
        };
    }
    let findings = doc.filename_lines.max(doc.description_headings);
    if findings > 1 {
        return Err(ImportError::SeveralFindings(findings));
    }

    let mut header = header(&doc.header);
    let trailing = take_trailing_fields(&mut doc, &mut header);

    // The report's own id: labelled id lines only. A header id line that
    // says nothing but the id is consumed; one that says more is kept as
    // other text, as is one that holds no id.
    let mut own_ids: Vec<String> = Vec::new();
    for f in &mut header.fields {
        if !ID_LABELS.contains(&f.key.as_str()) {
            continue;
        }
        if fnd_ids(&f.value).is_empty() {
            continue;
        }
        let ids = leading_ids(&f.value);
        f.used = !ids.is_empty() && !says_more_than(&f.value, &ids);
        own_ids.extend(ids);
    }
    let (section_ids, id_text) = take_id_lines(&mut doc);
    own_ids.extend(section_ids);
    let mut seen: HashSet<String> = HashSet::new();
    own_ids.retain(|id| seen.insert(id.clone()));

    // Text no field consumed, as (where it came from, text): appended to
    // `references` so nothing in the imported report is lost.
    let mut other: Vec<(String, String)> = Vec::new();

    let title = header
        .title
        .clone()
        .ok_or(ImportError::MissingField("title"))?;
    let unknown = || "Unknown".to_string();
    let ownership = Ownership {
        owner: header.take(&["owner"]).unwrap_or_else(unknown),
        product: header.take(&["product"]).unwrap_or_else(unknown),
        affected_component: header.take(&["affected component"]).unwrap_or_else(unknown),
        source_repository: header.take(&["source repository"]).unwrap_or_else(unknown),
    };
    let (tickets, ticket_text) = tickets(header.take(&["existing ticket references"]));
    let (impact, impact_note) = risk("Impact", header.take(&["impact"]))?;
    let (likelihood, likelihood_note) = likelihood(header.take(&["likelihood"]))?;
    let (risk_rating, risk_rating_note) = risk("Risk Rating", header.take(&["risk rating"]))?;
    let (risk_factor, risk_factor_note) = risk("Risk Factor", header.take(&["risk factor"]))?;
    let rating = Rating {
        impact,
        likelihood,
        risk_rating,
        risk_factor,
        cvss_v3: header
            .take(&["cvss v3 base score", "cvss v3", "cvss"])
            .unwrap_or_else(unknown),
    };
    let category = header
        .take(&["category"])
        .ok_or(ImportError::MissingField("Category"))?;
    let attack_vector = header
        .take(&["attack vector"])
        .ok_or(ImportError::MissingField("Attack Vector"))?;
    let cwe_field = header.take(&["cwe"]);

    let required = |s: Sec| section(&doc, s).ok_or(ImportError::MissingSection(s.name()));
    let description = required(Sec::Description)?.text();
    let impact_text = required(Sec::Impact)?.text();
    let location = location(&required(Sec::Location)?);
    let root_cause = required(Sec::RootCause)?.text();
    let (call_chain, chain_claims) = call_chain(section(&doc, Sec::CallChain).as_ref());
    let mut evidence = evidence(&required(Sec::Evidence)?);
    if evidence.is_empty() {
        return Err(ImportError::MissingSection(Sec::Evidence.name()));
    }
    evidence.extend(chain_claims);
    let remediation = required(Sec::Remediation)?.text();
    let recommended_patch = patch(section(&doc, Sec::Patch).as_ref());
    let ci_cd_detection = ci(section(&doc, Sec::CiCd).as_ref());
    let regression_test = regression(section(&doc, Sec::Regression).as_ref());
    let replication_steps = steps(&required(Sec::Replication)?);
    if replication_steps.is_empty() {
        return Err(ImportError::MissingSection(Sec::Replication.name()));
    }
    let references_body = required(Sec::References)?;
    let mut references = references_body.text();
    let cwe = match &cwe_field {
        Some(v) => cwe_ids(v),
        None => fallback_cwe_ids(&category, &references_body),
    };
    let (cross_references, verbatim) = cross_refs(section(&doc, Sec::CrossRefs).as_ref());
    if let Some(text) = verbatim {
        references = format!("{references}\n\nCross-references (imported):\n\n{text}");
    }
    let (artifacts, artifact_text) = section(&doc, Sec::Artifacts)
        .map(|b| artifacts(&b))
        .unwrap_or_default();

    other.extend(header.leftovers());
    other.extend(id_text);
    let notes = [
        ("Ticket references", ticket_text),
        ("Impact rating", impact_note),
        ("Likelihood rating", likelihood_note),
        ("Risk Rating", risk_rating_note),
        ("Risk Factor", risk_factor_note),
        ("CWE", cwe_field.filter(|v| cwe_leftover(v))),
    ];
    for (at, text) in notes {
        if let Some(text) = text {
            other.push((at.to_string(), text));
        }
    }
    for i in trailing {
        if let Some(raw) = header.unused_raw(i) {
            other.push(("References".to_string(), raw));
        }
    }
    other.extend(artifact_text.map(|t| ("Artifacts".to_string(), t)));
    other.extend(
        section(&doc, Sec::Provenance)
            .and_then(|b| provenance(&b))
            .map(|t| ("Provenance".to_string(), t)),
    );
    if let Some(block) = other_block(&other) {
        references = format!("{references}\n\n{block}");
    }

    Ok(Parsed::Report {
        report: FindingReport {
            title,
            ownership,
            tickets,
            rating,
            category,
            attack_vector,
            cwe,
            description,
            impact: impact_text,
            location,
            root_cause,
            call_chain,
            evidence,
            remediation,
            recommended_patch,
            ci_cd_detection,
            regression_test,
            replication_steps,
            cross_references,
            references,
            artifacts,
            verification: None,
        },
        own_ids,
    })
}

/// The ids on `line` when it is a labelled id line ([`ID_LABELS`]) naming at
/// least one `fnd_` id, and whether it says more than its ids.
fn id_label(line: &str) -> Option<(Vec<String>, bool)> {
    let (key, value) = labelled(line)?;
    if !ID_LABELS.contains(&key.as_str()) {
        return None;
    }
    if fnd_ids(&value).is_empty() {
        return None;
    }
    let ids = leading_ids(&value);
    let more = ids.is_empty() || says_more_than(&value, &ids);
    Some((ids, more))
}

/// The `fnd_` ids a labelled id line's value starts with. Only those name
/// the report's own finding: `fnd_A` or `` `fnd_A` (merged from an earlier
/// run) `` does, `unassigned, duplicate of fnd_B` does not (that line is kept
/// as other text instead).
fn leading_ids(value: &str) -> Vec<String> {
    let lead = |c: char| c.is_whitespace() || matches!(c, '`' | '*' | '_' | '(' | '[' | '"' | '\'');
    let sep = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '`' | '*' | ',' | ';' | '/' | '&' | ')' | ']' | '"' | '\''
            )
    };
    let mut out: Vec<String> = Vec::new();
    let mut rest = value.trim_start_matches(lead);
    while let Some(id) = id_at_start(rest) {
        rest = rest[id.len()..].trim_start_matches(sep);
        if !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// The `fnd_<ULID>` id `s` starts with, if any (same shape as [`fnd_ids`]).
fn id_at_start(s: &str) -> Option<&str> {
    let body = s.strip_prefix("fnd_")?.as_bytes();
    let ulid = body.get(..26)?;
    let whole = ulid
        .iter()
        .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        && !body.get(26).is_some_and(|b| b.is_ascii_alphanumeric());
    whole.then(|| &s[..30])
}

/// Whether `value` holds text beyond `ids` (and the punctuation, emphasis
/// or code span around them).
fn says_more_than(value: &str, ids: &[String]) -> bool {
    let mut rest = value.to_string();
    for id in ids {
        rest = rest.replace(id.as_str(), "");
    }
    rest.chars().any(char::is_alphanumeric)
}

/// Whether each line is inside a fenced code block (the fence lines
/// included).
fn code_mask(lines: &[String]) -> Vec<bool> {
    let mut fence: Option<(char, usize)> = None;
    lines
        .iter()
        .map(|line| {
            if let Some(open) = fence {
                if closes(line, open) {
                    fence = None;
                }
                true
            } else if let Some(open) = opens(line) {
                fence = Some(open);
                true
            } else {
                false
            }
        })
        .collect()
}

/// Four spaces or a tab of indentation: an indented code line.
fn indented_code(line: &str) -> bool {
    let unspaced = line.trim_start_matches(' ');
    line.len() - unspaced.len() >= 4 || unspaced.starts_with('\t')
}

/// The ids of the header's labelled id lines, outside code.
fn header_id_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .zip(code_mask(lines))
        .filter(|(_, code)| !code)
        .filter_map(|(l, _)| id_label(l))
        .flat_map(|(ids, _)| ids)
        .collect()
}

/// Take the labelled id lines out of every section but Cross-References
/// (where an id names another finding), outside code. Returns their ids,
/// and each line that says more than its id as (section, line) for Other
/// imported text. In Provenance, whose text is all kept as other text, such
/// a line stays where it is instead.
fn take_id_lines(doc: &mut Doc) -> (Vec<String>, Vec<(String, String)>) {
    let mut ids: Vec<String> = Vec::new();
    let mut kept: Vec<(String, String)> = Vec::new();
    for (s, body) in &mut doc.sections {
        let sec = *s;
        if sec == Sec::CrossRefs {
            continue;
        }
        let code = code_mask(body);
        let mut i = 0;
        body.retain(|line| {
            let in_code = code[i] || indented_code(line);
            i += 1;
            let Some((found, more)) = (!in_code).then(|| id_label(line)).flatten() else {
                return true;
            };
            ids.extend(found);
            if more && sec == Sec::Provenance {
                return true;
            }
            if more {
                kept.push((sec.name().to_string(), line.trim_end().to_string()));
            }
            false
        });
    }
    (ids, kept)
}

/// CWE ids when the report has no CWE field: every id in Category, and
/// every id on a References line that starts with one (`CWE-639: …`,
/// `- CWE-639 …`). An id mentioned in passing ("unlike CWE-79 …") is not
/// taken.
fn fallback_cwe_ids(category: &str, references: &Body) -> Vec<String> {
    let mut text = category.to_string();
    for (line, code) in references.lines() {
        let t = strip_marker(line).trim_start_matches(['[', '*', '_', '`', '(']);
        let starts = t.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("cwe-"))
            && t[4..].starts_with(|c: char| c.is_ascii_digit());
        if !code && starts {
            text.push('\n');
            text.push_str(line);
        }
    }
    cwe_ids(&text)
}

/// The heading `Other imported text:` and one `<where>:` block per source,
/// adjacent entries from the same place merged.
fn other_block(entries: &[(String, String)]) -> Option<String> {
    let mut merged: Vec<(&str, String)> = Vec::new();
    for (at, text) in entries {
        let text = text.trim_matches('\n');
        match merged.last_mut() {
            Some((last, body)) if *last == at.as_str() => {
                body.push_str("\n\n");
                body.push_str(text);
            }
            _ => merged.push((at.as_str(), text.to_string())),
        }
    }
    if merged.is_empty() {
        return None;
    }
    let parts: Vec<String> = merged
        .into_iter()
        .map(|(at, body)| {
            if body.trim().is_empty() {
                format!("{at}:")
            } else {
                format!("{at}:\n\n{body}")
            }
        })
        .collect();
    Some(format!("{OTHER_TEXT}\n\n{}", parts.join("\n\n")))
}

/// Distinct `fnd_<ULID>` ids in `text`, in first-seen order. An id must be
/// exactly 26 upper-case alphanumerics after `fnd_`, not glued to a longer
/// word on either side.
pub fn fnd_ids(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut from = 0;
    while let Some(off) = text[from..].find("fnd_") {
        let start = from + off;
        let tail = &text[start + 4..];
        let n = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        let glued = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let glued_before = text[..start].chars().next_back().is_some_and(glued);
        if n == 26
            && !glued_before
            && !tail[26..].starts_with('_')
            && tail[..26]
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        {
            let id = format!("fnd_{}", &tail[..26]);
            if seen.insert(id.clone()) {
                out.push(id);
            }
        }
        from = start + 4;
    }
    out
}

/// Drop cross-references to findings not in `known`: the ledger accepts
/// references to its own findings only. Their text is already in
/// `references`. An emptied list becomes `None`.
///
/// The parser links every `fnd_` id the Cross-References section names,
/// the report's own included, so the caller passes the ledger's ids *minus*
/// the id the report is being attached to, and a self-reference is dropped
/// here.
pub fn retain_known_cross_references(report: &mut FindingReport, known: &HashSet<String>) {
    if let OrSentinel::Value(refs) = &mut report.cross_references {
        refs.retain(|r| known.contains(&r.finding_id));
        if refs.is_empty() {
            report.cross_references = OrSentinel::Sentinel("None".into());
        }
    }
}

// ---- document structure ----------------------------------------------------

#[derive(Debug, Default)]
struct Doc {
    header: Vec<String>,
    sections: Vec<(Sec, Vec<String>)>,
    /// `Filename: ….pdf` lines outside code: the layout starts every
    /// finding with one.
    filename_lines: usize,
    /// Description headings at levels 0–2 (bare, bold, `#`, `##`), not only
    /// the sections' level: a second finding in another spelling still has
    /// one.
    description_headings: usize,
}

/// Number of leading `#`s (0 for a bare or bold heading line).
fn heading_level(line: &str) -> usize {
    line.trim_start().chars().take_while(|&c| c == '#').count()
}

/// Split into the header block and the sections, by heading lines outside
/// code blocks. A section may appear more than once; `section` merges them.
///
/// The layout's section headings share one level, and the most common level
/// among lines that name a section is taken as that level (a tie goes to
/// the shallower one). So a deeper heading inside prose (the exporter
/// shifts those to h3 and below) and a stray bare line that happens to say
/// "Evidence" are not sections. A section with no heading at that level
/// takes its first heading at another level instead, so an optional section
/// written one level off is still read rather than reported missing.
fn split(md: &str) -> Doc {
    let lines: Vec<&str> = md.lines().collect();
    let mut in_code = vec![false; lines.len()];
    let mut candidates: Vec<(usize, Sec, usize)> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(open) = fence {
            in_code[i] = true;
            if closes(line, open) {
                fence = None;
            }
        } else if let Some(open) = opens(line) {
            in_code[i] = true;
            fence = Some(open);
        } else if let Some(sec) = heading(line) {
            candidates.push((i, sec, heading_level(line)));
        }
    }
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
    for (_, _, level) in &candidates {
        *counts.entry(*level).or_default() += 1;
    }
    let level = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(l, _)| *l);
    let mut headings: HashMap<usize, Sec> = candidates
        .iter()
        .filter(|(_, _, l)| Some(*l) == level)
        .map(|(i, s, _)| (*i, *s))
        .collect();
    let at_level: HashSet<Sec> = headings.values().copied().collect();
    let mut adopted: HashSet<Sec> = HashSet::new();
    for (i, s, _) in &candidates {
        if !at_level.contains(s) && adopted.insert(*s) {
            headings.insert(*i, *s);
        }
    }

    let mut doc = Doc {
        // `###` and deeper is prose: the exporter shifts an author's own
        // headings there.
        description_headings: candidates
            .iter()
            .filter(|(_, s, l)| *s == Sec::Description && *l <= 2)
            .count(),
        ..Doc::default()
    };
    for (i, line) in lines.iter().enumerate() {
        if let Some(sec) = headings.get(&i) {
            doc.sections.push((*sec, Vec::new()));
            continue;
        }
        let names_a_pdf =
            |(k, v): (String, String)| k == "filename" && v.to_ascii_lowercase().ends_with(".pdf");
        if !in_code[i] && field(line).is_some_and(names_a_pdf) {
            doc.filename_lines += 1;
        }
        match doc.sections.last_mut() {
            Some((_, body)) => body.push(line.to_string()),
            None => doc.header.push(line.to_string()),
        }
    }
    doc
}

/// Move the CVSS / Risk Factor lines that follow References' text into the
/// header fields, fence-aware, and return their indices there. Only
/// References is searched, so a prose line elsewhere that happens to start
/// `Risk factor:` stays prose.
fn take_trailing_fields(doc: &mut Doc, into: &mut Header) -> Vec<usize> {
    let mut taken = Vec::new();
    for (s, body) in &mut doc.sections {
        if *s != Sec::References {
            continue;
        }
        let mut fence: Option<(char, usize)> = None;
        body.retain(|line| {
            if let Some(open) = fence {
                if closes(line, open) {
                    fence = None;
                }
                return true;
            }
            if let Some(open) = opens(line) {
                fence = Some(open);
                return true;
            }
            match field(line) {
                Some((key, value)) if TRAILING_FIELDS.contains(&key.as_str()) => {
                    taken.push(into.fields.len());
                    into.fields.push(HField {
                        key,
                        value,
                        raw: vec![line.trim_end().to_string()],
                        used: false,
                    });
                    false
                }
                _ => true,
            }
        });
    }
    taken
}

fn section(doc: &Doc, s: Sec) -> Option<Body> {
    let mut lines: Vec<String> = Vec::new();
    for (_, body) in doc.sections.iter().filter(|(k, _)| *k == s) {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(body.iter().cloned());
    }
    let body = Body::parse(&lines);
    (!body.text().is_empty()).then_some(body)
}

/// A fence opener: 3+ backticks or tildes (a backtick fence's info string
/// holds no backtick, or the line is inline code, not a fence).
fn opens(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start();
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = t.chars().take_while(|&x| x == c).count();
    let info_ok = c == '~' || !t[n..].contains('`');
    (n >= 3 && info_ok).then_some((c, n))
}

fn closes(line: &str, (c, n): (char, usize)) -> bool {
    let t = line.trim();
    t.len() >= n && t.chars().all(|x| x == c)
}

fn heading(line: &str) -> Option<Sec> {
    if indented_code(line) {
        return None;
    }
    let mut t = line.trim();
    let atx = t.starts_with('#');
    t = t.trim_start_matches('#').trim();
    if atx {
        // An optional closing sequence: `## Evidence ##`.
        let open = t.trim_end_matches('#');
        if open.len() < t.len() && (open.is_empty() || open.ends_with([' ', '\t'])) {
            t = open.trim_end();
        }
    }
    // "## 3. Root Cause"
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        if let Some(r) = t[digits..].strip_prefix(['.', ')']) {
            t = r.trim();
        }
    }
    let t = strip_pair(strip_pair(t, "**"), "__");
    let key = t.trim().trim_end_matches(':').trim().to_ascii_lowercase();
    SECTIONS.iter().find(|(n, _)| *n == key).map(|(_, s)| *s)
}

fn strip_pair<'a>(s: &'a str, p: &str) -> &'a str {
    s.strip_prefix(p)
        .and_then(|r| r.strip_suffix(p))
        .unwrap_or(s)
}

/// `**Label:** value`, `**Label**: value` or `Label: value`, as
/// (lower-case label, value without surrounding emphasis).
fn labelled(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let (label, rest) = if let Some(r) = t.strip_prefix("**") {
        let end = r.find("**")?;
        let (inner, after) = (&r[..end], &r[end + 2..]);
        match inner.strip_suffix(':') {
            Some(l) => (l, after),
            None => (inner, after.strip_prefix(':')?),
        }
    } else {
        let colon = t.find(':')?;
        (&t[..colon], &t[colon + 1..])
    };
    let label = label.trim();
    if label.is_empty() || label.len() > 40 || label.contains('`') {
        return None;
    }
    Some((
        label.to_ascii_lowercase(),
        unwrap_emphasis(rest.trim()).to_string(),
    ))
}

fn field(line: &str) -> Option<(String, String)> {
    labelled(line).filter(|(k, _)| FIELDS.contains(&k.as_str()))
}

/// `_text_` / `*text*` → `text` (the exporter's notes).
fn unwrap_emphasis(s: &str) -> &str {
    if s.len() > 2 && !s.starts_with("**") && !s.starts_with("__") {
        for p in ["_", "*"] {
            if let Some(inner) = s.strip_prefix(p).and_then(|r| r.strip_suffix(p)) {
                return inner.trim();
            }
        }
    }
    s
}

fn is_thematic_break(t: &str) -> bool {
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

/// The text after a leading `Step N:` / `Step N.`.
fn step_marker(line: &str) -> Option<&str> {
    let t = line.trim();
    if !t.get(..5)?.eq_ignore_ascii_case("step ") {
        return None;
    }
    let r = &t[5..];
    let d = r.chars().take_while(|c| c.is_ascii_digit()).count();
    if d == 0 {
        return None;
    }
    r[d..].strip_prefix([':', '.']).map(str::trim)
}

/// The text after a leading `- `, `* `, `+ `, `N. ` or `N) `.
fn list_marker(line: &str) -> Option<&str> {
    let t = line.trim();
    for m in ["- ", "* ", "+ "] {
        if let Some(r) = t.strip_prefix(m) {
            return Some(r.trim());
        }
    }
    let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if d == 0 {
        return None;
    }
    t[d..]
        .strip_prefix(". ")
        .or_else(|| t[d..].strip_prefix(") "))
        .map(str::trim)
}

/// A leading `Step N:`, `- `, `* `, `+ `, `N. ` or `N) ` removed.
fn strip_marker(line: &str) -> &str {
    step_marker(line)
        .or_else(|| list_marker(line))
        .unwrap_or_else(|| line.trim())
}

fn is_list_item(line: &str) -> bool {
    strip_marker(line).len() != line.trim().len()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn missing<T>() -> OrSentinel<T> {
    OrSentinel::Sentinel(format!("{NOT_PROVIDED_PREFIX}{SECTION_MISSING}"))
}

const NOT_GIVEN: &str = "not given in the imported report";

/// `text` without the emphasis around all of it (`_x_`, `*x*`, `**x**`,
/// `__x__`, nested).
fn strip_emphasis(text: &str) -> &str {
    let mut t = text.trim();
    loop {
        let inner = ["**", "__", "*", "_"]
            .iter()
            .find_map(|p| t.strip_prefix(p).and_then(|r| r.strip_suffix(p)))
            .map(str::trim);
        match inner {
            Some(i) if !i.is_empty() && i.len() < t.len() => t = i,
            _ => return t,
        }
    }
}

/// Text that stands in for content instead of giving it: `None`, `N/A`,
/// `TBD`, `Not provided …` and the like, emphasis and a trailing `.`
/// ignored.
fn sentinel_like(text: &str) -> bool {
    let t = strip_emphasis(text.trim().trim_end_matches('.'))
        .trim_end_matches('.')
        .trim()
        .to_ascii_lowercase();
    matches!(
        t.as_str(),
        "none" | "none provided" | "n/a" | "na" | "not applicable" | "unknown" | "tbd" | "-"
    ) || t.trim_start_matches(['*', '_']).starts_with("not provided")
}

/// A whole section that says "Not provided …", normalized to the sentinel.
fn not_provided(text: &str) -> Option<String> {
    let t = strip_emphasis(text).trim_start_matches(['*', '_']);
    if !t.get(..12)?.eq_ignore_ascii_case("not provided") {
        return None;
    }
    let why = t[12..].trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, ':' | '—' | '–' | '-' | '*' | '_')
    });
    let why = one_line(why);
    Some(format!(
        "{NOT_PROVIDED_PREFIX}{}",
        if why.chars().any(char::is_alphanumeric) {
            why.as_str()
        } else {
            NOT_GIVEN
        }
    ))
}

/// The sentinel for a section whose whole text is sentinel-like: the
/// `Not provided …` normalization, else the text itself as the reason.
fn stands_in(text: &str) -> Option<String> {
    if let Some(s) = not_provided(text) {
        return Some(s);
    }
    if !sentinel_like(text) {
        return None;
    }
    let said = one_line(strip_emphasis(text));
    Some(format!(
        "{NOT_PROVIDED_PREFIX}{}",
        if said.chars().any(char::is_alphanumeric) {
            said.as_str()
        } else {
            NOT_GIVEN
        }
    ))
}

// ---- header ----------------------------------------------------------------

/// A header field: its lower-case label, its value (continuation lines
/// included), the lines it was read from, and whether a report field took
/// it.
#[derive(Debug)]
struct HField {
    key: String,
    value: String,
    raw: Vec<String>,
    used: bool,
}

/// The header block, line by line, so what no field takes can be kept in
/// document order.
#[derive(Debug)]
enum HLine {
    Field(usize),
    /// A line of text outside code: a title candidate.
    Text(String),
    /// A line of a code block: kept, never the title.
    Code(String),
    Gap,
    /// A `#` line (trimmed) before the first section. A title candidate;
    /// otherwise an unknown heading, and the text under it is kept under its
    /// name.
    Heading(String),
}

#[derive(Debug, Default)]
struct Header {
    title: Option<String>,
    fields: Vec<HField>,
    lines: Vec<HLine>,
}

impl Header {
    /// The first non-blank value for any of `keys`, marked as taken.
    fn take(&mut self, keys: &[&str]) -> Option<String> {
        let f = self
            .fields
            .iter_mut()
            .find(|f| keys.contains(&f.key.as_str()) && !f.value.trim().is_empty())?;
        f.used = true;
        Some(f.value.trim().to_string())
    }

    /// Field `i`'s lines, unless a report field took it, it is empty, or it
    /// is never kept.
    fn unused_raw(&self, i: usize) -> Option<String> {
        let f = &self.fields[i];
        let keep = !f.used && !f.value.trim().is_empty() && !NEVER_KEPT.contains(&f.key.as_str());
        keep.then(|| f.raw.join("\n"))
    }

    /// The header's text that is neither the title nor a taken field, as
    /// (`Header` or an unknown heading's name, text).
    fn leftovers(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut at = "Header".to_string();
        let mut buf: Vec<String> = Vec::new();
        let mut flush = |at: &str, buf: &mut Vec<String>, heading: bool| {
            let text = buf.join("\n").trim_matches('\n').to_string();
            buf.clear();
            if heading || !text.trim().is_empty() {
                out.push((at.to_string(), text));
            }
        };
        let mut under_heading = false;
        for l in &self.lines {
            match l {
                HLine::Heading(h) => {
                    flush(&at, &mut buf, under_heading);
                    at = h.trim_matches('#').trim().to_string();
                    under_heading = true;
                }
                HLine::Text(t) | HLine::Code(t) => buf.push(t.clone()),
                // One blank line between kept paragraphs, however many there were.
                HLine::Gap if buf.last().is_some_and(|x| !x.is_empty()) => buf.push(String::new()),
                HLine::Gap => {}
                HLine::Field(i) => buf.extend(self.unused_raw(*i)),
            }
        }
        flush(&at, &mut buf, under_heading);
        out
    }

    /// Take the title out of the lines: a level-1 `#` heading, else the
    /// first non-blank line after `Filename:` when that is not a field, else
    /// the first line that is not a field. Anything before it (a banner) is
    /// left as other text.
    fn take_title(&mut self) {
        fn title_of(l: &HLine) -> Option<String> {
            let (HLine::Text(raw) | HLine::Heading(raw)) = l else {
                return None;
            };
            let t = raw.trim().trim_start_matches('#').trim();
            let t = strip_pair(strip_pair(t, "**"), "__").trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        /// `# Title`: exactly one `#`, then a space or nothing.
        fn level_one(l: &HLine) -> bool {
            let HLine::Heading(raw) = l else {
                return false;
            };
            raw.strip_prefix('#')
                .is_some_and(|r| r.is_empty() || r.starts_with([' ', '\t']))
        }
        let lines = &self.lines;
        let atx = || {
            lines
                .iter()
                .position(|l| level_one(l) && title_of(l).is_some())
        };
        let after_filename = || {
            let f = lines
                .iter()
                .position(|l| matches!(l, HLine::Field(i) if self.fields[*i].key == "filename"))?;
            let next = f
                + 1
                + lines[f + 1..]
                    .iter()
                    .position(|l| !matches!(l, HLine::Gap))?;
            title_of(&lines[next]).map(|_| next)
        };
        let first = || lines.iter().position(|l| title_of(l).is_some());
        if let Some(i) = atx().or_else(after_filename).or_else(first) {
            self.title = title_of(&self.lines[i]);
            self.lines[i] = HLine::Gap;
        }
    }
}

/// Whether `line` continues the field `f` above it: an indented line or a
/// list item that is not a field of its own. A ticket list's own labels
/// (`Identifier:`, `URL:` …) belong to it. `Filename:` and the id lines are
/// single values and never continue.
fn continues_field(f: &HField, line: &str) -> bool {
    let tickets = f.key == "existing ticket references";
    f.key != "filename"
        && !ID_LABELS.contains(&f.key.as_str())
        && (line.starts_with(char::is_whitespace) || is_list_item(line))
        && (tickets || field(line).is_none())
}

fn header(lines: &[String]) -> Header {
    let mut h = Header::default();
    // The field the previous line belongs to, while no blank line has
    // intervened.
    let mut open_field: Option<usize> = None;
    let mut fence: Option<(char, usize)> = None;
    for line in lines {
        if let Some(open) = fence {
            h.lines.push(HLine::Code(line.to_string()));
            if closes(line, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = opens(line) {
            h.lines.push(HLine::Code(line.to_string()));
            fence = Some(open);
            open_field = None;
            continue;
        }
        let t = line.trim();
        if t.is_empty() || is_thematic_break(t) {
            h.lines.push(HLine::Gap);
            open_field = None;
            continue;
        }
        if let Some(i) = open_field.filter(|i| continues_field(&h.fields[*i], line)) {
            // A ticket list's lines stay lines.
            let f = &mut h.fields[i];
            let joint = if f.key == "existing ticket references" {
                '\n'
            } else {
                ' '
            };
            if !f.value.is_empty() {
                f.value.push(joint);
            }
            f.value.push_str(t);
            f.raw.push(line.trim_end().to_string());
            continue;
        }
        open_field = None;
        if let Some((key, value)) = field(line) {
            open_field = Some(h.fields.len());
            h.lines.push(HLine::Field(h.fields.len()));
            h.fields.push(HField {
                key,
                value,
                raw: vec![line.trim_end().to_string()],
                used: false,
            });
        } else if t.starts_with('#') {
            h.lines.push(HLine::Heading(t.to_string()));
        } else {
            h.lines.push(HLine::Text(line.trim_end().to_string()));
        }
    }
    h.take_title();
    h
}

/// A rating value's leading level word (lower-case), and what follows it
/// when that says anything (`High — only for tenants with sharing enabled`).
fn split_level(v: &str) -> (String, Option<String>) {
    let t = v.trim();
    let n = t.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    let rest = t[n..].trim();
    let note = rest
        .chars()
        .any(char::is_alphanumeric)
        .then(|| rest.to_string());
    (t[..n].to_ascii_lowercase(), note)
}

fn risk(
    field: &'static str,
    v: Option<String>,
) -> Result<(RiskLevel, Option<String>), ImportError> {
    let v = v.ok_or(ImportError::MissingField(field))?;
    let (word, note) = split_level(&v);
    let level = match word.as_str() {
        "low" => RiskLevel::Low,
        "medium" => RiskLevel::Medium,
        "high" => RiskLevel::High,
        "critical" => RiskLevel::Critical,
        _ => {
            return Err(ImportError::BadValue {
                field,
                value: v,
                allowed: "Low, Medium, High, Critical",
            })
        }
    };
    Ok((level, note))
}

fn likelihood(v: Option<String>) -> Result<(Likelihood, Option<String>), ImportError> {
    let v = v.ok_or(ImportError::MissingField("Likelihood"))?;
    let (word, note) = split_level(&v);
    let level = match word.as_str() {
        "low" => Likelihood::Low,
        "medium" => Likelihood::Medium,
        "high" => Likelihood::High,
        _ => {
            return Err(ImportError::BadValue {
                field: "Likelihood",
                value: v,
                allowed: "Low, Medium, High",
            })
        }
    };
    Ok((level, note))
}

fn cwe_ids(text: &str) -> Vec<String> {
    let upper = text.to_ascii_uppercase();
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut rest = upper.as_str();
    while let Some(i) = rest.find("CWE-") {
        let tail = &rest[i + 4..];
        let n = tail.chars().take_while(|c| c.is_ascii_digit()).count();
        if n > 0 {
            let id = format!("CWE-{}", &tail[..n]);
            if seen.insert(id.clone()) {
                out.push(id);
            }
        }
        rest = tail;
    }
    out
}

/// Whether a `CWE:` field says more than its ids (`CWE-639 (IDOR)`).
fn cwe_leftover(v: &str) -> bool {
    let upper = v.to_ascii_uppercase();
    let mut rest = upper.as_str();
    let mut said = String::new();
    while let Some(i) = rest.find("CWE-") {
        said.push_str(&rest[..i]);
        let tail = &rest[i + 4..];
        let n = tail.chars().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            said.push_str("CWE-");
        }
        rest = &tail[n..];
    }
    said.push_str(rest);
    said.chars().any(char::is_alphanumeric)
}

fn is_url(w: &str) -> bool {
    w.starts_with("http://") || w.starts_with("https://")
}

/// One ticket from free text. The exporter writes
/// `<type> <identifier> (<url>) — <notes>`; the identifier is taken to be
/// the last word when that word has a digit in it (`NB-42`, `#17`), else the
/// whole text is the identifier of an `Other` ticket.
fn loose_ticket(item: &str) -> Ticket {
    let (head, notes) = match item.split_once(" — ") {
        Some((h, n)) => (
            h.trim(),
            Some(n.trim().to_string()).filter(|n| !n.is_empty()),
        ),
        None => (item.trim(), None),
    };
    let bracketed = head
        .strip_suffix(')')
        .and_then(|h| h.rsplit_once(" ("))
        .filter(|(_, u)| is_url(u));
    let (head, url) = match bracketed {
        Some((h, u)) => (h.trim().to_string(), Some(u.to_string())),
        None => {
            fn bare(w: &str) -> &str {
                w.trim_matches(['(', ')', '<', '>', ','])
            }
            let url = head.split_whitespace().map(bare).find(|w| is_url(w));
            let rest: Vec<&str> = head
                .split_whitespace()
                .filter(|w| !is_url(bare(w)))
                .collect();
            (rest.join(" "), url.map(str::to_string))
        }
    };
    let split = head
        .rsplit_once(' ')
        .filter(|(_, id)| id.chars().any(|c| c.is_ascii_digit()));
    let (kind, identifier) = match split {
        Some((k, id)) => (k.trim().to_string(), id.to_string()),
        None if !head.is_empty() => ("Other".to_string(), head),
        None => (
            "Other".to_string(),
            url.clone().unwrap_or_else(|| "Unknown".to_string()),
        ),
    };
    Ticket {
        kind,
        identifier,
        url,
        notes,
    }
}

/// The tickets, and any text of the field no ticket took (a structured
/// entry's `Status:` line, the reason after `Not provided`).
fn tickets(v: Option<String>) -> (OrSentinel<Vec<Ticket>>, Option<String>) {
    let unknown = || OrSentinel::Sentinel("Unknown".to_string());
    let Some(v) = v else {
        return (unknown(), None);
    };
    if sentinel_like(&v) {
        let word = strip_emphasis(v.trim().trim_end_matches('.'))
            .trim_end_matches('.')
            .trim()
            .to_ascii_lowercase();
        let sentinel = match word.as_str() {
            "unknown" | "tbd" => unknown(),
            _ => OrSentinel::Sentinel("None Provided".into()),
        };
        // `Not provided: tracked outside the project` keeps its reason.
        let reason = not_provided(&v)
            .filter(|s| !s.ends_with(NOT_GIVEN))
            .map(|_| v.trim().to_string());
        return (sentinel, reason);
    }
    let stated = |s: &str| !(s.is_empty() || sentinel_like(s));
    let structured = v
        .lines()
        .any(|l| matches!(labelled(strip_marker(l)), Some((k, _)) if k == "type"));
    let mut out: Vec<Ticket> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    if structured {
        for l in v.lines() {
            let Some((k, val)) = labelled(strip_marker(l)) else {
                rest.push(l);
                continue;
            };
            match (k.as_str(), out.last_mut()) {
                ("type", _) => out.push(Ticket {
                    kind: if stated(&val) { val } else { "Other".into() },
                    identifier: "Unknown".into(),
                    url: None,
                    notes: None,
                }),
                ("identifier", Some(t)) if stated(&val) => t.identifier = val,
                ("identifier", Some(_)) => {}
                ("url", Some(t)) => t.url = stated(&val).then_some(val),
                ("notes", Some(t)) => t.notes = stated(&val).then_some(val),
                _ => rest.push(l),
            }
        }
    } else {
        out = v
            .split(['\n', ';'])
            .map(strip_marker)
            .filter(|s| !s.is_empty())
            .map(loose_ticket)
            .collect();
    }
    let rest = (!rest.is_empty()).then(|| rest.join("\n"));
    if out.is_empty() {
        (unknown(), Some(v.trim().to_string()))
    } else {
        (OrSentinel::Value(out), rest)
    }
}

// ---- section bodies --------------------------------------------------------

#[derive(Debug, Clone)]
enum Block {
    Text(Vec<String>),
    Fence {
        info: String,
        content: String,
        raw: String,
    },
}

#[derive(Debug, Clone, Default)]
struct Body(Vec<Block>);

/// The exporter puts a `\` before a `<` that starts a prose line (after at
/// most three spaces), so the line cannot open an HTML block; this takes it
/// back out. A line opening an autolink was never escaped, so it is left
/// alone.
fn unescape_line_start(line: &str) -> String {
    let indent = line.bytes().take_while(|b| *b == b' ').count();
    match line[indent..].strip_prefix("\\<") {
        Some(after) if indent <= 3 && !starts_autolink(after) => {
            format!("{}<{after}", &line[..indent])
        }
        _ => line.to_string(),
    }
}

impl Body {
    fn parse(lines: &[String]) -> Body {
        let mut blocks = Vec::new();
        let mut text: Vec<String> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = &lines[i];
            if let Some(open) = opens(line) {
                if !text.is_empty() {
                    blocks.push(Block::Text(std::mem::take(&mut text)));
                }
                let info = line
                    .trim_start()
                    .trim_start_matches(open.0)
                    .trim()
                    .to_string();
                let mut raw = vec![line.clone()];
                let mut content = Vec::new();
                i += 1;
                while i < lines.len() {
                    raw.push(lines[i].clone());
                    if closes(&lines[i], open) {
                        break;
                    }
                    content.push(lines[i].clone());
                    i += 1;
                }
                i += 1;
                blocks.push(Block::Fence {
                    info,
                    content: content.join("\n"),
                    raw: raw.join("\n"),
                });
            } else {
                text.push(unescape_line_start(line.trim_end()));
                i += 1;
            }
        }
        if !text.is_empty() {
            blocks.push(Block::Text(text));
        }
        // A section's own edges: blank lines, a rule that separated it from
        // the next finding, or a setext underline below its heading. A rule
        // inside the section is the author's and stays.
        if let Some(Block::Text(first)) = blocks.first_mut() {
            let skip = first
                .iter()
                .take_while(|l| {
                    let t = l.trim();
                    t.is_empty() || is_thematic_break(t) || t.chars().all(|c| c == '=')
                })
                .count();
            first.drain(..skip);
        }
        if let Some(Block::Text(last)) = blocks.last_mut() {
            while last.last().is_some_and(|l| {
                let t = l.trim();
                t.is_empty() || is_thematic_break(t)
            }) {
                last.pop();
            }
        }
        blocks.retain(|b| !matches!(b, Block::Text(l) if l.is_empty()));
        Body(blocks)
    }

    fn text(&self) -> String {
        render(&self.0)
    }

    /// Every line, with whether it belongs to a code block.
    fn lines(&self) -> Vec<(&str, bool)> {
        let mut out = Vec::new();
        for b in &self.0 {
            match b {
                Block::Text(ls) => out.extend(ls.iter().map(|l| (l.as_str(), false))),
                Block::Fence { raw, .. } => out.extend(raw.lines().map(|l| (l, true))),
            }
        }
        out
    }
}

fn render(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            Block::Text(lines) => lines.join("\n"),
            Block::Fence { raw, .. } => raw.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn without(blocks: &[Block], skip: usize) -> Vec<Block> {
    blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != skip)
        .map(|(_, b)| b.clone())
        .collect()
}

fn non_blank_or_not_stated(s: String) -> String {
    if s.trim().is_empty() {
        NOT_STATED.to_string()
    } else {
        s
    }
}

fn claim(text: String) -> EvidenceClaim {
    EvidenceClaim {
        claim: text,
        file: None,
        lines: None,
        binary_va: None,
        excerpt: None,
        lang: None,
        sha256: None,
        artifact: None,
    }
}

fn fence_lang(info: &str) -> Option<String> {
    let w = info.split_whitespace().next()?;
    w.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '#' | '.'))
        .then(|| w.to_string())
}

/// `s` split after a leading code span: (its content, the rest). The one
/// space of padding a span adds around content that starts or ends with a
/// backtick is removed.
fn split_code_span(s: &str) -> Option<(&str, &str)> {
    let n = s.bytes().take_while(|b| *b == b'`').count();
    if n == 0 {
        return None;
    }
    let body = &s[n..];
    let mut from = 0;
    loop {
        let i = from + body[from..].find('`')?;
        let run = body[i..].bytes().take_while(|b| *b == b'`').count();
        if run == n {
            let inner = &body[..i];
            let unpadded = inner
                .strip_prefix(' ')
                .and_then(|x| x.strip_suffix(' '))
                .filter(|x| !x.trim().is_empty());
            return Some((unpadded.unwrap_or(inner), &body[i + run..]));
        }
        from = i + run;
    }
}

/// `a-b` (or a single line `a`) as `[a, b]`.
fn line_range(r: &str) -> Option<[u32; 2]> {
    let (a, b) = r.split_once(['-', '–']).unwrap_or((r, r));
    Some([a.trim().parse().ok()?, b.trim().parse().ok()?])
}

/// The leading `0x<hex>` of `s`.
fn hex_va(s: &str) -> Option<&str> {
    let r = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    let n = r.chars().take_while(|c| c.is_ascii_hexdigit()).count();
    (n > 0).then(|| &s[..2 + n])
}

/// A binary VA as the exporter writes one: `0x…` or `binary@0x…`.
fn looks_like_va(s: &str) -> bool {
    let after = s.rsplit_once('@').map_or(s, |(_, v)| v);
    hex_va(after).is_some_and(|h| h.len() == after.len())
}

/// A file named on its own: one word with a `/` or `.` in it, so
/// `src/app.rs` is a file and `GET /s/{token}` is not.
fn looks_like_path(s: &str) -> bool {
    !s.is_empty() && !s.contains(char::is_whitespace) && s.contains(['/', '.'])
}

type Place = (Option<String>, Option<[u32; 2]>, Option<String>);

/// The exporter's location string (`file:a-b`, `file`, a binary VA, or
/// `file:a-b @ va`) as (file, lines, binary VA); `None` when the code span
/// is not a location at all.
fn exported_location(s: &str) -> Option<Place> {
    let s = s.trim();
    let (at, va) = match s.split_once(" @ ") {
        Some((l, v)) if looks_like_va(v.trim()) => (l.trim(), Some(v.trim().to_string())),
        Some(_) => return None,
        None if looks_like_va(s) => return Some((None, None, Some(s.to_string()))),
        None => (s, None),
    };
    let ranged = at
        .rsplit_once(':')
        .and_then(|(p, r)| Some((p, line_range(r)?)))
        .filter(|(p, _)| !p.trim().is_empty());
    match ranged {
        Some((p, lines)) => Some((Some(p.to_string()), Some(lines), va)),
        None if looks_like_path(at) => Some((Some(at.to_string()), None, va)),
        None => None,
    }
}

/// The first `path:12` / `path:12-30` token whose path is workspace-relative,
/// with backticks and punctuation around it ignored.
fn location_token(text: &str) -> Option<(String, [u32; 2])> {
    text.split_whitespace().find_map(|raw| {
        let tok = raw
            .trim_start_matches(['`', '*', '(', '[', '"', '\''])
            .trim_end_matches(['`', '*', ')', ']', ',', ';', '"', '\'', '.', ':']);
        let (path, range) = tok.rsplit_once(':')?;
        let [a, b] = line_range(range)?;
        let relative = !path.is_empty()
            && !path.starts_with('/')
            && !path.contains("..")
            && !path.contains(':')
            && !path.contains('\\')
            && (path.contains('.') || path.contains('/'));
        (relative && a >= 1 && b >= a).then(|| (path.to_string(), [a, b]))
    })
}

/// A binary VA in free text: a `binary@0x…` word, else the `0x…` after an
/// `@`.
fn binary_va(text: &str) -> Option<String> {
    let word = text.split_whitespace().find_map(|raw| {
        let tok = raw.trim_matches(['`', '*', '(', ')', '[', ']', ',', ';', '.', ':', '"', '\'']);
        let (name, _) = tok.rsplit_once('@')?;
        (!name.is_empty() && looks_like_va(tok)).then(|| tok.to_string())
    });
    if word.is_some() {
        return word;
    }
    let i = text.find('@')?;
    hex_va(text[i + 1..].trim_start()).map(str::to_string)
}

fn location(body: &Body) -> ReportLocation {
    let mut input: Vec<String> = Vec::new();
    let mut output: Vec<String> = Vec::new();
    let mut labelled_any = false;
    let mut current = 0u8; // 1 input, 2 output
    for line in body.text().lines() {
        match labelled(line) {
            Some((k, v)) if k == "input" => {
                labelled_any = true;
                current = 1;
                input.push(v);
            }
            Some((k, v)) if k == "output" => {
                labelled_any = true;
                current = 2;
                output.push(v);
            }
            _ if current == 2 => output.push(line.to_string()),
            _ => input.push(line.to_string()),
        }
    }
    let join = |v: Vec<String>| non_blank_or_not_stated(v.join("\n").trim().to_string());
    if !labelled_any {
        return ReportLocation {
            input: body.text(),
            output: NOT_STATED.into(),
        };
    }
    ReportLocation {
        input: join(input),
        output: join(output),
    }
}

fn role(i: usize, n: usize) -> HopRole {
    if n == 1 || i + 1 == n {
        HopRole::Sink
    } else if i == 0 {
        HopRole::Source
    } else {
        HopRole::Hop
    }
}

/// A hop with a bold label, the exporter's spelling:
/// `**label** — `file:a-b` — gate: g (passes because p) — role: r`, every
/// part after the label optional. Also whether it stated its role. `None`
/// for any other spelling.
fn bold_hop(line: &str, default: HopRole) -> Option<(ChainHop, bool)> {
    let r = line.strip_prefix("**")?;
    let end = r.find("**")?;
    let mut h = ChainHop {
        label: r[..end].trim().to_string(),
        file: None,
        lines: None,
        binary_va: None,
        gate: None,
        passes_because: None,
        role: default,
    };
    let after = &r[end + 2..];
    if after.trim().is_empty() {
        return Some((h, false));
    }
    let mut rest = after.strip_prefix(" — ")?;
    let mut stated = false;
    let (head, last) = rest.rsplit_once(" — ").unwrap_or(("", rest));
    if let Some(word) = last.strip_prefix("role: ") {
        h.role = match word.trim() {
            "source" => HopRole::Source,
            "hop" => HopRole::Hop,
            "sink" => HopRole::Sink,
            _ => return None,
        };
        stated = true;
        rest = head;
    }
    if let Some((loc, after)) = split_code_span(rest) {
        let tail = after.strip_prefix(" — ");
        let place = exported_location(loc).filter(|_| after.is_empty() || tail.is_some());
        if let Some(place) = place {
            (h.file, h.lines, h.binary_va) = place;
            rest = tail.unwrap_or("");
        }
    }
    if let Some(g) = rest.strip_prefix("gate: ") {
        match g.rsplit_once(" (passes because ") {
            Some((gate, why)) => {
                h.gate = Some(gate.trim().to_string());
                h.passes_because = Some(why.strip_suffix(')').unwrap_or(why).trim().to_string());
            }
            None => h.gate = Some(g.trim().to_string()),
        }
    } else if let Some(p) = rest.strip_prefix("passes because ") {
        h.passes_because = Some(p.trim().to_string());
    } else if !rest.trim().is_empty() {
        // Not the exporter's: keep the text in the label.
        if h.file.is_none() && h.binary_va.is_none() {
            if let Some((f, l)) = location_token(rest) {
                h.file = Some(f);
                h.lines = Some(l);
            }
            h.binary_va = binary_va(rest);
        }
        h.label = format!("{} — {}", h.label, rest.trim());
    }
    Some((h, stated))
}

fn hop(line: &str, role: HopRole) -> ChainHop {
    if let Some((h, _)) = bold_hop(line, role) {
        return h;
    }
    let mut h = ChainHop {
        label: line.to_string(),
        file: None,
        lines: None,
        binary_va: binary_va(line),
        gate: None,
        passes_because: None,
        role,
    };
    if let Some((f, l)) = location_token(line) {
        h.file = Some(f);
        h.lines = Some(l);
    }
    h
}

fn call_chain(body: Option<&Body>) -> (OrSentinel<Vec<ChainHop>>, Vec<EvidenceClaim>) {
    let Some(body) = body else {
        return (missing(), Vec::new());
    };
    if let Some(s) = stands_in(&body.text()) {
        return (OrSentinel::Sentinel(s), Vec::new());
    }
    let mut lines: Vec<String> = Vec::new();
    let mut claims = Vec::new();
    for b in &body.0 {
        match b {
            // A `Step N:` alone on its line is followed by its hop.
            Block::Text(ls) => lines.extend(
                ls.iter()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty() && !is_thematic_break(l))
                    .filter(|l| step_marker(l) != Some("")),
            ),
            Block::Fence { info, content, .. } => {
                let mut c = claim(match lines.last() {
                    Some(h) => format!("Call chain, at: {}", strip_marker(h)),
                    None => "Call chain excerpt".to_string(),
                });
                c.excerpt = Some(content.clone());
                c.lang = fence_lang(info);
                claims.push(c);
            }
        }
    }
    // One line of `a → b → c` is the whole chain, unless it is a single hop
    // in the exporter's spelling whose label has an arrow in it.
    let one_exported_hop = lines.len() == 1
        && bold_hop(strip_marker(&lines[0]), HopRole::Sink).is_some_and(|(_, stated)| stated);
    if lines.len() == 1 && !one_exported_hop && (lines[0].contains('→') || lines[0].contains("->"))
    {
        let one = lines.remove(0);
        lines = one
            .split('→')
            .flat_map(|p| p.split("->"))
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
    }
    if lines.is_empty() {
        return (
            OrSentinel::Sentinel(format!(
                "{NOT_PROVIDED_PREFIX}the imported call chain is code only; see the evidence claims"
            )),
            claims,
        );
    }
    let n = lines.len();
    let hops = lines
        .iter()
        .enumerate()
        .map(|(i, l)| hop(strip_marker(l), role(i, n)))
        .collect();
    (OrSentinel::Value(hops), claims)
}

fn paragraphs(lines: &[String]) -> Vec<String> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for l in lines {
        let t = l.trim();
        // A blank line ends a paragraph, and so does a rule, which is never
        // a claim of its own. A `Step N:` alone on its line ends one too:
        // its claim starts on the next line.
        if t.is_empty() || is_thematic_break(t) || step_marker(t) == Some("") {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if is_list_item(t) && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(t);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|p| p.join("\n")).collect()
}

/// A claim without the exporter's trailing ` (sha256 <first 12>)`: a display
/// prefix of rupu's hash of the claim's file when the report was recorded.
/// A prefix cannot be checked against the file, so it is dropped (an
/// imported claim is stored unhashed). A hash of any other length is the
/// author's and stays in the claim.
fn strip_sha_suffix(t: &str) -> &str {
    t.strip_suffix(')')
        .and_then(|x| x.rsplit_once(" (sha256 "))
        .filter(|(_, h)| h.len() == 12 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map_or(t, |(before, _)| before)
}

/// A claim and the artifact of the exporter's trailing
/// `` — proven by `path` ``.
fn split_artifact(t: &str) -> (&str, Option<String>) {
    let found = t.rsplit_once(" — proven by ").and_then(|(before, span)| {
        let (inner, rest) = split_code_span(span)?;
        rest.is_empty().then(|| (before, inner.to_string()))
    });
    match found {
        Some((before, a)) => (before, Some(a)),
        None => (t, None),
    }
}

fn claim_from(para: &str) -> EvidenceClaim {
    let (text, artifact) = split_artifact(strip_sha_suffix(strip_marker(para)));
    // The exporter's spelling: **`file:a-b`** — claim
    let exported = text
        .strip_prefix("**")
        .and_then(split_code_span)
        .and_then(|(loc, rest)| Some((exported_location(loc)?, rest.strip_prefix("** — ")?)));
    let mut c = match exported {
        Some((place, rest)) => {
            let mut c = claim(rest.trim().to_string());
            (c.file, c.lines, c.binary_va) = place;
            c
        }
        None => {
            let mut c = claim(text.trim().to_string());
            if let Some((f, l)) = location_token(text) {
                c.file = Some(f);
                c.lines = Some(l);
            }
            c.binary_va = binary_va(text);
            c
        }
    };
    c.artifact = artifact;
    c
}

fn evidence(body: &Body) -> Vec<EvidenceClaim> {
    if unwrap_emphasis(body.text().trim()) == NO_EVIDENCE {
        return Vec::new();
    }
    let mut claims: Vec<EvidenceClaim> = Vec::new();
    for b in &body.0 {
        match b {
            Block::Text(lines) => claims.extend(paragraphs(lines).iter().map(|p| claim_from(p))),
            Block::Fence { info, content, .. } => match claims.last_mut() {
                Some(c) if c.excerpt.is_none() => {
                    c.excerpt = Some(content.clone());
                    c.lang = fence_lang(info);
                }
                _ => {
                    let mut c = claim(EXCERPT_ONLY.to_string());
                    c.excerpt = Some(content.clone());
                    c.lang = fence_lang(info);
                    claims.push(c);
                }
            },
        }
    }
    claims
}

fn is_diff(info: &str, content: &str) -> bool {
    let lang = info
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    lang == "diff"
        || lang == "patch"
        || content.starts_with("--- ")
        || content.starts_with("diff ")
        || content.starts_with("@@")
}

/// The first diff block is the patch and the rest of the section its notes,
/// even when the text opens "Not provided …" (a diff that follows is still
/// a diff). With no diff block, the section is a sentinel.
fn patch(body: Option<&Body>) -> OrSentinel<Patch> {
    let Some(body) = body else { return missing() };
    let found = body.0.iter().enumerate().find_map(|(i, b)| match b {
        Block::Fence { info, content, .. } if is_diff(info, content) => Some((i, content.clone())),
        _ => None,
    });
    if let Some((i, diff)) = found {
        let rest = render(&without(&body.0, i));
        return OrSentinel::Value(Patch {
            diff,
            notes: (!rest.is_empty()).then_some(rest),
        });
    }
    let text = body.text();
    OrSentinel::Sentinel(not_provided(&text).unwrap_or_else(|| {
        format!(
            "{NOT_PROVIDED_PREFIX}the imported report gives no unified diff: {}",
            one_line(&text)
        )
    }))
}

/// The value of the first line labelled with one of `keys`, taken out of the
/// blocks' text. An earlier key wins over a later one, so the exporter's own
/// label is preferred to a look-alike line in the prose. Other labelled
/// lines stay where they are.
fn take_label(blocks: &mut [Block], keys: &[&str]) -> Option<String> {
    for key in keys {
        for b in blocks.iter_mut() {
            let Block::Text(lines) = b else { continue };
            let hit = lines
                .iter()
                .enumerate()
                .find_map(|(i, l)| match labelled(l) {
                    Some((k, v)) if k == *key && !v.is_empty() => Some((i, v)),
                    _ => None,
                });
            if let Some((i, v)) = hit {
                lines.remove(i);
                return Some(v);
            }
        }
    }
    None
}

const SHELLS: &[&str] = &["", "sh", "bash", "shell", "console", "zsh"];

/// The command's code block, taken out: the last one tagged `sh` (the
/// exporter's), else the last one-line block tagged as a shell or untagged.
fn take_command_fence(blocks: &mut Vec<Block>) -> Option<String> {
    let lang = |info: &str| {
        info.split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let tagged_sh = blocks.iter().rposition(|b| {
        matches!(b, Block::Fence { info, content, .. }
            if lang(info) == "sh" && !content.trim().is_empty())
    });
    let one_line_shell = || {
        blocks.iter().rposition(|b| {
            matches!(b, Block::Fence { info, content, .. }
                if SHELLS.contains(&lang(info).as_str()) && content.trim().lines().count() == 1)
        })
    };
    let i = tagged_sh.or_else(one_line_shell)?;
    match blocks.remove(i) {
        Block::Fence { content, .. } => Some(content.trim_matches('\n').to_string()),
        Block::Text(_) => None,
    }
}

const EXPECT: &[&str] = &["fails when", "expect", "expected", "expected result"];
const VULNERABLE: &[&str] = &[
    "vulnerable build",
    "on the vulnerable build",
    "expect vulnerable",
];
const PATCHED: &[&str] = &["patched build", "on the patched build", "expect patched"];

fn ci(body: Option<&Body>) -> OrSentinel<CiDetection> {
    let Some(body) = body else { return missing() };
    if let Some(s) = stands_in(&body.text()) {
        return OrSentinel::Sentinel(s);
    }
    let mut blocks = body.0.clone();
    let stage = take_label(&mut blocks, &["stage"]);
    let expect = take_label(&mut blocks, EXPECT);
    let command = take_label(&mut blocks, &["command"]).or_else(|| take_command_fence(&mut blocks));
    OrSentinel::Value(CiDetection {
        stage: stage.unwrap_or_else(|| NOT_STATED.into()),
        body: non_blank_or_not_stated(render(&blocks)),
        command,
        expect: expect.unwrap_or_else(|| NOT_STATED.into()),
    })
}

fn regression(body: Option<&Body>) -> OrSentinel<RegressionTest> {
    let Some(body) = body else { return missing() };
    if let Some(s) = stands_in(&body.text()) {
        return OrSentinel::Sentinel(s);
    }
    let mut blocks = body.0.clone();
    let vulnerable = take_label(&mut blocks, VULNERABLE);
    let patched = take_label(&mut blocks, PATCHED);
    let command = take_label(&mut blocks, &["command"]).or_else(|| take_command_fence(&mut blocks));
    OrSentinel::Value(RegressionTest {
        body: non_blank_or_not_stated(render(&blocks)),
        command: command.unwrap_or_else(|| NOT_STATED.into()),
        expect_vulnerable: vulnerable.unwrap_or_else(|| NOT_STATED.into()),
        expect_patched: patched.unwrap_or_else(|| NOT_STATED.into()),
    })
}

/// The steps, in order. When any line starts `Step N:` only those lines
/// start a step, so a list inside a step stays in it; otherwise any list
/// item does. Other lines, blank-line breaks and code blocks continue the
/// step before them.
fn steps(body: &Body) -> Vec<String> {
    let lines = body.lines();
    let numbered = lines
        .iter()
        .any(|(l, code)| !code && step_marker(l).is_some());
    let mut out: Vec<String> = Vec::new();
    let mut gap = false;
    for (line, code) in lines {
        // A rule between steps separates them; it is not part of either.
        if !code && (line.trim().is_empty() || is_thematic_break(line.trim())) {
            gap = !out.is_empty();
            continue;
        }
        let start = match (code, numbered) {
            (true, _) => None,
            (false, true) => step_marker(line),
            (false, false) => list_marker(line),
        };
        match (start, out.last_mut()) {
            (Some(s), _) => out.push(s.to_string()),
            (None, None) => out.push(line.trim().to_string()),
            // `Step N:` alone on its line: the step starts on the next one.
            (None, Some(last)) if last.is_empty() => {
                last.push_str(if code { line } else { line.trim() })
            }
            (None, Some(last)) => {
                last.push_str(if gap { "\n\n" } else { "\n" });
                last.push_str(line);
            }
        }
        gap = false;
    }
    out.retain(|s| !s.is_empty());
    out
}

fn relation(line: &str) -> Relation {
    let l = line.to_ascii_lowercase();
    if l.contains("duplicate") {
        Relation::Duplicate
    } else if l.contains("prerequisite") {
        Relation::Prerequisite
    } else if l.contains("supersede") {
        Relation::Supersedes
    } else {
        Relation::Sibling
    }
}

/// A cross-reference in the exporter's spelling, `- fnd_… (relation) — note`
/// (the note optional). The exporter prints a finding it has a display
/// number for by that number instead; such a line has no id to read.
fn exported_cross_ref(line: &str) -> Option<CrossRef> {
    let rest = line.trim().strip_prefix("- ")?;
    let (id, rest) = rest.split_once(" (")?;
    if fnd_ids(id) != [id] {
        return None;
    }
    let (rel, note) = rest.split_once(')')?;
    let relation = match rel {
        "duplicate" => Relation::Duplicate,
        "sibling" => Relation::Sibling,
        "prerequisite" => Relation::Prerequisite,
        "supersedes" => Relation::Supersedes,
        _ => return None,
    };
    let note = match note {
        "" => None,
        n => Some(n.strip_prefix(" — ")?.to_string()),
    };
    Some(CrossRef {
        finding_id: id.to_string(),
        relation,
        note,
    })
}

/// Longest note a cross-reference read from free text gets.
const NOTE_MAX_CHARS: usize = 300;

/// `s` cut to `max` characters, with `…` when anything was cut.
fn capped(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// The cross-references (a link for every `fnd_` id the section names, the
/// report's own included: the caller knows which id is its own and drops it
/// with [`retain_known_cross_references`]), and the section's verbatim text
/// for `references`.
fn cross_refs(body: Option<&Body>) -> (OrSentinel<Vec<CrossRef>>, Option<String>) {
    let none = || OrSentinel::Sentinel("None".to_string());
    let Some(body) = body else {
        return (none(), None);
    };
    let text = body.text();
    let bare = unwrap_emphasis(text.trim()).trim_end_matches('.');
    if ["none", "none provided", "n/a", "not applicable"]
        .iter()
        .any(|n| bare.eq_ignore_ascii_case(n))
    {
        return (none(), None);
    }
    let mut refs: Vec<CrossRef> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for line in text.lines() {
        if let Some(x) = exported_cross_ref(line) {
            if seen.insert(x.finding_id.clone()) {
                refs.push(x);
            }
            continue;
        }
        let ids = fnd_ids(line);
        if ids.is_empty() {
            continue;
        }
        // Once per line, however many ids it names; the note is capped
        // because every id on the line shares it, and the whole line is in
        // `references` anyway.
        let relation = relation(line);
        let note = capped(&one_line(strip_marker(line)), NOTE_MAX_CHARS);
        for id in ids {
            if seen.insert(id.clone()) {
                refs.push(CrossRef {
                    finding_id: id,
                    relation,
                    note: Some(note.clone()),
                });
            }
        }
    }
    let refs = if refs.is_empty() {
        none()
    } else {
        OrSentinel::Value(refs)
    };
    (refs, Some(text))
}

/// A GFM table row's cells, with the exporter's `\|` escapes undone (it
/// writes `k` backslashes before a `|` as `2k + 1`).
fn table_cells(row: &str) -> Vec<String> {
    let t = row.trim();
    let inner = t.strip_prefix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut slashes = 0usize;
    for c in inner.chars() {
        match c {
            '\\' => {
                slashes += 1;
                continue;
            }
            '|' if slashes % 2 == 1 => {
                cur.push_str(&"\\".repeat(slashes / 2));
                cur.push('|');
            }
            '|' => {
                cur.push_str(&"\\".repeat(slashes));
                cells.push(std::mem::take(&mut cur).trim().to_string());
            }
            other => {
                cur.push_str(&"\\".repeat(slashes));
                cur.push(other);
            }
        }
        slashes = 0;
    }
    cur.push_str(&"\\".repeat(slashes));
    if !cur.trim().is_empty() {
        cells.push(cur.trim().to_string());
    }
    cells
}

/// The column headers of the exporter's artifact table: every column but
/// the path is recomputed by rupu when the report is written.
const EXPORTED_ARTIFACT_HEADERS: &[&str] = &[
    "Path",
    "SHA-256 (first 12 chars)",
    "Size",
    "Kind",
    "Stored",
    "Host",
];

/// Joined non-blank-edged lines, or `None` when they hold no text.
fn kept(lines: &[&str]) -> Option<String> {
    let text = lines.join("\n");
    let text = text.trim_matches('\n');
    (!text.trim().is_empty()).then(|| text.to_string())
}

/// Artifact paths, from the exporter's table or a list, and the section's
/// other text. rupu fills in the hash, size and storage when the report is
/// written, so the exporter's table is otherwise consumed; any other table
/// is also kept as text, since its other columns are the author's. A list
/// item's path is its leading code span, else its first word; an item that
/// says more than that is also kept as text.
fn artifacts(body: &Body) -> (Vec<ArtifactRef>, Option<String>) {
    let mut out = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    // Inside a table: whether it is the exporter's.
    let mut table: Option<bool> = None;
    for (line, code) in body.lines() {
        let t = line.trim();
        if code || !t.starts_with('|') {
            table = None;
        }
        if code || t.is_empty() {
            rest.push(line);
            continue;
        }
        let path = if t.starts_with('|') {
            let cells = table_cells(t);
            let exported = match table {
                Some(exported) => exported,
                None => {
                    let exported = cells == EXPORTED_ARTIFACT_HEADERS;
                    table = Some(exported);
                    if !exported {
                        rest.push(line);
                    }
                    continue;
                }
            };
            if !exported {
                rest.push(line);
            }
            let first = cells.into_iter().next().unwrap_or_default();
            if first.chars().all(|c| matches!(c, '-' | ':')) {
                continue;
            }
            first
        } else if is_list_item(t) {
            let item = strip_marker(t);
            let (path, said) = match split_code_span(item) {
                Some((span, after)) => (span, after),
                None => {
                    let word = item.split_whitespace().next().unwrap_or("");
                    (word, &item[word.len()..])
                }
            };
            if said.chars().any(char::is_alphanumeric) {
                rest.push(line);
            }
            path.to_string()
        } else {
            rest.push(line);
            continue;
        };
        let path = path.trim_matches('`').trim();
        if !path.is_empty() {
            out.push(ArtifactRef {
                path: path.to_string(),
                sha256: String::new(),
                size: 0,
                kind: None,
                stored: None,
                host: None,
            });
        }
    }
    (out, kept(&rest))
}

/// A Provenance section's text. Its id line was already taken out by
/// [`take_id_lines`] when it said nothing but the id.
fn provenance(body: &Body) -> Option<String> {
    let lines: Vec<&str> = body.lines().into_iter().map(|(l, _)| l).collect();
    kept(&lines)
}
