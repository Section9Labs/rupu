/// The number of a requested CWE: `CWE-79`, `cwe-79`, `cwe_79`, `cwe79` or a
/// bare `79` (surrounding whitespace ignored). `None` for anything else.
pub fn parse_cwe(raw: &str) -> Option<u32> {
    let s = raw.trim();
    let digits = match s.get(..3) {
        Some(p) if p.eq_ignore_ascii_case("cwe") => {
            let rest = &s[3..];
            rest.strip_prefix(['-', '_']).unwrap_or(rest)
        }
        _ => s,
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The CWE number a `concern_id` names, by the web's rule (`lib/cwe.ts`
/// `cweFromFinding`: the first `cwe[-_]?<digits>`, case-insensitive). The
/// whole digit run is read, so `cwe-top25-2023:cwe-798-hardcoded-credentials`
/// is 798 and never 79, and `cwe-79` / `cwe-79-xss` are 79.
pub fn concern_cwe(concern_id: &str) -> Option<u32> {
    let lower = concern_id.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("cwe") {
        let rest = &lower[from + at + 3..];
        let rest = rest.strip_prefix(['-', '_']).unwrap_or(rest);
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            return rest[..digits].parse().ok();
        }
        from += at + 3;
    }
    None
}

/// The CWE number a MITRE definition URL names, by the web's rule (`lib/cwe.ts`
/// `cweFromFinding`: the first `cwe.mitre.org/data/definitions/<digits>`,
/// case-insensitive, anywhere in the string).
pub fn reference_cwe(reference: &str) -> Option<u32> {
    const PATH: &str = "cwe.mitre.org/data/definitions/";
    let lower = reference.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(PATH) {
        let rest = &lower[from + at + PATH.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            return rest[..digits].parse().ok();
        }
        from += at + PATH.len();
    }
    None
}

/// Every CWE number a finding names, by the web's rule (`lib/cwe.ts`
/// `findingCweIds`), so the query, the facets, the CWE column and the export
/// agree: each `report.cwe` entry [`parse_cwe`] can read, then the CWE its
/// `concern_id` names ([`concern_cwe`]) or, when it names none, the first
/// `evidence.references` URL [`reference_cwe`] can read. Deduplicated, in
/// that order.
pub fn finding_cwes(r: &crate::ledger::events::FindingRecord) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let from_report = r
        .report
        .iter()
        .flat_map(|rep| rep.cwe.iter())
        .filter_map(|c| parse_cwe(c));
    let derived = r
        .concern_id
        .as_deref()
        .and_then(concern_cwe)
        .or_else(|| r.evidence.references.iter().find_map(|u| reference_cwe(u)));
    for n in from_report.chain(derived) {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{
        Attribution, FindingEvidence, FindingRecord, FindingScope, Surface,
    };

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
        assert_eq!(
            concern_cwe("cwe-top25-2023:cwe-798-hardcoded-credentials"),
            Some(798)
        );
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
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
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
        // No concern CWE: the first MITRE definition URL in the evidence
        // references names it, as the web's `cweFromFinding` reads it.
        r.evidence.references = vec![
            "https://owasp.org/Top10/A03_2021-Injection/".into(),
            "https://cwe.mitre.org/data/definitions/79.html".into(),
            "https://cwe.mitre.org/data/definitions/116.html".into(),
        ];
        assert_eq!(finding_cwes(&r), vec![79]);
        // A concern CWE wins over the references, which are then not read.
        r.concern_id = Some("cwe-89-sqli".into());
        assert_eq!(finding_cwes(&r), vec![89]);
        r.concern_id = Some("authz-idor".into());
        assert_eq!(finding_cwes(&r), vec![79]);
    }

    #[test]
    fn reference_cwe_reads_a_mitre_definition_url() {
        for (url, want) in [
            ("https://cwe.mitre.org/data/definitions/79.html", Some(79)),
            ("http://CWE.MITRE.ORG/data/definitions/1004.html", Some(1004)),
            ("cwe.mitre.org/data/definitions/22", Some(22)),
            ("see https://cwe.mitre.org/data/definitions/639.html", Some(639)),
            (
                "https://cwe.mitre.org/data/definitions/x.html https://cwe.mitre.org/data/definitions/20.html",
                Some(20),
            ),
            ("https://cwe.mitre.org/data/downloads.html", None),
            ("https://cwe.mitre.org/data/definitions/.html", None),
            ("https://cwemitre.org/data/definitions/79.html", None),
            ("https://owasp.org/cwe-79", None),
        ] {
            assert_eq!(reference_cwe(url), want, "{url}");
        }
    }
}
