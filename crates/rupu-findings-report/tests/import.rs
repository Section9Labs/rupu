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
        Parsed::Report { report, cited_ids } => (report, cited_ids),
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
    let (r, cited) = report_of(PLAIN);
    assert_eq!(cited, vec![ID1.to_string()]);
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
    let (r, cited) = report_of(MARKDOWN);
    // The cross-reference's id is not the report's own.
    assert_eq!(cited, vec![ID2.to_string()]);
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
    let (r, cited) = report_of(&md);
    assert_eq!(cited, vec![ID1.to_string()]);
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
    assert_eq!(r.references, original.references);
    assert_valid(&r, &[]);
}

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

    let (r, cited) = report_of(&exported(original.clone()));
    assert_eq!(cited, vec![ID1.to_string()]);
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
            "{}\n\nCross-references (imported):\n\n- {ID2} (sibling) — Share pages read notes through the same store.",
            original.references
        )
    );
    // rupu fills in an artifact's hash, size and storage when it is written.
    let paths: Vec<&str> = r.artifacts.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, ["out/access.log"]);
    // Verification is a claim about a run in the exporting installation.
    assert!(r.verification.is_none());
    assert_valid(&r, &[ID2]);
}
