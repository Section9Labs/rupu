use rupu_mcp::ToolFilter;
use std::collections::BTreeSet;

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}
fn set(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn empty_keeps_everything() {
    let all = names(&["runs.get", "scm.prs.get"]);
    assert_eq!(
        ToolFilter::parse(&[]).apply(&all).unwrap(),
        set(&["runs.get", "scm.prs.get"])
    );
}

#[test]
fn namespace_glob_keeps_only_that_namespace() {
    let all = names(&["runs.get", "runs.list", "scm.prs.get"]);
    let f = ToolFilter::parse(&names(&["runs.*"]));
    assert_eq!(f.apply(&all).unwrap(), set(&["runs.get", "runs.list"]));
}

#[test]
fn core_glob_keeps_catalog_core_tools() {
    let all = names(&["read_file", "bash", "runs.get"]);
    let f = ToolFilter::parse(&names(&["core.*"]));
    assert_eq!(f.apply(&all).unwrap(), set(&["read_file", "bash"]));
}

#[test]
fn alias_resolves_to_canonical() {
    let all = names(&["findings.report"]);
    let f = ToolFilter::parse(&names(&["report_finding"]));
    assert_eq!(f.apply(&all).unwrap(), set(&["findings.report"]));
}

#[test]
fn unknown_entry_fails_naming_it() {
    let all = names(&["runs.get", "scm.prs.get"]);
    let err = ToolFilter::parse(&names(&["nope.*"]))
        .apply(&all)
        .unwrap_err()
        .to_string();
    assert!(err.contains("`nope.*`"), "{err}");
    assert!(err.contains("runs"), "{err}");
}
