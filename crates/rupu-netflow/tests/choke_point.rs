//! rupu has exactly ONE door for outbound HTTP. Two layers enforce it,
//! and they cover different things — neither is sufficient alone.
//!
//! * `clippy.toml`'s `disallowed-methods` is the PRIMARY guard. It
//!   resolves calls semantically, so it catches a bare `.build()` — the
//!   idiomatic way the real bypass would actually be written.
//! * This test is a SECONDARY, text-level backstop. It only matches
//!   fully-qualified paths (`reqwest::ClientBuilder::build(...)`), so it
//!   is structurally blind to bare method calls (`builder.build()`
//!   without the type spelled out). That is permanent, not a gap to be
//!   closed later — it exists so a regression is still caught even when
//!   someone runs a bare `cargo test` without clippy.
//!
//! Consequence: `PENDING_PLAN_2` below is a strict SUBSET of the real
//! migration debt — see its doc comment. The authoritative checklist for
//! "is anything still unmigrated" is `cargo clippy --workspace 2>&1 |
//! grep disallowed`, not this constant.
//!
//! `Client::builder()` is deliberately NOT banned by either layer:
//! `client_from` requires every legitimate caller to build a tuned
//! `ClientBuilder` first, so banning that call would produce a false
//! positive at every correct call site. The actual bypass is calling
//! `.build()` on that builder instead of handing it to `client_from` —
//! that is what `BANNED` (below) and `clippy.toml` both target.

use std::path::Path;

/// Method paths that bypass netflow capture. Mirrors `clippy.toml`'s
/// `disallowed-methods`.
const BANNED: &[&str] = &[
    "reqwest::Client::new",
    "reqwest::Client::default",
    "reqwest::ClientBuilder::new",
    "reqwest::ClientBuilder::default",
    "reqwest::ClientBuilder::build",
    // Unlike `.build()`, this one IS written fully-qualified at every
    // call site (it's a free function, not a method on a local
    // variable), so — unusually for this text scanner — it can actually
    // catch a `reqwest::get(...)` bypass, not just backstop clippy.
    // Found live in crates/rupu-cli/src/output/theme.rs (2026-08-04) and
    // migrated onto rupu_netflow::http::client in the same fix.
    "reqwest::get",
];

/// Files permitted to construct or finalize a raw reqwest client.
const ALLOWED: &[&str] = &[
    // The factory itself: `client()`'s panicking fallback and
    // `client_with()`'s `ClientBuilder::build()` are the one legitimate
    // `.build()` call site in the repo.
    "crates/rupu-netflow/src/http/mod.rs",
    // Its `#[cfg(test)]` module builds a throwaway client (`Client::new`)
    // to exercise `refresh`. Inline test modules live in `src/`, so the
    // `/tests/` skip below does not cover them.
    "crates/rupu-netflow/src/asn/acquire.rs",
];

/// NOT yet migrated. Plan 2 empties this list; it must never grow.
/// Every entry here is a client whose egress is currently invisible to
/// THIS scanner.
///
/// Built from the actual failure output of
/// `no_raw_reqwest_client_outside_rupu_netflow` on 2026-08-03 (fix round
/// 1, after inverting the banned-method set — see module docs above),
/// not copied from the task brief — the repo moved since the brief was
/// written.
///
/// COVERAGE NOTE: this is a plain-text scan, so it only sees banned
/// calls written as fully-qualified paths (`reqwest::Client::new(...)`).
/// It cannot see `ClientBuilder::build()` invoked as a bare method call
/// on a previously-built builder (`.build()`), which is how idiomatic
/// Rust normally writes it — that pattern has no literal
/// `reqwest::ClientBuilder::build` substring anywhere on the line.
///
/// As of Plan 2 Task 5 (2026-08-04, fix round 1), `rupu-scm` no longer has
/// ANY file in that blind spot: all nine hand-rolled `reqwest` call sites
/// (`client_options.rs`, `connectors/gitlab/{client,events}.rs`,
/// `connectors/linear/{events,issues}.rs`, `connectors/jira/{events,
/// issues}.rs`, `connectors/github/{client,events}.rs`) are migrated onto
/// `rupu_netflow::http::client_from`. None of them ever appeared in this
/// list — this scanner never saw them (the blind spot above) — but they
/// no longer trip the semantic `clippy::disallowed_methods` lint either
/// (`cargo clippy -p rupu-scm --all-targets`, the authoritative check,
/// which resolves the receiver type regardless of call syntax). With the
/// crate clean, `rupu-scm`'s former `#![warn(clippy::disallowed_methods)]`
/// override in `lib.rs` has been REMOVED — `#![deny(clippy::all)]` alone
/// now enforces it, so a regression is a hard build error, not a warning.
///
/// Plan 2 Task 7 (2026-08-04) migrated the last two entries —
/// `crates/rupu-cli/src/cmd/cp.rs`'s test-only client and
/// `crates/rupu-cp/src/host/http.rs`'s fleet-host `HttpHostConnector` (both
/// constructors, `new` and `new_with_timeout`, now route through
/// `rupu_netflow::http::client_from` with their existing timeout tuning
/// preserved) — onto `rupu_netflow::http::client` / `client_from`. This
/// list is now empty; the authoritative check remains
/// `cargo clippy --workspace --all-targets 2>&1 | grep disallowed`, not
/// this constant.
const PENDING_PLAN_2: &[&str] = &[];

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/<name> has a grandparent")
        .to_path_buf()
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == "node_modules" || name == ".git" {
                continue;
            }
            walk(&path, out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// Strip a trailing line comment WITHOUT being fooled by `//` inside a
/// string literal (e.g. `"http://x"`).
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i + 1 < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'/' if !in_string && bytes[i + 1] == b'/' => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

fn scan(skip: impl Fn(&str) -> bool) -> Vec<String> {
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);

    let mut offenders = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");

        // Tests may build throwaway clients; they are not rupu's egress.
        if rel.contains("/tests/") || skip(&rel) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let code = strip_comment(line);
            for method in BANNED {
                if code.contains(method) {
                    offenders.push(format!("{rel}:{} ({method})", i + 1));
                }
            }
        }
    }
    offenders
}

#[test]
fn no_raw_reqwest_client_outside_rupu_netflow() {
    let offenders = scan(|rel| ALLOWED.contains(&rel) || PENDING_PLAN_2.contains(&rel));

    assert!(
        offenders.is_empty(),
        "raw reqwest clients bypass netflow capture — route them through \
         rupu_netflow::http::client / client_from instead:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn pending_migration_list_matches_reality() {
    let root = repo_root();
    for rel in PENDING_PLAN_2 {
        assert!(
            root.join(rel).exists(),
            "{rel} is listed as pending migration but does not exist — \
             remove it from PENDING_PLAN_2"
        );
    }
}

#[test]
fn strip_comment_is_not_fooled_by_a_url_literal_before_a_banned_call() {
    // A `"http://…"` literal earlier on the line contains `//`; a naive
    // `line.split("//").next()` would truncate there and silently miss
    // the banned call that follows.
    let line = r#"let base = "http://x"; let client = reqwest::Client::new();  // never seen"#;
    let stripped = strip_comment(line);
    assert!(
        stripped.contains("reqwest::Client::new"),
        "stripper truncated at the URL literal's `//` instead of the real \
         comment: {stripped:?}"
    );
    assert!(
        !stripped.contains("never seen"),
        "stripper failed to remove the trailing comment: {stripped:?}"
    );
}
