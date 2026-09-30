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
//! hop labels) stay collapsed, and so do claim hashes it shortens: rupu
//! rehashes a claim's file whenever a report is written.
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

// `Report` dwarfs `NotAReport`, but one value is built per file and moved
// straight out, and the variant's shape is the module's contract.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// A finding report, and the finding ids it cites outside its
    /// cross-references. Its own id is expected to be the only one.
    Report {
        report: FindingReport,
        cited_ids: Vec<String>,
    },
    /// Fewer than three of the layout's section headings: not a finding
    /// report (an index, a README). Skipped, not an error.
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

/// Field labels of the layout's header block (lower-case).
const FIELDS: &[&str] = &[
    "filename",
    "identifier",
    "finding id",
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
    // CommonMark also ends a line at a lone `\r`.
    let md = md.replace("\r\n", "\n").replace('\r', "\n");
    let mut doc = split(&md);
    let kinds: HashSet<Sec> = doc.sections.iter().map(|(s, _)| *s).collect();
    if kinds.len() < 3 {
        return Ok(Parsed::NotAReport);
    }
    let descriptions = doc
        .sections
        .iter()
        .filter(|(s, _)| *s == Sec::Description)
        .count();
    let findings = doc.filename_lines.max(descriptions);
    if findings > 1 {
        return Err(ImportError::SeveralFindings(findings));
    }

    let mut header = header(&doc.header);
    take_trailing_fields(&mut doc, &mut header.fields);

    let cited_ids = {
        let mut text = doc.header.join("\n");
        for (s, body) in &doc.sections {
            if *s != Sec::CrossRefs {
                text.push('\n');
                text.push_str(&body.join("\n"));
            }
        }
        fnd_ids(&text)
    };

    let title = header
        .title
        .clone()
        .ok_or(ImportError::MissingField("title"))?;
    let or_unknown = |keys: &[&str]| header.get(keys).unwrap_or_else(|| "Unknown".to_string());
    let ownership = Ownership {
        owner: or_unknown(&["owner"]),
        product: or_unknown(&["product"]),
        affected_component: or_unknown(&["affected component"]),
        source_repository: or_unknown(&["source repository"]),
    };
    let rating = Rating {
        impact: risk("Impact", header.get(&["impact"]))?,
        likelihood: likelihood(header.get(&["likelihood"]))?,
        risk_rating: risk("Risk Rating", header.get(&["risk rating"]))?,
        risk_factor: risk("Risk Factor", header.get(&["risk factor"]))?,
        cvss_v3: or_unknown(&["cvss v3 base score", "cvss v3", "cvss"]),
    };
    let category = header
        .get(&["category"])
        .ok_or(ImportError::MissingField("Category"))?;
    let attack_vector = header
        .get(&["attack vector"])
        .ok_or(ImportError::MissingField("Attack Vector"))?;

    let required = |s: Sec| section(&doc, s).ok_or(ImportError::MissingSection(s.name()));
    let description = required(Sec::Description)?.text();
    let impact = required(Sec::Impact)?.text();
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
    let mut references = required(Sec::References)?.text();
    let cwe = match header.get(&["cwe"]) {
        Some(v) => cwe_ids(&v),
        None => cwe_ids(&format!("{category}\n{references}")),
    };
    let (cross_references, verbatim) =
        cross_refs(section(&doc, Sec::CrossRefs).as_ref(), &cited_ids);
    if let Some(text) = verbatim {
        references = format!("{references}\n\nCross-references (imported):\n\n{text}");
    }
    let artifacts = section(&doc, Sec::Artifacts)
        .map(|b| artifacts(&b))
        .unwrap_or_default();

    Ok(Parsed::Report {
        report: FindingReport {
            title,
            ownership,
            tickets: tickets(header.get(&["existing ticket references"])),
            rating,
            category,
            attack_vector,
            cwe,
            description,
            impact,
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
        cited_ids,
    })
}

/// Distinct `fnd_<ULID>` ids in `text`, in first-seen order. An id must be
/// exactly 26 upper-case alphanumerics after `fnd_`, not glued to a longer
/// word on either side.
pub fn fnd_ids(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
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
            if !out.contains(&id) {
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
    filename_lines: usize,
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
/// "Evidence" are not sections.
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
    let headings: HashMap<usize, Sec> = candidates
        .into_iter()
        .filter(|(_, _, l)| Some(*l) == level)
        .map(|(i, s, _)| (i, s))
        .collect();

    let mut doc = Doc::default();
    for (i, line) in lines.iter().enumerate() {
        if let Some(sec) = headings.get(&i) {
            doc.sections.push((*sec, Vec::new()));
            continue;
        }
        if !in_code[i] && matches!(field(line), Some((k, _)) if k == "filename") {
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
/// header fields, fence-aware. Only References is searched, so a prose line
/// elsewhere that happens to start `Risk factor:` stays prose.
fn take_trailing_fields(doc: &mut Doc, into: &mut Vec<(String, String)>) {
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
                Some((k, v)) if TRAILING_FIELDS.contains(&k.as_str()) => {
                    into.push((k, v));
                    false
                }
                _ => true,
            }
        });
    }
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
    let mut t = line.trim();
    t = t.trim_start_matches('#').trim();
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

/// A whole section that says "Not provided …", normalized to the sentinel.
fn not_provided(text: &str) -> Option<String> {
    let t = unwrap_emphasis(text.trim());
    if !t.get(..12)?.eq_ignore_ascii_case("not provided") {
        return None;
    }
    let why = t[12..]
        .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '—' | '–' | '-'));
    let why = one_line(why);
    Some(format!(
        "{NOT_PROVIDED_PREFIX}{}",
        if why.is_empty() {
            "not given in the imported report"
        } else {
            why.as_str()
        }
    ))
}

