//! System-prompt guidance appended to agents recording under the full
//! profile, so an agent needs no external reporting-standard file.

use crate::report::{FindingProfile, FindingWriteOptions};

const FULL_GUIDANCE: &str = "\
## Recording findings

Findings in this run use the full report profile. Record each finding with one \
`report_finding` call whose `report` object is complete. rupu generates the \
Markdown, HTML and PDF reports from it, so do not also write a report file.

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

/// The full guidance for a run: the findings-contract text (full profile only)
/// followed by the engagement section (only when an engagement is active).
/// With no engagement the output is exactly what it was before profiles
/// existed.
pub fn guidance(opts: &FindingWriteOptions) -> Option<String> {
    let findings = findings_guidance(opts);
    let engagement = engagement_guidance(opts);
    match (findings, engagement) {
        (Some(f), Some(e)) => Some(format!("{f}\n\n{e}")),
        (f, e) => f.or(e),
    }
}

fn findings_guidance(opts: &FindingWriteOptions) -> Option<String> {
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

/// What the active engagement profiles declare, written for the agent: the
/// asset kinds it may name (with their coordinates), the evidence block kinds
/// and classification systems the profile expects, and the depth ladder
/// `asset_mark` accepts. `None` when no engagement is active.
///
/// Public so an agent that has `asset_mark` but records no findings (and so
/// gets no findings guidance) can still be told what to mark.
pub fn engagement_guidance(opts: &FindingWriteOptions) -> Option<String> {
    let engagement = opts.engagement.as_deref()?;
    let mut s = format!(
        "## Engagement profiles\n\n\
This run is scoped to the engagement profile(s): {}. A finding names what it is \
about with the optional `asset` field of `report_finding`: `kind` (one of the kinds \
below), `locator` (the coordinates that identify it: a list of single-key objects \
such as `{{\"sha256\": \"<hex>\"}}` or `{{\"address\": 4198400}}`) and, optionally, `parent` \
and `label`. Record how deeply you have examined an asset with `asset_mark` (`kind`, \
`locator`, and a `depth` from the profile's ladder below), when it is among your tools. \
A kind or depth that no active profile declares is rejected.",
        engagement.ids().join(", ")
    );
    for p in engagement.profiles() {
        if p.asset_kinds.is_empty()
            && p.evidence_blocks.is_empty()
            && p.classification_systems.is_empty()
            && p.coverage.depth_ladder.is_empty()
        {
            // A composite that only groups other profiles has nothing of its own.
            continue;
        }
        s.push_str(&format!("\n\n### {} — {}", p.id, p.name));
        if !p.asset_kinds.is_empty() {
            s.push_str("\nAsset kinds:");
            for k in &p.asset_kinds {
                s.push_str(&format!("\n- `{}`", k.id));
                if let Some(parent) = &k.parent {
                    s.push_str(&format!(" (inside `{parent}`)"));
                }
                if !k.coordinates.is_empty() {
                    s.push_str(&format!(" — coordinates: {}", k.coordinates.join(", ")));
                }
            }
        }
        if !p.evidence_blocks.is_empty() {
            s.push_str(&format!(
                "\nEvidence block kinds: {}",
                p.evidence_blocks.join(", ")
            ));
        }
        if !p.classification_systems.is_empty() {
            s.push_str(&format!(
                "\nClassification systems: {}",
                p.classification_systems.join(", ")
            ));
        }
        if !p.coverage.depth_ladder.is_empty() {
            s.push_str(&format!(
                "\nDepth ladder, shallowest to deepest: {}",
                p.coverage.depth_ladder.join(" -> ")
            ));
        }
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

    fn engaged(ids: &[&str]) -> FindingWriteOptions {
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let set = crate::profile::builtin_registry()
            .unwrap()
            .active_set(&ids)
            .unwrap();
        FindingWriteOptions::default().with_engagement(Some(std::sync::Arc::new(set)))
    }

    #[test]
    fn no_engagement_guidance_is_the_unchanged_full_text() {
        // Byte-identical to the pre-engagement output.
        assert_eq!(
            guidance(&FindingWriteOptions::default()).as_deref(),
            Some(FULL_GUIDANCE)
        );
        assert!(engagement_guidance(&FindingWriteOptions::default()).is_none());
    }

    #[test]
    fn an_engagement_adds_kinds_coordinates_blocks_systems_and_ladder() {
        let g = guidance(&engaged(&["binary"])).unwrap();
        // The full-profile text is still there, unchanged, ahead of the section.
        assert!(g.starts_with(FULL_GUIDANCE), "{g}");
        assert!(g.contains("## Engagement profiles"), "{g}");
        assert!(g.contains("binary"), "{g}");
        // Kinds (namespaced), with their coordinates and parent.
        assert!(g.contains("`binary:function`"), "{g}");
        assert!(g.contains("sha256, address, symbol"), "{g}");
        assert!(g.contains("`binary:binary`"), "{g}");
        // Evidence blocks and classification systems (the profile loader
        // sorts and dedups both), then the depth ladder in its authored order.
        assert!(g.contains("code_slice, diff, disasm, hexdump, text"), "{g}");
        assert!(g.contains("CVE, CWE"), "{g}");
        assert!(g.contains("located -> disassembled -> analyzed"), "{g}");
    }

    #[test]
    fn engagement_guidance_reaches_a_summary_profile_run_too() {
        let o = engaged(&["binary"]).with_profile(FindingProfile::Summary);
        let g = guidance(&o).expect("an engagement is worth guiding even under summary");
        assert!(g.starts_with("## Engagement profiles"), "{g}");
        assert!(!g.contains("## Recording findings"), "{g}");
        assert_eq!(Some(g), engagement_guidance(&o));
    }

    #[test]
    fn every_active_profile_gets_its_own_subsection() {
        let g = engagement_guidance(&engaged(&["code", "binary"])).unwrap();
        assert!(g.contains("### code"), "{g}");
        assert!(g.contains("`code:file`"), "{g}");
        assert!(g.contains("unreviewed -> reviewed"), "{g}");
        assert!(g.contains("### binary"), "{g}");
        assert!(g.contains("located -> disassembled -> analyzed"), "{g}");
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
