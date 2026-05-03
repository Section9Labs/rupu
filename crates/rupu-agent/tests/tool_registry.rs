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

#[test]
fn to_tool_definitions_returns_all_six() {
    let r = default_tool_registry();
    let defs = r.to_tool_definitions();
    assert_eq!(defs.len(), 6);
    let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
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
fn to_tool_definitions_descriptions_non_empty() {
    let r = default_tool_registry();
    for d in r.to_tool_definitions() {
        assert!(!d.description.is_empty(), "{}: empty description", d.name);
    }
}

#[test]
fn to_tool_definitions_schemas_are_objects() {
    let r = default_tool_registry();
    for d in r.to_tool_definitions() {
        assert_eq!(
            d.input_schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "{}: schema.type should be 'object'",
            d.name
        );
        assert!(
            d.input_schema.get("properties").is_some(),
            "{}: missing properties",
            d.name
        );
    }
}

#[test]
fn filtered_registry_tool_definitions_match_filter() {
    let r = default_tool_registry();
    let filtered = r.filter_to(&["bash".into(), "read_file".into()]);
    let defs = filtered.to_tool_definitions();
    assert_eq!(defs.len(), 2);
    let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["bash", "read_file"]);
}
