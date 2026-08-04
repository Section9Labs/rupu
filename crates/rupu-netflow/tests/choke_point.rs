//! rupu has exactly ONE door for outbound HTTP. This test fails if a new
//! one appears, so a regression is a red build rather than a review miss.
//!
//! Paired with the `clippy.toml` `disallowed-methods` lint: the lint
//! catches it at compile time in CI, this catches it even when someone
//! runs a bare `cargo test`.

use std::path::Path;

/// Files permitted to construct a raw reqwest client.
const ALLOWED: &[&str] = &[
    // The factory itself.
    "crates/rupu-netflow/src/http/mod.rs",
    // Its `#[cfg(test)]` module builds a throwaway client to exercise
    // `refresh`. Inline test modules live in `src/`, so the `/tests/`
    // skip below does not cover them.
    "crates/rupu-netflow/src/asn/acquire.rs",
    // `client_from` takes a caller-tuned `reqwest::ClientBuilder` by
    // design (timeouts, `http1_only`, proxies). These two sites build
    // one and hand it straight to `client_from` — they never call
    // `.build()` on it directly, so capture is never bypassed. Both
    // carry a matching `#[allow(clippy::disallowed_methods)]`.
    "crates/rupu-providers/src/tuning.rs",
    "crates/rupu-providers/src/anthropic.rs",
];

/// NOT yet migrated. Plan 2 empties this list; it must never grow.
/// Every entry here is a client whose egress is currently invisible.
///
/// Built from the actual failure output of
/// `no_raw_reqwest_client_outside_rupu_netflow` on 2026-08-03 (see
/// `pending_migration_list_matches_reality` below), not copied from the
/// task brief — the repo moved since the brief was written.
const PENDING_PLAN_2: &[&str] = &[
    "crates/rupu-auth/src/oauth/device.rs",
    "crates/rupu-auth/src/oauth/callback.rs",
    "crates/rupu-auth/src/resolver.rs",
    "crates/rupu-cli/src/cmd/cp.rs",
    "crates/rupu-cp/src/host/http.rs",
    "crates/rupu-update/src/github.rs",
    "crates/rupu-scm/src/client_options.rs",
    "crates/rupu-scm/src/connectors/github/client.rs",
    "crates/rupu-scm/src/connectors/github/events.rs",
    "crates/rupu-scm/src/connectors/gitlab/events.rs",
    "crates/rupu-scm/src/connectors/jira/events.rs",
    "crates/rupu-scm/src/connectors/jira/issues.rs",
    "crates/rupu-scm/src/connectors/linear/events.rs",
    "crates/rupu-scm/src/connectors/linear/issues.rs",
];

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

#[test]
fn no_raw_reqwest_client_outside_rupu_netflow() {
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
        if rel.contains("/tests/")
            || ALLOWED.contains(&rel.as_str())
            || PENDING_PLAN_2.contains(&rel.as_str())
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            if code.contains("reqwest::Client::new") || code.contains("reqwest::Client::builder") {
                offenders.push(format!("{rel}:{}", i + 1));
            }
        }
    }

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
