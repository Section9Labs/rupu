use super::*;
use std::{path::PathBuf, sync::Arc};

use rupu_tools::{
    AliasScope, AllowAlways, Decision, DenyReason, PermissionPolicy, PromptAnswer, PromptRequest,
    Prompter, ToolCatalog, ToolContext, ToolServices,
};

// Explicitly test-only for the workspace's source-level assembly guard.
#[cfg(test)]
fn options(services: ToolServices, permission: PermissionPolicy) -> AgentRunOpts {
    let mut context = ToolContext::default();
    context.identity_mut().run_id = "run_readiness".into();
    context.identity_mut().agent = "rupu-session".into();
    context.identity_mut().provider = "mock".into();
    context.identity_mut().model = "mock-model".into();
    context.services = services;
    AgentRunOpts {
        system_prompt: "private prompt omitted from snapshot".into(),
        prompt: rupu_agent::UserTurn::new("private turn omitted from snapshot"),
        provider: Box::new(rupu_agent::MockProvider::new(vec![
            rupu_agent::ScriptedTurn::Panic("readiness must not call the provider".into()),
        ])),
        limits: rupu_providers::model_limits::ModelLimits::unknown(),
        recovery: Default::default(),
        permission,
        grant: Default::default(),
        alias_scope: AliasScope::Everywhere,
        tool_context: context,
        pins: Default::default(),
        concerns: None,
        max_turns: 1,
        stream: Default::default(),
        hooks: Default::default(),
        pause: None,
        collectors: Vec::new(),
        transcript_path: PathBuf::from("unused-readiness-transcript.jsonl"),
    }
}

fn resolve(opts: &mut AgentRunOpts, declared: &[&str], cap: Option<&[&str]>) {
    let declared: Vec<_> = declared.iter().map(|name| (*name).to_owned()).collect();
    let cap: Option<Vec<String>> =
        cap.map(|names| names.iter().map(|name| (*name).to_owned()).collect());
    opts.grant = rupu_agent::grant::resolve_run_grant(rupu_agent::grant::RunGrantInputs {
        declared: Some(&declared),
        step_actions: &[],
        connector_cap: cap.as_deref(),
        concerns: opts.concerns.is_some(),
        origin: None,
        messaging: None,
        tool_context: &opts.tool_context,
        alias_scope: opts.alias_scope,
    })
    .unwrap();
}

fn row<'a>(snapshot: &'a RunReadiness, name: &str) -> &'a ToolReadiness {
    snapshot.tools.iter().find(|row| row.name == name).unwrap()
}

