//! Best-effort import of Markdown finding reports back into `FindingReport`.

mod common;

use common::*;
use rupu_coverage::report::{
    validate_report, HopRole, Likelihood, OrSentinel, Relation, RiskLevel, ValidateCtx,
    NOT_PROVIDED_PREFIX,
};
use rupu_coverage::Severity;
use rupu_findings_report::import::{
    fnd_ids, parse_report, retain_known_cross_references, ImportError, Parsed, NOT_STATED,
};
use rupu_findings_report::number::number_map;
use rupu_findings_report::{render_finding, Format};
use std::collections::HashSet;

const PLAIN: &str = include_str!("fixtures/import/notebin_plain.md");
const MARKDOWN: &str = include_str!("fixtures/import/notebin_markdown.md");
const ID1: &str = "fnd_01J00000000000000000000001";
const ID2: &str = "fnd_01J00000000000000000000002";

fn report_of(md: &str) -> (rupu_coverage::FindingReport, Vec<String>) {
    match parse_report(md).expect("parses") {
        Parsed::Report { report, own_ids } => (report, own_ids),
        Parsed::NotAReport => panic!("expected a report"),
    }
}

fn assert_valid(r: &rupu_coverage::FindingReport, known: &[&str]) {
    let known: Vec<String> = known.iter().map(|s| s.to_string()).collect();
    validate_report(
        r,
        &ValidateCtx {
            known_finding_ids: &known,
            max_bytes: 256 * 1024,
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_plain_layout_parses_into_a_valid_report() {
    let (r, own) = report_of(PLAIN);
    assert_eq!(own, vec![ID1.to_string()]);
    assert_eq!(r.title, "Notes API returns another user's note by id");
    assert_eq!(r.ownership.owner, "Unknown");
    assert_eq!(r.ownership.product, "Notebin (sample app)");
    assert_eq!(r.tickets, OrSentinel::Sentinel("None Provided".into()));
    assert_eq!(r.rating.impact, RiskLevel::High);
    assert_eq!(r.rating.likelihood, Likelihood::High);
    assert_eq!(r.rating.risk_rating, RiskLevel::Critical);
    assert_eq!(r.rating.risk_factor, RiskLevel::High);
    assert_eq!(r.rating.cvss_v3, "8.1");
    assert_eq!(r.cwe, vec!["CWE-639".to_string(), "CWE-862".to_string()]);
    assert_eq!(r.location.input, "GET /api/notes/{id} path parameter id.");
    assert_eq!(
        r.location.output,
        "The JSON body of the note, including title and body."
    );
    let OrSentinel::Value(hops) = &r.call_chain else {
        panic!("{:?}", r.call_chain)
    };
    assert_eq!(hops.len(), 3);
    assert_eq!(hops[0].role, HopRole::Source);
    assert_eq!(hops[1].role, HopRole::Hop);
    assert_eq!(hops[2].role, HopRole::Sink);
    assert_eq!(hops[1].file.as_deref(), Some("src/routes/notes.rs"));
    assert_eq!(hops[1].lines, Some([40, 58]));
    assert_eq!(r.evidence.len(), 1);
    assert_eq!(r.evidence[0].file.as_deref(), Some("src/routes/notes.rs"));
    assert_eq!(r.evidence[0].lang.as_deref(), Some("rust"));
    assert!(r.evidence[0]
        .excerpt
        .as_deref()
        .unwrap()
        .contains("find_by_id(id)"));
    let OrSentinel::Value(p) = &r.recommended_patch else {
        panic!()
    };
    assert!(p.diff.contains("find_by_id_for_owner"));
    assert_eq!(p.notes, None);
    let OrSentinel::Value(ci) = &r.ci_cd_detection else {
        panic!()
    };
    assert_eq!(ci.stage, NOT_STATED);
    assert_eq!(ci.expect, NOT_STATED);
    assert_eq!(ci.command, None);
    assert!(ci.body.starts_with("Integration test: user B"));
    let OrSentinel::Value(rt) = &r.regression_test else {
        panic!()
    };
    assert_eq!(
        rt.command,
        "cargo test --test notes_access other_users_note_is_404"
    );
    assert_eq!(rt.expect_vulnerable, "fails: got 200");
    assert_eq!(rt.expect_patched, "passes: got 404");
    assert_eq!(
        rt.body,
        "notes_access::other_users_note_is_404 creates two users and one note."
    );
    assert_eq!(r.cross_references, OrSentinel::Sentinel("None".into()));
    assert!(
        !r.references.contains("CVSS"),
        "trailing rating lines are fields, not prose"
    );
    assert_eq!(r.replication_steps.len(), 3);
    assert_eq!(r.replication_steps[1], "Sign up as user B.");
    assert!(r.artifacts.is_empty());
    assert!(r.verification.is_none());
    assert_valid(&r, &[]);
}

#[test]
fn the_markdown_layout_keeps_tickets_cross_references_and_chain_code() {
    let (r, own) = report_of(MARKDOWN);
    // The cross-reference's id is not the report's own; the `**Finding ID:**`
    // line's is.
    assert_eq!(own, vec![ID2.to_string()]);
    let OrSentinel::Value(t) = &r.tickets else {
        panic!("{:?}", r.tickets)
    };
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].kind, "Example Tracker");
    assert_eq!(t[0].identifier, "NB-42");
    assert_eq!(
        t[0].url.as_deref(),
        Some("https://tracker.example.com/NB-42")
    );
    assert_eq!(
        t[0].notes.as_deref(),
        Some("Product team tracking remediation")
    );
    assert_eq!(r.cwe, vec!["CWE-613".to_string()]);
    assert_eq!(r.rating.cvss_v3, "6.5");
    assert_eq!(r.location.input, "`GET /s/{token}`");
    let OrSentinel::Value(hops) = &r.call_chain else {
        panic!()
    };
    assert_eq!(hops.len(), 3);
    assert_eq!(hops[2].file.as_deref(), Some("src/share/token.rs"));
    // Two bullet claims (the second takes the code block) and the call
    // chain's code block as a claim of its own.
    assert_eq!(r.evidence.len(), 3);
    assert_eq!(r.evidence[0].lines, Some([5, 9]));
    assert!(r.evidence[0].excerpt.is_none());
    assert!(r.evidence[1]
        .excerpt
        .as_deref()
        .unwrap()
        .contains("check_sig"));
    assert!(r.evidence[2].claim.starts_with("Call chain"));
    assert!(r.evidence[2]
        .excerpt
        .as_deref()
        .unwrap()
        .contains("ShareToken::verify"));
    let OrSentinel::Value(p) = &r.recommended_patch else {
        panic!()
    };
    assert_eq!(
        p.notes.as_deref(),
        Some("Existing links minted without `exp` should be treated as expired.")
    );
    let OrSentinel::Value(ci) = &r.ci_cd_detection else {
        panic!()
    };
    assert_eq!(ci.stage, "nightly");
    assert_eq!(
        ci.command.as_deref(),
        Some("cargo test --test share_expiry")
    );
    assert_eq!(ci.expect, "an expired token verifies");
    assert_eq!(
        ci.body,
        "A test mints a token with a past `exp` and requires `verify` to reject it."
    );
    let OrSentinel::Value(rt) = &r.regression_test else {
        panic!()
    };
    assert_eq!(
        rt.command,
        "cargo test --test share_expiry expired_link_is_rejected"
    );
    assert!(
        !rt.body.contains("```"),
        "the command's code block moved to `command`"
    );
    let OrSentinel::Value(x) = &r.cross_references else {
        panic!("{:?}", r.cross_references)
    };
    assert_eq!(x.len(), 1);
    assert_eq!(x[0].finding_id, ID1);
    assert_eq!(x[0].relation, Relation::Prerequisite);
    // Cross-reference text is also kept verbatim, so nothing is lost when an
    // id cannot be linked.
    assert!(r.references.starts_with("OWASP A07:2021"));
    assert!(r.references.contains("Cross-references (imported):"));
    assert!(r.references.contains("NB-007 covers link revocation."));
    assert_valid(&r, &[ID1]);
}

#[test]
fn an_exported_report_round_trips() {
    let original = full_report();
    let f = numbered(vec![input(
        "notebin",
        Some("audit"),
        full_record(ID1, Severity::Critical, original.clone()),
    )])
    .remove(0);
    let md = String::from_utf8(
        render_finding(&f, &number_map(std::slice::from_ref(&f)), Format::Markdown).unwrap(),
    )
    .unwrap();
    let (r, own) = report_of(&md);
    // From the exporter's Provenance row, `**Finding ID:** fnd_…`.
    assert_eq!(own, vec![ID1.to_string()]);
    assert!(
        md.contains(&format!("**Finding ID:** {ID1}")),
        "the exporter's spelling changed:\n{md}"
    );
    assert_eq!(r.title, original.title);
    assert_eq!(r.ownership, original.ownership);
    assert_eq!(r.tickets, original.tickets);
    assert_eq!(r.rating, original.rating);
    assert_eq!(r.category, original.category);
    assert_eq!(r.attack_vector, original.attack_vector);
    assert_eq!(r.cwe, original.cwe);
    assert_eq!(r.description, original.description);
    assert_eq!(r.impact, original.impact);
    assert_eq!(r.location, original.location);
    assert_eq!(r.root_cause, original.root_cause);
    assert_eq!(r.call_chain, original.call_chain);
    assert_eq!(r.evidence, original.evidence);
    assert_eq!(r.remediation, original.remediation);
    let (OrSentinel::Value(a), OrSentinel::Value(b)) =
        (&r.recommended_patch, &original.recommended_patch)
    else {
        panic!()
    };
    assert_eq!(a.diff.trim_end(), b.diff.trim_end());
    assert_eq!(a.notes, b.notes);
    assert_eq!(r.ci_cd_detection, original.ci_cd_detection);
    assert_eq!(r.regression_test, original.regression_test);
    assert_eq!(r.replication_steps, original.replication_steps);
    assert_eq!(r.cross_references, original.cross_references);
    // The export's display number and the record's provenance have no report
    // field; they are kept after the references, never dropped.
    let (references, other) = r
        .references
        .split_once("\n\nOther imported text:\n\n")
        .unwrap();
    assert_eq!(references, original.references);
    assert_eq!(other, EXPORTED_OTHER_TEXT);
    assert_valid(&r, &[]);
}

/// What an export of the fixture carries that no report field takes.
const EXPORTED_OTHER_TEXT: &str = "Header:\n\n**Identifier:** SEC-001\n\nProvenance:\n\n**Project:** notebin\n**Workflow:** audit\n**Run:** run_01\n**Surface:** workflow\n**Model:** claude-x\n**Declared:** 2026-03-01T10:00:00Z\n**Scope:** repo";

#[test]
fn a_file_that_is_not_a_report_is_skipped() {
    let readme = "# Notebin\n\nA toy notes app.\n\n## Description\n\nNotes, shared.\n";
    assert_eq!(parse_report(readme).unwrap(), Parsed::NotAReport);
}

#[test]
fn a_missing_required_section_fails() {
    let md = PLAIN.replace("Root Cause\n", "Root cause notes\n");
    assert_eq!(
        parse_report(&md).unwrap_err(),
        ImportError::MissingSection("Root Cause")
    );
}

#[test]
fn a_rating_outside_the_scale_fails() {
    let md = PLAIN.replace("Likelihood: High", "Likelihood: Very High");
    assert!(matches!(
        parse_report(&md).unwrap_err(),
        ImportError::BadValue {
            field: "Likelihood",
            ..
        }
    ));
}

#[test]
fn several_findings_in_one_file_fail() {
    let md = format!("{PLAIN}\n---\n\n{MARKDOWN}");
    assert_eq!(
        parse_report(&md).unwrap_err(),
        ImportError::SeveralFindings(2)
    );
}

#[test]
fn missing_sentinel_sections_say_so() {
    let md = PLAIN
        .split("CI/CD Detection\n")
        .next()
        .unwrap()
        .to_string()
        + "References\nCWE-639\n\nRisk Factor: High\n\nReplication Steps\nStep 1: Request another user's note.\n";
    let (r, _) = report_of(&md);
    let missing = format!("{NOT_PROVIDED_PREFIX}section missing from the imported report");
    assert_eq!(r.ci_cd_detection, OrSentinel::Sentinel(missing.clone()));
    assert_eq!(r.regression_test, OrSentinel::Sentinel(missing));
    assert_eq!(r.cross_references, OrSentinel::Sentinel("None".into()));
    assert_valid(&r, &[]);
}

#[test]
fn a_heading_inside_a_code_block_is_not_a_heading() {
    let md = PLAIN.replace(
        "Ok(Json(note))\n",
        "Ok(Json(note))\n// Remediation\nRemediation\n",
    );
    let (r, _) = report_of(&md);
    assert!(r.evidence[0]
        .excerpt
        .as_deref()
        .unwrap()
        .contains("\nRemediation"));
    assert!(r.remediation.starts_with("Scope the lookup"));
}

#[test]
fn a_deeper_heading_inside_prose_is_not_a_section() {
    let md = MARKDOWN.replace(
        "no expiry claim, so a link keeps working after the owner stops sharing the note.",
        "no expiry claim.\n\n#### Remediation\n\nA link keeps working after sharing stops.",
    );
    let (r, _) = report_of(&md);
    assert!(
        r.description.contains("#### Remediation"),
        "{}",
        r.description
    );
    assert!(
        r.remediation.starts_with("Add an `exp` claim"),
        "{}",
        r.remediation
    );
}

#[test]
fn a_lone_carriage_return_ends_a_line() {
    let (r, _) = report_of(&PLAIN.replace('\n', "\r"));
    assert_eq!(r.replication_steps.len(), 3);
}

#[test]
fn a_patch_section_without_a_diff_keeps_its_text_in_the_sentinel() {
    let start = PLAIN.find("Recommended Patch\n").unwrap();
    let end = PLAIN.find("CI/CD Detection\n").unwrap();
    let md = format!(
        "{}Recommended Patch\nCall find_by_id_for_owner instead of find_by_id.\n\n{}",
        &PLAIN[..start],
        &PLAIN[end..]
    );
    let (r, _) = report_of(&md);
    let OrSentinel::Sentinel(s) = &r.recommended_patch else {
        panic!()
    };
    assert!(s.starts_with(NOT_PROVIDED_PREFIX), "{s}");
    assert!(
        s.contains("Call find_by_id_for_owner instead of find_by_id."),
        "{s}"
    );
}

#[test]
fn fnd_ids_finds_whole_ulid_ids_only() {
    let text = format!(
        "{ID1}, again {ID1}; x{ID2} fnd_short fnd_{}",
        "A".repeat(27)
    );
    assert_eq!(fnd_ids(&text), vec![ID1.to_string()]);
}

#[test]
fn unknown_cross_references_are_dropped_to_none() {
    let (mut r, _) = report_of(MARKDOWN);
    retain_known_cross_references(&mut r, &HashSet::new());
    assert_eq!(r.cross_references, OrSentinel::Sentinel("None".into()));
    assert!(r
        .references
        .contains("fnd_01J00000000000000000000001 is a prerequisite"));
}

fn exported(report: rupu_coverage::FindingReport) -> String {
    let f = numbered(vec![input(
        "notebin",
        Some("audit"),
        full_record(ID1, Severity::Critical, report),
    )])
    .remove(0);
    String::from_utf8(
        render_finding(&f, &number_map(std::slice::from_ref(&f)), Format::Markdown).unwrap(),
    )
    .unwrap()
}

#[test]
fn an_exported_report_with_every_optional_part_round_trips() {
    use rupu_coverage::report::{
        ArtifactKind, ArtifactRef, ArtifactStorage, ChainHop, CrossRef, EvidenceClaim, Patch,
        Ticket, Verification, VerificationStatus,
    };
    let mut original = full_report();
    original.tickets = OrSentinel::Value(vec![
        Ticket {
            kind: "Example Tracker".into(),
            identifier: "NB-42".into(),
            url: Some("https://tracker.example.com/NB-42".into()),
            notes: Some("Tracking the fix".into()),
        },
        Ticket {
            kind: "Other".into(),
            identifier: "NB-43".into(),
            url: None,
            notes: None,
        },
    ]);
    // A rule inside prose, and a line-leading `<` the exporter escapes.
    original.description =
        "Notes are served by id.\n\n---\n\n<script> tags in a note body are stored as-is.".into();
    let OrSentinel::Value(hops) = &mut original.call_chain else {
        panic!()
    };
    hops[0].gate = None;
    hops.insert(
        2,
        ChainHop {
            label: "libnotes.so lookup -> row".into(),
            file: None,
            lines: None,
            binary_va: Some("libnotes.so@0x4010a0".into()),
            gate: None,
            passes_because: None,
            role: HopRole::Hop,
        },
    );
    hops.insert(
        3,
        ChainHop {
            label: "store module".into(),
            file: Some("src/store/mod.rs".into()),
            lines: None,
            binary_va: None,
            gate: Some("none".into()),
            passes_because: None,
            role: HopRole::Hop,
        },
    );
    original.evidence.push(EvidenceClaim {
        claim: "The access log shows user B reading user A's note.".into(),
        file: Some("src/store/notes.rs".into()),
        lines: Some([88, 97]),
        binary_va: Some("libnotes.so@0x4010a0".into()),
        excerpt: None,
        lang: None,
        sha256: Some("ab".repeat(32)),
        artifact: Some("out/access.log".into()),
    });
    original.evidence.push(EvidenceClaim {
        claim: "The template renders the body unescaped:\n<div>{{ note.body }}</div>".into(),
        file: None,
        lines: None,
        binary_va: None,
        excerpt: None,
        lang: None,
        sha256: None,
        artifact: None,
    });
    original.recommended_patch = OrSentinel::Value(Patch {
        diff: "--- a/src/routes/notes.rs\n+++ b/src/routes/notes.rs\n@@\n-a\n+b".into(),
        notes: Some("Apply the same change to the share handler.".into()),
    });
    original.ci_cd_detection =
        OrSentinel::Sentinel("Not Provided — the pipeline has no integration stage yet".into());
    original.replication_steps = vec![
        "Sign up as user A and create a note; record its id.".into(),
        "As user B, request the note:\n- with curl\n- or in the browser".into(),
        "Check the response:\n\n```sh\ncurl -s https://notebin.example.com/api/notes/1\n```".into(),
        "Send a body of:\n<note id=\"1\"/>".into(),
    ];
    original.cross_references = OrSentinel::Value(vec![CrossRef {
        finding_id: ID2.into(),
        relation: Relation::Sibling,
        note: Some("Share pages read notes through the same store.".into()),
    }]);
    original.artifacts = vec![ArtifactRef {
        path: "out/access.log".into(),
        sha256: "cd".repeat(32),
        size: 2048,
        kind: Some(ArtifactKind::Text),
        stored: Some(ArtifactStorage::Copied),
        host: None,
    }];
    original.verification = Some(Verification {
        status: VerificationStatus::Confirmed,
        by_run: Some("run_02".into()),
        notes: None,
    });

    let (r, own) = report_of(&exported(original.clone()));
    assert_eq!(own, vec![ID1.to_string()]);
    assert_eq!(r.tickets, original.tickets);
    assert_eq!(r.description, original.description);
    assert_eq!(r.call_chain, original.call_chain);
    // Claim hashes are shortened on export, and rupu rehashes claim files
    // whenever a report is written anyway.
    let mut evidence = original.evidence.clone();
    for c in &mut evidence {
        c.sha256 = None;
    }
    assert_eq!(r.evidence, evidence);
    assert_eq!(r.recommended_patch, original.recommended_patch);
    assert_eq!(r.ci_cd_detection, original.ci_cd_detection);
    assert_eq!(r.regression_test, original.regression_test);
    assert_eq!(r.replication_steps, original.replication_steps);
    assert_eq!(r.cross_references, original.cross_references);
    assert_eq!(
        r.references,
        format!(
            "{}\n\nCross-references (imported):\n\n- {ID2} (sibling) — Share pages read notes through the same store.\n\nOther imported text:\n\n{EXPORTED_OTHER_TEXT}\n**Verification:** Confirmed by run_02",
            original.references
        )
    );
    // rupu fills in an artifact's hash, size and storage when it is written.
    let paths: Vec<&str> = r.artifacts.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, ["out/access.log"]);
    // Verification is a claim about a run in the exporting installation: it
    // is kept as text (above), not as a typed verification.
    assert!(r.verification.is_none());
    assert_valid(&r, &[ID2]);
}

