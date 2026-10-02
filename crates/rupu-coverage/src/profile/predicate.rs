//! The profile completeness vocabulary: a profile declares a checklist of
//! [`CompletenessCheck`]s, each satisfied (or not) by a tiny [`Predicate`]
//! grammar evaluated against a finding + its asset locator. Deliberately not a
//! DSL — `All`/`Any` is the ceiling. If a profile ever needs more, that is a
//! signal to add a *primitive*, not a scripting language.

use crate::asset::{Coordinate, Locator};
use crate::report::types::{FindingReport, RiskLevel};
use serde::{Deserialize, Serialize};

/// Why a predicate could not be evaluated. Every one is a profile-authoring
/// error — a typo'd field, coordinate, or block kind — surfaced loudly rather
/// than silently treated as "not satisfied".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PredicateError {
    #[error("unknown report field `{0}` in a completeness predicate")]
    UnknownField(String),
    #[error("unknown coordinate `{0}` in a completeness predicate")]
    UnknownCoordinate(String),
    #[error("unknown evidence-block kind `{0}` in a completeness predicate")]
    UnknownBlockKind(String),
}

/// One completeness check a profile declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletenessCheck {
    pub id: String,
    pub label: String,
    #[serde(default = "default_true")]
    pub required: bool,
    pub satisfied_when: Predicate,
}

pub(crate) fn default_true() -> bool {
    true
}

/// The predicate grammar. `snake_case` so TOML reads
/// `{ has_field = "root_cause" }`, `{ any = [ … ] }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    /// A report field is present and non-empty / non-sentinel.
    HasField(String),
    /// At least one evidence block of this kind is present.
    HasBlockKind(String),
    /// At least one classification from this taxonomy is present.
    HasClassificationSystem(String),
    /// The finding's asset locator carries this coordinate.
    LocatorHasCoordinate(String),
    /// The finding's risk rating is at least this level.
    MinSeverity(RiskLevel),
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
}

/// The report fields a `has_field` predicate may name.
fn field_present(r: &FindingReport, name: &str) -> Result<bool, PredicateError> {
    let ok = match name {
        "root_cause" => !r.root_cause.trim().is_empty(),
        "remediation" => !r.remediation.trim().is_empty(),
        "impact" => !r.impact.trim().is_empty(),
        "description" => !r.description.trim().is_empty(),
        "attack_vector" => !r.attack_vector.trim().is_empty(),
        "category" => !r.category.trim().is_empty(),
        "replication_steps" => !r.replication_steps.is_empty(),
        "evidence" => !r.evidence.is_empty() || !r.blocks.is_empty(),
        _ => return Err(PredicateError::UnknownField(name.to_string())),
    };
    Ok(ok)
}

fn rank(l: RiskLevel) -> u8 {
    match l {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 1,
        RiskLevel::High => 2,
        RiskLevel::Critical => 3,
    }
}

fn known_block_kind(kind: &str) -> bool {
    matches!(
        kind,
        "text"
            | "code_slice"
            | "diff"
            | "table"
            | "image"
            | "hexdump"
            | "disasm"
            | "decompile"
            | "http_exchange"
            | "scan_output"
            | "pcap_ref"
    )
}

/// Evaluate a predicate against a finding and its asset locator.
pub fn evaluate(p: &Predicate, r: &FindingReport, loc: &Locator) -> Result<bool, PredicateError> {
    Ok(match p {
        Predicate::HasField(f) => field_present(r, f)?,
        Predicate::HasBlockKind(k) => {
            if !known_block_kind(k) {
                return Err(PredicateError::UnknownBlockKind(k.clone()));
            }
            r.block_kinds().contains(k.as_str())
        }
        Predicate::HasClassificationSystem(s) => r
            .all_classifications()
            .iter()
            .any(|c| c.system.eq_ignore_ascii_case(s)),
        Predicate::LocatorHasCoordinate(tag) => {
            if !Coordinate::known_tag(tag) {
                return Err(PredicateError::UnknownCoordinate(tag.clone()));
            }
            loc.has(tag)
        }
        Predicate::MinSeverity(min) => rank(r.rating.risk_rating) >= rank(*min),
        Predicate::All(ps) => {
            for q in ps {
                if !evaluate(q, r, loc)? {
                    return Ok(false);
                }
            }
            true
        }
        Predicate::Any(ps) => {
            let mut any = false;
            for q in ps {
                if evaluate(q, r, loc)? {
                    any = true;
                    break;
                }
            }
            any
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::types::{Classification, EvidenceBlock as EB};

    fn report() -> FindingReport {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    #[test]
    fn has_field_and_unknown_field_errors() {
        let r = report();
        let loc = Locator::default();
        assert!(evaluate(&Predicate::HasField("root_cause".into()), &r, &loc).unwrap());
        assert_eq!(
            evaluate(&Predicate::HasField("nope".into()), &r, &loc),
            Err(PredicateError::UnknownField("nope".into()))
        );
    }

    #[test]
    fn block_kind_and_classification_system() {
        let mut r = report();
        r.blocks = vec![EB::ScanOutput {
            tool: "nmap".into(),
            output: "open".into(),
        }];
        r.classifications = vec![Classification {
            system: "CVE".into(),
            id: "CVE-1".into(),
            vector: None,
        }];
        let loc = Locator::default();
        assert!(evaluate(&Predicate::HasBlockKind("scan_output".into()), &r, &loc).unwrap());
        assert!(!evaluate(&Predicate::HasBlockKind("disasm".into()), &r, &loc).unwrap());
        // case-insensitive, and folds legacy cwe
        assert!(evaluate(&Predicate::HasClassificationSystem("cve".into()), &r, &loc).unwrap());
        assert_eq!(
            evaluate(&Predicate::HasBlockKind("bogus".into()), &r, &loc),
            Err(PredicateError::UnknownBlockKind("bogus".into()))
        );
    }

    #[test]
    fn locator_coordinate_and_unknown_coordinate_errors() {
        let r = report();
        let loc = Locator(vec![Coordinate::Host("h".into())]);
        assert!(evaluate(&Predicate::LocatorHasCoordinate("host".into()), &r, &loc).unwrap());
        assert!(!evaluate(&Predicate::LocatorHasCoordinate("port".into()), &r, &loc).unwrap());
        assert_eq!(
            evaluate(&Predicate::LocatorHasCoordinate("cidr".into()), &r, &loc),
            Err(PredicateError::UnknownCoordinate("cidr".into()))
        );
    }

    #[test]
    fn all_any_compose() {
        let r = report();
        let loc = Locator(vec![
            Coordinate::Host("h".into()),
            Coordinate::Port {
                number: 22,
                proto: crate::asset::Proto::Tcp,
            },
        ]);
        let all = Predicate::All(vec![
            Predicate::LocatorHasCoordinate("host".into()),
            Predicate::LocatorHasCoordinate("port".into()),
        ]);
        assert!(evaluate(&all, &r, &loc).unwrap());
        let any = Predicate::Any(vec![
            Predicate::LocatorHasCoordinate("url".into()),
            Predicate::LocatorHasCoordinate("host".into()),
        ]);
        assert!(evaluate(&any, &r, &loc).unwrap());
        // an unknown name inside All/Any still errors (not silently false)
        let bad = Predicate::Any(vec![Predicate::HasField("nope".into())]);
        assert!(evaluate(&bad, &r, &loc).is_err());
    }
}
