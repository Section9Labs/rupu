use crate::asset::Locator;
use crate::report::types::{FindingReport, RiskLevel};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PredicateError {
    #[error("unknown report field in completeness predicate: {0}")]
    UnknownField(String),
    #[error("unknown coordinate tag in completeness predicate: {0}")]
    UnknownCoordinate(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    HasField(String),
    HasBlockKind(String),
    HasClassificationSystem(String),
    LocatorHasCoordinate(String),
    MinSeverity(RiskLevel),
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletenessCheck {
    pub id: String,
    pub label: String,
    #[serde(default = "default_true")]
    pub required: bool,
    pub satisfied_when: Predicate,
}

fn default_true() -> bool {
    true
}

/// The report fields a `has_field` predicate may name (present & non-empty /
/// non-sentinel). Unknown names are an error, never a silent `false`.
fn field_present(r: &FindingReport, name: &str) -> Result<bool, PredicateError> {
    let ok = match name {
        "root_cause" => !r.root_cause.trim().is_empty(),
        "remediation" => !r.remediation.trim().is_empty(),
        "impact" => !r.impact.trim().is_empty(),
        "description" => !r.description.trim().is_empty(),
        "attack_vector" => !r.attack_vector.trim().is_empty(),
        "category" => !r.category.trim().is_empty(),
        "replication_steps" => !r.replication_steps.is_empty(),
        "evidence" => !r.evidence.is_empty(),
        _ => return Err(PredicateError::UnknownField(name.to_string())),
    };
    Ok(ok)
}

pub fn evaluate(p: &Predicate, r: &FindingReport, loc: &Locator) -> Result<bool, PredicateError> {
    Ok(match p {
        Predicate::HasField(f) => field_present(r, f)?,
        Predicate::HasBlockKind(k) => r.evidence.iter().any(|c| c.blocks.iter().any(|b| b.kind() == k)),
        Predicate::HasClassificationSystem(s) => {
            r.all_classifications().iter().any(|c| c.system.eq_ignore_ascii_case(s))
        }
        Predicate::LocatorHasCoordinate(tag) => {
            if !crate::asset::Coordinate::known_tag(tag) {
                return Err(PredicateError::UnknownCoordinate(tag.clone()));
            }
            loc.has(tag)
        }
        Predicate::MinSeverity(min) => rank(r.rating.risk_rating) >= rank(*min),
        Predicate::All(ps) => {
            for q in ps { if !evaluate(q, r, loc)? { return Ok(false); } }
            true
        }
        Predicate::Any(ps) => {
            for q in ps { if evaluate(q, r, loc)? { return Ok(true); } }
            false
        }
    })
}

fn rank(l: RiskLevel) -> u8 {
    match l { RiskLevel::Low => 0, RiskLevel::Medium => 1, RiskLevel::High => 2, RiskLevel::Critical => 3 }
}

/// `(satisfied_required, total_required)`.
pub fn score(checks: &[CompletenessCheck], r: &FindingReport, loc: &Locator) -> Result<(u32, u32), PredicateError> {
    let mut total = 0;
    let mut ok = 0;
    for c in checks.iter().filter(|c| c.required) {
        total += 1;
        if evaluate(&c.satisfied_when, r, loc)? { ok += 1; }
    }
    Ok((ok, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Coordinate, Proto};
    use crate::report::types::EvidenceBlock;

    fn build_report() -> FindingReport {
        serde_json::from_str(include_str!("../../tests/fixtures/finding_report/valid_full.json")).unwrap()
    }

    fn cls(system: &str, id: &str) -> crate::report::types::Classification {
        crate::report::types::Classification {
            system: system.to_string(),
            id: id.to_string(),
            vector: None,
        }
    }

    fn disasm_block() -> EvidenceBlock {
        EvidenceBlock::Disasm {
            arch: "x86_64".to_string(),
            listing: vec![
                crate::report::types::DisasmLine {
                    addr: "0x401000".to_string(),
                    text: "push rbp".to_string(),
                },
            ],
        }
    }

    #[test]
    fn predicates_evaluate_and_score() {
        let mut r = build_report();
        r.evidence[0].blocks = vec![disasm_block()];
        r.classifications = vec![cls("CWE", "CWE-306")];
        let loc = Locator(vec![Coordinate::Host("h".into()), Coordinate::Port { number: 1, proto: Proto::Tcp }]);

        assert!(evaluate(&Predicate::HasBlockKind("disasm".into()), &r, &loc).unwrap());
        assert!(evaluate(&Predicate::HasClassificationSystem("CWE".into()), &r, &loc).unwrap());
        assert!(evaluate(&Predicate::All(vec![
            Predicate::LocatorHasCoordinate("host".into()),
            Predicate::LocatorHasCoordinate("port".into()),
        ]), &r, &loc).unwrap());

        let checks = vec![
            CompletenessCheck { id: "listing".into(), label: "x".into(), required: true,
                satisfied_when: Predicate::HasBlockKind("disasm".into()) },
            CompletenessCheck { id: "http".into(), label: "y".into(), required: true,
                satisfied_when: Predicate::HasBlockKind("http_exchange".into()) },
        ];
        assert_eq!(score(&checks, &r, &loc).unwrap(), (1, 2));
    }

    #[test]
    fn unknown_field_is_an_error() {
        let r = build_report();
        let loc = Locator(vec![]);
        assert!(evaluate(&Predicate::HasField("nope".into()), &r, &loc).is_err());
    }
}
