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

/// I-28 migration shim: `[cp].agent_authoring_ui` used to select the CP web
/// app's classic-vs-next agent-authoring UI. The classic UI is gone and the
/// key is now accepted-as-a-no-op (not "gone" -- `deny_unknown_fields` would
/// otherwise reject any config.toml still carrying it). This test only
/// proves the key no longer drives anything; it does not assert a "classic"
/// default because there is no longer a UI selection to default.
#[test]
fn cp_config_agent_authoring_ui_is_accepted_as_a_no_op() {
    let cfg: rupu_config::policy_config::CpConfig = toml::from_str("").expect("empty [cp] parses");
    assert!(cfg.agent_authoring_ui.is_none());
}

#[test]
fn cp_config_accepts_next_agent_authoring_ui_as_a_no_op() {
    let cfg: rupu_config::policy_config::CpConfig =
        toml::from_str("agent_authoring_ui = \"next\"").unwrap();
    assert!(cfg.agent_authoring_ui.is_some());
}

/// I-29 migration shim: `[cp].workflow_editor_ui` -- same shape as
/// `agent_authoring_ui` above, see that test's doc comment.
#[test]
fn cp_config_workflow_editor_ui_is_accepted_as_a_no_op() {
    let cfg: rupu_config::policy_config::CpConfig = toml::from_str("").expect("empty [cp] parses");
    assert!(cfg.workflow_editor_ui.is_none());
}

#[test]
fn cp_config_accepts_next_workflow_editor_ui_as_a_no_op() {
    let cfg: rupu_config::policy_config::CpConfig =
        toml::from_str("workflow_editor_ui = \"next\"").unwrap();
    assert!(cfg.workflow_editor_ui.is_some());
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

/// I-28/I-29 migration shim: a `config.toml` that still sets
/// `[cp].agent_authoring_ui` must parse, and every *other* `[cp]` key in the
/// same file must survive with its value intact. `gate_sweep_interval_secs`
/// is asserted against a non-default value (60 is the compiled default) --
/// this is the assertion that would have caught the Arc 1 `[retry]`
/// regression, where `.unwrap_or_default()` silently discarded the entire
/// config on a parse failure. A test that only checks "did it parse" cannot
/// distinguish "the whole config loaded" from "the whole config was
/// silently replaced by defaults".
#[test]
fn cp_agent_authoring_ui_still_parses_with_sibling_cp_keys_intact() {
    let toml = r#"
        [cp]
        agent_authoring_ui = "next"
        gate_sweep_interval_secs = 15
        gate_sweep_enabled = false
        max_workspace_bytes = 1048576
    "#;
    let cfg: Config = toml::from_str(toml).expect("`[cp].agent_authoring_ui` must not fail the parse");

    // The deprecated key is accepted opaquely...
    assert!(cfg.cp.agent_authoring_ui.is_some());

    // ...and every sibling `[cp]` key in the same table kept its own
    // (non-default) value rather than the whole section silently resetting.
    assert_eq!(cfg.cp.gate_sweep_interval_secs, 15);
    assert!(!cfg.cp.gate_sweep_enabled);
    assert_eq!(cfg.cp.max_workspace_bytes, Some(1_048_576));

    assert!(cfg.validate().is_ok());
}

#[test]
fn cp_workflow_editor_ui_still_parses_with_sibling_cp_keys_intact() {
    let toml = r#"
        [cp]
        workflow_editor_ui = "next"
        gate_sweep_interval_secs = 15
        gate_sweep_enabled = false
        max_workspace_bytes = 1048576
    "#;
    let cfg: Config =
        toml::from_str(toml).expect("`[cp].workflow_editor_ui` must not fail the parse");

    assert!(cfg.cp.workflow_editor_ui.is_some());

    assert_eq!(cfg.cp.gate_sweep_interval_secs, 15);
    assert!(!cfg.cp.gate_sweep_enabled);
    assert_eq!(cfg.cp.max_workspace_bytes, Some(1_048_576));

    assert!(cfg.validate().is_ok());
}

/// Deprecated keys round-trip out of any TOML rewrite (`skip_serializing`),
/// mirroring the `[retry]` shim's contract.
#[test]
fn cp_deprecated_ui_keys_never_round_trip_back_out() {
    let toml = r#"
        [cp]
        agent_authoring_ui = "next"
        workflow_editor_ui = "next"
    "#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let round_tripped = toml::to_string(&cfg).expect("serialize");
    assert!(
        !round_tripped.contains("agent_authoring_ui"),
        "deprecated key must not be re-serialized: {round_tripped}"
    );
    assert!(
        !round_tripped.contains("workflow_editor_ui"),
        "deprecated key must not be re-serialized: {round_tripped}"
    );
}

/// A minimal `tracing::Subscriber` that records every event's fields as a
/// single formatted string, so tests can assert a specific `tracing::warn!`
/// fired without pulling in an extra dev-dependency crate.
mod capture {
    use std::fmt;
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::{span, Event, Metadata, Subscriber};

    #[derive(Clone, Default)]
    pub struct CapturingSubscriber {
        pub messages: Arc<Mutex<Vec<String>>>,
    }

    struct FieldVisitor(String);

    impl Visit for FieldVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if !self.0.is_empty() {
                self.0.push(' ');
            }
            self.0.push_str(&format!("{}={:?}", field.name(), value));
        }
    }

    impl Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }

        fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

        fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut visitor = FieldVisitor(String::new());
            event.record(&mut visitor);
            self.messages.lock().unwrap().push(visitor.0);
        }

        fn enter(&self, _span: &span::Id) {}

        fn exit(&self, _span: &span::Id) {}
    }
}

