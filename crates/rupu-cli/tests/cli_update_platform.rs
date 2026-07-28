//! `rupu update --print-platform` is the release pipeline's only source
//! for an asset's platform name. If this contract breaks, published
//! assets stop matching what `rupu update` looks for — the exact defect
//! the flag exists to prevent — so it is tested end-to-end through the
//! real binary rather than as a unit test.

use assert_cmd::Command;

#[test]
fn print_platform_matches_the_update_crate_mapping() {
    let expected = format!("{}\n", rupu_update::current_platform());

    Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .args(["update", "--print-platform"])
        .assert()
        .success()
        .stdout(expected);
}

#[test]
fn print_platform_needs_no_config_or_credentials() {
    // The flag must short-circuit before config loading and before any
    // GitHub API call, so the release pipeline can invoke it inside a
    // sandboxed build container with no credentials and no network.
    Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .args(["update", "--print-platform"])
        .env("RUPU_HOME", "/nonexistent-on-purpose")
        .assert()
        .success();
}

/// The output is consumed by shell as `rupu-$(rupu update --print-platform)`,
/// so it must be exactly one bare line — no ANSI, no banner, no trailing
/// whitespace that would end up embedded in an asset filename.
#[test]
fn print_platform_emits_one_clean_line_safe_for_shell_interpolation() {
    let out = Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .args(["update", "--print-platform"])
        .output()
        .expect("run rupu");

    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    assert_eq!(stdout.lines().count(), 1, "expected exactly one line");

    let line = stdout.lines().next().unwrap();
    assert_eq!(line, line.trim(), "no leading/trailing whitespace");
    assert!(!line.contains('\u{1b}'), "no ANSI escapes: {line:?}");
    assert!(
        line.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "asset names must be filename-safe, got {line:?}"
    );
    assert!(line.contains('-'), "expected <os>-<arch>, got {line:?}");
}
