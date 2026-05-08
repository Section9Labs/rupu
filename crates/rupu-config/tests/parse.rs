use rupu_config::Config;

#[test]
fn parses_minimal_config() {
    let toml = r#"
        default_provider = "anthropic"
        default_model = "claude-sonnet-4-6"
    "#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    assert_eq!(cfg.default_provider.as_deref(), Some("anthropic"));
    assert_eq!(cfg.default_model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(cfg.permission_mode, None);
}

#[test]
fn parses_full_config() {
    let toml = r#"
        default_provider = "anthropic"
        default_model = "claude-sonnet-4-6"
        permission_mode = "ask"
        log_level = "info"

        [bash]
        timeout_secs = 60
        env_allowlist = ["MY_VAR", "AWS_PROFILE"]

        [retry]
        max_attempts = 3
        initial_delay_ms = 200

        [autoflow]
        enabled = true
        repo = "github:Section9Labs/rupu"
        checkout = "worktree"
        worktree_root = "~/.rupu/autoflows/worktrees"
        permission_mode = "bypass"
        strict_templates = true
        max_active = 2
        cleanup_after = "7d"
    "#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    assert_eq!(cfg.permission_mode.as_deref(), Some("ask"));
    assert_eq!(cfg.log_level.as_deref(), Some("info"));
    assert_eq!(cfg.bash.timeout_secs, Some(60));
    assert_eq!(
        cfg.bash.env_allowlist,
        Some(vec!["MY_VAR".into(), "AWS_PROFILE".into()])
    );
    assert_eq!(cfg.retry.max_attempts, Some(3));
    assert_eq!(cfg.autoflow.enabled, Some(true));
    assert_eq!(
        cfg.autoflow.repo.as_deref(),
        Some("github:Section9Labs/rupu")
    );
    assert_eq!(cfg.autoflow.max_active, Some(2));
}

#[test]
fn empty_config_is_valid() {
    let cfg: Config = toml::from_str("").expect("parse");
    assert_eq!(cfg.default_provider, None);
}
