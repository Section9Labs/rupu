//! Derived views of a finding report for list rows and completeness meters.
//! Pure functions over `FindingReport`, so the CP and exports compute them
//! identically.

use crate::report::types::{FindingReport, OrSentinel, VerificationStatus, NOT_PROVIDED_PREFIX};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Completeness {
    pub filled: usize,
    pub total: usize,
    /// Field names whose value is an `Unknown` / `Not Provided — …` sentinel,
    /// in report order.
    pub gaps: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportSummary {
    pub owner: String,
    pub product: String,
    pub cwe: Vec<String>,
    pub root_cause: String,
    /// Call-chain hop labels, source → sink; empty when the chain is a sentinel.
    pub chain: Vec<String>,
    pub completeness: Completeness,
    pub has_poc: bool,
    pub verification_status: Option<VerificationStatus>,
}

fn is_unknown(s: &str) -> bool {
    s.trim() == "Unknown"
}

fn is_not_provided<T>(v: &OrSentinel<T>) -> bool {
    matches!(v, OrSentinel::Sentinel(s) if s.starts_with(NOT_PROVIDED_PREFIX))
}

pub fn completeness(r: &FindingReport) -> Completeness {
    let checks: [(&'static str, bool); 11] = [
        ("owner", is_unknown(&r.ownership.owner)),
        ("product", is_unknown(&r.ownership.product)),
        (
            "affected_component",
            is_unknown(&r.ownership.affected_component),
        ),
        (
            "source_repository",
            is_unknown(&r.ownership.source_repository),
        ),
        (
            "tickets",
            matches!(&r.tickets, OrSentinel::Sentinel(s) if is_unknown(s)),
        ),
        ("cvss_v3", is_unknown(&r.rating.cvss_v3)),
        ("attack_vector", is_unknown(&r.attack_vector)),
        ("call_chain", is_not_provided(&r.call_chain)),
        ("recommended_patch", is_not_provided(&r.recommended_patch)),
        ("ci_cd_detection", is_not_provided(&r.ci_cd_detection)),
        ("regression_test", is_not_provided(&r.regression_test)),
    ];
    let gaps: Vec<&'static str> = checks
        .iter()
        .filter(|(_, gap)| *gap)
        .map(|(n, _)| *n)
        .collect();
    Completeness {
        filled: checks.len() - gaps.len(),
        total: checks.len(),
        gaps,
    }
}

pub fn summarize(r: &FindingReport) -> ReportSummary {
    let chain = match &r.call_chain {
        OrSentinel::Value(hops) => hops.iter().map(|h| h.label.clone()).collect(),
        OrSentinel::Sentinel(_) => Vec::new(),
    };
    ReportSummary {
        owner: r.ownership.owner.clone(),
        product: r.ownership.product.clone(),
        cwe: r.cwe.clone(),
        root_cause: r.root_cause.clone(),
        chain,
        completeness: completeness(r),
        has_poc: !r.artifacts.is_empty(),
        verification_status: r.verification.as_ref().map(|v| v.status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{FindingReport, OrSentinel};

    fn fixture() -> FindingReport {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    #[test]
    fn fixture_gaps_are_owner_and_cvss() {
        // valid_full.json has owner "Unknown" and cvss_v3 "Unknown".
        let c = completeness(&fixture());
        assert_eq!(c.total, 11);
        assert_eq!(c.gaps, vec!["owner", "cvss_v3"]);
        assert_eq!(c.filled, 9);
    }

    #[test]
    fn not_provided_sections_are_gaps_but_none_answers_are_not() {
        let mut r = fixture();
        r.regression_test = OrSentinel::Sentinel("Not Provided — needs hardware".into());
        r.ownership.source_repository = "Not Applicable".into();
        r.tickets = OrSentinel::Sentinel("None Provided".into());
        let c = completeness(&r);
        assert!(c.gaps.contains(&"regression_test"));
        assert!(!c.gaps.contains(&"source_repository"));
        assert!(!c.gaps.contains(&"tickets"));
    }

    #[test]
    fn unknown_tickets_is_a_gap() {
        let mut r = fixture();
        r.tickets = OrSentinel::Sentinel("Unknown".into());
        assert!(completeness(&r).gaps.contains(&"tickets"));
    }

    #[test]
    fn summarize_carries_list_row_fields() {
        let s = summarize(&fixture());
        assert_eq!(s.owner, "Unknown");
        assert_eq!(s.product, "Notebin (sample app)");
        assert_eq!(s.cwe, vec!["CWE-639".to_string(), "CWE-862".to_string()]);
        assert!(s.root_cause.contains("find_by_id"));
        assert_eq!(s.chain.len(), 3);
        assert!(!s.has_poc);
        assert_eq!(s.verification_status, None);
    }
}