#[test]
fn absent_services_preserve_explicit_and_wildcard_failures_without_inventing_handles() {
    let mut opts = options(ToolServices::default(), PermissionPolicy::bypass());
    resolve(&mut opts, &["read_file", "dispatch", "agents.*"], None);
    let snapshot = inspect(&opts);
    let dispatch = row(&snapshot, "dispatch");
    assert!(dispatch.selected && dispatch.unavailable);
    assert!(!dispatch.offered && !dispatch.service_available);
    assert_eq!(dispatch.required_services, ["launcher"]);
    assert_eq!(dispatch.missing_services, ["launcher"]);
    let agents = row(&snapshot, "agents.list");
    assert!(agents.selected && agents.skipped && !agents.unavailable);
    assert!(!agents.offered);
    assert_eq!(agents.missing_services, ["catalog"]);
    assert!(!row(&snapshot, "write_file").selected);
    assert!(snapshot
        .connectors
        .iter()
        .all(|connector| !connector.registry_service_constructed));
    // A projection must not silently drop offered tools or grant provenance.
    for entry in opts.grant.entries.values() {
        let tool = row(&snapshot, &entry.canonical);
        assert!(tool.selected && tool.offered && tool.service_available);
        assert_eq!(
            tool.grant_reasons,
            entry
                .reasons
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn connector_narrowing_and_empty_registry_remain_distinct_from_missing_service() {
    let mut opts = options(
        ToolServices {
            scm: Some(Arc::new(rupu_scm::Registry::default())),
            ..Default::default()
        },
        PermissionPolicy::unattended(PermissionMode::Readonly),
    );
    resolve(
        &mut opts,
        &["scm.files.read", "scm.prs.create", "read_file"],
        Some(&["scm.files.read"]),
    );
    let snapshot = inspect(&opts);
    let removed = row(&snapshot, "scm.prs.create");
    assert!(removed.selected && removed.narrowed && removed.service_available);
    assert!(!removed.offered && !removed.unavailable && !removed.skipped);
    assert_eq!(removed.permission, "denied_readonly");
    assert!(row(&snapshot, "scm.files.read").offered);
    assert!(row(&snapshot, "read_file").offered);
    for connector in &snapshot.connectors {
        assert!(connector.registry_service_constructed);
        assert!(connector.known_accounts.is_empty());
        assert_eq!(connector.target_access, "unverified");
    }
}

struct NeverGenerate;
#[async_trait::async_trait]
impl rupu_tools::WorkflowGenerator for NeverGenerate {
    async fn generate(
        &self,
        _: &str,
        _: &ToolContext,
    ) -> Result<rupu_tools::GeneratedWorkflow, rupu_tools::LaunchError> {
        panic!("readiness must not invoke a service")
    }
}

#[test]
fn permission_projection_agrees_with_real_policy_for_read_record_write_external_and_spawn() {
    let catalog = ToolCatalog::builtin();
    for mode in [PermissionMode::Readonly, PermissionMode::Bypass] {
        let mut opts = options(
            ToolServices {
                workflow_generator: Some(Arc::new(NeverGenerate)),
                scm: Some(Arc::new(rupu_scm::Registry::default())),
                ..Default::default()
            },
            PermissionPolicy::unattended(mode),
        );
        let names = [
            "read_file",
            "findings.report",
            "write_file",
            "scm.prs.create",
            "workflows.generate",
        ];
        resolve(&mut opts, &names, None);
        let snapshot = inspect(&opts);
        for name in names {
            let tool = row(&snapshot, name);
            assert!(tool.offered);
            let decision = opts.permission.decide(
                catalog.descriptor(name).unwrap(),
                &serde_json::json!({}),
                &mut AllowAlways::default(),
            );
            match decision {
                Decision::Allow => assert_eq!(tool.permission, "allowed"),
                Decision::Deny {
                    reason: DenyReason::Readonly,
                } => {
                    assert_eq!(tool.permission, "denied_readonly")
                }
                Decision::Spawn { ceiling } => {
                    assert_eq!(tool.permission, "spawn_with_ceiling");
                    assert_eq!(tool.child_ceiling.as_deref(), Some(ceiling.as_str()));
                }
                other => panic!("unexpected decision {other:?}"),
            }
            assert!(!tool.permission_degraded);
        }
    }
}

struct NeverPrompt;
impl Prompter for NeverPrompt {
    fn ask(&self, _: &PromptRequest<'_>) -> PromptAnswer {
        panic!("inspecting readiness must never ask for approval")
    }
}

#[test]
fn ask_reports_the_approval_channel_without_prompting_or_hiding_legacy_degradation() {
    let mut opts = options(
        ToolServices::default(),
        PermissionPolicy::new(PermissionMode::Ask, Some(Arc::new(NeverPrompt))),
    );
    resolve(&mut opts, &["write_file"], None);
    let interactive = inspect(&opts);
    assert!(interactive.approval_channel_available);
    assert_eq!(
        row(&interactive, "write_file").permission,
        "approval_required"
    );
    assert!(!row(&interactive, "write_file").permission_degraded);

    opts.permission = PermissionPolicy::unattended(PermissionMode::Ask);
    let detached = inspect(&opts);
    assert!(!detached.approval_channel_available);
    assert_eq!(
        row(&detached, "write_file").permission,
        "approval_unavailable"
    );
    assert!(row(&detached, "write_file").permission_degraded);
    assert_eq!(
        opts.permission.decide(
            ToolCatalog::builtin().descriptor("write_file").unwrap(),
            &serde_json::json!({}),
            &mut AllowAlways::default(),
        ),
        Decision::AllowDegraded,
    );
}

#[test]
fn real_concerns_and_qualified_installed_skill_add_ambient_tools_to_empty_declaration() {
    let home = tempfile::tempdir().unwrap();
    let skill_dir = home.path().join("skills/audit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: audit\ndescription: Inspect a small change.\n---\nRead the affected source.\n",
    )
    .unwrap();
    let dirs = rupu_skills::LayerDirs::for_layers(home.path(), None);
    let catalog = rupu_skills::SkillCatalog::discover(&dirs).unwrap();
    let grants = vec!["global:audit".to_owned()];
    let skills = rupu_skills::resolve_run_skills(
        &catalog,
        &[rupu_skills::SkillRequest {
            reason: rupu_skills::SkillReason::Session,
            grants: &grants,
            preload: &[],
        }],
    )
    .unwrap();
    let mut opts = options(
        ToolServices {
            skills: Some(Arc::new(skills)),
            ..Default::default()
        },
        PermissionPolicy::unattended(PermissionMode::Readonly),
    );
    opts.concerns = Some(rupu_agent::RunConcerns {
        block: rupu_agent::builtin::session_agent().concerns.unwrap(),
        full_mode_max_concerns: 10,
    });
    resolve(&mut opts, &[], None);
    let ready = inspect(&opts);
    assert!(row(&ready, "skills.load").offered);
    assert_eq!(ready.skills.len(), 1);
    assert_eq!(ready.skills[0].qualified_name, "global:audit");
    assert!(ready.skills[0].indexed && ready.skills[0].model_loadable);
    assert!(!ready.skills[0].preload);
    assert!(row(&ready, "coverage.mark").offered);
    assert_eq!(row(&ready, "coverage.mark").permission, "allowed");
    assert!(!row(&ready, "write_file").selected);

    opts.tool_context.services.skills = None;
    opts.concerns = None;
    resolve(&mut opts, &[], None);
    let without = inspect(&opts);
    assert!(without.skills.is_empty());
    assert!(!row(&without, "skills.load").selected);
    assert!(!row(&without, "coverage.mark").selected);
    assert_eq!(row(&without, "skills.load").missing_services, ["skills"]);
}

struct FixtureCredentials;
#[async_trait::async_trait]
impl rupu_auth::CredentialResolver for FixtureCredentials {
    async fn get(
        &self,
        account: &str,
        _: Option<rupu_providers::AuthMode>,
    ) -> anyhow::Result<(
        rupu_providers::AuthMode,
        rupu_providers::auth::AuthCredentials,
    )> {
        anyhow::ensure!(account == "github", "no fixture account");
        Ok((
            rupu_providers::AuthMode::ApiKey,
            rupu_providers::auth::AuthCredentials::ApiKey {
                key: "fake-readiness-token-never-sent".into(),
            },
        ))
    }

    async fn refresh(
        &self,
        _: &str,
        _: rupu_providers::AuthMode,
    ) -> anyhow::Result<rupu_providers::auth::AuthCredentials> {
        panic!("readiness must not refresh credentials")
    }
}

#[tokio::test]
async fn constructed_account_stays_unverified_and_owned_snapshot_omits_private_inputs() {
    // The real discovery path constructs clients with this intentionally fake
    // credential; it does not verify authentication or call a target.
    rupu_scm::install_default_crypto_provider();
    let registry = rupu_scm::Registry::discover(
        &FixtureCredentials,
        &rupu_config::Config::default(),
        Arc::new(rupu_netflow::NullSink),
    )
    .await;
    let mut opts = options(
        ToolServices {
            scm: Some(Arc::new(registry)),
            ..Default::default()
        },
        PermissionPolicy::bypass(),
    );
    resolve(&mut opts, &["scm.files.read"], None);
    let snapshot = inspect(&opts);
    drop(opts);
    let github = snapshot
        .connectors
        .iter()
        .find(|c| c.platform == "github")
        .unwrap();
    assert!(github.registry_service_constructed);
    assert_eq!(github.known_accounts, ["github"]);
    assert_eq!(github.target_access, "unverified");
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains("fake-readiness-token-never-sent"));
    assert!(!encoded.contains("private prompt"));
    assert!(!encoded.contains("private turn"));
    assert_eq!(
        serde_json::from_str::<RunReadiness>(&encoded).unwrap(),
        snapshot
    );
    assert_eq!(snapshot.applies, "assembled_turn");
    assert_eq!(snapshot.run_id, "run_readiness");
    assert!(chrono::DateTime::parse_from_rfc3339(&snapshot.observed_at).is_ok());
}

struct NeverInvokeOwned;
#[async_trait::async_trait]
impl rupu_tools::ToolInvoker for NeverInvokeOwned {
    async fn invoke(
        &self,
        _: serde_json::Value,
        _: &ToolContext,
    ) -> Result<rupu_tools::ToolOutput, rupu_tools::ToolError> {
        panic!("readiness and validation must never invoke an owned tool")
    }
}
fn owned_catalog() -> ToolCatalog {
    ToolCatalog::builtin()
        .with_owned(
            rupu_tools::BoundTool::owned(
                rupu_tools::ToolDefinition::remote(
                    "mcp.fixture".into(),
                    rupu_tools::RemoteToolIdentity {
                        server: "fixture-server".into(),
                        tool: "query".into(),
                    },
                    "fixture".into(),
                    serde_json::json!({"type":"object"}),
                    Effect::External,
                )
                .unwrap(),
                Arc::new(NeverInvokeOwned),
            )
            .unwrap(),
        )
        .unwrap()
}

#[test]
fn owned_readiness_uses_the_resolved_snapshot_without_probing_or_claiming_permission() {
    let mut opts = options(
        ToolServices {
            tool_catalog: owned_catalog(),
            ..Default::default()
        },
        PermissionPolicy::unattended(PermissionMode::Readonly),
    );
    resolve(&mut opts, &["mcp.fixture"], None);
    // The original context can disappear; the assembled grant owns its evidence.
    opts.tool_context.services.tool_catalog = ToolCatalog::builtin();
    let snapshot = inspect(&opts);
    let tool = row(&snapshot, "mcp.fixture");
    assert!(tool.selected && tool.offered);
    assert_eq!(tool.effect, "external");
    assert_eq!(tool.permission, "denied_readonly");
    assert_eq!(tool.grant_reasons, ["declared"]);
}

#[test]
fn owned_settings_validation_is_explicit_and_does_not_expand_the_legacy_wire_contract() {
    let settings = crate::session_settings::SessionSettings {
        tools: Some(vec!["mcp.fixture".into()]),
        ..Default::default()
    };
    assert!(settings.validate().is_err());
    assert!(settings.validate_with_catalog(&owned_catalog()).is_ok());
    let invalid = crate::session_settings::SessionSettings {
        tools: Some(vec!["mcp.other".into()]),
        ..Default::default()
    };
    assert!(invalid.validate_with_catalog(&owned_catalog()).is_err());
    assert!(crate::session_settings::SessionSettings::default()
        .validate_with_catalog(&owned_catalog())
        .is_ok());
}