fn other_text(r: &rupu_coverage::FindingReport) -> &str {
    r.references
        .split_once("Other imported text:\n\n")
        .map_or("", |(_, o)| o)
}

// ---- fix round 1 -----------------------------------------------------------

#[test]
fn a_clean_report_keeps_no_other_text_but_its_identifier() {
    let (r, _) = report_of(&PLAIN.replace("Identifier: NB-001\n", ""));
    assert!(
        !r.references.contains("Other imported text:"),
        "{}",
        r.references
    );
    // The legacy identifier has no report field and is kept; the file name
    // and the finding id are not.
    let (r, _) = report_of(PLAIN);
    assert_eq!(other_text(&r), "Header:\n\nIdentifier: NB-001");
}

#[test]
fn unknown_header_lines_are_kept() {
    let md = PLAIN.replace(
        "Identifier: NB-001\n",
        "Identifier: NB-001\nStatus: Open, reported to vendor 2024-05-01\n\nOne user's notes are readable by every other user.\n",
    );
    let (r, _) = report_of(&md);
    let other = other_text(&r);
    assert!(
        other.contains("Status: Open, reported to vendor 2024-05-01"),
        "{other}"
    );
    assert!(
        other.contains("One user's notes are readable by every other user."),
        "{other}"
    );
}