// ---- header ----------------------------------------------------------------

#[derive(Debug, Default)]
struct Header {
    title: Option<String>,
    fields: Vec<(String, String)>,
}

impl Header {
    /// The first non-blank value for any of `keys`.
    fn get(&self, keys: &[&str]) -> Option<String> {
        self.fields
            .iter()
            .find(|(k, v)| keys.contains(&k.as_str()) && !v.trim().is_empty())
            .map(|(_, v)| v.trim().to_string())
    }
}

fn header(lines: &[String]) -> Header {
    let mut h = Header::default();
    let mut in_tickets = false;
    let mut fence: Option<(char, usize)> = None;
    for line in lines {
        if let Some(open) = fence {
            if closes(line, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = opens(line) {
            fence = Some(open);
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        // The structured ticket list continues the field on indented or
        // bulleted lines.
        if in_tickets && (line.starts_with(char::is_whitespace) || is_list_item(line)) {
            if let Some((_, v)) = h.fields.last_mut() {
                if !v.is_empty() {
                    v.push('\n');
                }
                v.push_str(line.trim());
            }
            continue;
        }
        in_tickets = false;
        if let Some((k, v)) = field(line) {
            in_tickets = k == "existing ticket references";
            h.fields.push((k, v));
        } else if h.title.is_none() {
            let t = line.trim().trim_start_matches('#').trim();
            let t = strip_pair(strip_pair(t, "**"), "__").trim();
            if !t.is_empty() && !is_thematic_break(t) {
                h.title = Some(t.to_string());
            }
        }
    }
    h
}

fn first_word(v: &str) -> String {
    v.trim()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn risk(field: &'static str, v: Option<String>) -> Result<RiskLevel, ImportError> {
    let v = v.ok_or(ImportError::MissingField(field))?;
    match first_word(&v).as_str() {
        "low" => Ok(RiskLevel::Low),
        "medium" => Ok(RiskLevel::Medium),
        "high" => Ok(RiskLevel::High),
        "critical" => Ok(RiskLevel::Critical),
        _ => Err(ImportError::BadValue {
            field,
            value: v,
            allowed: "Low, Medium, High, Critical",
        }),
    }
}

fn likelihood(v: Option<String>) -> Result<Likelihood, ImportError> {
    let v = v.ok_or(ImportError::MissingField("Likelihood"))?;
    match first_word(&v).as_str() {
        "low" => Ok(Likelihood::Low),
        "medium" => Ok(Likelihood::Medium),
        "high" => Ok(Likelihood::High),
        _ => Err(ImportError::BadValue {
            field: "Likelihood",
            value: v,
            allowed: "Low, Medium, High",
        }),
    }
}

fn cwe_ids(text: &str) -> Vec<String> {
    let upper = text.to_ascii_uppercase();
    let mut out: Vec<String> = Vec::new();
    let mut rest = upper.as_str();
    while let Some(i) = rest.find("CWE-") {
        let tail = &rest[i + 4..];
        let n = tail.chars().take_while(|c| c.is_ascii_digit()).count();
        if n > 0 {
            let id = format!("CWE-{}", &tail[..n]);
            if !out.contains(&id) {
                out.push(id);
            }
        }
        rest = tail;
    }
    out
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

fn tickets(v: Option<String>) -> OrSentinel<Vec<Ticket>> {
    let unknown = || OrSentinel::Sentinel("Unknown".to_string());
    let Some(v) = v else { return unknown() };
    let bare = unwrap_emphasis(v.trim()).trim_end_matches('.');
    if bare.eq_ignore_ascii_case("none provided") || bare.eq_ignore_ascii_case("none") {
        return OrSentinel::Sentinel("None Provided".into());
    }
    if bare.eq_ignore_ascii_case("unknown") {
        return unknown();
    }
    let stated = |s: &str| {
        !(s.is_empty()
            || s.eq_ignore_ascii_case("unknown")
            || s.eq_ignore_ascii_case("not provided")
            || s.eq_ignore_ascii_case("none"))
    };
    let structured = v
        .lines()
        .any(|l| matches!(labelled(strip_marker(l)), Some((k, _)) if k == "type"));
    let mut out: Vec<Ticket> = Vec::new();
    if structured {
        for l in v.lines() {
            let Some((k, val)) = labelled(strip_marker(l)) else {
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
                ("url", Some(t)) => t.url = stated(&val).then_some(val),
                ("notes", Some(t)) => t.notes = stated(&val).then_some(val),
                _ => {}
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
    if out.is_empty() {
        unknown()
    } else {
        OrSentinel::Value(out)
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

/// The exporter's location string (`file:a-b`, `file`, a binary VA, or
/// `file:a-b @ va`) as (file, lines, binary VA).
fn exported_location(s: &str) -> (Option<String>, Option<[u32; 2]>, Option<String>) {
    let (at, va) = match s.split_once(" @ ") {
        Some((l, v)) => (l.trim(), Some(v.trim().to_string())),
        None if looks_like_va(s.trim()) => ("", Some(s.trim().to_string())),
        None => (s.trim(), None),
    };
    if at.is_empty() {
        return (None, None, va);
    }
    let ranged = at
        .rsplit_once(':')
        .and_then(|(p, r)| Some((p, line_range(r)?)));
    match ranged {
        Some((p, lines)) => (Some(p.to_string()), Some(lines), va),
        None => (Some(at.to_string()), None, va),
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
        if after.is_empty() || tail.is_some() {
            (h.file, h.lines, h.binary_va) = exported_location(loc);
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
    if let Some(s) = not_provided(&body.text()) {
        return (OrSentinel::Sentinel(s), Vec::new());
    }
    let mut lines: Vec<String> = Vec::new();
    let mut claims = Vec::new();
    for b in &body.0 {
        match b {
            Block::Text(ls) => lines.extend(
                ls.iter()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty() && !is_thematic_break(l)),
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
        if t.is_empty() {
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
/// prefix of rupu's hash of the claim's file, which rupu recomputes whenever
/// a report is written.
fn strip_sha_suffix(t: &str) -> &str {
    t.strip_suffix(')')
        .and_then(|x| x.rsplit_once(" (sha256 "))
        .filter(|(_, h)| !h.is_empty() && h.len() <= 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
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
        .and_then(|(loc, rest)| Some((loc, rest.strip_prefix("** — ")?)));
    let mut c = match exported {
        Some((loc, rest)) => {
            let mut c = claim(rest.trim().to_string());
            (c.file, c.lines, c.binary_va) = exported_location(loc);
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

fn patch(body: Option<&Body>) -> OrSentinel<Patch> {
    let Some(body) = body else { return missing() };
    let text = body.text();
    if let Some(s) = not_provided(&text) {
        return OrSentinel::Sentinel(s);
    }
    let found = body.0.iter().enumerate().find_map(|(i, b)| match b {
        Block::Fence { info, content, .. } if is_diff(info, content) => Some((i, content.clone())),
        _ => None,
    });
    match found {
        Some((i, diff)) => {
            let rest = render(&without(&body.0, i));
            OrSentinel::Value(Patch {
                diff,
                notes: (!rest.is_empty()).then_some(rest),
            })
        }
        None => OrSentinel::Sentinel(format!(
            "{NOT_PROVIDED_PREFIX}the imported report gives no unified diff: {}",
            one_line(&text)
        )),
    }
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
    if let Some(s) = not_provided(&body.text()) {
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
    if let Some(s) = not_provided(&body.text()) {
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
        if !code && line.trim().is_empty() {
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
            (None, Some(last)) => {
                last.push_str(if gap { "\n\n" } else { "\n" });
                last.push_str(line);
            }
        }
        gap = false;
    }
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

/// The cross-references (links for the `fnd_` ids it names, other than the
/// report's own), and the section's verbatim text for `references`.
fn cross_refs(body: Option<&Body>, own: &[String]) -> (OrSentinel<Vec<CrossRef>>, Option<String>) {
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
    let add = |x: CrossRef, refs: &mut Vec<CrossRef>| {
        if !own.contains(&x.finding_id) && !refs.iter().any(|r| r.finding_id == x.finding_id) {
            refs.push(x);
        }
    };
    for line in text.lines() {
        if let Some(x) = exported_cross_ref(line) {
            add(x, &mut refs);
            continue;
        }
        for id in fnd_ids(line) {
            let x = CrossRef {
                finding_id: id,
                relation: relation(line),
                note: Some(one_line(strip_marker(line))),
            };
            add(x, &mut refs);
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

/// Artifact paths, from the exporter's table or a list. rupu fills in the
/// hash, size and storage when the report is written.
fn artifacts(body: &Body) -> Vec<ArtifactRef> {
    let mut out = Vec::new();
    let mut seen_header = false;
    for (line, code) in body.lines() {
        if code {
            continue;
        }
        let t = line.trim();
        let path = if t.starts_with('|') {
            let first = table_cells(t).into_iter().next().unwrap_or_default();
            let delimiter_row = first.chars().all(|c| matches!(c, '-' | ':'));
            let header_row = !delimiter_row && !seen_header;
            seen_header |= header_row;
            if delimiter_row || header_row {
                continue;
            }
            first
        } else if is_list_item(t) {
            strip_marker(t).to_string()
        } else {
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
    out
}
