//! Guards spec §3.2's feature split: `rupu-netflow`'s `default` feature set
//! (types + ledger + ASN only) must never drag `reqwest` — or any of the
//! HTTP stack behind the `http` feature — into a crate that only wants the
//! schema. `rupu-transcript` depends on this crate with
//! `default-features = false` specifically so `Event::NetFlow { flow:
//! FlowRecord }` doesn't inherit an HTTP client.
//!
//! Runs the real `cargo tree`, not a hand-rolled `Cargo.toml` parse — the
//! failure mode this guards against is a transitive dependency creeping in
//! through some OTHER crate's feature unification, which only `cargo`'s
//! own resolver can see.

use std::process::Command;

#[test]
fn no_default_features_build_has_no_reqwest_in_its_dependency_tree() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "rupu-netflow",
            "--no-default-features",
            "--prefix",
            "none",
        ])
        .output()
        .expect("failed to run `cargo tree` — is `cargo` on PATH?");

    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = String::from_utf8_lossy(&output.stdout);
    assert!(
        !tree.contains("reqwest"),
        "rupu-netflow's default (non-\"http\") feature set must not pull in \
         reqwest — this is the guard for spec §3.2's feature split. Full \
         tree:\n{tree}"
    );
}