#[test]
fn a_field_continued_on_the_next_line_keeps_its_continuation() {
    let md = PLAIN.replace(
        "Attack Vector: Authenticated HTTP request with another user's note id",
        "Attack Vector: Authenticated HTTP request\n  with another user's note id",
    );
    let (r, _) = report_of(&md);
    assert_eq!(
        r.attack_vector,
        "Authenticated HTTP request with another user's note id"
    );
}

#[test]
fn a_code_block_in_the_header_is_kept_verbatim() {
    let md = PLAIN.replace(
        "Identifier: NB-001\n",
        "Identifier: NB-001\n```\nGET /api/notes/7 -> 200\n```\n",
    );
    let (r, _) = report_of(&md);
    assert!(
        other_text(&r).contains("```\nGET /api/notes/7 -> 200\n```"),
        "{}",
        r.references
    );
}

#[test]
fn an_unknown_section_before_the_first_known_one_is_kept_under_its_name() {
    let md = MARKDOWN.replace(
        "## Description",
        "## Executive Summary\n\nAttackers can read shared notes forever; fix before GA.\n\n## Description",
    );
    let (r, _) = report_of(&md);
    assert!(
        other_text(&r).contains(
            "Executive Summary:\n\nAttackers can read shared notes forever; fix before GA."
        ),
        "{}",
        r.references
    );
}

