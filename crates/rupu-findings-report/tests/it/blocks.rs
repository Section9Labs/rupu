use crate::common::*;
use rupu_coverage::report::{
    ArtifactKind, ArtifactRef, ArtifactStorage, ChainHop, CiDetection, HopRole, OrSentinel, Patch,
    RegressionTest, Ticket, Verification, VerificationStatus,
};
use rupu_coverage::Severity;
use rupu_findings_report::blocks::{finding_blocks, Block};
use rupu_findings_report::model::ExportFinding;
use rupu_findings_report::Blobs;
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
    finding_blocks(&f, &HashMap::new(), Blobs::NONE)
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
    let blocks = finding_blocks(&f, &HashMap::new(), Blobs::NONE);
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
            "CWE",
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
                kind: "Tracker".into(),
                identifier: "SEC-12".into(),
                url: Some("https://tracker.example/SEC-12".into()),
                notes: None,
            },
            Ticket {
                kind: "GitHub".into(),
                identifier: "#40".into(),
                url: None,
                notes: Some("filed by\nthe scanner".into()),
            },
        ])
    });
    assert_eq!(
        fields(&listed, "Existing Ticket References"),
        Some("Tracker SEC-12 (https://tracker.example/SEC-12); GitHub #40 — filed by the scanner")
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
        "**router: GET /api/notes/{id}** — `src/app.rs:12-30` — gate: session cookie (passes because any signed-in user has one) — role: source"
    );
    assert_eq!(
        steps[1],
        "**get_note()** — `src/routes/notes.rs:40-58` — role: hop"
    );
    assert!(steps[2].ends_with(" — role: sink"), "{}", steps[2]);

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
    assert_eq!(
        steps[0],
        "**entry** — `0x401000` — gate: auth — role: source"
    );
    assert_eq!(
        steps[1],
        "**sink** — `a.rs` — passes because no check — role: sink"
    );
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
    assert_eq!(steps[0], "**x** — ``we`ird.rs:1-2`` — role: hop");
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
            Block::Prose("**Command:**".into()),
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

fn note_of(blocks: &[Block]) -> &str {
    blocks
        .iter()
        .find_map(|b| match b {
            Block::Note(n) => Some(n.as_str()),
            _ => None,
        })
        .expect("a Note block")
}

#[test]
fn cwe_ids_are_a_comma_joined_row_after_category_and_omitted_when_empty() {
    let blocks = with_report(|_| {});
    assert_eq!(fields(&blocks, "CWE"), Some("CWE-639, CWE-862"));
    let blocks = with_report(|r| r.cwe.clear());
    assert_eq!(fields(&blocks, "CWE"), None);
    let Block::Fields(rows) = &blocks[2] else {
        panic!("expected Fields");
    };
    assert!(rows.iter().any(|(k, _)| k == "Category"));
}

#[test]
fn ticket_parts_are_omitted_when_absent() {
    let blocks = with_report(|r| {
        r.tickets = OrSentinel::Value(vec![Ticket {
            kind: "Tracker".into(),
            identifier: "SEC-1".into(),
            url: None,
            notes: Some("reopened".into()),
        }])
    });
    assert_eq!(
        fields(&blocks, "Existing Ticket References"),
        Some("Tracker SEC-1 — reopened")
    );
}

#[test]
fn provenance_carries_surface_and_for_full_findings_the_record_and_verification() {
    let f = numbered(vec![input("notebin", Some("audit"), {
        let mut rec = full_record("fnd_p", Severity::High, {
            let mut r = full_report();
            r.verification = Some(Verification {
                status: VerificationStatus::Confirmed,
                by_run: Some("run_77".into()),
                by_agent: None,
                notes: Some("reproduced\non staging".into()),
            });
            r
        });
        rec.concern_id = Some("authz-idor".into());
        rec.file_path = Some("src/routes/notes.rs".into());
        rec.line_range = Some([40, 58]);
        rec
    })])
    .remove(0);
    let blocks = finding_blocks(&f, &HashMap::new(), Blobs::NONE);
    let Block::Fields(rows) = after_heading(&blocks, "Provenance")[0].clone() else {
        panic!("expected Fields");
    };
    let rows: Vec<(&str, &str)> = rows.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        rows,
        [
            ("Finding ID", "fnd_p"),
            ("Project", "notebin"),
            ("Workflow", "audit"),
            ("Run", "run_01"),
            ("Surface", "workflow"),
            ("Model", "claude-x"),
            ("Declared", "2026-03-01T10:00:00Z"),
            ("Concern", "authz-idor"),
            ("Scope", "repo"),
            ("Location", "src/routes/notes.rs:40-58"),
            (
                "Verification",
                "Confirmed by run_77 — reproduced on staging"
            ),
        ]
    );

    // Without a concern, locator or verification those rows are absent.
    let blocks = with_report(|_| {});
    let prov = after_heading(&blocks, "Provenance");
    assert_eq!(fields(prov, "Concern"), None);
    assert_eq!(fields(prov, "Location"), None);
    assert_eq!(fields(prov, "Verification"), None);
    assert_eq!(fields(prov, "Scope"), Some("repo"));
    assert_eq!(fields(prov, "Surface"), Some("workflow"));
}

