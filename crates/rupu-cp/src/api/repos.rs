use crate::{
    error::{ApiError, ApiResult},
    repos::{RepoEntry, RepoLister},
    state::AppState,
};
use axum::{routing::get, Json, Router};
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/repos", get(list_repos))
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
