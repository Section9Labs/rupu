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
        Self {
            record,
            project: None,
            ws_id: None,
            workflow: None,
        }
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
    match r
        .report
        .as_ref()
        .and_then(|rep| rep.verification.as_ref())
        .map(|v| v.status)
    {
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
    r.report
        .as_ref()
        .map(|rep| rep.title.as_str())
        .unwrap_or(&r.summary)
}

fn text_hit(r: &FindingRecord, needle: &str) -> bool {
    let n = needle.to_lowercase();
    [
        Some(title_of(r)),
        Some(r.summary.as_str()),
        Some(r.id.as_str()),
        r.file_path.as_deref(),
    ]
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
            "poc" => r
                .report
                .as_ref()
                .is_some_and(|rep| !rep.artifacts.is_empty()),
            "cwe" => !finding_cwes(r).is_empty(),
            _ => false,
        }),
        Key::Project => any(&|x| v.project.is_some_and(|p| ci(p, x)) || v.ws_id == Some(x)),
        Key::Cwe => {
            let have = finding_cwes(r);
            any(&|x| parse_cwe(x).is_some_and(|n| have.contains(&n)))
        }
        Key::Owner => any(&|x| {
            r.report
                .as_ref()
                .is_some_and(|rep| ci(&rep.ownership.owner, x))
        }),
        Key::Product => any(&|x| {
            r.report
                .as_ref()
                .is_some_and(|rep| ci(&rep.ownership.product, x))
        }),
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
                || runs
                    .get(x)
                    .is_some_and(|s| s.contains(&r.declared_by.run_id))
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
    let mut out: Vec<&'a T> = items
        .iter()
        .filter(|i| matches(q, &view(i), runs))
        .collect();
    out.sort_by_cached_key(|i| {
        let r = view(i).record;
        (
            std::cmp::Reverse(severity_rank(r.severity)),
            std::cmp::Reverse(r.declared_at),
            r.id.clone(),
        )
    });
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FacetValue {
    pub value: String,
    pub count: usize,
}

fn ranked(counts: HashMap<String, usize>) -> Vec<FacetValue> {
    let mut v: Vec<FacetValue> = counts
        .into_iter()
        .map(|(value, count)| FacetValue { value, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    v
}

/// Values in use per key, for autocomplete and the severity tiles.
pub fn facets<'a>(
    views: impl IntoIterator<Item = FindingView<'a>>,
) -> BTreeMap<&'static str, Vec<FacetValue>> {
    let keys = [
        "tag", "project", "owner", "product", "cwe", "agent", "workflow", "concern", "verified",
        "profile", "scope",
    ];
    let mut counts: HashMap<&'static str, HashMap<String, usize>> =
        keys.iter().map(|k| (*k, HashMap::new())).collect();
    let mut sev = [0usize; 5];
    let mut bump = |k: &'static str, v: &str| {
        *counts
            .get_mut(k)
            .expect("known key")
            .entry(v.to_string())
            .or_default() += 1;
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
            .map(|s| FacetValue {
                value: s.to_string(),
                count: sev[severity_rank(sev_of(s)) as usize],
            })
            .collect(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{
        Attribution, FindingEvidence, FindingRecord, FindingScope, Surface,
    };
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
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
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
            rec(
                "fnd_crit",
                Severity::Critical,
                2,
                &["class:sqli", "needs-poc"],
            ),
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
        let got: Vec<_> = select(&f, FindingView::bare, &q, &runs)
            .into_iter()
            .map(|r| r.id.clone())
            .collect();
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
            .map(|r| FindingView {
                record: r,
                project: Some("shop-web"),
                ws_id: Some("ws1"),
                workflow: Some("review"),
            })
            .collect();
        assert!(matches(&q, &views[0], &RunScopes::new()));
        assert!(matches(
            &parse_query("project:ws1").unwrap(),
            &views[0],
            &RunScopes::new()
        ));
        assert!(!matches(
            &parse_query("project:other").unwrap(),
            &views[0],
            &RunScopes::new()
        ));
    }

    #[test]
    fn facets_count_values_most_used_first() {
        let f = fixture();
        let fc = facets(f.iter().map(FindingView::bare));
        let sev: Vec<(&str, usize)> = fc["severity"]
            .iter()
            .map(|v| (v.value.as_str(), v.count))
            .collect();
        assert_eq!(
            sev,
            [
                ("critical", 1),
                ("high", 1),
                ("medium", 0),
                ("low", 1),
                ("info", 0)
            ]
        );
        let tags: Vec<(&str, usize)> = fc["tag"]
            .iter()
            .map(|v| (v.value.as_str(), v.count))
            .collect();
        assert_eq!(tags, [("class:sqli", 2), ("needs-poc", 1)]);
        assert_eq!(fc["cwe"][0].value, "CWE-89");
        assert_eq!(fc["verified"][0].value, "unverified");
        assert!(fc["project"].is_empty());
    }
}
