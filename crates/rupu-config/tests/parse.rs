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

        [ui]
        color = "always"
        theme = "Solarized (light)"
        live_view = "full"
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
    assert_eq!(cfg.ui.color.as_deref(), Some("always"));
    assert_eq!(cfg.ui.theme.as_deref(), Some("Solarized (light)"));
    assert_eq!(cfg.ui.live_view.as_deref(), Some("full"));
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

#[test]
fn cp_config_defaults_agent_authoring_ui_to_classic() {
    let cfg: rupu_config::policy_config::CpConfig = toml::from_str("").expect("empty [cp] parses");
    assert_eq!(cfg.agent_authoring_ui, "classic");
}

#[test]
fn cp_config_accepts_next_agent_authoring_ui() {
    let cfg: rupu_config::policy_config::CpConfig =
        toml::from_str("agent_authoring_ui = \"next\"").unwrap();
    assert_eq!(cfg.agent_authoring_ui, "next");
}

#[test]
fn cp_config_defaults_workflow_editor_ui_to_classic() {
    let cfg: rupu_config::policy_config::CpConfig = toml::from_str("").expect("empty [cp] parses");
    assert_eq!(cfg.workflow_editor_ui, "classic");
}

#[test]
fn cp_config_accepts_next_workflow_editor_ui() {
    let cfg: rupu_config::policy_config::CpConfig =
        toml::from_str("workflow_editor_ui = \"next\"").unwrap();
    assert_eq!(cfg.workflow_editor_ui, "next");
}

#[test]
fn cp_config_defaults_gate_sweep_enabled_true_and_interval_60() {
    let cfg: rupu_config::policy_config::CpConfig = toml::from_str("").expect("empty [cp] parses");
    assert!(cfg.gate_sweep_enabled);
    assert_eq!(cfg.gate_sweep_interval_secs, 60);
}

#[test]
fn cp_config_overrides_gate_sweep_flags_from_toml() {
    let cfg: rupu_config::policy_config::CpConfig =
        toml::from_str("gate_sweep_enabled = false\ngate_sweep_interval_secs = 15").unwrap();
    assert!(!cfg.gate_sweep_enabled);
    assert_eq!(cfg.gate_sweep_interval_secs, 15);
}

#[test]
fn locked_global_key_survives_a_project_override() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("global.toml");
    let project = dir.path().join("project.toml");
    std::fs::write(
        &global,
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
    )
    .unwrap();
    std::fs::write(&project, "permission_mode = \"bypass\"\n").unwrap();

    // Unlocked loader: project wins (today's behavior, unchanged).
    let plain = rupu_config::layer_files(Some(&global), Some(&project)).unwrap();
    assert_eq!(plain.permission_mode.as_deref(), Some("bypass"));

    // Lock-aware loader: the global lock holds.
    let locked = rupu_config::layer_files_locked(Some(&global), Some(&project)).unwrap();
    assert_eq!(
        locked.permission_mode.as_deref(),
        Some("readonly"),
        "a locked global key must not be overridable by a project config"
    );
}

#[test]
fn unlocked_keys_still_layer_project_over_global() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("global.toml");
    let project = dir.path().join("project.toml");
    std::fs::write(
        &global,
        "default_model = \"g\"\n[policy]\nlock = [\"permission_mode\"]\n",
    )
    .unwrap();
    std::fs::write(&project, "default_model = \"p\"\n").unwrap();

    let locked = rupu_config::layer_files_locked(Some(&global), Some(&project)).unwrap();
    assert_eq!(locked.default_model.as_deref(), Some("p"));
}

/// I-13 migration shim: `[retry]` was deleted as a live key, but `Config` is
/// `deny_unknown_fields` and seven CLI paths (plus `rupu-cp`) load config with
/// `.unwrap_or_default()`. A hard parse failure there would silently discard
/// EVERY key the user set, not just the dead one — the exact data-loss class
/// Arc 1 exists to eliminate. So the section is accepted as an inert no-op and
/// a `tracing::warn!` names it.
#[test]
fn a_config_still_carrying_retry_loads_and_keeps_every_other_key() {
    let toml = r#"
        default_provider = "anthropic"
        default_model = "claude-sonnet-4-6"
        permission_mode = "bypass"
        log_level = "debug"

        [retry]
        max_attempts = 3
        initial_delay_ms = 200

        [bash]
        timeout_secs = 60

        [providers.anthropic]
        max_retries = 4
    "#;
    let cfg: Config = toml::from_str(toml).expect("`[retry]` must not fail the whole parse");

    // (a) every other key survived.
    assert_eq!(cfg.default_provider.as_deref(), Some("anthropic"));
    assert_eq!(cfg.default_model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(cfg.permission_mode.as_deref(), Some("bypass"));
    assert_eq!(cfg.log_level.as_deref(), Some("debug"));
    assert_eq!(cfg.bash.timeout_secs, Some(60));
    assert_eq!(cfg.providers["anthropic"].max_retries, Some(4));

    // The section is carried opaquely and read by nothing.
    assert!(cfg.retry.is_some());
    assert!(cfg.validate().is_ok());

    // (c) and it never travels back out — `/api/config` and any TOML rewrite
    // drop it, so the key disappears the first time a config is rewritten.
    let round_tripped = toml::to_string(&cfg).expect("serialize");
    assert!(
        !round_tripped.contains("[retry]"),
        "deprecated section must not be re-serialized: {round_tripped}"
    );
    assert!(!round_tripped.contains("max_attempts"));
}

/// The same config through the real on-disk load paths, both layers.
#[test]
fn retry_survives_layer_files_and_the_lock_aware_loader() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("global.toml");
    let project = dir.path().join("project.toml");
    std::fs::write(
        &global,
        "default_model = \"g\"\n[retry]\nmax_attempts = 3\ninitial_delay_ms = 200\n",
    )
    .unwrap();
    std::fs::write(&project, "log_level = \"debug\"\n").unwrap();

    let plain = rupu_config::layer_files(Some(&global), Some(&project))
        .expect("`[retry]` must not fail layer_files");
    assert_eq!(plain.default_model.as_deref(), Some("g"));
    assert_eq!(plain.log_level.as_deref(), Some("debug"));

    let locked = rupu_config::layer_files_locked(Some(&global), Some(&project))
        .expect("`[retry]` must not fail the lock-aware loader");
    assert_eq!(locked.default_model.as_deref(), Some("g"));
    assert_eq!(locked.log_level.as_deref(), Some("debug"));
}
