//! Smoke test: spawn the actual rupu binary against a TempDir and
//! parse its stdout to confirm CREATED lines for every template plus
//! the final tally line.

use std::process::Command;

#[test]
fn rupu_init_with_samples_smoke() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(["init", "--with-samples", tmp.path().to_str().unwrap()])
        .output()
        .expect("spawn rupu");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for needle in [
        ".rupu/agents/review-diff.md",
        ".rupu/agents/scm-pr-review.md",
        ".rupu/workflows/investigate-then-fix.yaml",
    ] {
        assert!(stdout.contains(needle), "stdout missing {needle}:\n{stdout}");
    }
    assert!(
        stdout.contains("init: created"),
        "stdout missing tally line: {stdout}"
    );

    // Spot-check a couple of files actually exist.
    assert!(tmp.path().join(".rupu/agents/review-diff.md").exists());
    assert!(tmp
        .path()
        .join(".rupu/workflows/investigate-then-fix.yaml")
        .exists());
    assert!(tmp.path().join(".rupu/config.toml").exists());
    assert!(tmp.path().join(".gitignore").exists());
}