#[test]
fn artifacts_prose_is_kept() {
    let md = format!(
        "{PLAIN}\nArtifacts\nThe capture was taken on the staging box at 10:02.\n\n- out/capture.pcap\n"
    );
    let (r, _) = report_of(&md);
    assert_eq!(r.artifacts.len(), 1);
    assert_eq!(r.artifacts[0].path, "out/capture.pcap");
    assert!(
        other_text(&r).contains("Artifacts:\n\nThe capture was taken on the staging box at 10:02."),
        "{}",
        r.references
    );
}

#[test]
fn provenance_text_is_kept_but_not_the_finding_id() {
    let md = format!(
        "{MARKDOWN}\n## Provenance\n\nFinding ID: {ID2}\n\nFound during the Q2 manual review by the red team.\n"
    );
    let (r, _) = report_of(&md);
    let other = other_text(&r);
    assert!(
        other.contains("Provenance:\n\nFound during the Q2 manual review by the red team."),
        "{other}"
    );
    assert!(!other.contains("Finding ID"), "{other}");
}

#[test]
fn unused_structured_ticket_lines_are_kept() {
    let md = MARKDOWN.replace(
        "  Notes: Product team tracking remediation\n",
        "  Notes: Product team tracking remediation\n  Status: Won't fix until 3.0\n",
    );
    let (r, _) = report_of(&md);
    let OrSentinel::Value(t) = &r.tickets else {
        panic!("{:?}", r.tickets)
    };
    assert_eq!(t.len(), 1);
    assert!(
        other_text(&r).contains("Ticket references:\n\nStatus: Won't fix until 3.0"),
        "{}",
        r.references
    );
}

#[test]
fn rating_qualifiers_are_kept() {
    let md = PLAIN
        .replace(
            "Impact: High",
            "Impact: High — only for tenants with sharing enabled",
        )
        .replace("Risk Factor: High", "Risk Factor: High (see appendix B)");
    let (r, _) = report_of(&md);
    assert_eq!(r.rating.impact, RiskLevel::High);
    assert_eq!(r.rating.risk_factor, RiskLevel::High);
    let other = other_text(&r);
    assert!(
        other.contains("Impact rating:\n\n— only for tenants with sharing enabled"),
        "{other}"
    );
    assert!(
        other.contains("Risk Factor:\n\n(see appendix B)"),
        "{other}"
    );
}

#[test]
fn identifier_and_severity_are_kept() {
    let md = PLAIN.replace("Owner: Unknown\n", "Owner: Unknown\nSeverity: Critical\n");
    let (r, _) = report_of(&md);
    let other = other_text(&r);
    assert!(other.contains("Identifier: NB-001"), "{other}");
    assert!(other.contains("Severity: Critical"), "{other}");
}

#[test]
fn a_byte_order_mark_is_ignored() {
    let (r, own) = report_of(&format!("\u{feff}{PLAIN}"));
    assert_eq!(r.title, "Notes API returns another user's note by id");
    assert_eq!(own, vec![ID1.to_string()]);
    let (r, _) = report_of(&format!(
        "\u{feff}{}",
        MARKDOWN.replace("Filename: NB-002 - Share links never expire.pdf\n\n", "")
    ));
    assert_eq!(r.title, "Share links never expire");
}

