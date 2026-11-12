//! Derive repository-write authority from this CP's exact registered checkout.
//! No shell, git network operation, workspace-name inference or raw remote echo.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use rupu_scm::forge_write::ForgeOwner;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, LazyLock},
};
static OWNERS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

pub(super) async fn resolve(s: &AppState, ws_id: &str) -> ApiResult<ForgeOwner> {
    if ws_id.is_empty()
        || ws_id.len() > 128
        || !ws_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(ApiError::bad_request(
            "An exact registered project ID is required",
        ));
    }
    let permit = OWNERS.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Checkout authority readers are busy".into(),
        )
    })?;
    let global = s.global_dir.clone();
    let id = ws_id.to_owned();
    let job = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let workspace = rupu_workspace::WorkspaceStore {
            root: global.join("workspaces"),
        }
        .load(&id)
        .map_err(|_| ApiError::internal("Project registration is unavailable"))?
        .ok_or_else(|| ApiError::not_found("Registered project was not found"))?;
        let canonical = Path::new(&workspace.path)
            .canonicalize()
            .map_err(|_| ApiError::conflict("Registered checkout is unavailable"))?;
        let root = canonical
            .to_str()
            .filter(|p| p.len() <= 4096)
            .ok_or_else(|| {
                ApiError::conflict("Registered checkout path cannot be represented safely")
            })?;
        let repo = git2::Repository::open(&canonical)
            .map_err(|_| ApiError::conflict("Registered project is not an available checkout"))?;
        if repo.is_bare()
            || repo.workdir().and_then(|p| p.canonicalize().ok()).as_ref() != Some(&canonical)
        {
            return Err(ApiError::conflict(
                "Registered project must be the exact checkout root",
            ));
        }
        let reference = repo
            .head()
            .map_err(|_| ApiError::conflict("Checkout needs an existing branch and commit"))?;
        if !reference.is_branch() {
            return Err(ApiError::conflict(
                "Detached checkout cannot prepare a repository write",
            ));
        }
        let branch = reference
            .shorthand()
            .filter(|s| !s.is_empty() && s.len() <= 1024)
            .ok_or_else(|| ApiError::conflict("Checkout branch is unavailable"))?
            .to_owned();
        let head = reference
            .target()
            .ok_or_else(|| ApiError::conflict("Checkout commit is unavailable"))?
            .to_string();
        let config = rupu_config::resolve(rupu_config::LayerPaths::global_only(
            &global.join("config.toml"),
        ))
        .map_err(|_| ApiError::conflict("Owning forge configuration is unavailable"))?;
        let hosts = rupu_scm::weburl::WebHosts::from_scm(&config.config.scm);
        let remote = repo
            .find_remote("origin")
            .map_err(|_| ApiError::conflict("Checkout origin is unavailable"))?;
        let web = remote
            .url()
            .and_then(|url| rupu_scm::weburl::parse_repo_remote_with(url, &hosts))
            .ok_or_else(|| {
                ApiError::conflict("Checkout origin is not a configured supported forge")
            })?;
        let origin_url_identity = web.home_url();
        let parsed = url::Url::parse(&origin_url_identity)
            .map_err(|_| ApiError::conflict("Checkout origin identity is invalid"))?;
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(ApiError::conflict("Checkout origin identity is unsafe"));
        }
        let origin_host = parsed
            .host_str()
            .ok_or_else(|| ApiError::conflict("Checkout origin host is unavailable"))?
            .to_owned();
        let checkout_identity = physical_identity(&canonical, repo.path())?;
        Ok(ForgeOwner {
            ws_id: id,
            canonical_root: root.into(),
            repo: rupu_scm::RepoRef {
                platform: web.platform,
                owner: web.owner,
                repo: web.repo,
            },
            origin_host,
            origin_url_identity,
            checkout_identity,
            branch,
            head,
        })
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), job)
        .await
        .map_err(|_| ApiError::not_available("Checkout authority read exceeded its deadline"))?
        .map_err(|_| ApiError::internal("Checkout authority read failed"))?
}

#[cfg(unix)]
fn physical_identity(root: &Path, git_dir: &Path) -> ApiResult<String> {
    use std::os::unix::fs::MetadataExt;
    let root_meta = root
        .metadata()
        .map_err(|_| ApiError::conflict("Checkout identity is unavailable"))?;
    let git_dir = git_dir
        .canonicalize()
        .map_err(|_| ApiError::conflict("Git directory identity is unavailable"))?;
    let git_meta = git_dir
        .metadata()
        .map_err(|_| ApiError::conflict("Git directory identity is unavailable"))?;
    let mut hash = Sha256::new();
    hash.update(root.as_os_str().as_encoded_bytes());
    hash.update([0]);
    hash.update(git_dir.as_os_str().as_encoded_bytes());
    for n in [
        root_meta.dev(),
        root_meta.ino(),
        git_meta.dev(),
        git_meta.ino(),
    ] {
        hash.update(n.to_be_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}
#[cfg(not(unix))]
fn physical_identity(_root: &Path, _git_dir: &Path) -> ApiResult<String> {
    Err(ApiError::not_available(
        "Guarded repository writes require Unix checkout identity support",
    ))
}
