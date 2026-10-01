//! Choosing which numbered findings go into a report.

use crate::model::ExportFinding;
use crate::number::rank;
use rupu_coverage::{FindingProfile, Severity};
use std::collections::HashSet;

/// What to keep. Every field narrows the result; unset fields keep everything.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// When non-empty, only these finding ids (and `include_summaries` does
    /// not apply to them: naming a summary finding asks for it).
    pub ids: Vec<String>,
    pub ws_id: Option<String>,
    /// Keep findings whose `declared_by.run_id` is in the set (a run and its
    /// sub-runs).
    pub run_ids: Option<HashSet<String>>,
    /// This severity and worse.
    pub min_severity: Option<Severity>,
    /// Exact match on `report.ownership.owner`.
    pub owner: Option<String>,
    /// A CWE (`CWE-79`, `cwe-79`, `cwe_79` or `79`), compared by number with
    /// each entry of `report.cwe` and with the CWE a `concern_id` names (see
    /// [`concern_cwe`]). A value [`parse_cwe`] cannot read matches nothing,
    /// so callers refuse one up front.
    pub cwe: Option<String>,
    /// Keep summary-profile findings. Off by default: a report is about the
    /// findings that carry a full write-up.
    pub include_summaries: bool,
}

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

/// Filter `numbered` (already numbered, so numbers stay stable whatever is
/// dropped), preserving its order. A finding must pass every filter that is
/// set, `ids` included.
pub fn select(numbered: Vec<ExportFinding>, sel: &Selection) -> Vec<ExportFinding> {
    let wanted: HashSet<&str> = sel.ids.iter().map(String::as_str).collect();
    // `Some(None)`: a CWE was asked for but is unreadable, so nothing matches.
    let cwe = sel.cwe.as_deref().map(parse_cwe);
    numbered
        .into_iter()
        .filter(|f| {
            let r = &f.input.record;
            let listed = wanted.contains(r.id.as_str());
            if !wanted.is_empty() && !listed {
                return false;
            }
            // Naming a summary finding asks for it; otherwise summaries are
            // opt-in.
            if !listed && !sel.include_summaries && r.profile == FindingProfile::Summary {
                return false;
            }
            if sel.ws_id.as_deref().is_some_and(|ws| f.input.ws_id != ws) {
                return false;
            }
            if sel
                .run_ids
                .as_ref()
                .is_some_and(|runs| !runs.contains(&r.declared_by.run_id))
            {
                return false;
            }
            if sel
                .min_severity
                .is_some_and(|min| rank(r.severity) > rank(min))
            {
                return false;
            }
            if let Some(owner) = &sel.owner {
                if r.report.as_ref().map(|rep| &rep.ownership.owner) != Some(owner) {
                    return false;
                }
            }
            if let Some(cwe) = cwe {
                let Some(n) = cwe else {
                    return false;
                };
                let in_report = r
                    .report
                    .as_ref()
                    .is_some_and(|rep| rep.cwe.iter().any(|c| parse_cwe(c) == Some(n)));
                let in_concern = r
                    .concern_id
                    .as_deref()
                    .is_some_and(|c| concern_cwe(c) == Some(n));
                if !in_report && !in_concern {
                    return false;
                }
            }
            true
        })
        .collect()
}

