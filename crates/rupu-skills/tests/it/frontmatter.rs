use rupu_skills::{parse_skill_md, FrontmatterError};

#[test]
fn minimal_skill() {
    let (fm, body) = parse_skill_md(
        "---\nname: pdf-processing\ndescription: Extract PDF text. Use when handling PDFs.\n---\n\n# PDF\n\nStep one.\n",
    )
    .unwrap();
    assert_eq!(fm.name, "pdf-processing");
    assert_eq!(fm.description, "Extract PDF text. Use when handling PDFs.");
    assert_eq!(fm.license, None);
    assert!(fm.allowed_tools.is_empty());
    assert!(!fm.disable_model_invocation);
    assert_eq!(body, "# PDF\n\nStep one.");
}

#[test]
fn optional_fields_and_unknown_keys() {
    let text = "---\n\
name: report-style\n\
description: House style for reports.\n\
license: Apache-2.0\n\
compatibility: Requires Python 3.12+ and uv\n\
metadata:\n  author: example-org\n  version: 1.0\n  beta: true\n\
allowed-tools: Bash(git status:*) Read, Grep\n\
disable-model-invocation: true\n\
argument-hint: \"[file]\"\n\
---\nBody\n";
    let (fm, body) = parse_skill_md(text).unwrap();
    assert_eq!(fm.license.as_deref(), Some("Apache-2.0"));
    assert_eq!(
        fm.compatibility.as_deref(),
        Some("Requires Python 3.12+ and uv")
    );
    assert_eq!(
        fm.metadata.get("author").map(String::as_str),
        Some("example-org")
    );
    assert_eq!(fm.metadata.get("version").map(String::as_str), Some("1.0"));
    assert_eq!(fm.metadata.get("beta").map(String::as_str), Some("true"));
    assert_eq!(fm.allowed_tools, vec!["Bash(git status:*)", "Read", "Grep"]);
    assert!(fm.disable_model_invocation);
    assert_eq!(body, "Body");
}

#[test]
fn allowed_tools_as_a_list() {
    let (fm, _) = parse_skill_md(
        "---\nname: a\ndescription: d\nallowed-tools:\n  - Read\n  - Bash(jq:*)\n---\n",
    )
    .unwrap();
    assert_eq!(fm.allowed_tools, vec!["Read", "Bash(jq:*)"]);
}

#[test]
fn bom_and_crlf() {
    let text = "\u{feff}---\r\nname: a\r\ndescription: d\r\n---\r\nLine one\r\nLine two\r\n";
    let (fm, body) = parse_skill_md(text).unwrap();
    assert_eq!(fm.name, "a");
    assert_eq!(body, "Line one\r\nLine two");
}

#[test]
fn empty_body_is_fine() {
    let (_, body) = parse_skill_md("---\nname: a\ndescription: d\n---\n").unwrap();
    assert_eq!(body, "");
}

#[test]
fn errors() {
    assert_eq!(
        parse_skill_md("# no frontmatter\n"),
        Err(FrontmatterError::MissingOpen)
    );
    assert_eq!(
        parse_skill_md("---\nname: a\n"),
        Err(FrontmatterError::MissingClose)
    );
    assert_eq!(
        parse_skill_md("---\ndescription: d\n---\n"),
        Err(FrontmatterError::MissingName)
    );
    assert_eq!(
        parse_skill_md("---\nname: a\n---\n"),
        Err(FrontmatterError::MissingDescription)
    );
    assert_eq!(
        parse_skill_md("---\nname: a\ndescription: \"\"\n---\n"),
        Err(FrontmatterError::MissingDescription)
    );
    assert_eq!(
        parse_skill_md("---\nname: a\ndescription: d\nmetadata:\n  nested: [1, 2]\n---\n"),
        Err(FrontmatterError::Metadata("nested".into()))
    );
    assert!(matches!(
        parse_skill_md("---\nname: [unclosed\n---\n"),
        Err(FrontmatterError::Yaml(_))
    ));
}

#[test]
fn block_scalar_description_is_trimmed() {
    let text = format!(
        "---\nname: a\ndescription: >\n  {}\n---\n",
        "x".repeat(1024)
    );
    let (fm, _) = parse_skill_md(&text).unwrap();
    assert_eq!(fm.description, "x".repeat(1024));

    let (fm, _) =
        parse_skill_md("---\nname: a\ndescription: |\n  Line one.\n  Line two.\n---\n").unwrap();
    assert_eq!(fm.description, "Line one.\nLine two.");
}

#[test]
fn whitespace_only_compatibility_is_empty() {
    assert_eq!(
        parse_skill_md("---\nname: a\ndescription: d\ncompatibility: \"   \"\n---\n"),
        Err(FrontmatterError::EmptyCompatibility)
    );
    let (fm, _) =
        parse_skill_md("---\nname: a\ndescription: d\ncompatibility: \" uv \"\n---\n").unwrap();
    assert_eq!(fm.compatibility.as_deref(), Some("uv"));
}

#[test]
fn null_metadata_value_is_an_empty_string() {
    let (fm, _) =
        parse_skill_md("---\nname: a\ndescription: d\nmetadata:\n  author:\n  v: 2\n---\n")
            .unwrap();
    assert_eq!(fm.metadata.get("author").map(String::as_str), Some(""));
    assert_eq!(fm.metadata.get("v").map(String::as_str), Some("2"));
}

#[test]
fn closing_paren_ends_a_tool_token() {
    let (fm, _) = parse_skill_md(
        "---\nname: a\ndescription: d\nallowed-tools: Bash(git:*)Read Bash(a (b) c)Grep\n---\n",
    )
    .unwrap();
    assert_eq!(
        fm.allowed_tools,
        vec!["Bash(git:*)", "Read", "Bash(a (b) c)", "Grep"]
    );
}

/// Published skills exceed the agentskills.io limits (anthropics/skills'
/// `claude-api` description is 1068 characters), so going over is a
/// warning, not an invalid skill.
#[test]
fn over_limit_lengths_are_warnings() {
    let (fm, _) = parse_skill_md(&format!(
        "---\nname: a\ndescription: {}\ncompatibility: {}\n---\n",
        "x".repeat(1068),
        "y".repeat(501)
    ))
    .unwrap();
    assert_eq!(fm.description.chars().count(), 1068);
    assert_eq!(
        fm.warnings,
        vec![
            "`description` is 1068 characters; the agentskills.io limit is 1024",
            "`compatibility` is 501 characters; the agentskills.io limit is 500",
        ]
    );
    let (fm, _) = parse_skill_md(&format!(
        "---\nname: a\ndescription: {}\ncompatibility: {}\n---\n",
        "x".repeat(1024),
        "y".repeat(500)
    ))
    .unwrap();
    assert!(fm.warnings.is_empty(), "{:?}", fm.warnings);
}
