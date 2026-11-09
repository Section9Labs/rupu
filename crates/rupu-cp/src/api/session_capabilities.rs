//! Read-only skill plans and evidence from an already assembled session turn.
//! These routes never assemble a run, resolve credentials, or start work.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{rejection::QueryRejection, Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_runtime::{
    session_orchestration::SessionOrchestration,
    session_readiness::RunReadiness,
    session_settings::SessionSettings,
    session_skill_preset::{
        self, FrozenSkillReadiness, PresetMetadata, SkillDiagnostic, SkillPlan,
    },
    sessions::{parse_session_record, SessionRecord},
};
use rupu_skills::LayerDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

pub(super) const FEATURE: &str = "session.capabilities.v1";

#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectQuery {
    /// Only the directly connected local instance is supported.
    host: Option<String>,
    /// Use rupu-session-v1 (default), rupu-session-full-v1, or none.
    skill_preset: Option<String>,
}

#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionQuery {
    /// Only the directly connected local instance is supported.
    host: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProjectCapabilities {
    schema_version: u32,
    host_id: &'static str,
    workspace_id: String,
    preset: PresetMetadata,
    skills: SkillPlan,
    declared_tools: Option<Vec<String>>,
    /// Creation-time declaration provenance; not an effective grant.
    builtin_tools_preset: Option<String>,
    scoped_reads_version: Option<u32>,
    permission_mode: &'static str,
    applies: &'static str,
    effective: Option<RunReadiness>,
    limitations: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct SessionCapabilities {
    tool_ceiling: Option<Vec<String>>,
    schema_version: u32,
    session_id: String,
    host_id: &'static str,
    workspace_id: String,
    archived: bool,
    preset: Option<PresetMetadata>,
    preset_diagnostics: Vec<SkillDiagnostic>,
    declared_tools: Option<Vec<String>>,
    /// Creation-time declaration provenance; not an effective grant.
    builtin_tools_preset: Option<String>,
    scoped_reads_version: Option<u32>,
    settings_revision: u64,
    orchestration_revision: u64,
    permission_mode: String,
    skills: FrozenSkillReadiness,
    effective: Option<RunReadiness>,
    effective_settings_revision: Option<u64>,
    effective_orchestration_revision: Option<u64>,
    effective_current: bool,
    effective_notice: Option<String>,
}

/// The worker writes this envelope after successful assembly. Every field is
/// required so a legacy, partial, or foreign file cannot imply availability.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCapabilities {
    schema_version: u32,
    session_id: String,
    workspace_id: String,
    settings_revision: u64,
    orchestration_revision: u64,
    effective: RunReadiness,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/:id/session-capabilities", get(get_project))
        .route("/api/sessions/:id/capabilities", get(get_session))
}

async fn get_project(
    State(s): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<ProjectQuery>, QueryRejection>,
) -> ApiResult<Json<ProjectCapabilities>> {
    let Query(query) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    super::runs::blocking(move || project_response(&s, &id, query).map(Json)).await
}

async fn get_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<SessionQuery>, QueryRejection>,
) -> ApiResult<Json<SessionCapabilities>> {
    let Query(query) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    super::runs::blocking(move || session_response(&s, &id, query).map(Json)).await
}

fn project_response(s: &AppState, id: &str, query: ProjectQuery) -> ApiResult<ProjectCapabilities> {
    super::fleet::require_local(query.host.as_deref(), "session capability planning")?;
    super::runs::validate_id(id)?;
    if query
        .skill_preset
        .as_deref()
        .is_some_and(|p| !session_skill_preset::valid_preset(p))
    {
        return Err(ApiError::bad_request(
            "skill_preset must be rupu-session-v1, rupu-session-full-v1 or none",
        ));
    }
    let workspace = super::projects::load_workspace(s, id)?;
    if workspace.id != id {
        return Err(ApiError::conflict(
            "project identity does not match its registered identifier",
        ));
    }
    let root = PathBuf::from(&workspace.path)
        .canonicalize()
        .map_err(|_| ApiError::conflict("registered project directory is missing"))?;
    if !root.is_dir() {
        return Err(ApiError::conflict(
            "registered project path must name a directory",
        ));
    }
    let dirs = LayerDirs::for_layers(&s.global_dir, Some(&root.join(".rupu")));
    let skills = session_skill_preset::plan_for(&dirs, query.skill_preset.as_deref(), &[], &[])
        .map_err(|e| ApiError::conflict(format!("cannot plan session skills: {e}")))?;
    Ok(ProjectCapabilities {
        schema_version: 1,
        host_id: "local",
        workspace_id: workspace.id,
        preset: skills.preset.clone(),
        skills,
        declared_tools: rupu_agent::builtin::session_agent().tools,
        scoped_reads_version: s.session_starter.as_ref().filter(|starter| starter.supports_session_scoped_reads()).map(|_| 1),
        builtin_tools_preset: s.session_starter.as_ref()
            .filter(|starter| starter.supports_session_scoped_reads())
            .map(|_| rupu_agent::builtin::SESSION_TOOLS_PRESET.to_owned()),
        permission_mode: "readonly",
        applies: "new_session",
        effective: None,
        limitations: vec![
            "This plan selects installed skills for a new session; it does not install skills or grant tools.",
            "Declared tools are not evidence of an assembled grant, constructed services, credentials, or target access.",
            "Effective capability evidence is recorded only after a session turn is successfully assembled.",
        ],
    })
}