#[test]
fn sentinel_spellings_are_sentinels() {
    let (r, _) = report_of(&PLAIN.replace(
        "Existing Ticket References: None Provided",
        "Existing Ticket References: N/A",
    ));
    assert_eq!(r.tickets, OrSentinel::Sentinel("None Provided".into()));
    let (r, _) = report_of(&PLAIN.replace(
        "Existing Ticket References: None Provided",
        "Existing Ticket References: _TBD_",
    ));
    assert_eq!(r.tickets, OrSentinel::Sentinel("Unknown".into()));

    let ci_start = PLAIN.find("CI/CD Detection\n").unwrap();
    let ci_end = PLAIN.find("Regression Test\n").unwrap();
    let md = format!(
        "{}CI/CD Detection\nNone\n\n{}",
        &PLAIN[..ci_start],
        &PLAIN[ci_end..]
    );
    let (r, _) = report_of(&md);
    assert_eq!(
        r.ci_cd_detection,
        OrSentinel::Sentinel("Not Provided — None".into())
    );

    let chain_start = PLAIN.find("Call Chain / Attack Flow\n").unwrap();
    let chain_end = PLAIN.find("Evidence\n").unwrap();
    let md = format!(
        "{}Call Chain / Attack Flow\n**N/A**\n\n{}",
        &PLAIN[..chain_start],
        &PLAIN[chain_end..]
    );
    let (r, _) = report_of(&md);
    assert_eq!(
        r.call_chain,
        OrSentinel::Sentinel("Not Provided — N/A".into())
    );

    let rt_start = PLAIN.find("Regression Test\n").unwrap();
    let rt_end = PLAIN.find("Cross-References\n").unwrap();
    let md = format!(
        "{}Regression Test\nNot provided.\n\n{}",
        &PLAIN[..rt_start],
        &PLAIN[rt_end..]
    );
    let (r, _) = report_of(&md);
    assert_eq!(
        r.regression_test,
        OrSentinel::Sentinel("Not Provided — not given in the imported report".into())
    );
    assert_valid(&r, &[]);
}

#[test]
fn an_authors_full_hash_stays_in_the_claim() {
    let hash = "a".repeat(64);
    let md = MARKDOWN.replace(
        "- Verification never compares a time (`src/share/token.rs:30-41`).",
        &format!(
            "- Verification never compares a time (`src/share/token.rs:30-41`) (sha256 {hash})"
        ),
    );
    let (r, _) = report_of(&md);
    assert!(
        r.evidence[1].claim.contains(&hash),
        "{}",
        r.evidence[1].claim
    );
}

#[test]
fn an_optional_section_one_level_off_is_still_read() {
    let (r, _) = report_of(&MARKDOWN.replace("## Recommended Patch", "### Recommended Patch"));
    let OrSentinel::Value(p) = &r.recommended_patch else {
        panic!("{:?}", r.recommended_patch)
    };
    assert!(p.diff.contains("check_exp"));
    assert!(!r.remediation.contains("```"), "{}", r.remediation);
}

#[test]
fn an_indented_code_line_is_not_a_heading() {
    let md = MARKDOWN.replace(
        "so a link keeps working after the owner stops sharing the note.",
        "so a link keeps working after the owner stops sharing the note.\n\nExample:\n\n    ## Remediation\n    body",
    );
    let (r, _) = report_of(&md);
    assert!(
        r.description.contains("    ## Remediation\n    body"),
        "{}",
        r.description
    );
    assert!(
        r.remediation.starts_with("Add an `exp` claim"),
        "{}",
        r.remediation
    );
}

