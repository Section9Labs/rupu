//! Goal evaluation: is an objective's measurable target met by the evidence
//! on disk? Finding goals are a pure predicate over the findings ledger;
//! asset goals match assets by kind + locator and a minimum rung on the
//! owning profile's depth ladder.

use crate::def::{AssetSelector, FindingSelector, Goal};
use rupu_coverage::{
    read_assets, read_findings, ActiveSet, Asset, Coordinate, CoveragePaths, FindingRecord,
    Locator, VerificationStatus,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GoalEvalError {
    #[error("reading evidence: {0}")]
    Io(String),
    #[error("goal `{0}`: {1}")]
    Bad(String, String),
}

/// The result of evaluating one [`Goal`] against the evidence on disk.
#[derive(Debug, Clone)]
pub struct GoalOutcome {
    pub id: String,
    pub met: bool,
    pub current: u64,
    pub target: u64,
    pub detail: String,
}

/// A finding is verified only when its report carries a `Confirmed`
/// verification. A record with no report (Summary profile) or no verification
/// block is unverified.
fn is_verified(rec: &FindingRecord) -> bool {
    rec.report
        .as_ref()
        .and_then(|r| r.verification.as_ref())
        .map(|v| v.status == VerificationStatus::Confirmed)
        .unwrap_or(false)
}

/// Whether the finding's report lists `class_id` among its classifications
/// (legacy `cwe` entries folded in). A record with no report matches nothing.
fn finding_has_classification(rec: &FindingRecord, class_id: &str) -> bool {
    match rec.report.as_ref() {
        Some(report) => report
            .all_classifications()
            .iter()
            .any(|c| c.id == class_id),
        None => false,
    }
}

/// Count the findings matching `sel`, optionally only the verified ones.
/// A selector with no classification matches every finding.
pub fn count_matching_findings(
    records: &[FindingRecord],
    sel: &FindingSelector,
    verified: bool,
) -> u64 {
    records
        .iter()
        .filter(|r| match &sel.classification {
            Some(id) => finding_has_classification(r, id),
            None => true,
        })
        .filter(|r| !verified || is_verified(r))
        .count() as u64
}

/// The locator-key tags a v1 asset goal can match on: the string-valued
/// coordinates. This is the single source of truth for both the matcher
/// ([`coord_value_matches`]) and `AgentiflowDef::validate`, so a goal that
/// validates can always be matched.
pub(crate) const V1_STRING_COORD_TAGS: [&str; 7] =
    ["host", "url", "path", "symbol", "sha256", "commit", "param"];

/// Whether `loc` carries a coordinate of the given string `tag` equal to
/// `want`.
///
/// v1 supports only the tags in [`V1_STRING_COORD_TAGS`]. Any other tag (the
/// structured coordinates `port` / `line_range` / `offset` / `address` /
/// `http_route` / `resource_id`, or an unknown tag) never matches here;
/// `AgentiflowDef::validate` rejects such a selector key up front so the goal
/// can't silently become unmeetable.
fn coord_value_matches(loc: &Locator, tag: &str, want: &str) -> bool {
    if !V1_STRING_COORD_TAGS.contains(&tag) {
        return false;
    }
    loc.0.iter().any(|c| match (tag, c) {
        ("host", Coordinate::Host(v)) => v == want,
        ("url", Coordinate::Url(v)) => v == want,
        ("path", Coordinate::Path(v)) => v == want,
        ("symbol", Coordinate::Symbol(v)) => v == want,
        ("sha256", Coordinate::Sha256(v)) => v == want,
        ("commit", Coordinate::Commit(v)) => v == want,
        ("param", Coordinate::Param(v)) => v == want,
        _ => false,
    })
}

/// Whether `a` satisfies `sel` at or above rung `min_depth_idx` of `ladder`:
/// the kind is equal, every locator entry of the selector matches, and the
/// asset's recorded depth sits at or above the minimum rung. An asset with no
/// recorded depth (or one that is not a rung of `ladder`) is not at/above.
fn asset_matches(a: &Asset, sel: &AssetSelector, min_depth_idx: usize, ladder: &[String]) -> bool {
    if a.kind != sel.kind {
        return false;
    }
    if !sel
        .locator
        .iter()
        .all(|(tag, want)| coord_value_matches(&a.locator, tag, want))
    {
        return false;
    }
    match a
        .depth
        .as_ref()
        .and_then(|d| ladder.iter().position(|r| r == d))
    {
        Some(idx) => idx >= min_depth_idx,
        None => false,
    }
}

pub struct GoalEvaluator;

impl GoalEvaluator {
    /// Evaluate `goal` against the ledgers under `paths`. Finding goals count
    /// matching findings; asset goals are evaluated against `active`.
    pub fn evaluate(
        goal: &Goal,
        paths: &CoveragePaths,
        active: &ActiveSet,
    ) -> Result<GoalOutcome, GoalEvalError> {
        if let Some(sel) = &goal.target.findings {
            let recs = read_findings(paths).map_err(|e| GoalEvalError::Io(e.to_string()))?;
            let target = goal.target.count_gte.unwrap_or(1);
            let current = count_matching_findings(&recs, sel, goal.target.verified);
            return Ok(GoalOutcome {
                id: goal.id.clone(),
                met: current >= target,
                current,
                target,
                detail: format!(
                    "{}/{} {}findings{}",
                    current,
                    target,
                    if goal.target.verified {
                        "verified "
                    } else {
                        ""
                    },
                    sel.classification
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                ),
            });
        }
        Self::evaluate_asset(goal, paths, active)
    }

    /// Count the assets matching the goal's asset selector at or above its
    /// `depth_at_least` rung (resolved on the ladder of the profile that owns
    /// the selector's kind). Defaults to "at least one exists".
    fn evaluate_asset(
        goal: &Goal,
        paths: &CoveragePaths,
        active: &ActiveSet,
    ) -> Result<GoalOutcome, GoalEvalError> {
        let sel = goal
            .target
            .asset
            .as_ref()
            .ok_or_else(|| GoalEvalError::Bad(goal.id.clone(), "no asset selector".into()))?;
        let depth = goal
            .target
            .depth_at_least
            .as_deref()
            .ok_or_else(|| GoalEvalError::Bad(goal.id.clone(), "no depth_at_least".into()))?;
        let profile = active.profile_for_kind(&sel.kind).ok_or_else(|| {
            GoalEvalError::Bad(goal.id.clone(), format!("kind `{}` unowned", sel.kind))
        })?;
        let ladder = &profile.coverage.depth_ladder;
        let min = ladder.iter().position(|r| r == depth).ok_or_else(|| {
            GoalEvalError::Bad(goal.id.clone(), format!("depth `{depth}` not in ladder"))
        })?;
        let assets = read_assets(&paths.assets).map_err(|e| GoalEvalError::Io(e.to_string()))?;
        let current = assets
            .iter()
            .filter(|a| asset_matches(a, sel, min, ladder))
            .count() as u64;
        let target = goal.target.count_gte.unwrap_or(1);
        Ok(GoalOutcome {
            id: goal.id.clone(),
            met: current >= target,
            current,
            target,
            detail: format!(
                "{}/{} assets {} @ depth>={}",
                current, target, sel.kind, depth
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_coverage::{
        Attribution, FindingEvidence, FindingProfile, FindingRecord, FindingReport, FindingScope,
        Severity, Surface, VerificationStatus,
    };
    use serde_json::json;

    /// Build a report through its serde shape (the type is large and strict;
    /// every required field is filled with a sentinel or a short string).
    fn mk_report(classification: &str, verified: Option<VerificationStatus>) -> FindingReport {
        let mut v = json!({
            "title": "t",
            "ownership": {
                "owner": "Unknown", "product": "p",
                "affected_component": "c", "source_repository": "r"
            },
            "tickets": "None Provided",
            "rating": {
                "impact": "High", "likelihood": "High", "risk_rating": "High",
                "risk_factor": "High", "cvss_v3": "Unknown"
            },
            "category": "cat",
            "attack_vector": "av",
            "description": "d",
            "impact": "i",
            "location": { "input": "in", "output": "out" },
            "root_cause": "rc",
            "call_chain": "None",
            "evidence": [],
            "remediation": "rem",
            "recommended_patch": "None",
            "ci_cd_detection": "None",
            "regression_test": "None",
            "replication_steps": [],
            "cross_references": "None",
            "references": "none",
            "classifications": [
                { "system": "CWE", "id": classification }
            ]
        });
        if let Some(status) = verified {
            v["verification"] = json!({ "status": status });
        }
        serde_json::from_value(v).expect("test report parses")
    }

    /// `report: None` models a Summary-profile record; otherwise a Full record
    /// carrying one classification and an optional verification.
    fn mk_finding(
        classification: Option<&str>,
        verified: Option<VerificationStatus>,
    ) -> FindingRecord {
        let report = classification.map(|c| mk_report(c, verified));
        // A verification without a classification is not a shape these tests
        // need; a `None` classification means "no report at all".
        FindingRecord {
            id: "fnd_test".into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: "s".into(),
            severity: Severity::High,
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
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: Utc::now(),
            profile: if report.is_some() {
                FindingProfile::Full
            } else {
                FindingProfile::Summary
            },
            report,
        }
    }

    #[test]
    fn counts_only_verified_findings_of_the_classification() {
        let recs = vec![
            mk_finding(Some("CWE-94"), Some(VerificationStatus::Confirmed)),
            mk_finding(Some("CWE-94"), Some(VerificationStatus::Unverified)),
            mk_finding(Some("CWE-94"), None), // full report, no verification block
            mk_finding(None, None),           // Summary profile, report: None
            mk_finding(Some("CWE-79"), Some(VerificationStatus::Confirmed)),
        ];
        let sel = crate::def::FindingSelector {
            classification: Some("CWE-94".into()),
        };
        // only the confirmed CWE-94
        assert_eq!(count_matching_findings(&recs, &sel, true), 1);
        // all CWE-94 regardless of verification (the report-less record has no classification)
        assert_eq!(count_matching_findings(&recs, &sel, false), 3);
    }

    fn host_asset(host: &str, depth: Option<&str>) -> Asset {
        let mut a = Asset::new(
            "network:host",
            Locator(vec![Coordinate::Host(host.into())]),
            host,
            None,
        );
        a.depth = depth.map(|d| d.to_string());
        a
    }

    #[test]
    fn asset_matches_kind_locator_and_min_depth() {
        let ladder = ["discovered", "enumerated", "tested", "exploited"].map(String::from);
        let sel = crate::def::AssetSelector {
            kind: "network:host".into(),
            locator: std::collections::BTreeMap::from([("host".into(), "1.1.2.2".into())]),
        };
        let min = 3; // "exploited"
        assert!(asset_matches(
            &host_asset("1.1.2.2", Some("exploited")),
            &sel,
            min,
            &ladder
        ));
        assert!(!asset_matches(
            &host_asset("1.1.2.2", Some("tested")),
            &sel,
            min,
            &ladder
        )); // below
        assert!(!asset_matches(
            &host_asset("9.9.9.9", Some("exploited")),
            &sel,
            min,
            &ladder
        )); // wrong host
        assert!(!asset_matches(
            &host_asset("1.1.2.2", None),
            &sel,
            min,
            &ladder
        )); // no depth yet
    }

    #[test]
    fn asset_matches_depth_is_at_least_not_equal() {
        let ladder = ["discovered", "enumerated", "tested", "exploited"].map(String::from);
        let sel = crate::def::AssetSelector {
            kind: "network:host".into(),
            locator: std::collections::BTreeMap::from([("host".into(), "1.1.2.2".into())]),
        };
        // min rung is "enumerated"
        let min = 1;
        // deeper than the minimum still matches (>=, not ==)
        assert!(asset_matches(
            &host_asset("1.1.2.2", Some("exploited")),
            &sel,
            min,
            &ladder
        ));
        // exactly the minimum matches
        assert!(asset_matches(
            &host_asset("1.1.2.2", Some("enumerated")),
            &sel,
            min,
            &ladder
        ));
        // shallower than the minimum does not
        assert!(!asset_matches(
            &host_asset("1.1.2.2", Some("discovered")),
            &sel,
            min,
            &ladder
        ));
        // a depth that is not a rung of the ladder is never at/above
        assert!(!asset_matches(
            &host_asset("1.1.2.2", Some("bogus")),
            &sel,
            min,
            &ladder
        ));
    }

    #[test]
    fn asset_matches_requires_all_locator_entries() {
        let ladder = ["discovered", "enumerated"].map(String::from);
        let mut a = Asset::new(
            "web:endpoint",
            Locator(vec![
                Coordinate::Host("example.test".into()),
                Coordinate::Path("/login".into()),
            ]),
            "login",
            None,
        );
        a.depth = Some("enumerated".into());
        let sel = |entries: &[(&str, &str)]| crate::def::AssetSelector {
            kind: "web:endpoint".into(),
            locator: entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        // both entries match
        assert!(asset_matches(
            &a,
            &sel(&[("host", "example.test"), ("path", "/login")]),
            0,
            &ladder
        ));
        // one entry wrong -> no match (all, not any)
        assert!(!asset_matches(
            &a,
            &sel(&[("host", "example.test"), ("path", "/admin")]),
            0,
            &ladder
        ));
        // an entry whose tag the asset does not carry -> no match
        assert!(!asset_matches(
            &a,
            &sel(&[
                ("host", "example.test"),
                ("url", "https://example.test/login")
            ]),
            0,
            &ladder
        ));
        // an empty locator matches on kind + depth alone
        assert!(asset_matches(&a, &sel(&[]), 0, &ladder));
        // a different kind never matches
        let mut other = sel(&[("host", "example.test")]);
        other.kind = "network:host".into();
        assert!(!asset_matches(&a, &other, 0, &ladder));
    }

    #[test]
    fn structured_coordinate_tags_never_match_in_v1() {
        let loc = Locator(vec![
            Coordinate::Host("1.1.2.2".into()),
            Coordinate::Port {
                number: 443,
                proto: rupu_coverage::Proto::Tcp,
            },
        ]);
        // supported string tag matches
        assert!(coord_value_matches(&loc, "host", "1.1.2.2"));
        // the asset genuinely carries a port, but `port` is not matchable in v1
        assert!(!coord_value_matches(&loc, "port", "443"));
        // nor are other structured tags / unknown tags
        assert!(!coord_value_matches(&loc, "line_range", "1-2"));
        assert!(!coord_value_matches(&loc, "hostname", "1.1.2.2"));

        let ladder = ["discovered".to_string()];
        let mut a = Asset::new("network:host", loc, "h", None);
        a.depth = Some("discovered".into());
        let sel = crate::def::AssetSelector {
            kind: "network:host".into(),
            locator: std::collections::BTreeMap::from([("port".into(), "443".into())]),
        };
        assert!(!asset_matches(&a, &sel, 0, &ladder));
    }
}
