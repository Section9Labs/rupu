use rupu_config::layer_files;
use std::io::Write;
use tempfile::NamedTempFile;

fn tmp_with(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

#[test]
fn project_overrides_global_scalar() {
    let g = tmp_with(
        r#"
default_provider = "anthropic"
default_model = "claude-sonnet-4-6"
"#,
    );
    let p = tmp_with(
        r#"
default_model = "claude-opus-4-7"
"#,
    );
    let cfg = layer_files(Some(g.path()), Some(p.path())).unwrap();
    assert_eq!(cfg.default_provider.as_deref(), Some("anthropic"));
    assert_eq!(cfg.default_model.as_deref(), Some("claude-opus-4-7"));
}

#[test]
fn project_overrides_global_table() {
    let g = tmp_with(
        r#"
[bash]
timeout_secs = 120
env_allowlist = ["A", "B"]
"#,
    );
    let p = tmp_with(
        r#"
[bash]
timeout_secs = 30
"#,
    );
    let cfg = layer_files(Some(g.path()), Some(p.path())).unwrap();
    assert_eq!(cfg.bash.timeout_secs, Some(30));
    // env_allowlist preserved from global because project didn't set it
    assert_eq!(cfg.bash.env_allowlist, Some(vec!["A".into(), "B".into()]));
}

#[test]
fn project_array_replaces_global_array_not_concat() {
    let g = tmp_with(
        r#"
[bash]
env_allowlist = ["A", "B", "C"]
"#,
    );
    let p = tmp_with(
        r#"
[bash]
env_allowlist = ["X"]
"#,
    );
    let cfg = layer_files(Some(g.path()), Some(p.path())).unwrap();
    // Critical: arrays REPLACE, never concat — so user can subtract
    assert_eq!(cfg.bash.env_allowlist, Some(vec!["X".into()]));
}

#[test]
fn missing_files_yield_empty_config() {
    let cfg = layer_files(None, None).unwrap();
    assert_eq!(cfg.default_provider, None);
}

#[test]
fn only_global_works() {
    let g = tmp_with(r#"default_provider = "openai""#);
    let cfg = layer_files(Some(g.path()), None).unwrap();
    assert_eq!(cfg.default_provider.as_deref(), Some("openai"));
}
