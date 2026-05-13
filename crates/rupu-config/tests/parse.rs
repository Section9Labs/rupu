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

        [ui]
        color = "always"
        theme = "Solarized (light)"
        pager = "never"

        [ui.syntax]
        theme = "InspiredGitHub"

        [ui.palette]
        theme = "github-light"

        [autoflow]
        enabled = true
        repo = "github:Section9Labs/rupu"
        checkout = "worktree"
        worktree_root = "~/.rupu/autoflows/worktrees"
        permission_mode = "bypass"
        strict_templates = true
        max_active = 2
        cleanup_after = "7d"

        [storage]
        archived_session_retention = "45d"
        archived_transcript_retention = "14d"
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
    assert_eq!(cfg.ui.color.as_deref(), Some("always"));
    assert_eq!(cfg.ui.theme.as_deref(), Some("Solarized (light)"));
    assert_eq!(cfg.ui.syntax.theme.as_deref(), Some("InspiredGitHub"));
    assert_eq!(cfg.ui.palette.theme.as_deref(), Some("github-light"));
    assert_eq!(cfg.autoflow.enabled, Some(true));
    assert_eq!(
        cfg.autoflow.repo.as_deref(),
        Some("github:Section9Labs/rupu")
    );
    assert_eq!(cfg.autoflow.max_active, Some(2));
    assert_eq!(
        cfg.storage.archived_session_retention.as_deref(),
        Some("45d")
    );
    assert_eq!(
        cfg.storage.archived_transcript_retention.as_deref(),
        Some("14d")
    );
}

#[test]
fn empty_config_is_valid() {
    let cfg: Config = toml::from_str("").expect("parse");
    assert_eq!(cfg.default_provider, None);
}
