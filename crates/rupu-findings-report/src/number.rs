use crate::model::{ExportFinding, ExportInput};
use rupu_coverage::Severity;
use std::collections::{BTreeMap, HashMap};

/// Sort rank of a severity: 0 is the worst (critical). Numbering sorts by
/// it; selection compares against it.
pub(crate) fn rank(s: Severity) -> u8 {
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
            out.push(ExportFinding {
                number: format!("{prefix}-{:03}", n + 1),
                input,
            });
        }
    }
    out
}

pub fn number_map(findings: &[ExportFinding]) -> HashMap<String, String> {
    findings
        .iter()
        .map(|f| (f.input.record.id.clone(), f.number.clone()))
        .collect()
}

/// The finding's display title: the report title for full findings, the
/// summary otherwise.
pub fn title(f: &ExportFinding) -> &str {
    f.input
        .record
        .report
        .as_ref()
        .map(|r| r.title.as_str())
        .unwrap_or(&f.input.record.summary)
}

/// Bidirectional and other invisible formatting characters that can make a
/// filename display as something it is not (e.g. right-to-left override).
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// `s` as one safe file-name component: whitespace runs (including newlines)
/// become a single space, and path separators, Windows-reserved characters,
/// control characters and bidi controls are dropped. A dropped `/` or `\`
/// means no `..` segment can survive as a path component.
fn clean_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        // Whitespace (including newlines/tabs) becomes a space *before* control
        // characters are stripped, so "a\nb" reads "a b", not "ab".
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| {
            !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                && !c.is_control()
                && !is_bidi_control(*c)
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Longest finding number (in chars) a file name will carry.
const MAX_NUMBER_CHARS: usize = 40;

/// Longest title (in chars) a file name will carry.
const MAX_TITLE_CHARS: usize = 80;

/// A title as safe file-name text: the cleaning [`filename`] applies to a
/// finding title (no separators, control or bidi characters; whitespace
/// collapsed; at most 80 characters). It may come back empty, and it does not
/// touch a leading dot: a caller that uses it as a whole name handles both.
pub fn sanitize_title(s: &str) -> String {
    let collapsed = clean_component(s);
    let short: String = collapsed.chars().take(MAX_TITLE_CHARS).collect();
    short.trim_end().to_string()
}

/// The display-number prefix used when none is configured.
pub const DEFAULT_PREFIX: &str = "SEC";

/// Whether `p` is acceptable as a display-number prefix: an ASCII letter then
/// up to 15 letters, digits, `_` or `-`. The configured value ends up in file
/// names and document text, and a project-level config is repo-controlled, so
/// anything else is refused rather than escaped.
pub fn is_valid_prefix(p: &str) -> bool {
    let mut chars = p.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && p.len() <= 16
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The suggested file name: `<number> - <title>.<ext>`. Both the title and
/// the number are cleaned: the number is caller-supplied, so it must not be
/// able to smuggle a path (`../../x`) into a download or zip entry name.
pub fn filename(f: &ExportFinding, ext: &str) -> String {
    let number: String = clean_component(&f.number)
        .trim_start_matches('.')
        .chars()
        .take(MAX_NUMBER_CHARS)
        .collect();
    let number = match number.trim() {
        "" => "finding",
        n => n,
    };
    format!("{number} - {}.{ext}", sanitize_title(title(f)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExportInput;
    use rupu_coverage::{
        Attribution, FindingEvidence, FindingProfile, FindingRecord, FindingScope, Severity,
        Surface,
    };

    fn rec(id: &str, sev: Severity, at: &str) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: format!("Summary of {id}"),
            severity: sev,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: Attribution {
                run_id: "run_1".into(),
                model: "m".into(),
                surface: Surface::Workflow,
            },
            declared_at: at.parse().unwrap(),
            profile: FindingProfile::Summary,
            report: None,
        }
    }
    fn input(ws: &str, r: FindingRecord) -> ExportInput {
        ExportInput {
            ws_id: ws.into(),
            project: ws.into(),
            workflow_name: None,
            record: r,
        }
    }

    #[test]
    fn numbers_per_project_by_severity_then_time() {
        let out = assign_numbers(
            vec![
                input("a", rec("fnd_low", Severity::Low, "2026-01-01T00:00:00Z")),
                input(
                    "a",
                    rec("fnd_crit_late", Severity::Critical, "2026-02-01T00:00:00Z"),
                ),
                input(
                    "a",
                    rec("fnd_crit_early", Severity::Critical, "2026-01-15T00:00:00Z"),
                ),
                input("b", rec("fnd_b", Severity::High, "2026-01-01T00:00:00Z")),
            ],
            "SEC",
        );
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
        assert_eq!(
            filename(&f, "pdf"),
            "SEC-001 - Path abc weird quoted title.pdf"
        );
    }

    #[test]
    fn filename_turns_newlines_into_spaces() {
        let mut r = rec("fnd_x", Severity::High, "2026-01-01T00:00:00Z");
        r.summary = "line one\nline two\r\n\tline\u{a0}three".into();
        let f = assign_numbers(vec![input("a", r)], "SEC").remove(0);
        assert_eq!(
            filename(&f, "md"),
            "SEC-001 - line one line two line three.md"
        );
    }

    #[test]
    fn filename_strips_bidi_and_format_controls() {
        let mut r = rec("fnd_x", Severity::High, "2026-01-01T00:00:00Z");
        r.summary = "a\u{200e}b\u{200f}c\u{202a}d\u{202b}e\u{202c}f\u{202d}g\u{202e}h\u{2066}i\u{2067}j\u{2068}k\u{2069}l".into();
        let f = assign_numbers(vec![input("a", r)], "SEC").remove(0);
        assert_eq!(filename(&f, "md"), "SEC-001 - abcdefghijkl.md");
    }

    #[test]
    fn sanitize_title_cleans_and_truncates_like_a_finding_title() {
        assert_eq!(
            sanitize_title("Q3 / \"audit\":\n  <report>\u{202e}  "),
            "Q3 audit report"
        );
        assert_eq!(sanitize_title("x".repeat(200).as_str()).chars().count(), 80);
        assert_eq!(sanitize_title("  \n "), "");
        // Leading dots are the caller's call: a title is not a whole name here.
        assert_eq!(sanitize_title("..hidden"), "..hidden");
    }

    #[test]
    fn prefix_validation_accepts_only_short_identifier_like_prefixes() {
        for ok in ["SEC", "VULN", "a", "Sec_1", "ACME-SEC", "A234567890123456"] {
            assert!(is_valid_prefix(ok), "{ok}");
        }
        for bad in [
            "",
            "1SEC",
            "-SEC",
            "_SEC",
            "SE C",
            "SEC/1",
            "../x",
            "SEC\n",
            "SEC\"",
            "SÉC",
            "A2345678901234567",
        ] {
            assert!(!is_valid_prefix(bad), "{bad:?}");
        }
    }

    #[test]
    fn filename_truncates_long_titles() {
        let mut r = rec("fnd_x", Severity::High, "2026-01-01T00:00:00Z");
        r.summary = "x".repeat(200);
        let f = assign_numbers(vec![input("a", r)], "SEC").remove(0);
        assert_eq!(
            filename(&f, "md"),
            format!("SEC-001 - {}.md", "x".repeat(80))
        );
    }

    #[test]
    fn filename_cleans_a_hostile_number_like_the_title() {
        let mut f = assign_numbers(
            vec![input(
                "a",
                rec("fnd_x", Severity::High, "2026-01-01T00:00:00Z"),
            )],
            "SEC",
        )
        .remove(0);
        for (number, want) in [
            ("../../evil", "evil - Summary of fnd_x.md"),
            ("a/b\\c", "abc - Summary of fnd_x.md"),
            ("C:\\win\\x", "Cwinx - Summary of fnd_x.md"),
            ("..", "finding - Summary of fnd_x.md"),
            ("", "finding - Summary of fnd_x.md"),
            ("  SEC \n 7 \u{202e}", "SEC 7 - Summary of fnd_x.md"),
            ("SEC.001", "SEC.001 - Summary of fnd_x.md"),
        ] {
            f.number = number.to_string();
            assert_eq!(filename(&f, "md"), want, "number {number:?}");
        }
        f.number = "N".repeat(500);
        assert_eq!(
            filename(&f, "md"),
            format!("{} - Summary of fnd_x.md", "N".repeat(MAX_NUMBER_CHARS))
        );
    }
}
