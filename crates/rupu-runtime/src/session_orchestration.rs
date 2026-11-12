//! Explicit session-owned coordination configuration. Conversation and settings
//! writers never overwrite this file. No Agentiflow identity is borrowed.
use fs2::FileExt;
use rupu_tools::{
    AliasScope, AmbientGrant, Effect, GrantInputs, GrantReason, PermissionMode, Service,
    ServiceSet, ToolCatalog,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOrchestration {
    pub revision: u64,
    pub enabled: bool,
}
impl SessionOrchestration {
    pub fn read(dir: &Path) -> anyhow::Result<Self> {
        match fs::read(dir.join("orchestration.json")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, dir: &Path) -> anyhow::Result<Self> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("orchestration.lock"))?;
        lock.lock_exclusive()?;
        let current = Self::read(dir)?;
        anyhow::ensure!(
            current.revision == self.revision,
            "orchestration changed; reload before saving"
        );
        let saved = Self {
            revision: self
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("orchestration revision overflow"))?,
            enabled: self.enabled,
        };
        atomic_json(dir, "orchestration.json", &saved)?;
        Ok(saved)
    }
}

pub fn atomic_json(dir: &Path, name: &str, value: &impl Serialize) -> anyhow::Result<()> {
    let temp = dir.join(format!("{name}-{}.tmp", ulid::Ulid::new()));
    let result = (|| -> anyhow::Result<()> {
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        f.write_all(&serde_json::to_vec_pretty(value)?)?;
        f.sync_all()?;
        fs::rename(&temp, dir.join(name))?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

/// The shared lead tools that have meaningful services in an ordinary session.
/// Status tools require a supervising envelope and are deliberately absent.
pub fn grant() -> AmbientGrant {
    let mut grant = AmbientGrant::flow_lead();
    grant
        .tools
        .retain(|n| !matches!(*n, "goal.status" | "goal.coverage" | "budget.status"));
    grant.reason = GrantReason::Ambient("session_orchestration");
    grant
}

#[derive(Debug, Serialize)]
pub struct ToolAvailability {
    pub name: &'static str,
    pub available: bool,
    pub reason: Option<String>,
    pub declared: bool,
    pub granted: bool,
    pub grant_reasons: Vec<String>,
    pub service_available: bool,
    pub permission_allowed: bool,
    pub effect: Effect,
}

/// Inspect the coordination inventory using the same catalog/grant resolver
/// as assembly. Credentials, catalog target existence and per-call child
/// allowlists are validated when a tool executes, not guessed here.
pub fn inspect(
    enabled: bool,
    declared: Option<&[String]>,
    messaging: bool,
    mode: PermissionMode,
) -> anyhow::Result<Vec<ToolAvailability>> {
    let catalog = ToolCatalog::builtin();
    let mut services = ServiceSet::new()
        .with(Service::Launcher)
        .with(Service::WorkflowGenerator)
        .with(Service::Catalog)
        .with(Service::Findings);
    let mut ambient = Vec::new();
    if messaging {
        services.insert(Service::MessageBus);
        ambient.push(AmbientGrant::messaging_default());
    }
    if enabled {
        ambient.push(grant());
    }
    // Resolve the configured grant independently of service availability so
    // an explicitly declared but unserviceable tool remains distinguishable.
    let mut all_services = ServiceSet::new();
    for descriptor in catalog.descriptors() {
        for service in descriptor.needs {
            all_services.insert(*service);
        }
    }
    let resolved = catalog.resolve_grant(GrantInputs {
        declared,
        step_actions: &[],
        connector_cap: None,
        ambient: &ambient,
        available: &all_services,
        alias_scope: AliasScope::Everywhere,
    })?;
    Ok(AmbientGrant::flow_lead()
        .tools
        .into_iter()
        .map(|name| {
            let d = catalog.descriptor(name).expect("lead catalog tool");
            let service_available = d.needs.iter().all(|s| services.contains(*s));
            let entry = resolved.entries.get(name);
            let granted = entry.is_some();
            let declared = entry.is_some_and(|e| {
                e.reasons
                    .iter()
                    .any(|r| matches!(r, GrantReason::Declared | GrantReason::DeclaredWildcard(_)))
            });
            let grant_reasons = entry
                .map(|e| e.reasons.iter().map(ToString::to_string).collect())
                .unwrap_or_default();
            let permission_allowed = mode != PermissionMode::Readonly
                || !matches!(d.effect, Effect::Write | Effect::External);
            let reason = if d.needs.contains(&Service::RunStatus) {
                Some("Requires an Agentiflow goal/budget envelope; this session has none".into())
            } else if messaging && d.needs.contains(&Service::WorkQueue) {
                Some("Requires an active execution with an exact Queue owner and recording binding; evaluated when the turn starts. The runtime grant is authoritative".into())
            } else if !service_available {
                Some("Messaging is disabled for this session".into())
            } else if !granted {
                Some("Not granted by session tools or orchestration configuration".into())
            } else if !permission_allowed {
                Some("Denied by the session's readonly permission mode".into())
            } else {
                None
            };
            ToolAvailability {
                name,
                available: granted && service_available && permission_allowed,
                reason,
                declared,
                granted,
                grant_reasons,
                service_available,
                permission_allowed,
                effect: d.effect,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_configuration_preserves_declaration_without_inventing_runtime_availability() {
        let declared = vec!["board.work_post".into()];
        let tools = inspect(false, Some(&declared), true, PermissionMode::Readonly).unwrap();
        let tool = tools.iter().find(|t| t.name == "board.work_post").unwrap();
        assert!(tool.declared && tool.granted && tool.permission_allowed);
        assert!(!tool.available && !tool.service_available);
        assert_eq!(tool.effect, Effect::Record);
        assert!(tool.reason.as_deref().unwrap().contains("evaluated when the turn starts"));
        let off = inspect(false, Some(&declared), false, PermissionMode::Readonly).unwrap();
        assert_eq!(off.iter().find(|t| t.name == "board.work_post").unwrap().reason.as_deref(), Some("Messaging is disabled for this session"));
    }
    #[test]
    fn config_revision_is_separate_and_strict() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("settings.json"), b"unchanged").unwrap();
        let c = SessionOrchestration {
            revision: 0,
            enabled: true,
        };
        assert_eq!(c.save(d.path()).unwrap().revision, 1);
        assert!(c.save(d.path()).is_err());
        assert!(SessionOrchestration::read(d.path()).unwrap().enabled);
        assert_eq!(
            fs::read(d.path().join("settings.json")).unwrap(),
            b"unchanged"
        );
        assert!(serde_json::from_str::<SessionOrchestration>(r#"{"enabled":true}"#).is_err());
        assert!(serde_json::from_str::<SessionOrchestration>(
            r#"{"enabled":true,"revision":0,"flow_id":"other"}"#
        )
        .is_err());
    }
    #[test]
    fn availability_distinguishes_services_grants_and_permissions() {
        let tools = inspect(true, Some(&[]), false, PermissionMode::Readonly).unwrap();
        assert!(
            tools
                .iter()
                .find(|t| t.name == "dispatch")
                .unwrap()
                .available
        );
        assert!(
            !tools
                .iter()
                .find(|t| t.name == "board.directive")
                .unwrap()
                .available
        );
        assert!(
            !tools
                .iter()
                .find(|t| t.name == "goal.status")
                .unwrap()
                .service_available
        );
        let explicit = inspect(
            false,
            Some(&["goal.status".into()]),
            true,
            PermissionMode::Readonly,
        )
        .unwrap();
        let goal = explicit.iter().find(|t| t.name == "goal.status").unwrap();
        assert!(goal.declared && goal.granted && !goal.service_available && !goal.available);
        let disabled = inspect(false, Some(&[]), true, PermissionMode::Bypass).unwrap();
        assert!(
            !disabled
                .iter()
                .find(|t| t.name == "dispatch")
                .unwrap()
                .granted
        );
    }
}