#[test]
fn a_verification_with_only_a_status_is_just_the_status() {
    let blocks = with_report(|r| {
        r.verification = Some(Verification {
            status: VerificationStatus::Disputed,
            by_run: None,
            by_agent: None,
            notes: None,
        })
    });
    let prov = after_heading(&blocks, "Provenance");
    assert_eq!(fields(prov, "Verification"), Some("Disputed"));
}

#[test]
fn evidence_names_the_artifact_and_the_hash_that_back_a_claim() {
    let blocks = with_report(|r| {
        r.evidence[0].artifact = Some("poc/req`1.txt".into());
        r.evidence[0].sha256 = Some("0123456789abcdef0123".into());
    });
    assert_eq!(
        after_heading(&blocks, "Evidence")[0],
        Block::Prose(
            "**`src/routes/notes.rs:40-58`** — The handler looks the note up by id alone. \
             — proven by ``poc/req`1.txt`` (sha256 0123456789ab)"
                .into()
        )
    );
    // Either one alone, and no location.
    let blocks = with_report(|r| {
        r.evidence[0].file = None;
        r.evidence[0].lines = None;
        r.evidence[0].sha256 = Some("abc".into());
    });
    assert_eq!(
        after_heading(&blocks, "Evidence")[0],
        Block::Prose("The handler looks the note up by id alone. (sha256 abc)".into())
    );
}

#[test]
fn the_artifacts_table_has_kind_and_host_columns_with_dashes_for_absent_values() {
    let blocks = with_report(|r| {
        r.artifacts = vec![
            ArtifactRef {
                path: "poc/a.py".into(),
                sha256: "0123456789abcdef".into(),
                size: 10,
                kind: Some(ArtifactKind::Text),
                stored: Some(ArtifactStorage::Copied),
                host: None,
            },
            ArtifactRef {
                path: "poc/big.bin".into(),
                sha256: String::new(),
                size: 9_000_000,
                kind: Some(ArtifactKind::Binary),
                stored: Some(ArtifactStorage::External),
                host: Some("kuki".into()),
            },
            ArtifactRef {
                path: "poc/raw".into(),
                sha256: String::new(),
                size: 0,
                kind: None,
                stored: None,
                host: None,
            },
        ]
    });
    let Block::Table { headers, rows } = &after_heading(&blocks, "Artifacts")[0] else {
        panic!("expected Table");
    };
    assert_eq!(
        headers,
        &[
            "Path",
            "SHA-256 (first 12 chars)",
            "Size",
            "Kind",
            "Stored",
            "Host"
        ]
    );
    assert_eq!(
        rows[0],
        ["poc/a.py", "0123456789ab", "10 B", "Text", "Copied", "—"]
    );
    assert_eq!(
        rows[1],
        [
            "poc/big.bin",
            "—",
            "9000000 B",
            "Binary",
            "External",
            "kuki"
        ]
    );
    assert_eq!(rows[2], ["poc/raw", "—", "0 B", "—", "—", "—"]);
}

#[test]
fn a_full_record_whose_report_did_not_load_says_so_and_a_summary_says_it_is_one() {
    let mut rec = summary_record("fnd_broken", Severity::High);
    rec.profile = rupu_coverage::FindingProfile::Full;
    let f = numbered(vec![input("notebin", None, rec)]).remove(0);
    let blocks = finding_blocks(&f, &HashMap::new(), Blobs::NONE);
    assert_eq!(
        note_of(&blocks),
        "Full report could not be loaded by this build — summary record shown."
    );
    assert!(blocks
        .iter()
        .any(|b| matches!(b, Block::Heading(h) if h == "Rationale")));

    let f = numbered(vec![input(
        "notebin",
        None,
        summary_record("fnd_sum", Severity::High),
    )])
    .remove(0);
    assert_eq!(
        note_of(&finding_blocks(&f, &HashMap::new(), Blobs::NONE)),
        "Summary finding — no full report was recorded."
    );
}

#[test]
fn a_hop_cannot_start_a_block_from_its_label_gate_or_reason() {
    let blocks = with_report(|r| {
        r.call_chain = OrSentinel::Value(vec![ChainHop {
            label: "entry\n## Root Cause\n\n---".into(),
            file: None,
            lines: None,
            binary_va: None,
            gate: Some("auth\n# owned".into()),
            passes_because: Some("any user\r\n- item".into()),
            role: HopRole::Hop,
        }])
    });
    let Block::Steps(steps) = &after_heading(&blocks, "Call Chain / Attack Flow")[0] else {
        panic!("expected Steps");
    };
    assert_eq!(
        steps[0],
        "**entry ## Root Cause ---** — gate: auth # owned (passes because any user - item) — role: hop"
    );
    assert!(!steps[0].contains('\n'));
}

#[test]
fn location_is_markdown_prose_not_plain_text_fields() {
    let blocks = with_report(|r| {
        r.location.input = "  `GET /api/notes/{id}` path parameter `id`.\n".into();
        r.location.output = "The JSON body, including `title`.".into();
    });
    assert_eq!(
        after_heading(&blocks, "Location"),
        [Block::Prose(
            "**Input:** `GET /api/notes/{id}` path parameter `id`.\n\n\
             **Output:** The JSON body, including `title`."
                .into()
        )]
    );
    // No plain-text Fields block carries the Input/Output labels any more.
    assert_eq!(fields(&blocks, "Input"), None);
    assert_eq!(fields(&blocks, "Output"), None);
}
