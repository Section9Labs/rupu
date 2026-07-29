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
    // typeset a page. clap_mangen (via the `roff` crate) always emits a
    // 2-line apostrophe-handling preamble ahead of it, so `.TH` is the
    // first *content* line rather than byte 0 of the document.
    let first_content_line = roff.lines().nth(2).unwrap_or("");
    assert!(
        first_content_line.starts_with(".TH"),
        "expected a .TH header on line 3, got: {:?}",
        &roff[..roff.len().min(120)]
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
