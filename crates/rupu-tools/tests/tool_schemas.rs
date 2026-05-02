use rupu_tools::{BashTool, EditFileTool, GlobTool, GrepTool, ReadFileTool, Tool, WriteFileTool};

fn assert_schema_well_formed(name: &str, schema: &serde_json::Value) {
    assert_eq!(
        schema.get("type").and_then(|v| v.as_str()),
        Some("object"),
        "{name}: schema.type should be 'object'"
    );
    assert!(
        schema.get("properties").is_some(),
        "{name}: schema must have properties"
    );
    assert!(
        schema.get("required").and_then(|v| v.as_array()).is_some(),
        "{name}: schema must have a required array"
    );
}

#[test]
fn bash_schema_has_command_required() {
    let s = BashTool.input_schema();
    assert_schema_well_formed("bash", &s);
    let required: Vec<String> = s["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(required.contains(&"command".to_string()));
}

#[test]
fn read_file_schema_has_path() {
    let s = ReadFileTool.input_schema();
    assert_schema_well_formed("read_file", &s);
    assert!(s["properties"]["path"].is_object());
}

#[test]
fn write_file_schema_has_path_and_content() {
    let s = WriteFileTool.input_schema();
    assert_schema_well_formed("write_file", &s);
    let req = s["required"].as_array().unwrap();
    let names: Vec<&str> = req.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(names.contains(&"path"));
    assert!(names.contains(&"content"));
}

#[test]
fn edit_file_schema_has_three_required_fields() {
    let s = EditFileTool.input_schema();
    assert_schema_well_formed("edit_file", &s);
    let req = s["required"].as_array().unwrap();
    assert_eq!(req.len(), 3);
}

#[test]
fn grep_schema_has_pattern_required_path_optional() {
    let s = GrepTool.input_schema();
    assert_schema_well_formed("grep", &s);
    let req = s["required"].as_array().unwrap();
    let names: Vec<&str> = req.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(names.contains(&"pattern"));
    assert!(!names.contains(&"path"));
    // path should appear in properties even though not required
    assert!(s["properties"]["path"].is_object());
}

#[test]
fn glob_schema_has_pattern() {
    let s = GlobTool.input_schema();
    assert_schema_well_formed("glob", &s);
    let req = s["required"].as_array().unwrap();
    let names: Vec<&str> = req.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(names.contains(&"pattern"));
}

#[test]
fn descriptions_are_non_empty() {
    for (name, desc) in [
        ("bash", BashTool.description()),
        ("read_file", ReadFileTool.description()),
        ("write_file", WriteFileTool.description()),
        ("edit_file", EditFileTool.description()),
        ("grep", GrepTool.description()),
        ("glob", GlobTool.description()),
    ] {
        assert!(!desc.is_empty(), "{name}: description must be non-empty");
        assert!(
            desc.len() > 50,
            "{name}: description should be substantive (>50 chars)"
        );
    }
}
