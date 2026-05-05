use rupu_agent::AgentSpec;

const SAMPLE: &str = r#"---
name: fix-bug
description: Investigate a failing test and propose a fix.
provider: anthropic
model: claude-sonnet-4-6
tools: [bash, read_file, write_file, edit_file, grep, glob]
maxTurns: 30
permissionMode: ask
---
You are a senior engineer.

When given a failing test, you investigate carefully.
"#;

#[test]
fn parses_full_frontmatter() {
    let spec = AgentSpec::parse(SAMPLE).unwrap();
    assert_eq!(spec.name, "fix-bug");
    assert_eq!(
        spec.description.as_deref(),
        Some("Investigate a failing test and propose a fix.")
    );
    assert_eq!(spec.provider.as_deref(), Some("anthropic"));
    assert_eq!(spec.model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(
        spec.tools,
        Some(vec![
            "bash".into(),
            "read_file".into(),
            "write_file".into(),
            "edit_file".into(),
            "grep".into(),
            "glob".into(),
        ])
    );
    assert_eq!(spec.max_turns, Some(30));
    assert_eq!(spec.permission_mode.as_deref(), Some("ask"));
    assert!(spec.system_prompt.contains("senior engineer"));
    assert!(spec.system_prompt.contains("investigate carefully"));
}

#[test]
fn parses_minimal_frontmatter() {
    let s = "---\nname: hello\n---\nyou are a bot\n";
    let spec = AgentSpec::parse(s).unwrap();
    assert_eq!(spec.name, "hello");
    assert_eq!(spec.description, None);
    assert_eq!(spec.provider, None);
    assert_eq!(spec.system_prompt.trim(), "you are a bot");
}

#[test]
fn missing_frontmatter_errors() {
    let s = "no frontmatter here";
    assert!(AgentSpec::parse(s).is_err());
}

#[test]
fn missing_name_errors() {
    let s = "---\ndescription: x\n---\nbody\n";
    assert!(AgentSpec::parse(s).is_err());
}

#[test]
fn unknown_frontmatter_field_errors() {
    // Compatibility note: we use deny_unknown_fields so typos like
    // `permision_mode` get caught at parse time.
    let s = "---\nname: x\npermision_mode: ask\n---\nbody\n";
    assert!(AgentSpec::parse(s).is_err());
}

#[test]
fn parses_anthropic_feature_flags() {
    let s = r#"---
name: deploy-bot
provider: anthropic
model: claude-sonnet-4-6
outputFormat: json
anthropicTaskBudget: 4000
anthropicContextManagement: tool_clearing
anthropicSpeed: fast
---
body
"#;
    let spec = AgentSpec::parse(s).unwrap();
    assert_eq!(
        spec.output_format,
        Some(rupu_providers::types::OutputFormat::Json)
    );
    assert_eq!(spec.anthropic_task_budget, Some(4000));
    assert_eq!(
        spec.anthropic_context_management,
        Some(rupu_providers::types::ContextManagement::ToolClearing)
    );
    assert_eq!(
        spec.anthropic_speed,
        Some(rupu_providers::types::Speed::Fast)
    );
}

#[test]
fn anthropic_feature_flags_default_to_none_when_omitted() {
    let s = "---\nname: hello\n---\nbody\n";
    let spec = AgentSpec::parse(s).unwrap();
    assert!(spec.output_format.is_none());
    assert!(spec.anthropic_task_budget.is_none());
    assert!(spec.anthropic_context_management.is_none());
    assert!(spec.anthropic_speed.is_none());
}

#[test]
fn rejects_invalid_output_format() {
    let s = "---\nname: x\noutputFormat: yaml\n---\nbody\n";
    assert!(
        AgentSpec::parse(s).is_err(),
        "outputFormat: yaml should reject; only text|json"
    );
}

#[test]
fn rejects_invalid_speed() {
    let s = "---\nname: x\nanthropicSpeed: turbo\n---\nbody\n";
    assert!(AgentSpec::parse(s).is_err(), "only `fast` is accepted");
}
