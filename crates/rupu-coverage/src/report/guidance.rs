//! System-prompt guidance appended to agents recording under the full
//! profile, so an agent needs no external reporting-standard file.

use crate::report::{FindingProfile, FindingWriteOptions};

const FULL_GUIDANCE: &str = "\
## Recording findings

Findings in this run use the full report profile. Record each finding with one \
`report_finding` call whose `report` object is complete. The report is stored as \
structured data and is the source of truth for this finding.

- Write in a concise, formal, factual tone. Do not speculate.
- Every field is required except `cwe` (may be an empty list) and `artifacts`. \
When information genuinely cannot be determined, use the field's sentinel instead of omitting or guessing it: `Unknown` for owner, \
product, affected_component, attack_vector and cvss_v3; `Not Applicable` for \
source_repository when no source-controlled code is involved; `None Provided` for \
tickets when none are mentioned; `None` for cross_references when no related \
finding exists.
- Never invent owners, products, repositories, ticket identifiers or CVSS scores.
- `root_cause` names the single underlying defect — the exact variable, check or \
assumption that is wrong — not the symptom.
- `call_chain` runs from the externally reachable entry point to the sink, one hop \
per entry, with file and lines (or binary_va), and every gate crossed and why it passes.
- `evidence` is a list of claims, each citing file and lines (or binary_va) with \
only the lines that matter.
- `recommended_patch`, `ci_cd_detection` and `regression_test` are mandatory: a \
minimal unified diff; a pipeline check that fails the build on this class of issue; \
a deterministic test with its exact command and its result on the vulnerable and the \
patched build. If one genuinely cannot be produced, write exactly \
`Not Provided — <one-line justification>`.
- List proof-of-concept files (scripts, outputs, harnesses) as workspace-relative \
paths in `artifacts`; rupu stores them with the finding.
- Cross-reference related findings by the `fnd_` id a previous `report_finding` \
call returned.
- A rejected call lists every problem at once. Fix all of them and call again.";

pub fn guidance(opts: &FindingWriteOptions) -> Option<String> {
    if opts.profile != FindingProfile::Full {
        return None;
    }
    let mut s = FULL_GUIDANCE.to_string();
    if !opts.ticket_patterns.is_empty() {
        s.push_str(
            "\n- Treat references matching these patterns as existing tickets (record them in `tickets`): ",
        );
        s.push_str(&opts.ticket_patterns.join(", "));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_full_gets_guidance() {
        assert!(guidance(&FindingWriteOptions::default()).is_some());
        let summary = FindingWriteOptions::default().with_profile(FindingProfile::Summary);
        assert!(guidance(&summary).is_none());
    }

    #[test]
    fn ticket_patterns_are_appended() {
        let o = FindingWriteOptions {
            ticket_patterns: vec!["ABC-[0-9]+".into()],
            ..Default::default()
        };
        assert!(guidance(&o).unwrap().contains("ABC-[0-9]+"));
    }
}
