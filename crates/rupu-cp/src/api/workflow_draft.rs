//! Explicit draft execution: the saved definition is only a guarded baseline.
//! Preview is read-only; launch repeats every check before pinning actual bytes.
use super::*;
use std::{
    io::{Read, Write},
    path::PathBuf,
};

const SOURCE_LIMIT: usize = 256 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    workspace_id: String,
    session_id: String,
    source: Baseline,
    draft_revision: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    name: String,
    scope_kind: ScopeKind,
    scope_id: Option<String>,
    revision: String,
}
struct Prepared {
    baseline_lock: Option<std::fs::File>,
    workflow: Workflow,
    working_dir: PathBuf,
    source: super::super::session_workflows::Source,
    mode: String,
    profiles: Vec<String>,
}

async fn prepare(s: &AppState, body: &LaunchYamlBody, lock_baseline: bool) -> ApiResult<Prepared> {
    let binding = body
        .draft
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("draft binding is required"))?;
    if body.host.as_deref().unwrap_or("local") != "local" {
        return Err(ApiError::not_available(
            "draft workflows require the owning local control plane",
        ));
    }
    if body.working_dir.is_some() {
        return Err(ApiError::bad_request(
            "draft workspace determines its working directory",
        ));
    }
    if body.yaml.len() > SOURCE_LIMIT {
        return Err(ApiError::bad_request("draft workflow exceeds 256 KiB"));
    }
    if revision(&body.yaml) != binding.draft_revision {
        return Err(ApiError::conflict(
            "draft source revision does not match its exact bytes",
        ));
    }
    let mode = body.mode.as_deref().unwrap_or("readonly");
    if !matches!(mode, "readonly" | "bypass") {
        return Err(ApiError::bad_request(
            "draft workflows require readonly or explicit bypass; Ask is unavailable",
        ));
    }
    fs_safety::validate_stem(&binding.source.name)?;
    super::super::runs::validate_id(&binding.workspace_id)?;
    if binding.source.scope_kind == ScopeKind::Project
        && binding.source.scope_id.as_ref() != Some(&binding.workspace_id)
    {
        return Err(ApiError::bad_request(
            "a project draft must execute in its owning workspace",
        ));
    }
    let workspace = store(s)
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .find(|w| w.id == binding.workspace_id)
        .ok_or_else(|| ApiError::not_found("execution workspace not found"))?;
    let working_dir = PathBuf::from(workspace.path)
        .canonicalize()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if !working_dir.is_dir() {
        return Err(ApiError::bad_request(
            "execution workspace is not a directory",
        ));
    }
    super::super::session_workflows::validate_attachment(
        s,
        &binding.session_id,
        &binding.workspace_id,
        &working_dir,
    )?;
    let selected = selected_definition(
        s,
        &binding.source.name,
        &ScopeQuery {
            scope_kind: Some(binding.source.scope_kind),
            scope_id: binding.source.scope_id.clone(),
        },
    )?;
    // Preview must not create a lock file. One opened baseline FD supplies its
    // exact bytes; launch separately repeats this check under the writer lock.
    let baseline_lock = if lock_baseline {
        Some(definition_lock(&selected.1).await?)
    } else {
        None
    };
    fs_safety::validate_within(&selected.0, &selected.1)?;
    let mut raw = String::new();
    std::fs::File::open(&selected.0)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .take((SOURCE_LIMIT + 1) as u64)
        .read_to_string(&mut raw)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if raw.len() > SOURCE_LIMIT {
        return Err(ApiError::bad_request("saved workflow exceeds 256 KiB"));
    }
    if revision(&raw) != binding.source.revision {
        return Err(ApiError::conflict(
            "saved baseline changed; reload before running the draft",
        ));
    }
    let baseline = Workflow::parse(&raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let workflow = Workflow::parse(&body.yaml).map_err(|e| ApiError::bad_request(e.to_string()))?;
    if baseline.name != binding.source.name || workflow.name != binding.source.name {
        return Err(ApiError::bad_request(
            "draft and saved workflow names must match the selected definition",
        ));
    }
    // Refuse placed units for every draft, not just built-in sessions: this
    // capability does not promise remote propagation of reviewed launch policy.
    if workflow.has_placed_step() {
        return Err(ApiError::not_available(
            "draft session workflows require local execution",
        ));
    }
    let profiles = crate::api::agents::parse_engagement_profiles(body.engagement_profiles.clone())?;
    Ok(Prepared {
        baseline_lock,
        workflow,
        working_dir,
        mode: mode.into(),
        profiles,
        source: super::super::session_workflows::Source {
            scope_kind: match binding.source.scope_kind {
                ScopeKind::Global => "global",
                ScopeKind::Project => "project",
            }
            .into(),
            scope_id: binding.source.scope_id.clone(),
            revision: binding.source.revision.clone(),
            draft_revision: Some(binding.draft_revision.clone()),
        },
    })
}

pub(super) async fn preview(
    State(s): State<AppState>,
    Json(body): Json<LaunchYamlBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let prepared = prepare(&s, &body, false).await?;
    let binding = body.draft.as_ref().unwrap();
    Ok(Json(serde_json::json!({
        "host_id":"local", "workspace_id":binding.workspace_id, "session_id":binding.session_id,
        "source":prepared.source, "workflow":prepared.workflow,
    })))
}

pub(super) async fn launch(
    s: AppState,
    body: LaunchYamlBody,
) -> ApiResult<Json<serde_json::Value>> {
    let launcher = s
        .launcher
        .as_ref()
        .filter(|launcher| launcher.supports_session_workflows())
        .ok_or_else(|| {
            ApiError::not_available(
                "this control plane cannot launch session-linked workflow drafts",
            )
        })?;
    let prepared = prepare(&s, &body, true).await?;
    let binding = body.draft.as_ref().unwrap();
    let dir = yaml_launch_dir(&s);
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let path = dir.join(format!("{}.yaml", ulid::Ulid::new()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    file.write_all(body.yaml.as_bytes())
        .and_then(|_| file.sync_all())
        .and_then(|_| std::fs::File::open(&dir)?.sync_all())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    // Release the saved-definition writer lock only after the actual draft
    // snapshot is durable. The session is rechecked by launch below.
    drop(prepared.baseline_lock);
    let source = prepared.source.clone();
    let association = super::super::session_workflows::launch(
        &s,
        launcher.as_ref(),
        &binding.session_id,
        &binding.workspace_id,
        prepared.source,
        crate::launcher::LaunchRequest {
            workflow: prepared.workflow.name,
            inputs: body.inputs,
            mode: Some(prepared.mode),
            target: None,
            working_dir: Some(prepared.working_dir.display().to_string()),
            engagement_profiles: prepared.profiles,
            file: Some(path.clone()),
        },
    )
    .await?;
    Ok(Json(serde_json::json!({
        "run_id":association.run_id, "host_id":"local", "workspace_id":binding.workspace_id,
        "session_id":binding.session_id, "source":source, "association":association,
        "file":path.display().to_string(),
    })))
}