#[test]
fn headings_may_close_with_hashes() {
    let closed: String = MARKDOWN
        .lines()
        .map(|l| {
            if l.starts_with("## ") {
                format!("{l} ##\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    assert_eq!(report_of(&closed), report_of(MARKDOWN));
}

#[test]
fn a_rule_is_never_a_claim_or_part_of_a_step() {
    let md = MARKDOWN.replace(
        "- The token claims have no expiry (`src/share/token.rs:5-9`).\n",
        "- The token claims have no expiry (`src/share/token.rs:5-9`).\n\n---\n\n",
    );
    let (r, _) = report_of(&md);
    assert_eq!(r.evidence.len(), 3);
    assert!(r.evidence.iter().all(|c| c.claim != "---"));
    let (r, _) = report_of(&PLAIN.replace(
        "Step 2: Sign up as user B.\n",
        "Step 2: Sign up as user B.\n\n---\n\n",
    ));
    assert_eq!(r.replication_steps[1], "Sign up as user B.");
}

#[test]
fn a_step_marker_alone_on_its_line_takes_the_next_line() {
    let (r, _) =
        report_of(&PLAIN.replace("Step 1: Sign up as user A", "Step 1:\nSign up as user A"));
    assert_eq!(
        r.replication_steps[0],
        "Sign up as user A and create a note; record its id."
    );
    assert_eq!(r.replication_steps.len(), 3);

    let (r, _) = report_of(&MARKDOWN.replace(
        "Step 1: `GET /s/{token}` route",
        "Step 1:\n`GET /s/{token}` route",
    ));
    let OrSentinel::Value(hops) = &r.call_chain else {
        panic!()
    };
    assert_eq!(hops.len(), 3);
    assert!(
        hops[0].label.starts_with("`GET /s/{token}`"),
        "{}",
        hops[0].label
    );
    assert_eq!(hops[0].role, HopRole::Source);
    assert_eq!(hops[0].file.as_deref(), Some("src/app.rs"));
}

#[test]
fn two_findings_in_different_layouts_are_detected_without_file_names() {
    let plain = PLAIN.split_once('\n').unwrap().1;
    let markdown = MARKDOWN.split_once('\n').unwrap().1;
    let md = format!("{plain}\n---\n\n{markdown}");
    assert_eq!(
        parse_report(&md).unwrap_err(),
        ImportError::SeveralFindings(2)
    );
}

#[test]
fn a_prose_filename_line_is_not_a_second_finding() {
    let md = PLAIN.replace(
        "by iterating ids.\n",
        "by iterating ids.\nFilename: notes.txt is ignored by the handler.\n",
    );
    let (r, _) = report_of(&md);
    assert!(r.impact.contains("Filename: notes.txt"), "{}", r.impact);
}

#[test]
fn a_self_reference_stays_until_the_caller_drops_it() {
    let md = MARKDOWN
        .replace(
            "- NB-007 covers link revocation.",
            &format!("- {ID2} duplicates an older write-up."),
        )
        .replace(
            "the owner stops sharing the note.",
            &format!("the owner stops sharing the note. See {ID1}."),
        );
    let (mut r, own) = report_of(&md);
    // Only the labelled id is the report's own; the one in the prose is not.
    assert_eq!(own, vec![ID2.to_string()]);
    let OrSentinel::Value(x) = &r.cross_references else {
        panic!("{:?}", r.cross_references)
    };
    let ids: Vec<&str> = x.iter().map(|c| c.finding_id.as_str()).collect();
    assert_eq!(ids, [ID1, ID2]);
    // The caller passes the ledger's ids minus the report's own.
    retain_known_cross_references(&mut r, &HashSet::from([ID1.to_string()]));
    let OrSentinel::Value(x) = &r.cross_references else {
        panic!("{:?}", r.cross_references)
    };
    assert_eq!(x.len(), 1);
    assert_eq!(x[0].finding_id, ID1);
    assert_valid(&r, &[ID1]);
}

#[test]
fn a_code_span_that_is_not_a_place_stays_text() {
    let md = MARKDOWN
        .replace(
            "Step 1: `GET /s/{token}` route (`src/app.rs:44-46`)",
            "Step 1: **Route** — `GET /s/{token}` — role: source",
        )
        .replace(
            "- The token claims have no expiry (`src/share/token.rs:5-9`).",
            "**`GET /s/{token}`** — returns the note to anyone holding the link.",
        );
    let (r, _) = report_of(&md);
    let OrSentinel::Value(hops) = &r.call_chain else {
        panic!()
    };
    assert_eq!(hops[0].file, None);
    assert!(
        hops[0].label.contains("`GET /s/{token}`"),
        "{}",
        hops[0].label
    );
    assert_eq!(hops[0].role, HopRole::Source);
    assert_eq!(r.evidence[0].file, None);
    assert!(
        r.evidence[0].claim.contains("GET /s/{token}"),
        "{}",
        r.evidence[0].claim
    );
}

// ---- deviations from the brief's parser ------------------------------------

#[test]
fn multibyte_text_at_a_marker_boundary_does_not_panic() {
    // Byte 5 (`Step `) and byte 12 (`Not provided`) fall inside a character.
    let md = PLAIN.replace(
        "Step 2: Sign up as user B.\n",
        "Step 2: Sign up as user B.\n日本語のメモを確認する。\n",
    );
    let (r, _) = report_of(&md);
    assert_eq!(
        r.replication_steps[1],
        "Sign up as user B.\n日本語のメモを確認する。"
    );
    let ci_start = PLAIN.find("CI/CD Detection\n").unwrap();
    let ci_end = PLAIN.find("Regression Test\n").unwrap();
    let md = format!(
        "{}CI/CD Detection\nCI: 日本語のテストで検出する。\n\n{}",
        &PLAIN[..ci_start],
        &PLAIN[ci_end..]
    );
    let (r, _) = report_of(&md);
    let OrSentinel::Value(ci) = &r.ci_cd_detection else {
        panic!()
    };
    assert_eq!(ci.body, "CI: 日本語のテストで検出する。");
}

#[test]
fn only_references_gives_up_its_rating_lines() {
    let md = PLAIN.replace(
        "by iterating ids.\n",
        "by iterating ids.\nRisk Factor: Medium once sharing ships.\n",
    );
    let (r, _) = report_of(&md);
    assert!(
        r.impact.contains("Risk Factor: Medium once sharing ships."),
        "{}",
        r.impact
    );
    assert_eq!(r.rating.risk_factor, RiskLevel::High);
}

#[test]
fn an_escaped_pipe_in_an_artifact_path_is_part_of_it() {
    let md = format!(
        "{PLAIN}\nArtifacts\n| Path | Notes |\n| --- | --- |\n| out/a\\|b.log | capture |\n"
    );
    let (r, _) = report_of(&md);
    assert_eq!(r.artifacts[0].path, "out/a|b.log");
}

#[test]
fn a_backtick_line_with_backticks_after_it_is_inline_code() {
    let md = PLAIN.replace(
        "The get_note handler loads",
        "```get_note``` is inline code.\nThe get_note handler loads",
    );
    let (r, _) = report_of(&md);
    assert!(
        r.description.starts_with("```get_note``` is inline code."),
        "{}",
        r.description
    );
    assert!(
        r.remediation.starts_with("Scope the lookup"),
        "{}",
        r.remediation
    );
}

#[test]
fn an_id_glued_to_a_trailing_underscore_is_not_an_id() {
    assert!(fnd_ids(&format!("{ID1}_suffix")).is_empty());
}

#[test]
fn a_binary_and_its_address_are_read_together() {
    let md = PLAIN.replace(
        "→ NoteStore::find_by_id() (src/store/notes.rs:88-97)",
        "→ lookup in libnotes.so@0x4010a0",
    );
    let (r, _) = report_of(&md);
    let OrSentinel::Value(hops) = &r.call_chain else {
        panic!()
    };
    assert_eq!(hops[2].binary_va.as_deref(), Some("libnotes.so@0x4010a0"));
}

#[test]
fn the_exporters_no_evidence_note_is_no_evidence() {
    let start = PLAIN.find("Evidence\n").unwrap();
    let end = PLAIN.find("Remediation\n").unwrap();
    let md = format!(
        "{}Evidence\n_No evidence recorded._\n\n{}",
        &PLAIN[..start],
        &PLAIN[end..]
    );
    assert_eq!(
        parse_report(&md).unwrap_err(),
        ImportError::MissingSection("Evidence")
    );
}

#[test]
fn the_exporters_labels_and_last_command_block_win() {
    let md = MARKDOWN
        .replace(
            "**Fails when:** an expired token verifies",
            "Expected: 404 from the share route.\n\n**Fails when:** an expired token verifies",
        )
        .replace(
            "`share_expiry::expired_link_is_rejected` mints a token that expired an hour ago.\n",
            "`share_expiry::expired_link_is_rejected` mints a token that expired an hour ago.\n\n```sh\ncargo build --tests\n```\n",
        );
    let (r, _) = report_of(&md);
    let OrSentinel::Value(ci) = &r.ci_cd_detection else {
        panic!()
    };
    assert_eq!(ci.expect, "an expired token verifies");
    assert!(
        ci.body.contains("Expected: 404 from the share route."),
        "{}",
        ci.body
    );
    let OrSentinel::Value(rt) = &r.regression_test else {
        panic!()
    };
    assert_eq!(
        rt.command,
        "cargo test --test share_expiry expired_link_is_rejected"
    );
    assert!(rt.body.contains("cargo build --tests"), "{}", rt.body);
}

// ---- fix round 2 -----------------------------------------------------------

#[test]
fn cross_reference_notes_are_bounded_on_a_line_of_many_ids() {
    let ids: Vec<String> = (0..2000u32).map(|i| format!("fnd_{i:026}")).collect();
    let md = MARKDOWN.replace(
        "- NB-007 covers link revocation.",
        &format!("- {}", ids.join(" ")),
    );
    let (r, _) = report_of(&md);
    let OrSentinel::Value(x) = &r.cross_references else {
        panic!()
    };
    assert_eq!(x.len(), 2001);
    for c in &x[1..] {
        let note = c.note.as_deref().unwrap();
        assert!(
            note.chars().count() <= 301,
            "{} chars",
            note.chars().count()
        );
        assert!(note.ends_with('…'));
    }
    // The whole line is still in the references.
    assert!(r.references.contains(&ids.join(" ")));
}

#[test]
fn a_deep_description_heading_in_prose_is_not_a_second_finding() {
    let md = MARKDOWN.replace(
        "stops sharing the note.\n",
        "stops sharing the note.\n\n#### Description\n\nA nested sub-heading.\n",
    );
    let (r, _) = report_of(&md);
    assert!(
        r.description.contains("#### Description"),
        "{}",
        r.description
    );
}

#[test]
fn a_title_right_under_the_file_name_is_the_title() {
    let (r, _) = report_of(&PLAIN.replacen(".pdf\n\n", ".pdf\n", 1));
    assert_eq!(r.title, "Notes API returns another user's note by id");
}

#[test]
fn a_plain_line_under_the_finding_id_is_kept() {
    let md = PLAIN.replace(
        &format!("Finding ID: {ID1}\n"),
        &format!("Finding ID: {ID1}\nFound by the nightly scanner on staging\n"),
    );
    let (r, own) = report_of(&md);
    assert_eq!(own, vec![ID1.to_string()]);
    assert!(
        other_text(&r).contains("Found by the nightly scanner on staging"),
        "{}",
        r.references
    );
}

#[test]
fn a_step_marker_alone_in_evidence_takes_the_next_line() {
    let md = MARKDOWN.replace(
        "- The token claims have no expiry",
        "Step 1:\n- The token claims have no expiry",
    );
    let (r, _) = report_of(&md);
    assert_eq!(r.evidence.len(), 3);
    assert_eq!(
        r.evidence[0].claim,
        "The token claims have no expiry (`src/share/token.rs:5-9`)."
    );
    assert_valid(&r, &[ID1]);
}

// ---- final review fixes ----------------------------------------------------

/// PLAIN without its `Finding ID:` line.
fn plain_unlabelled() -> String {
    PLAIN.replace(&format!("Finding ID: {ID1}\n"), "")
}

#[test]
fn an_id_mentioned_only_in_prose_is_not_the_reports_own() {
    // The final review's probe: no id line, and the description names one
    // other finding. That finding must not be taken for this report's.
    let md = plain_unlabelled().replace(
        "The get_note handler loads",
        &format!("Like {ID2}, the get_note handler loads"),
    );
    let (r, own) = report_of(&md);
    assert!(own.is_empty(), "{own:?}");
    assert!(
        r.description.starts_with(&format!("Like {ID2}")),
        "{}",
        r.description
    );
}

#[test]
fn every_id_label_spelling_is_read_and_never_taken_for_the_title() {
    for label in [
        "Finding ID:",
        "Native Finding:",
        "native finding id:",
        "RUPU FINDING:",
        "Rupu Finding ID:",
        "Native Rupu Finding:",
        "**Native Finding:**",
        "**Native Finding**:",
    ] {
        let md = PLAIN.replace("Finding ID:", label);
        let (r, own) = report_of(&md);
        assert_eq!(own, [ID1], "{label}");
        assert_eq!(r.title, "Notes API returns another user's note by id");
        // A line that says nothing but the id is consumed.
        assert!(!other_text(&r).contains(ID1), "{label}: {}", other_text(&r));
    }
    // First in a file with no `Filename:` line, the id line is a field, not
    // the title.
    let md = format!(
        "Native Finding: {ID1}\n\n{}",
        plain_unlabelled().split_once("\n\n").unwrap().1
    );
    let (r, own) = report_of(&md);
    assert_eq!(own, [ID1]);
    assert_eq!(r.title, "Notes API returns another user's note by id");
}

#[test]
fn an_id_line_counts_in_any_section_but_cross_references_and_code() {
    // In a section: read, and taken out of the section's text.
    let md = plain_unlabelled().replace(
        "Description\n",
        &format!("Description\nNative Finding: {ID1}\n"),
    );
    let (r, own) = report_of(&md);
    assert_eq!(own, [ID1]);
    assert!(
        !r.description.contains("Native Finding"),
        "{}",
        r.description
    );
    assert_valid(&r, &[]);

    // In Cross-References an id names another finding.
    let md = plain_unlabelled().replace(
        "Cross-References\nNone\n",
        &format!("Cross-References\nFinding ID: {ID2}\n"),
    );
    let (_, own) = report_of(&md);
    assert!(own.is_empty(), "{own:?}");

    // In a code block it is code.
    let md = plain_unlabelled().replace(
        "Ok(Json(note))\n",
        &format!("Ok(Json(note))\n// Finding ID: {ID2}\nFinding ID: {ID2}\n"),
    );
    let (r, own) = report_of(&md);
    assert!(own.is_empty(), "{own:?}");
    assert!(r.evidence[0].excerpt.as_deref().unwrap().contains(ID2));
}

#[test]
fn every_distinct_labelled_id_is_reported() {
    let md = PLAIN.replace(
        "Owner: Unknown\n",
        &format!("Owner: Unknown\nNative Finding: {ID2}\n"),
    );
    let (_, own) = report_of(&md);
    assert_eq!(own, [ID1, ID2]);
    // The same id on two lines is one id.
    let md = format!("{PLAIN}\nProvenance\n**Finding ID:** {ID1}\n");
    let (_, own) = report_of(&md);
    assert_eq!(own, [ID1]);
}

#[test]
fn text_beyond_the_id_on_an_id_line_is_kept() {
    let md = PLAIN.replace(
        &format!("Finding ID: {ID1}\n"),
        &format!("Finding ID: {ID1} (merged from an earlier duplicate report)\n"),
    );
    let (r, own) = report_of(&md);
    assert_eq!(own, [ID1]);
    assert!(
        other_text(&r).contains(&format!(
            "Header:\n\nIdentifier: NB-001\nFinding ID: {ID1} (merged from an earlier duplicate report)"
        )),
        "{}",
        other_text(&r)
    );

    // In a section it moves to Other imported text under the section's name.
    let md = plain_unlabelled().replace(
        "Impact\nAny signed-in",
        &format!("Impact\nNative Finding: {ID1}, first seen on staging\nAny signed-in"),
    );
    let (r, own) = report_of(&md);
    assert_eq!(own, [ID1]);
    assert!(!r.impact.contains("Native Finding"), "{}", r.impact);
    assert!(
        other_text(&r).contains(&format!(
            "Impact:\n\nNative Finding: {ID1}, first seen on staging"
        )),
        "{}",
        other_text(&r)
    );

    // In Provenance it stays in place with the rest of that section.
    let md = format!("{PLAIN}\nProvenance\nFinding ID: {ID1} (renumbered)\nRun: run_7\n");
    let (r, _) = report_of(&md);
    assert!(
        other_text(&r).contains(&format!(
            "Provenance:\n\nFinding ID: {ID1} (renumbered)\nRun: run_7"
        )),
        "{}",
        other_text(&r)
    );

    // An id line with no id on it is not an id line, and is kept.
    let md = PLAIN.replace(
        &format!("Finding ID: {ID1}\n"),
        "Finding ID: not assigned yet\n",
    );
    let (r, own) = report_of(&md);
    assert!(own.is_empty(), "{own:?}");
    assert!(
        other_text(&r).contains("Finding ID: not assigned yet"),
        "{}",
        other_text(&r)
    );
}

#[test]
fn a_labelled_id_without_the_report_sections_fails() {
    let md = format!("# Share links never expire\n\nFinding ID: {ID2}\n\n## Description\n\nLinks never expire.\n");
    let err = parse_report(&md).unwrap_err();
    assert_eq!(err, ImportError::MissingReportSections);
    assert_eq!(
        err.to_string(),
        "has a Finding ID line but is missing the report sections"
    );
    // Also when the id line is in the one section there is.
    let md = format!("# Notes\n\n## Description\n\n**Native Finding:** {ID2}\n");
    assert_eq!(
        parse_report(&md).unwrap_err(),
        ImportError::MissingReportSections
    );
    // Without an id line the same file is simply not a report.
    let md = "# Share links never expire\n\n## Description\n\nLinks never expire.\n";
    assert_eq!(parse_report(md).unwrap(), Parsed::NotAReport);
}

#[test]
fn a_banner_before_the_title_is_kept_as_other_text() {
    const BANNER: &str = "CONFIDENTIAL — sample";
    // Plain layout: the line after `Filename:`.
    let (r, _) = report_of(&format!("{BANNER}\n{PLAIN}"));
    assert_eq!(r.title, "Notes API returns another user's note by id");
    assert!(
        other_text(&r).starts_with(&format!("Header:\n\n{BANNER}")),
        "{}",
        other_text(&r)
    );
    // Exported layout: the `#` heading, with or without a file name line.
    let (r, _) = report_of(&format!("{BANNER}\n\n{MARKDOWN}"));
    assert_eq!(r.title, "Share links never expire");
    assert!(other_text(&r).contains(BANNER), "{}", other_text(&r));
    let no_filename = MARKDOWN.replace("Filename: NB-002 - Share links never expire.pdf\n\n", "");
    let (r, _) = report_of(&format!("{BANNER}\n\n{no_filename}"));
    assert_eq!(r.title, "Share links never expire");
    assert!(other_text(&r).contains(BANNER), "{}", other_text(&r));
}

#[test]
fn an_artifact_list_item_is_its_path_and_the_rest_is_kept() {
    let md = format!(
        "{PLAIN}\nArtifacts\n- `poc/fetch_other_note.sh` — the proof-of-concept script\n- out/capture.pcap raw capture from staging\n- out/plain.log\n"
    );
    let (r, _) = report_of(&md);
    let paths: Vec<&str> = r.artifacts.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "poc/fetch_other_note.sh",
            "out/capture.pcap",
            "out/plain.log"
        ]
    );
    let other = other_text(&r);
    assert!(
        other.contains(
            "Artifacts:\n\n- `poc/fetch_other_note.sh` — the proof-of-concept script\n- out/capture.pcap raw capture from staging"
        ),
        "{other}"
    );
    assert!(!other.contains("out/plain.log"), "{other}");
}

#[test]
fn a_diff_after_not_provided_is_still_the_patch() {
    let start = PLAIN.find("Recommended Patch\n").unwrap();
    let end = PLAIN.find("CI/CD Detection\n").unwrap();
    let md = format!(
        "{}Recommended Patch\nNot provided upstream; a suggested change:\n\n```diff\n--- a/src/routes/notes.rs\n+++ b/src/routes/notes.rs\n@@\n-a\n+b\n```\n\n{}",
        &PLAIN[..start],
        &PLAIN[end..]
    );
    let (r, _) = report_of(&md);
    let OrSentinel::Value(p) = &r.recommended_patch else {
        panic!("{:?}", r.recommended_patch)
    };
    assert!(
        p.diff.starts_with("--- a/src/routes/notes.rs"),
        "{}",
        p.diff
    );
    assert_eq!(
        p.notes.as_deref(),
        Some("Not provided upstream; a suggested change:")
    );
    // With no diff block, the same opening is the sentinel.
    let md = format!(
        "{}Recommended Patch\nNot provided: the vendor owns this code.\n\n{}",
        &PLAIN[..start],
        &PLAIN[end..]
    );
    let (r, _) = report_of(&md);
    assert_eq!(
        r.recommended_patch,
        OrSentinel::Sentinel("Not Provided — the vendor owns this code.".into())
    );
}

#[test]
fn cwe_ids_come_from_category_and_reference_lines_that_start_with_one() {
    let references = |text: &str| {
        let start = PLAIN.find("References\nCWE-639").unwrap();
        let end = PLAIN.find("\nCVSS v3").unwrap();
        format!("{}References\n{text}\n{}", &PLAIN[..start], &PLAIN[end..])
    };
    // Mentioned in passing: not taken.
    let (r, _) = report_of(&references(
        "Unlike CWE-79 (XSS), this is an access-control flaw.",
    ));
    assert!(r.cwe.is_empty(), "{:?}", r.cwe);
    assert!(r.references.contains("Unlike CWE-79"), "{}", r.references);
    assert_valid(&r, &[]);
    // A line that starts with an id gives every id on it; other lines none.
    let (r, _) = report_of(&references(
        "- CWE-639 Authorization bypass (see also CWE-862)\nSee CWE-200 for the data exposed.\n**CWE-285**: Improper Authorization",
    ));
    assert_eq!(r.cwe, ["CWE-639", "CWE-862", "CWE-285"]);
    // The fixture's References line starts `CWE-639:` and continues
    // `; CWE-862: …`.
    let (r, _) = report_of(PLAIN);
    assert_eq!(r.cwe, ["CWE-639", "CWE-862"]);
    // Category is always read.
    let (r, _) = report_of(&references("OWASP A01:2021 Broken Access Control").replace(
        "Category: Authorization Bypass Through User-Controlled Key",
        "Category: CWE-639 Authorization Bypass Through User-Controlled Key",
    ));
    assert_eq!(r.cwe, ["CWE-639"]);
}
