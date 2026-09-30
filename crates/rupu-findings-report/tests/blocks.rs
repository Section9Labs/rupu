mod common;

use common::*;
use rupu_coverage::report::{
    ChainHop, CiDetection, HopRole, OrSentinel, Patch, RegressionTest, Ticket,
};
use rupu_coverage::Severity;
use rupu_findings_report::blocks::{finding_blocks, Block};
use rupu_findings_report::model::ExportFinding;
use std::collections::HashMap;

fn with_report(edit: impl FnOnce(&mut rupu_coverage::FindingReport)) -> Vec<Block> {
    let mut report = full_report();
    edit(&mut report);
    let f: ExportFinding = numbered(vec![input(
        "notebin",
        None,
        full_record("fnd_x", Severity::High, report),
    )])
    .remove(0);
    finding_blocks(&f, &HashMap::new())
}

fn after_heading<'a>(blocks: &'a [Block], name: &str) -> &'a [Block] {
    let i = blocks
        .iter()
        .position(|b| matches!(b, Block::Heading(h) if h == name))
        .unwrap_or_else(|| panic!("no heading {name}"));
    let end = blocks[i + 1..]
        .iter()
        .position(|b| matches!(b, Block::Heading(_)))
        .map_or(blocks.len(), |n| i + 1 + n);
    &blocks[i + 1..end]
}

fn fields<'a>(blocks: &'a [Block], key: &str) -> Option<&'a str> {
    blocks.iter().find_map(|b| match b {
        Block::Fields(rows) => rows.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()),
        _ => None,
    })
}

#[test]
fn the_first_blocks_are_filename_title_then_the_identity_fields_in_order() {
    let f = full_finding();
    let blocks = finding_blocks(&f, &HashMap::new());
    assert_eq!(
        blocks[0],
        Block::Filename("SEC-001 - Notes API returns another user's note by id.pdf".into())
    );
    assert_eq!(
        blocks[1],
        Block::Title("Notes API returns another user's note by id".into())
    );
    let Block::Fields(rows) = &blocks[2] else {
        panic!("expected Fields, got {:?}", blocks[2]);
    };
    let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "Identifier",
            "Owner",
            "Product",
            "Affected Component",
            "Source Repository",
            "Existing Ticket References",
            "Impact",
            "Category",
            "Attack Vector",
            "Likelihood",
            "Risk Rating",
        ]
    );
    assert_eq!(rows[0].1, "SEC-001");
}

#[test]
fn tickets_render_verbatim_sentinels_or_type_identifier_and_url() {
    let sentinel = with_report(|r| r.tickets = OrSentinel::Sentinel("Unknown".into()));
    assert_eq!(
        fields(&sentinel, "Existing Ticket References"),
        Some("Unknown")
    );

    let listed = with_report(|r| {
        r.tickets = OrSentinel::Value(vec![
            Ticket {
                kind: "Jira".into(),
                identifier: "SEC-12".into(),
                url: Some("https://jira.example/SEC-12".into()),
                notes: None,
            },
            Ticket {
                kind: "GitHub".into(),
                identifier: "#40".into(),
                url: None,
                notes: Some("ignored".into()),
            },
        ])
    });
    assert_eq!(
        fields(&listed, "Existing Ticket References"),
        Some("Jira SEC-12 (https://jira.example/SEC-12); GitHub #40")
    );
}

#[test]
fn call_chain_steps_omit_absent_parts() {
    let blocks = with_report(|_| {});
    let Block::Steps(steps) = &after_heading(&blocks, "Call Chain / Attack Flow")[0] else {
        panic!("expected Steps");
    };
    assert_eq!(steps.len(), 3);
    assert_eq!(
        steps[0],
        "**router: GET /api/notes/{id}** — `src/app.rs:12-30` — gate: session cookie (passes because any signed-in user has one)"
    );
    assert_eq!(steps[1], "**get_note()** — `src/routes/notes.rs:40-58`");

    let blocks = with_report(|r| {
        r.call_chain = OrSentinel::Value(vec![
            ChainHop {
                label: "entry".into(),
                file: None,
                lines: None,
                binary_va: Some("0x401000".into()),
                gate: Some("auth".into()),
                passes_because: None,
                role: HopRole::Source,
            },
            ChainHop {
                label: "sink".into(),
                file: Some("a.rs".into()),
                lines: None,
                binary_va: None,
                gate: None,
                passes_because: Some("no check".into()),
                role: HopRole::Sink,
            },
        ])
    });
    let Block::Steps(steps) = &after_heading(&blocks, "Call Chain / Attack Flow")[0] else {
        panic!("expected Steps");
    };
    assert_eq!(steps[0], "**entry** — `0x401000` — gate: auth");
    assert_eq!(steps[1], "**sink** — `a.rs` — passes because no check");
}

