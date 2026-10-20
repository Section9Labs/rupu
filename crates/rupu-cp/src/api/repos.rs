//! `GET /api/repos` (the SCM platforms' repo listing, through the
//! `RepoLister` port) and the local repo registry `rupu repos
//! attach/prefer/tracked/forget` manages:
//!
//! - `GET /api/repos/tracked`: the tracked checkouts.
//! - `POST /api/repos/tracked`: attach a checkout to a repo.
//! - `POST /api/repos/tracked/prefer`: attach it and make it the preferred path.
//! - `DELETE /api/repos/tracked/:id[?path=]`: forget a repo, or one of its paths.
//!
//! The registry endpoints are local-only. A row never carries the
//! checkout's origin urls, which can embed a credential.

use crate::{
    api::config::require_writable_to,
    api::scm::{local_only, LocalHostQuery},
    error::{ApiError, ApiResult},
    repos::{RepoEntry, RepoLister},
    state::AppState,
};
use axum::{
    extract::{Path as AxPath, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use rupu_workspace::{
    repo_ref_key, RepoRegistryStore, RepoStoreError, TrackCheckoutError, TrackedRepo,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/repos", get(list_repos))
        .route("/api/repos/tracked", get(list_tracked).post(attach))
        .route("/api/repos/tracked/prefer", post(prefer))
        .route("/api/repos/tracked/:id", delete(forget))
}

/// Core, testable without axum State: returns the port's entries or 501.
async fn list_repos_with(port: Option<Arc<dyn RepoLister>>) -> ApiResult<Vec<RepoEntry>> {
    let port = port.ok_or_else(|| {
        ApiError::not_available("repo listing requires `rupu cp serve` with SCM credentials")
    })?;
    port.list()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

async fn list_repos(
    axum::extract::State(s): axum::extract::State<AppState>,
) -> ApiResult<Json<Vec<RepoEntry>>> {
    Ok(Json(list_repos_with(s.repos.clone()).await?))
}

/// A repo-registry failure as a 500. A record that doesn't parse is named
/// by its file only: the TOML error quotes the offending line, which can be
/// an origin url with a token in it.
fn store_error(e: RepoStoreError) -> ApiError {
    match e {
        RepoStoreError::Parse { path, .. } => {
            ApiError::internal(format!("tracked repo record {path} is not valid TOML"))
        }
        other => ApiError::internal(other.to_string()),
    }
}

fn store(s: &AppState) -> RepoRegistryStore {
    RepoRegistryStore {
        root: s.global_dir.join("repos"),
    }
}

/// One tracked repo, as `rupu repos tracked` lists it.
#[derive(Debug, Serialize)]
struct TrackedRepoDto {
    /// The record's key: what `DELETE /api/repos/tracked/:id` takes.
    id: String,
    /// `<platform>:<owner>/<repo>`.
    repo: String,
    preferred_path: String,
    known_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_branch: Option<String>,
    last_seen_at: String,
}

impl From<TrackedRepo> for TrackedRepoDto {
    fn from(r: TrackedRepo) -> Self {
        Self {
            id: repo_ref_key(&r.repo_ref),
            repo: r.repo_ref,
            preferred_path: r.preferred_path,
            known_paths: r.known_paths,
            default_branch: r.default_branch,
            last_seen_at: r.last_seen_at,
        }
    }
}

async fn list_tracked(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
) -> ApiResult<Json<Vec<TrackedRepoDto>>> {
    local_only(q.host.as_deref(), "the tracked-repo registry")?;
    let store = store(&s);
    let repos = tokio::task::spawn_blocking(move || store.list())
        .await
        .map_err(|e| ApiError::internal(format!("list task: {e}")))?
        .map_err(store_error)?;
    Ok(Json(repos.into_iter().map(Into::into).collect()))
}

/// `POST /api/repos/tracked` and `…/prefer`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TrackBody {
    /// The repo in run-target syntax: `github:owner/repo`, `gitlab:group/sub/project`.
    repo: String,
    /// Absolute path of a local git checkout whose `origin` points at `repo`.
    path: String,
}

async fn attach(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
    Json(body): Json<TrackBody>,
) -> ApiResult<Json<TrackedRepoDto>> {
    track(&s, q, body, false).await
}

async fn prefer(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
    Json(body): Json<TrackBody>,
) -> ApiResult<Json<TrackedRepoDto>> {
    track(&s, q, body, true).await
}

async fn track(
    s: &AppState,
    q: LocalHostQuery,
    body: TrackBody,
    prefer: bool,
) -> ApiResult<Json<TrackedRepoDto>> {
    local_only(q.host.as_deref(), "the tracked-repo registry")?;
    require_writable_to(s, "tracking a repo")?;
    let repo = rupu_scm::run_target::parse_run_target(&body.repo)
        .map_err(|e| ApiError::bad_request(e.to_string()))?
        .repo_ref()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let path = PathBuf::from(&body.path);
    if !path.is_absolute() {
        return Err(ApiError::bad_request(format!(
            "`path` must be absolute, got `{}`",
            body.path
        )));
    }
    let store = store(s);
    let rec = tokio::task::spawn_blocking(move || store.track_checkout(&repo, &path, prefer))
        .await
        .map_err(|e| ApiError::internal(format!("track task: {e}")))?
        .map_err(|e| match e {
            TrackCheckoutError::Path { .. }
            | TrackCheckoutError::Checkout(_)
            | TrackCheckoutError::Mismatch { .. } => ApiError::bad_request(e.to_string()),
            TrackCheckoutError::Store(e) => store_error(e),
            TrackCheckoutError::Git { .. } => ApiError::internal(e.to_string()),
        })?;
    Ok(Json(rec.into()))
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct ForgetQuery {
    /// `local` or omitted; any other host is refused with `HTTP 501`.
    host: Option<String>,
    /// Forget only this tracked path; omitted forgets the whole repo.
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct ForgetReply {
    repo: String,
    /// The path forgotten, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    /// The record left, or `null` when the repo is no longer tracked.
    remaining: Option<TrackedRepoDto>,
}

async fn forget(
    State(s): State<AppState>,
    AxPath(id): AxPath<String>,
    Query(q): Query<ForgetQuery>,
) -> ApiResult<Json<ForgetReply>> {
    local_only(q.host.as_deref(), "the tracked-repo registry")?;
    require_writable_to(&s, "forgetting a tracked repo")?;
    let store = store(&s);
    tokio::task::spawn_blocking(move || {
        let internal = store_error;
        let repo_ref = store
            .list()
            .map_err(internal)?
            .into_iter()
            .find(|r| repo_ref_key(&r.repo_ref) == id)
            .map(|r| r.repo_ref)
            .ok_or_else(|| ApiError::not_found(format!("no tracked repo `{id}`")))?;
        match q.path {
            Some(path) => {
                if !store
                    .forget_path(&repo_ref, std::path::Path::new(&path))
                    .map_err(internal)?
                {
                    return Err(ApiError::not_found(format!(
                        "path `{path}` is not tracked for {repo_ref}"
                    )));
                }
                let remaining = store.load(&repo_ref).map_err(internal)?.map(Into::into);
                Ok(Json(ForgetReply {
                    repo: repo_ref,
                    path: Some(path),
                    remaining,
                }))
            }
            None => {
                store.forget_repo(&repo_ref).map_err(internal)?;
                Ok(Json(ForgetReply {
                    repo: repo_ref,
                    path: None,
                    remaining: None,
                }))
            }
        }
    })
    .await
    .map_err(|e| ApiError::internal(format!("forget task: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::{RepoEntry, RepoListError, RepoLister};
    use std::sync::Arc;

    struct MockRepos(Vec<RepoEntry>);
    #[async_trait::async_trait]
    impl RepoLister for MockRepos {
        async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn lists_from_port() {
        let entry = RepoEntry {
            platform: "github".into(),
            account: "github".into(),
            repo: "o/r".into(),
            default_branch: "main".into(),
            private: false,
        };
        let port: Arc<dyn RepoLister> = Arc::new(MockRepos(vec![entry]));
        let out = list_repos_with(Some(port)).await.expect("ok");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].repo, "o/r");
    }

    #[tokio::test]
    async fn missing_port_is_not_available() {
        let err = list_repos_with(None).await.expect_err("no port");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
}