/// Guard against a `tracing` behaviour that made the two capture-based tests
/// below flake at roughly 8% (5 failures in 60 runs of the test binary), with
/// one of the two expected keys simply missing from the captured output.
///
/// `tracing` caches each callsite's `Interest` **process-wide** the first time
/// that callsite is reached. `with_default` installs a subscriber on the
/// *calling thread only*, so when some other test in this binary reaches one
/// of `validate()`'s `warn!` callsites first — and several of them call
/// `validate()` with no subscriber at all — that callsite registers as
/// `Interest::never()` and is then silenced for every thread, including one
/// holding a capturing subscriber. Which of the deprecation callsites lost the
/// race was down to thread scheduling, hence the flake.
///
/// Installing an always-enabled global default fixes it in both directions:
/// `set_global_default` rebuilds the interest cache, promoting any callsite
/// already cached as `never`, and from then on no thread can register `never`
/// because a subscriber is always present. Registration happens once per
/// callsite, so nothing can demote it again afterwards.
///
/// The global default only discards events; the assertions still read from the
/// thread-local `with_default` subscriber. It may only be set once per
/// process, hence the `Once`.
fn stop_other_threads_silencing_our_callsites() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(capture::CapturingSubscriber::default());
    });
}

/// The second half of the binding test: presence of either deprecated key
/// must emit a one-line `tracing::warn!` naming it, mirroring `[retry]`'s
/// `warn_deprecated_keys` contract.
#[test]
fn cp_deprecated_ui_keys_emit_a_deprecation_warning_each() {
    stop_other_threads_silencing_our_callsites();
    let cfg: Config = toml::from_str(
        r#"
        [cp]
        agent_authoring_ui = "next"
        workflow_editor_ui = "next"
    "#,
    )
    .expect("parse");

    let subscriber = capture::CapturingSubscriber::default();
    let messages = subscriber.messages.clone();
    tracing::subscriber::with_default(subscriber, || {
        cfg.validate().expect("validate");
    });

    let logs = messages.lock().unwrap();
    let joined = logs.join("\n");
    assert!(
        joined.contains("cp.agent_authoring_ui"),
        "expected a deprecation warning naming cp.agent_authoring_ui, got: {joined}"
    );
    assert!(
        joined.contains("cp.workflow_editor_ui"),
        "expected a deprecation warning naming cp.workflow_editor_ui, got: {joined}"
    );
}

/// ISSUES.md I-73: `[scm.default].owner`/`.repo` have zero consumers and
/// are now formally deprecated (unlike `.platform`, which I-15 wired up).
/// Deprecated keys round-trip out of any TOML rewrite, mirroring the
/// `[cp]` UI flags' contract above.
#[test]
fn scm_default_owner_and_repo_never_round_trip_back_out() {
    let toml = r#"
        [scm.default]
        platform = "github"
        owner = "section9labs"
        repo = "rupu"
    "#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let round_tripped = toml::to_string(&cfg).expect("serialize");
    assert!(
        round_tripped.contains("platform = \"github\""),
        "the live `.platform` key must still round-trip: {round_tripped}"
    );
    assert!(
        !round_tripped.contains("owner"),
        "deprecated key must not be re-serialized: {round_tripped}"
    );
    assert!(
        !round_tripped.contains("section9labs"),
        "deprecated key's value must not be re-serialized: {round_tripped}"
    );
    assert!(
        !round_tripped.contains("repo = "),
        "deprecated key must not be re-serialized: {round_tripped}"
    );
}

/// The second half of the binding test: presence of either deprecated key
/// must emit a one-line `tracing::warn!` naming it, mirroring `[retry]`'s
/// `warn_deprecated_keys` contract.
#[test]
fn scm_default_owner_and_repo_emit_deprecation_warnings() {
    stop_other_threads_silencing_our_callsites();
    let cfg: Config = toml::from_str(
        r#"
        [scm.default]
        owner = "section9labs"
        repo = "rupu"
    "#,
    )
    .expect("parse");

    let subscriber = capture::CapturingSubscriber::default();
    let messages = subscriber.messages.clone();
    tracing::subscriber::with_default(subscriber, || {
        cfg.validate().expect("validate");
    });

    let logs = messages.lock().unwrap();
    let joined = logs.join("\n");
    assert!(
        joined.contains("scm.default.owner"),
        "expected a deprecation warning naming scm.default.owner, got: {joined}"
    );
    assert!(
        joined.contains("scm.default.repo"),
        "expected a deprecation warning naming scm.default.repo, got: {joined}"
    );
}
