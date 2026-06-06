//! Reconstruct a replayable invocation from a `RunManifest`, gated by
//! surface. v1 supports the agent surface; other surfaces return an
//! explicit error (never a silent no-op).

use crate::ledger::events::Surface;
use crate::ledger::manifest::RunManifest;

/// The validated subset of a manifest needed to replay an agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunInvocation {
    pub agent_name: String,
    pub user_prompt: String,
    pub permission_mode: String,
    pub workspace_path: std::path::PathBuf,
}

/// Why a run can't be replayed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RerunError {
    #[error("rerun of {0} runs not yet supported")]
    UnsupportedSurface(String),
}

/// Validate the manifest's surface and reconstruct the replay invocation.
/// v1: only `Surface::Agent` is dispatchable.
///
/// NOTE: v1 agent replay dispatches through `rupu run <agent>`, which
/// re-resolves `provider` / `model` / `concerns` from the agent's CURRENT
/// frontmatter + config — it does NOT apply the manifest's recorded values
/// for those. Only `agent_name` / `user_prompt` / `permission_mode` /
/// `workspace_path` drive the replay. The other manifest fields are a record
/// of the original run (and the basis for a future higher-fidelity replay
/// that pins them); a session/workflow rerun author must not assume they are
/// authoritative for dispatch.
pub fn plan_rerun(manifest: &RunManifest) -> Result<RerunInvocation, RerunError> {
    match manifest.surface {
        Surface::Agent => Ok(RerunInvocation {
            agent_name: manifest.agent_name.clone(),
            user_prompt: manifest.user_prompt.clone(),
            permission_mode: manifest.permission_mode.clone(),
            workspace_path: manifest.workspace_path.clone(),
        }),
        Surface::Session => Err(RerunError::UnsupportedSurface("session".to_string())),
        Surface::Workflow => Err(RerunError::UnsupportedSurface("workflow".to_string())),
        Surface::Autoflow => Err(RerunError::UnsupportedSurface("autoflow".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};
    use chrono::{DateTime, Utc};

    fn manifest_with_surface(surface: Surface) -> RunManifest {
        RunManifest {
            run_id: "run_a".to_string(),
            started_at: DateTime::<Utc>::from_timestamp(1, 0).unwrap(),
            surface,
            agent_name: "reviewer".to_string(),
            provider: "anthropic".to_string(),
            model: "m".to_string(),
            permission_mode: "bypass".to_string(),
            user_prompt: "Review.".to_string(),
            concerns: ConcernsBlock {
                entries: vec![ConcernsEntry::Include(IncludeDirective {
                    include: "stride".to_string(),
                    overrides: vec![],
                    mode: crate::catalog::types::CatalogMode::Auto,
                    filter: None,
                })],
            },
            scope_name: "reviewer".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp/repo"),
        }
    }

    #[test]
    fn agent_surface_reconstructs_invocation() {
        let inv = plan_rerun(&manifest_with_surface(Surface::Agent)).unwrap();
        assert_eq!(inv.agent_name, "reviewer");
        assert_eq!(inv.user_prompt, "Review.");
        assert_eq!(inv.permission_mode, "bypass");
        assert_eq!(inv.workspace_path, std::path::PathBuf::from("/tmp/repo"));
    }

    #[test]
    fn session_surface_is_unsupported() {
        let err = plan_rerun(&manifest_with_surface(Surface::Session)).unwrap_err();
        assert_eq!(err, RerunError::UnsupportedSurface("session".to_string()));
        assert_eq!(err.to_string(), "rerun of session runs not yet supported");
    }

    #[test]
    fn workflow_and_autoflow_are_unsupported() {
        assert!(plan_rerun(&manifest_with_surface(Surface::Workflow)).is_err());
        assert!(plan_rerun(&manifest_with_surface(Surface::Autoflow)).is_err());
    }
}
