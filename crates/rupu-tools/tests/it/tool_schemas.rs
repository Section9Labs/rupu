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

/// The connector tools moved from `rupu-mcp` into this crate (W4) with their
/// names, descriptions and schemas unchanged: `connector_tools.json` is
/// `rupu-mcp`'s `tools/list` snapshot of them from before the move.
#[test]
fn connector_tools_are_unchanged_by_the_move() {
    let snapshot: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../snapshots/connector_tools.json")).unwrap();
    let connectors: Vec<_> = rupu_tools::ToolCatalog::all()
        .iter()
        .filter(|d| d.is_connector())
        .collect();
    assert_eq!(connectors.len(), snapshot.len());
    for (d, want) in connectors.iter().zip(&snapshot) {
        assert_eq!(d.name, want["name"], "catalog order");
        assert_eq!(d.description, want["description"], "{}", d.name);
        assert_eq!((d.input_schema)(), want["inputSchema"], "{}", d.name);
    }
}

/// W4 §6.4: a summary-profile run's model sees today's summary schema, and a
/// full-profile run today's full schema (snapshots of the schemas before
/// findings got one implementation). Re-bless with `BLESS=1`.
#[test]
fn findings_report_schema_follows_the_runs_profile() {
    use rupu_coverage::{FindingProfile, FindingWriteOptions};
    use rupu_tools::findings::report::FindingsReportTool;
    for (profile, file) in [
        (FindingProfile::Summary, "findings_report_summary.json"),
        (FindingProfile::Full, "findings_report_full.json"),
    ] {
        let got = FindingsReportTool::new(FindingWriteOptions::default().with_profile(profile))
            .input_schema();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/snapshots")
            .join(file);
        if std::env::var("BLESS").is_ok() {
            std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap()).unwrap();
        }
        let want: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(got, want, "{file}");
    }
}

/// Every catalog schema compiles as a JSON Schema (it is what `rupu mcp
/// serve`, `/api/tools` and the provider requests carry).
#[test]
fn every_catalog_schema_compiles() {
    for d in rupu_tools::ToolCatalog::all() {
        jsonschema::JSONSchema::compile(&(d.input_schema)())
            .unwrap_or_else(|e| panic!("{} has an invalid input_schema: {e}", d.name));
    }
}
