use rupu_agent::default_tool_registry;

#[test]
fn default_registry_contains_six_tools() {
    let r = default_tool_registry();
    for name in [
        "bash",
        "read_file",
        "write_file",
        "edit_file",
        "grep",
        "glob",
    ] {
        assert!(r.get(name).is_some(), "expected tool {name}");
    }
}

#[test]
fn unknown_tool_is_none() {
    let r = default_tool_registry();
    assert!(r.get("teleport").is_none());
}

#[test]
fn known_tools_returns_sorted_list() {
    let r = default_tool_registry();
    let mut names = r.known_tools().to_vec();
    names.sort();
    assert_eq!(
        names,
        vec![
            "bash",
            "edit_file",
            "glob",
            "grep",
            "read_file",
            "write_file"
        ]
    );
}

#[test]
fn registry_respects_agent_tools_filter() {
    let r = default_tool_registry();
    let filtered = r.filter_to(&["bash".into(), "read_file".into()]);
    assert!(filtered.get("bash").is_some());
    assert!(filtered.get("read_file").is_some());
    assert!(filtered.get("write_file").is_none());
}
