//! `rupu man` renders the man page the packaging job installs. It is
//! generated from the live clap Command, so it cannot drift from the CLI.

use assert_cmd::Command;

#[test]
fn man_emits_a_roff_document() {
    let out = Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .arg("man")
        .output()
        .expect("run rupu man");

    assert!(out.status.success(), "rupu man must exit 0");
    let roff = String::from_utf8(out.stdout).expect("roff is utf-8");

    // `.TH` is the man-page title header — the first thing troff needs to
    // typeset a page. clap_mangen (via the `roff` crate, a transitive dep
    // whose layout we don't control) emits a short apostrophe-handling
    // preamble ahead of it, so `.TH` isn't necessarily byte 0 — but it
    // must appear near the top. `.take(10)` still catches a malformed page
    // where `.TH` only turns up deep in the body.
    assert!(
        roff.lines().take(10).any(|l| l.starts_with(".TH")),
        "expected a .TH header near the top, got: {:?}",
        &roff[..roff.len().min(160)]
    );
    assert!(roff.contains("rupu"), "must document the rupu command");
    assert!(!roff.is_empty());
}

#[test]
fn man_documents_a_known_subcommand() {
    // Guards against rendering an empty shell of a page: `update` is a real
    // subcommand and must appear.
    let out = Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .arg("man")
        .output()
        .expect("run rupu man");
    let roff = String::from_utf8(out.stdout).unwrap();
    assert!(
        roff.contains("update"),
        "man page should mention the update subcommand"
    );
}
