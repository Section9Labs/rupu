use assert_cmd::Command;
use rupu_cp::node::protocol::{FeaturesReport, CAP_WORKFLOW_RESUME_IF_UNFINISHED};

/// `rupu __features` is how an SSH coordinator learns what this binary
/// honours before it builds a command for it: stdout is the report the
/// coordinator parses, nothing else.
#[test]
fn prints_this_builds_features_report() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rupu")
        .unwrap()
        .arg("__features")
        .env("RUPU_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: FeaturesReport = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report, FeaturesReport::current());
    assert!(report.supports(CAP_WORKFLOW_RESUME_IF_UNFINISHED));
}
