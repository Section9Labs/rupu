//! Opt-in exact-source deletion; legacy mutation requests retain their behavior.
use super::*;
use crate::transcript_mutator::{
    ExactTranscriptContext, ExactTranscriptLimits, ExactTranscriptScope, TranscriptMutateError,
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContextQuery {
    host: Option<String>,
    ws_id: String,
    path: String,
}
pub(crate) fn available(s: &AppState) -> bool {
    cfg!(unix)
        && s.transcript_mutator
            .as_ref()
            .is_some_and(|m| m.supports_exact_delete())
}
fn failure(e: TranscriptMutateError) -> ApiError {
    match e {
        TranscriptMutateError::NotFound(message) => ApiError::not_found(message),
        TranscriptMutateError::Invalid(message) => ApiError::conflict(message),
        TranscriptMutateError::Unsupported(message) => ApiError::not_available(message),
        TranscriptMutateError::Failed { message, .. } => ApiError::internal(message),
    }
}
fn scope(
    id: &str,
    workspace: &str,
    path: &str,
    host: Option<&str>,
) -> ApiResult<ExactTranscriptScope> {
    super::super::fleet::require_local(host, "exact transcript deletion")?;
    super::super::runs::validate_id(id)?;
    super::super::runs::validate_id(workspace)?;
    if id.len() > 128
        || workspace.len() > 128
        || path.is_empty()
        || path.len() > 4096
        || !std::path::Path::new(path).is_absolute()
        || path.contains('\0')
        || std::path::Path::new(path)
            .file_name()
            .and_then(|v| v.to_str())
            != Some(format!("{id}.jsonl").as_str())
        || std::path::Path::new(path).components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(ApiError::bad_request("invalid exact transcript source"));
    }
    Ok(ExactTranscriptScope {
        workspace_id: workspace.into(),
        path: path.into(),
    })
}
fn adapter(
    s: &AppState,
) -> ApiResult<std::sync::Arc<dyn crate::transcript_mutator::TranscriptMutator>> {
    if !available(s) {
        return Err(ApiError::not_available(
            "exact transcript deletion requires the owning supported CLI adapter",
        ));
    }
    s.transcript_mutator
        .clone()
        .ok_or_else(|| ApiError::not_available("exact transcript adapter unavailable"))
}
fn revision_valid(v: &str) -> bool {
    v.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn check_context(
    c: &ExactTranscriptContext,
    id: &str,
    scope: &ExactTranscriptScope,
) -> ApiResult<()> {
    let limits = ExactTranscriptLimits::default();
    if c.schema != EXACT_DELETE_FEATURE
        || c.id != id
        || c.workspace_id != scope.workspace_id
        || c.path != scope.path
        || c.host_id != "local"
        || c.coordination != "advisory_retention_lease_metadata_revision"
        || c.effects.len() > limits.effect_entries
        || c.limits.metadata_bytes != limits.metadata_bytes
        || c.limits.prologue_bytes != limits.prologue_bytes
        || c.limits.effect_entries != limits.effect_entries
        || c.limits.context_bytes != limits.context_bytes
        || chrono::DateTime::parse_from_rfc3339(&c.observed_at).is_err()
        || serde_json::to_vec(c)
            .map_err(|_| ApiError::internal("invalid exact transcript context"))?
            .len()
            > limits.context_bytes
    {
        return Err(ApiError::internal(
            "exact transcript context identity or bounds differ",
        ));
    }
    if c.source_kind.as_deref().is_some_and(|k| {
        !matches!(
            k,
            "global_active" | "global_archive" | "project_active" | "project_archive"
        )
    }) {
        return Err(ApiError::internal("invalid exact transcript source kind"));
    }
    if c.state == "ready" {
        let metadata = std::path::Path::new(&scope.path).with_file_name(format!("{id}.meta.json"));
        let mut transcript = false;
        let mut sidecar = false;
        for effect in &c.effects {
            if effect.bytes.is_empty()
                || !effect.bytes.bytes().all(|b| b.is_ascii_digit())
                || effect.bytes.parse::<u64>().is_err()
            {
                return Err(ApiError::internal("invalid exact transcript effect size"));
            }
            match effect.kind.as_str() {
                "transcript" if !transcript && effect.path == scope.path => transcript = true,
                "metadata" if !sidecar && std::path::Path::new(&effect.path) == metadata => {
                    sidecar = true
                }
                _ => {
                    return Err(ApiError::internal(
                        "exact transcript effects differ from selected source",
                    ))
                }
            }
        }
        if !transcript || !sidecar {
            return Err(ApiError::internal("exact transcript effects incomplete"));
        }
    }
    match c.state.as_str() {
        "ready"
            if c.reason.is_none()
                && c.revision.as_deref().is_some_and(revision_valid)
                && c.effects.len() == 2
                && c.source_kind.is_some()
                && c.archived
                    == c.source_kind
                        .as_deref()
                        .map(|kind| kind.ends_with("_archive")) =>
        {
            ()
        }
        "unavailable"
            if c.reason
                .as_ref()
                .is_some_and(|r| !r.is_empty() && r.len() <= 1024)
                && c.revision.is_none()
                && c.effects.is_empty() =>
        {
            ()
        }
        _ => return Err(ApiError::internal("invalid exact transcript context state")),
    }
    Ok(())
}
pub(super) async fn context(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ContextQuery>,
) -> ApiResult<Json<ExactTranscriptContext>> {
    let scope = scope(&id, &q.ws_id, &q.path, q.host.as_deref())?;
    let context = adapter(&s)?
        .exact_delete_context(&id, &scope)
        .await
        .map_err(failure)?;
    check_context(&context, &id, &scope)?;
    Ok(Json(context))
}
pub(super) async fn delete(
    s: &AppState,
    id: &str,
    q: TranscriptHostQuery,
) -> ApiResult<Json<serde_json::Value>> {
    if q.ignore_liveness {
        return Err(ApiError::bad_request(
            "exact transcript deletion forbids ignore_liveness",
        ));
    }
    let (Some(workspace), Some(path), Some(revision)) =
        (q.ws_id, q.expected_path, q.expected_revision)
    else {
        return Err(ApiError::bad_request(
            "ws_id, expected_path and expected_revision must be supplied together",
        ));
    };
    if !revision_valid(&revision) {
        return Err(ApiError::bad_request(
            "invalid expected transcript revision",
        ));
    }
    let scope = scope(id, &workspace, &path, q.host.as_deref())?;
    adapter(s)?
        .delete_exact(id, &scope, &revision)
        .await
        .map_err(failure)?;
    Ok(Json(
        serde_json::json!({"ok":true,"id":id,"workspace_id":workspace,"host_id":"local","path":path,"revision":revision}),
    ))
}

#[cfg(all(test, unix))]
#[path = "transcripts_exact_tests.rs"]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuardedDeleteQuery {
    host: Option<String>,
    ws_id: String,
    expected_path: String,
    expected_revision: String,
}
pub(super) async fn guarded_delete(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<GuardedDeleteQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    delete(
        &s,
        &id,
        TranscriptHostQuery {
            host: q.host,
            ignore_liveness: false,
            ws_id: Some(q.ws_id),
            expected_path: Some(q.expected_path),
            expected_revision: Some(q.expected_revision),
        },
    )
    .await
}
