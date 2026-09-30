use assert_cmd::Command;

#[test]
fn prints_the_embedded_schema() {
    let out = Command::cargo_bin("rupu")
        .unwrap()
        .args(["findings", "schema"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["title"], "rupu finding report");
    assert!(v["properties"]["regression_test"]["oneOf"].is_array());
}

#[test]
fn advertised_flag_prints_the_provider_safe_copy() {
    let out = Command::cargo_bin("rupu")
        .unwrap()
        .args(["findings", "schema", "--advertised"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("\"oneOf\""));
    assert!(text.contains("\"anyOf\""));
}