/// The selection in words, for the report's scope line, e.g.
/// `Project notebin · severity ≥ high · full reports only`. `run` is the run
/// id the caller expanded into `run_ids` (the set itself is not printable);
/// `chosen` is the result of [`select`].
pub fn describe(sel: &Selection, run: Option<&str>, chosen: &[ExportFinding]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !sel.ids.is_empty() {
        parts.push(format!("{} selected findings", chosen.len()));
    } else if sel.ws_id.is_some() {
        let project = chosen.first().map_or("?", |f| f.input.project.as_str());
        parts.push(format!("Project {project}"));
    } else {
        parts.push("All projects".to_string());
    }
    if let Some(run) = run {
        parts.push(format!("run {run}"));
    }
    if let Some(min) = sel.min_severity {
        parts.push(format!("severity ≥ {}", format!("{min:?}").to_lowercase()));
    }
    if let Some(owner) = &sel.owner {
        parts.push(format!("owner {owner}"));
    }
    if let Some(cwe) = &sel.cwe {
        parts.push(format!("CWE {cwe}"));
    }
    if sel.ids.is_empty() {
        parts.push(
            if sel.include_summaries {
                "including summary findings"
            } else {
                "full reports only"
            }
            .to_string(),
        );
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExportInput;
    use rupu_coverage::{
        Attribution, FindingEvidence, FindingRecord, FindingReport, FindingScope, Surface,
    };

    fn report() -> FindingReport {
        serde_json::from_str(include_str!(
            "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    fn make(id: &str, sev: Severity, run: &str, report: Option<FindingReport>) -> ExportFinding {
        ExportFinding {
            number: format!("SEC-{id}"),
            input: ExportInput {
                ws_id: "ws1".into(),
                project: "p".into(),
                workflow_name: None,
                record: FindingRecord {
                    id: id.into(),
                    file_path: None,
                    line_range: None,
                    target_ref: None,
                    scope: FindingScope::Repo,
                    summary: format!("summary {id}"),
                    severity: sev,
                    concern_id: None,
                    evidence: FindingEvidence {
                        code_excerpt: None,
                        rationale: "r".into(),
                        references: vec![],
                    },
                    declared_by: Attribution {
                        run_id: run.into(),
                        model: "m".into(),
                        surface: Surface::Workflow,
                        codename: None,
                        agent: None,
                        provider: None,
                    },
                    declared_at: "2026-01-01T00:00:00Z".parse().unwrap(),
                    profile: if report.is_some() {
                        FindingProfile::Full
                    } else {
                        FindingProfile::Summary
                    },
                    report,
                    asset: None,
                },
            },
        }
    }

    fn with_summaries() -> Selection {
        Selection {
            include_summaries: true,
            ..Selection::default()
        }
    }

    fn ids(v: &[ExportFinding]) -> Vec<&str> {
        v.iter().map(|f| f.input.record.id.as_str()).collect()
    }

    fn sample() -> Vec<ExportFinding> {
        vec![
            make("crit", Severity::Critical, "run_a", Some(report())),
            make("high", Severity::High, "run_a", Some(report())),
            make("med", Severity::Medium, "run_b", Some(report())),
            make("low", Severity::Low, "run_b", None),
            make("info", Severity::Info, "run_c", None),
        ]
    }

    #[test]
    fn min_severity_keeps_that_severity_and_worse() {
        let sel = Selection {
            min_severity: Some(Severity::High),
            ..with_summaries()
        };
        assert_eq!(ids(&select(sample(), &sel)), ["crit", "high"]);
        let sel = Selection {
            min_severity: Some(Severity::Info),
            ..with_summaries()
        };
        assert_eq!(select(sample(), &sel).len(), 5);
    }

    #[test]
    fn summaries_are_dropped_unless_asked_for() {
        assert_eq!(
            ids(&select(sample(), &Selection::default())),
            ["crit", "high", "med"]
        );
        assert_eq!(select(sample(), &with_summaries()).len(), 5);
    }

    #[test]
    fn listed_ids_override_include_summaries_and_keep_input_order() {
        let sel = Selection {
            ids: vec!["info".into(), "crit".into(), "nope".into()],
            include_summaries: false,
            ..Selection::default()
        };
        // Order follows the (numbered) input, not the request.
        assert_eq!(ids(&select(sample(), &sel)), ["crit", "info"]);
    }

    #[test]
    fn listed_ids_still_obey_the_other_filters() {
        let sel = Selection {
            ids: vec!["crit".into(), "info".into()],
            min_severity: Some(Severity::High),
            ..Selection::default()
        };
        assert_eq!(ids(&select(sample(), &sel)), ["crit"]);
    }

    #[test]
    fn numbers_are_left_untouched() {
        let sel = Selection {
            ids: vec!["med".into()],
            ..Selection::default()
        };
        assert_eq!(select(sample(), &sel)[0].number, "SEC-med");
    }

    #[test]
    fn owner_matches_the_report_owner_exactly() {
        let mut mine = report();
        mine.ownership.owner = "Platform Team".into();
        let all = vec![
            make("mine", Severity::High, "run_a", Some(mine)),
            make("other", Severity::High, "run_a", Some(report())),
            make("summary", Severity::High, "run_a", None),
        ];
        let by = |o: &str| Selection {
            owner: Some(o.into()),
            ..with_summaries()
        };
        assert_eq!(ids(&select(all.clone(), &by("Platform Team"))), ["mine"]);
        // Exact: neither a prefix nor a different case matches.
        assert!(select(all.clone(), &by("Platform")).is_empty());
        assert!(select(all, &by("platform team")).is_empty());
    }

    #[test]
    fn cwe_matches_the_report_list_or_the_concern_id() {
        let mut by_concern = make("concern", Severity::High, "run_a", None);
        by_concern.input.record.concern_id = Some("cwe-639-idor".into());
        let all = vec![
            make("full", Severity::High, "run_a", Some(report())),
            by_concern,
            make("neither", Severity::High, "run_a", None),
        ];
        let sel = Selection {
            cwe: Some("CWE-639".into()),
            ..with_summaries()
        };
        assert_eq!(ids(&select(all.clone(), &sel)), ["full", "concern"]);
        let sel = Selection {
            cwe: Some("CWE-862".into()),
            ..with_summaries()
        };
        assert_eq!(ids(&select(all.clone(), &sel)), ["full"]);
        // A report CWE is matched whole: CWE-6 is not CWE-639.
        let sel = Selection {
            cwe: Some("CWE-6".into()),
            ..with_summaries()
        };
        assert!(!ids(&select(all, &sel)).contains(&"full"));
    }

    #[test]
    fn a_requested_cwe_is_read_as_its_number() {
        for (raw, want) in [
            ("CWE-79", Some(79)),
            ("cwe-79", Some(79)),
            ("Cwe_79", Some(79)),
            ("cwe79", Some(79)),
            (" 79 ", Some(79)),
            ("CWE-0798", Some(798)),
            ("CWE-", None),
            ("CWE-79x", None),
            ("xss", None),
            ("", None),
            ("CWE-99999999999", None),
        ] {
            assert_eq!(parse_cwe(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn a_concern_id_names_the_cwe_of_its_whole_digit_run() {
        for (id, want) in [
            ("cwe-79", Some(79)),
            ("cwe-79-xss", Some(79)),
            ("CWE-79", Some(79)),
            ("cwe-798-hardcoded-credentials", Some(798)),
            ("cwe-top25-2023:cwe-79-xss", Some(79)),
            ("cwe-top25-2023:cwe-787-out-of-bounds-write", Some(787)),
            ("cwe-research:cwe-1004-sensitive-cookie", Some(1004)),
            ("cwe_20-input", Some(20)),
            ("authz-idor", None),
            ("owasp-top10-2021:a03-injection", None),
        ] {
            assert_eq!(concern_cwe(id), want, "{id:?}");
        }
    }

    #[test]
    fn a_cwe_never_matches_a_longer_cwe_that_starts_with_its_digits() {
        let with_concern = |id: &str, concern: &str| {
            let mut f = make(id, Severity::High, "run_a", None);
            f.input.record.concern_id = Some(concern.into());
            f
        };
        let with_report_cwe = |id: &str, cwe: &str| {
            let mut r = report();
            r.cwe = vec![cwe.into()];
            make(id, Severity::High, "run_a", Some(r))
        };
        let all = vec![
            with_concern("xss", "cwe-79"),
            with_concern("xss_slug", "cwe-top25-2023:cwe-79-xss"),
            with_concern("creds", "cwe-top25-2023:cwe-798-hardcoded-credentials"),
            with_concern("ssrf_like", "cwe-792-incomplete-filtering"),
            with_concern("cmd", "cwe-top25-2023:cwe-78-os-command-injection"),
            with_concern("oob", "cwe-top25-2023:cwe-787-out-of-bounds-write"),
            with_concern("prefixed", "cwe-179-early-validation"),
            with_report_cwe("reported_xss", "CWE-79"),
            with_report_cwe("reported_creds", "CWE-798"),
        ];
        let by = |c: &str| Selection {
            cwe: Some(c.into()),
            ..with_summaries()
        };
        let want_79 = ["xss", "xss_slug", "reported_xss"];
        assert_eq!(ids(&select(all.clone(), &by("CWE-79"))), want_79);
        // Any spelling of the same number selects the same findings.
        for spelling in ["cwe-79", "cwe_79", "79"] {
            assert_eq!(
                ids(&select(all.clone(), &by(spelling))),
                want_79,
                "{spelling}"
            );
        }
        assert_eq!(ids(&select(all.clone(), &by("CWE-78"))), ["cmd"]);
        assert_eq!(
            ids(&select(all.clone(), &by("CWE-798"))),
            ["creds", "reported_creds"]
        );
        // An unreadable CWE matches nothing rather than everything.
        assert!(select(all, &by("xss")).is_empty());
    }

    #[test]
    fn run_ids_match_the_declaring_run() {
        let run_set: HashSet<String> = ["run_b".to_string(), "run_c".to_string()].into();
        let sel = Selection {
            run_ids: Some(run_set),
            ..with_summaries()
        };
        assert_eq!(ids(&select(sample(), &sel)), ["med", "low", "info"]);
        let none = Selection {
            run_ids: Some(HashSet::new()),
            ..with_summaries()
        };
        assert!(select(sample(), &none).is_empty());
    }

    #[test]
    fn describe_names_each_narrowing_in_words() {
        let all = sample();
        assert_eq!(
            describe(&Selection::default(), None, &all),
            "All projects · full reports only"
        );
        let sel = Selection {
            ws_id: Some("ws1".into()),
            min_severity: Some(Severity::High),
            owner: Some("Platform Team".into()),
            cwe: Some("CWE-639".into()),
            include_summaries: true,
            ..Selection::default()
        };
        assert_eq!(
            describe(&sel, Some("run_a"), &all),
            "Project p · run run_a · severity ≥ high · owner Platform Team · CWE CWE-639 · \
             including summary findings"
        );
        let listed = Selection {
            ids: vec!["crit".into(), "high".into()],
            ..Selection::default()
        };
        assert_eq!(describe(&listed, None, &all[..2]), "2 selected findings");
    }

    #[test]
    fn ws_id_scopes_to_one_project() {
        let mut all = sample();
        all[0].input.ws_id = "ws2".into();
        let sel = Selection {
            ws_id: Some("ws2".into()),
            ..with_summaries()
        };
        assert_eq!(ids(&select(all, &sel)), ["crit"]);
    }
}