/// Read one exact directory, including its own settings and snapshot. Refuse
/// duplicate active/archive identities instead of silently choosing a scope.
fn load_session(s: &AppState, id: &str) -> ApiResult<(PathBuf, bool, SessionRecord)> {
    super::runs::validate_id(id)?;
    let mut found = None;
    for (parent, archived) in [("sessions", false), ("sessions-archive", true)] {
        let dir = s.global_dir.join(parent).join(id);
        let bytes = match fs::read(dir.join("session.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(ApiError::internal(format!(
                    "cannot read session record: {e}"
                )))
            }
        };
        if found.is_some() {
            return Err(ApiError::conflict(
                "session exists in both active and archived storage",
            ));
        }
        let record = parse_session_record(&bytes)
            .map_err(|e| ApiError::conflict(format!("cannot inspect session record: {e}")))?;
        if record.version == 0 || record.session_id != id {
            return Err(ApiError::conflict(
                "session identity or version does not match its stored record",
            ));
        }
        found = Some((dir, archived, record));
    }
    found.ok_or_else(|| ApiError::not_found(format!("session {id} not found")))
}

fn session_response(s: &AppState, id: &str, query: SessionQuery) -> ApiResult<SessionCapabilities> {
    super::fleet::require_local(query.host.as_deref(), "session capability inspection")?;
    let (dir, archived, record) = load_session(s, id)?;
    let settings = SessionSettings::read(&dir)
        .map_err(|e| ApiError::conflict(format!("cannot read session settings: {e}")))?;
    settings
        .validate()
        .map_err(|e| ApiError::conflict(format!("invalid session settings: {e}")))?;
    let orchestration = SessionOrchestration::read(&dir)
        .map_err(|e| ApiError::conflict(format!("cannot read session orchestration: {e}")))?;
    let mode = rupu_tools::PermissionMode::parse(&record.permission_mode)
        .map_err(|e| ApiError::conflict(format!("invalid session permission mode: {e}")))?;
    let dirs = LayerDirs::for_layers(
        &s.global_dir,
        record
            .project_root
            .as_ref()
            .map(|p| p.join(".rupu"))
            .as_deref(),
    );
    let skills = session_skill_preset::frozen(&dirs, &record.skill_index);
    let snapshot = match fs::read(dir.join("capabilities.json")) {
        Ok(bytes) => {
            let snapshot: StoredCapabilities = serde_json::from_slice(&bytes).map_err(|e| {
                ApiError::conflict(format!("invalid stored session capabilities: {e}"))
            })?;
            if snapshot.schema_version != 1
                || snapshot.effective.schema_version != 1
                || snapshot.effective.applies != "assembled_turn"
                || snapshot.session_id != record.session_id
                || snapshot.workspace_id != record.workspace_id
                || snapshot.effective.run_id.is_empty()
                || !record
                    .runs
                    .iter()
                    .any(|run| run.run_id == snapshot.effective.run_id)
            {
                return Err(ApiError::conflict(
                    "stored session capabilities have an unsupported schema or do not belong to this session, project, and recorded turn",
                ));
            }
            Some(snapshot)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(ApiError::internal(format!(
                "cannot read stored session capabilities: {e}"
            )));
        }
    };
    let current_run = record
        .active_run_id
        .as_deref()
        .or(record.last_run_id.as_deref());
    let effective_current = snapshot.as_ref().is_some_and(|snapshot| {
        current_run == Some(snapshot.effective.run_id.as_str())
            && settings.revision == snapshot.settings_revision
            && orchestration.revision == snapshot.orchestration_revision
            && record.tool_ceiling == snapshot.effective.tool_ceiling
    });
    let notice = match &snapshot {
        None => "No assembled-turn capability snapshot has been recorded for this session; effective tools and services are unobserved.",
        Some(_) if !effective_current => "This is a historical assembled-turn snapshot. Its run or settings revisions differ from the current session; it does not describe the next turn.",
        Some(_) => "This snapshot describes the current recorded turn and revisions. It does not predict the next turn or verify credentials and target access.",
    };
    Ok(SessionCapabilities {
        tool_ceiling: record.tool_ceiling,
        schema_version: 1,
        session_id: record.session_id,
        host_id: "local",
        workspace_id: record.workspace_id,
        archived,
        preset: record.skill_preset,
        preset_diagnostics: record.skill_preset_diagnostics,
        declared_tools: settings.tools.or(record.agent_tools),
        builtin_tools_preset: record.builtin_tools_preset,
        scoped_reads_version: record.scoped_reads_version,
        settings_revision: settings.revision,
        orchestration_revision: orchestration.revision,
        permission_mode: mode.as_str().to_owned(),
        skills,
        effective_settings_revision: snapshot.as_ref().map(|s| s.settings_revision),
        effective_orchestration_revision: snapshot.as_ref().map(|s| s.orchestration_revision),
        effective_current,
        effective: snapshot.map(|s| s.effective),
        effective_notice: Some(notice.to_owned()),
    })
}

#[cfg(test)]
#[path = "session_capabilities_tests.rs"]
mod tests;