#[test]
fn a_backtick_in_a_path_cannot_close_its_code_span() {
    let blocks = with_report(|r| {
        r.call_chain = OrSentinel::Value(vec![ChainHop {
            label: "x".into(),
            file: Some("we`ird.rs".into()),
            lines: Some([1, 2]),
            binary_va: None,
            gate: None,
            passes_because: None,
            role: HopRole::Hop,
        }])
    });
    let Block::Steps(steps) = &after_heading(&blocks, "Call Chain / Attack Flow")[0] else {
        panic!("expected Steps");
    };
    assert_eq!(steps[0], "**x** — ``we`ird.rs:1-2``");
}

#[test]
fn sentinel_sections_become_notes_and_not_provided_is_reworded() {
    let blocks = with_report(|r| {
        r.call_chain = OrSentinel::Sentinel("Not Provided — black-box test".into());
        r.recommended_patch = OrSentinel::Sentinel("Not Provided — vendor owns the fix".into());
        r.ci_cd_detection = OrSentinel::Sentinel("Unknown".into());
        r.regression_test = OrSentinel::Sentinel("Not Provided — none possible".into());
        r.cross_references = OrSentinel::Sentinel("None".into());
    });
    let only_note = |name: &str, text: &str| {
        assert_eq!(
            after_heading(&blocks, name),
            [Block::Note(text.to_string())],
            "{name}"
        );
    };
    only_note("Call Chain / Attack Flow", "Not provided: black-box test");
    only_note("Recommended Patch", "Not provided: vendor owns the fix");
    only_note("CI/CD Detection", "Unknown");
    only_note("Regression Test", "Not provided: none possible");
    only_note("Cross-References", "None");
}

#[test]
fn patch_ci_and_regression_blocks_have_the_specified_shape() {
    let blocks = with_report(|r| {
        r.recommended_patch = OrSentinel::Value(Patch {
            diff: "-a\n+b\n".into(),
            notes: Some("Backport to 1.x.".into()),
        });
        r.ci_cd_detection = OrSentinel::Value(CiDetection {
            stage: "nightly".into(),
            body: "Runs the probe.".into(),
            command: None,
            expect: "exit code 1".into(),
        });
        r.regression_test = OrSentinel::Value(RegressionTest {
            body: "Two users.".into(),
            command: "cargo test x".into(),
            expect_vulnerable: "fails".into(),
            expect_patched: "passes".into(),
        });
    });
    assert_eq!(
        after_heading(&blocks, "Recommended Patch"),
        [
            Block::Code {
                lang: Some("diff".into()),
                text: "-a\n+b\n".into()
            },
            Block::Prose("Backport to 1.x.".into()),
        ]
    );
    // No command: no code block.
    assert_eq!(
        after_heading(&blocks, "CI/CD Detection"),
        [
            Block::Prose("**Stage:** nightly".into()),
            Block::Prose("Runs the probe.".into()),
            Block::Prose("**Fails when:** exit code 1".into()),
        ]
    );
    assert_eq!(
        after_heading(&blocks, "Regression Test"),
        [
            Block::Prose("Two users.".into()),
            Block::Code {
                lang: Some("sh".into()),
                text: "cargo test x".into()
            },
            Block::Fields(vec![
                ("Vulnerable build".into(), "fails".into()),
                ("Patched build".into(), "passes".into()),
            ]),
        ]
    );
}

#[test]
fn evidence_pairs_a_located_claim_with_its_excerpt_in_the_claims_language() {
    let blocks = with_report(|_| {});
    assert_eq!(
        after_heading(&blocks, "Evidence"),
        [
            Block::Prose(
                "**`src/routes/notes.rs:40-58`** — The handler looks the note up by id alone."
                    .into()
            ),
            Block::Code {
                lang: Some("rust".into()),
                text: "let note = store.find_by_id(id).await?;\nOk(Json(note))".into()
            },
        ]
    );
}

#[test]
fn a_hostile_language_tag_is_reduced_to_safe_characters() {
    let blocks = with_report(|r| {
        r.evidence[0].lang = Some("rust\n```\n# owned".into());
    });
    let ev = after_heading(&blocks, "Evidence");
    let Block::Code { lang, .. } = &ev[1] else {
        panic!("expected Code");
    };
    assert_eq!(lang.as_deref(), Some("rust#owned"));

    let blocks = with_report(|r| r.evidence[0].lang = Some("`\n".into()));
    let Block::Code { lang, .. } = &after_heading(&blocks, "Evidence")[1] else {
        panic!("expected Code");
    };
    assert_eq!(*lang, None);
}

#[test]
fn a_finding_with_no_evidence_says_so_instead_of_an_empty_section() {
    let blocks = with_report(|r| r.evidence.clear());
    assert_eq!(
        after_heading(&blocks, "Evidence"),
        [Block::Note("No evidence recorded.".into())]
    );
}
