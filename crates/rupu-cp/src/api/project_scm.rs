//! Read-only SCM observation on the instance that owns a registered project.
//! libgit2 performs no shell, hook, external diff, credential or network calls.
//! Limits bound response size and concurrent work, not repository scan cost.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use base64::Engine;
use git2::{DiffFormat, DiffOptions, Repository, Status, StatusOptions};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::Path as FsPath,
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::sync::Semaphore;

const MAX_CHANGES: usize = 200;
const MAX_DIFF: usize = 256 * 1024;
pub(super) static REVIEW_READERS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));
static READERS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

#[derive(Default, Deserialize, schemars::JsonSchema)]
pub struct ContextQuery {
    /// Owning instance only: omit or use `local`; remote selectors are refused.
    pub host: Option<String>,
    /// Query current branch reviews through the owning instance's SCM account.
    /// False by default; local checkout inspection does not contact the forge.
    #[serde(default)]
    pub include_review: bool,
}

pub async fn context(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
    Query(q): Query<ContextQuery>,
) -> ApiResult<Json<Value>> {
    super::fleet::require_local(q.host.as_deref(), "project SCM context")?;
    let reviews = s.repos.clone();
    // Do not queue an unbounded number of large repository scans. Timed-out
    // scans retain their permit until libgit2 returns; no detached scan storm.
    let permit = READERS.clone().try_acquire_owned().map_err(|_| {
        ApiError::conflict("SCM readers are busy; refresh after the current observation finishes")
    })?;
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let workspace = rupu_workspace::WorkspaceStore {
            root: s.global_dir.join("workspaces"),
        }
        .load(&ws_id)
        .map_err(|_| ApiError::internal("unable to read project registration"))?
        .ok_or_else(|| ApiError::not_found("registered project not found"))?;
        let config_path = s.global_dir.join("config.toml");
        let hosts = rupu_config::resolve(rupu_config::LayerPaths::global_only(&config_path))
            .map(|r| rupu_scm::weburl::WebHosts::from_scm(&r.config.scm))
            .unwrap_or_default();
        observe_with_hosts(&ws_id, FsPath::new(&workspace.path), &hosts)
    });
    let mut value = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .map_err(|_| {
            ApiError(
                axum::http::StatusCode::GATEWAY_TIMEOUT,
                "SCM observation deadline exceeded; no project files were changed".into(),
            )
        })?
        .map_err(|_| ApiError::internal("SCM observation failed"))??;
    if q.include_review {
        value["pull_request"] = review_context(reviews, &value).await;
    }
    Ok(Json(value))
}

/// Every target is captured from a prior checkout/review observation. These
/// fields are preconditions, not authority overrides or write/CAS promises.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct ReviewDetailQuery {
    pub host: Option<String>,
    pub expected_origin: String,
    pub expected_account: String,
    pub expected_branch: String,
    pub expected_head: String,
    #[serde(default)]
    pub include_diff: bool,
}

pub async fn review_detail(
    State(s): State<AppState>,
    Path((ws_id, number)): Path<(String, u32)>,
    Query(q): Query<ReviewDetailQuery>,
) -> ApiResult<Json<Value>> {
    super::fleet::require_local(q.host.as_deref(), "project review detail")?;
    if number == 0
        || q.expected_account.is_empty()
        || q.expected_head.is_empty()
        || q.expected_branch.is_empty()
        || q.expected_origin.is_empty()
        || q.expected_account.len() > 256
        || q.expected_head.len() > 128
        || q.expected_branch.len() > 1024
        || q.expected_origin.len() > 4096
    {
        return Err(ApiError::bad_request(
            "A complete captured review selection is required",
        ));
    }
    let _permit = REVIEW_READERS.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Review readers are busy; retry after the current observation".into(),
        )
    })?;
    let port = s.repos.clone().ok_or_else(|| {
        ApiError(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "SCM review adapter unavailable".into(),
        )
    })?;
    // Reuse exact-root, local-host, registered-project observation. Never accept
    // an arbitrary cwd, repository or account from the request as authority.
    let Json(observed) = context(
        State(s),
        Path(ws_id.clone()),
        Query(ContextQuery::default()),
    )
    .await
    .map_err(|e| {
        if e.0 == axum::http::StatusCode::CONFLICT {
            ApiError(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Checkout readers are busy; retry after the current observation".into(),
            )
        } else {
            e
        }
    })?;
    if observed["state"] != "ready"
        || observed["branch"].as_str() != Some(q.expected_branch.as_str())
        || observed["repository"]["home_url"].as_str() != Some(q.expected_origin.as_str())
    {
        return Err(ApiError::conflict(
            "Repository or branch changed; refresh repository context",
        ));
    }
    let repository = &observed["repository"];
    let platform = repository["forge"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ApiError::conflict("Repository platform is unavailable"))?;
    let repo = rupu_scm::RepoRef {
        platform,
        owner: repository["owner"]
            .as_str()
            .ok_or_else(|| ApiError::conflict("Repository owner is unavailable"))?
            .into(),
        repo: repository["name"]
            .as_str()
            .ok_or_else(|| ApiError::conflict("Repository name is unavailable"))?
            .into(),
    };
    let origin_host = url::Url::parse(&q.expected_origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .ok_or_else(|| ApiError::bad_request("Origin authority is unavailable"))?;
    let cwd = observed["checkout_path"]
        .as_str()
        .ok_or_else(|| ApiError::conflict("Checkout path is unavailable"))?;
    let selection = crate::repos::ReviewSelection {
        account: q.expected_account,
        branch: q.expected_branch,
        head: q.expected_head,
        include_diff: q.include_diff,
    };
    let mut result = tokio::time::timeout(
        Duration::from_secs(25),
        port.review_detail(&repo, FsPath::new(cwd), &origin_host, number, &selection),
    )
    .await
    .map_err(|_| {
        ApiError(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            "Review detail deadline exceeded; refresh to retry".into(),
        )
    })?
    .map_err(|e| match e {
        crate::repos::RepoListError::SelectionChanged => {
            ApiError::conflict("Review or account changed; refresh repository context")
        }
        _ => ApiError(
            axum::http::StatusCode::BAD_GATEWAY,
            "Review detail unavailable from the owning account".into(),
        ),
    })?;
    result["ws_id"] = json!(ws_id);
    result["host_id"] = json!("local");
    result["repository"] = repository.clone();
    result["observed_at"] = json!(chrono::Utc::now().to_rfc3339());
    result["atomic"] = json!(false);
    Ok(Json(result))
}

async fn review_context(port: Option<Arc<dyn crate::repos::RepoLister>>, value: &Value) -> Value {
    let unavailable = |reason: &str| json!({"state":"unavailable","reason":reason,"reviews":[],"truncated":false});
    let Some(port) = port else {
        return unavailable("This control plane has no SCM review adapter");
    };
    let Some(branch) = value["branch"].as_str().filter(|b| !b.is_empty()) else {
        return unavailable("The checkout has no current branch");
    };
    let Some(platform) = value["repository"]["forge"]
        .as_str()
        .and_then(|p| p.parse().ok())
    else {
        return unavailable("The checkout origin is not a recognized SCM repository");
    };
    let (Some(owner), Some(repo), Some(cwd)) = (
        value["repository"]["owner"].as_str(),
        value["repository"]["name"].as_str(),
        value["checkout_path"].as_str(),
    ) else {
        return unavailable("Repository identity is unavailable");
    };
    let Some(origin_host) = value["repository"]["home_url"]
        .as_str()
        .and_then(|url| url::Url::parse(url).ok())
        .and_then(|url| url.host_str().map(str::to_owned))
    else {
        return unavailable("Origin authority is unavailable");
    };
    let repo = rupu_scm::RepoRef {
        platform,
        owner: owner.into(),
        repo: repo.into(),
    };
    match tokio::time::timeout(
        Duration::from_secs(15),
        port.reviews(&repo, FsPath::new(cwd), branch, &origin_host),
    )
    .await
    {
        Ok(Ok(value)) => value,
        // Upstream error bodies/URLs may contain server or account data. Keep
        // failure explicit without forwarding transport diagnostics to clients.
        Ok(Err(_)) => {
            unavailable("Review query failed or no unambiguous authorized account is configured")
        }
        Err(_) => unavailable("Review query deadline exceeded; refresh to try again"),
    }
}

#[cfg(test)]
fn observe(ws_id: &str, path: &FsPath) -> ApiResult<Value> {
    observe_with_hosts(ws_id, path, &rupu_scm::weburl::WebHosts::default())
}

fn observe_with_hosts(
    ws_id: &str,
    path: &FsPath,
    hosts: &rupu_scm::weburl::WebHosts,
) -> ApiResult<Value> {
    let path = path
        .canonicalize()
        .map_err(|_| ApiError::not_found("registered project directory is unavailable"))?;
    let mut result = json!({"ws_id":ws_id,"host_id":"local","source":"local_checkout",
        "checkout_path":path,"state":"not_repository","branch":null,"head":null,"detached":false,
        "repository":null,"changes":[],"changes_truncated":false,
        "working_diff":{"text":"","truncated":false},"staged_diff":{"text":"","truncated":false},
        "pull_request":{"state":"not_queried","reason":"Remote pull requests and merge requests are separate from this local checkout observation."},
        "observed_at":chrono::Utc::now().to_rfc3339(),"atomic":false,
        "scan_limit":"Response size and concurrency are bounded; Git may scan the entire index and worktree."});
    // Exact root only: never silently reveal a parent repository for a nested
    // registered directory. A worktree .git file is supported by open().
    let repo = match Repository::open(&path) {
        Ok(repo) => repo,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(result),
        Err(_) => {
            result["state"] = json!("unavailable");
            result["reason"] = json!("Unable to open the registered checkout");
            return Ok(result);
        }
    };
    if repo.is_bare() || repo.workdir().and_then(|p| p.canonicalize().ok()).as_ref() != Some(&path)
    {
        result["state"] = json!("unavailable");
        result["reason"] = json!("The registered directory must be the checkout root");
        return Ok(result);
    }
    result["state"] = json!("ready");
    if let Ok(head) = repo.head() {
        result["head"] = json!(head.target().map(|oid| oid.to_string()));
        result["detached"] = json!(!head.is_branch());
        if head.is_branch() {
            result["branch"] = json!(head.shorthand());
        }
    } else if let Ok(head) = repo.find_reference("HEAD") {
        result["branch"] = json!(head
            .symbolic_target()
            .and_then(|name| name.strip_prefix("refs/heads/")));
    }
    // Emit only parsed known-forge identity, never a raw origin URL (which may
    // contain credentials). Unknown forges remain unknown rather than guessed.
    if let Ok(remote) = repo.find_remote("origin") {
        if let Some(web) = remote
            .url()
            .and_then(|url| rupu_scm::weburl::parse_repo_remote_with(url, hosts))
        {
            let url = web.home_url();
            if safe_web_identity(&url) {
                result["repository"] = json!({"forge":web.platform.to_string(),"home_url":url,"owner":web.owner,"name":web.repo});
            }
        }
    }
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(false)
        .update_index(false);
    let statuses = repo
        .statuses(Some(&mut options))
        .map_err(|_| ApiError::internal("Unable to inspect checkout status"))?;
    let changes: Vec<Value> = statuses.iter().take(MAX_CHANGES).map(|entry| {
        let delta = entry.head_to_index().or_else(|| entry.index_to_workdir());
        let current = entry.index_to_workdir().or_else(|| entry.head_to_index());
        let raw_path = current.and_then(|d| d.new_file().path().map(|p|p.as_os_str().as_encoded_bytes().to_vec()))
            .unwrap_or_else(|| entry.path_bytes().to_vec());
        let path = String::from_utf8_lossy(&raw_path);
        let lossy = std::str::from_utf8(&raw_path).is_err();
        json!({"path":path,"path_lossy":lossy,"path_bytes_base64":lossy.then(||base64::engine::general_purpose::STANDARD.encode(&raw_path)),"original_path":delta.filter(|d| d.status() == git2::Delta::Renamed).and_then(|d|d.old_file().path().map(|p|p.to_string_lossy().into_owned())),
            "index_status":index_status(entry.status()),"worktree_status":worktree_status(entry.status())})
    }).collect();
    result["changes"] = json!(changes);
    result["changes_truncated"] = json!(statuses.len() > MAX_CHANGES);
    let tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut working_options = diff_options();
    let working = repo
        .diff_index_to_workdir(None, Some(&mut working_options))
        .map_err(|_| ApiError::internal("Unable to inspect working changes"))?;
    result["working_diff"] = bounded_diff(&working)?;
    let mut staged_options = diff_options();
    let staged = repo
        .diff_tree_to_index(tree.as_ref(), None, Some(&mut staged_options))
        .map_err(|_| ApiError::internal("Unable to inspect staged changes"))?;
    result["staged_diff"] = bounded_diff(&staged)?;
    result["untracked_content_included"] = json!(false);
    Ok(result)
}

fn safe_web_identity(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    })
}
fn diff_options() -> DiffOptions {
    let mut options = DiffOptions::new();
    options
        .ignore_submodules(true)
        .include_untracked(false)
        .show_binary(false)
        .max_size(1024 * 1024);
    options
}
fn bounded_diff(diff: &git2::Diff<'_>) -> ApiResult<Value> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let printed = diff.print(DiffFormat::Patch, |_, _, line| {
        let prefix = matches!(line.origin(), '+' | '-' | ' ');
        if bytes.len() + line.content().len() + usize::from(prefix) > MAX_DIFF {
            truncated = true;
            return false;
        }
        if prefix {
            bytes.push(line.origin() as u8);
        }
        bytes.extend_from_slice(line.content());
        true
    });
    if printed.is_err() && !truncated {
        return Err(ApiError::internal("Unable to render checkout diff"));
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > MAX_DIFF {
        let mut boundary = MAX_DIFF;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        truncated = true;
    }
    Ok(json!({"text":text,"truncated":truncated}))
}
fn index_status(s: Status) -> &'static str {
    if s.is_conflicted() {
        "conflicted"
    } else if s.is_index_new() {
        "added"
    } else if s.is_index_deleted() {
        "deleted"
    } else if s.is_index_renamed() {
        "renamed"
    } else if s.is_index_typechange() {
        "type_changed"
    } else if s.is_index_modified() {
        "modified"
    } else {
        "unchanged"
    }
}
fn worktree_status(s: Status) -> &'static str {
    if s.is_conflicted() {
        "conflicted"
    } else if s.is_wt_new() {
        "untracked"
    } else if s.is_wt_deleted() {
        "deleted"
    } else if s.is_wt_renamed() {
        "renamed"
    } else if s.is_wt_typechange() {
        "type_changed"
    } else if s.is_wt_modified() {
        "modified"
    } else {
        "unchanged"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct ReviewSpy(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl crate::repos::RepoLister for ReviewSpy {
        async fn list(&self) -> Result<Vec<crate::repos::RepoEntry>, crate::repos::RepoListError> {
            panic!("SCM inspection must not enumerate all repositories");
        }
        async fn reviews(
            &self,
            repo: &rupu_scm::RepoRef,
            cwd: &FsPath,
            branch: &str,
            origin_host: &str,
        ) -> Result<Value, crate::repos::RepoListError> {
            assert_eq!(repo.owner, "example");
            assert_eq!(origin_host, "github.com");
            assert!(cwd.is_absolute());
            assert!(!branch.is_empty());
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(json!({"state":"queried","reviews":[],"truncated":false}))
        }
    }
    struct DetailSpy(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl crate::repos::RepoLister for DetailSpy {
        async fn list(&self) -> Result<Vec<crate::repos::RepoEntry>, crate::repos::RepoListError> {
            panic!("No repository enumeration")
        }
        async fn review_detail(
            &self,
            repo: &rupu_scm::RepoRef,
            cwd: &FsPath,
            origin_host: &str,
            number: u32,
            selection: &crate::repos::ReviewSelection,
        ) -> Result<Value, crate::repos::RepoListError> {
            assert_eq!(repo.owner, "example");
            assert_eq!(repo.repo, "repo");
            assert_eq!(origin_host, "github.com");
            assert!(cwd.is_absolute());
            assert_eq!(number, 9);
            assert_eq!(selection.account, "fixture");
            assert_eq!(selection.head, "captured-review-head");
            assert!(!selection.include_diff);
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(
                json!({"state":"queried","account":"fixture","number":9,"branch":selection.branch,"head_sha":selection.head}),
            )
        }
    }
    #[tokio::test]
    async fn review_detail_requires_registered_local_project_and_captured_origin_branch() {
        let home = tempfile::tempdir().unwrap();
        let (project, repo) = committed();
        repo.remote("origin", "https://github.com/example/repo.git")
            .unwrap();
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let spy = Arc::new(DetailSpy(std::sync::atomic::AtomicUsize::new(0)));
        let state = AppState::new(home.path().into(), rupu_config::PricingConfig::default())
            .with_repos(Some(spy.clone()));
        let ws = rupu_workspace::register(
            &rupu_workspace::WorkspaceStore {
                root: home.path().join("workspaces"),
            },
            project.path(),
        )
        .unwrap();
        let query = || ReviewDetailQuery {
            host: None,
            expected_origin: "https://github.com/example/repo".into(),
            expected_account: "fixture".into(),
            expected_branch: branch.clone(),
            expected_head: "captured-review-head".into(),
            include_diff: false,
        };
        let mut remote = query();
        remote.host = Some("other-host".into());
        assert!(review_detail(
            State(state.clone()),
            Path((ws.id.clone(), 9)),
            Query(remote)
        )
        .await
        .is_err());
        let mut wrong_origin = query();
        wrong_origin.expected_origin = "https://github.com/foreign/repo".into();
        assert_eq!(
            review_detail(
                State(state.clone()),
                Path((ws.id.clone(), 9)),
                Query(wrong_origin)
            )
            .await
            .unwrap_err()
            .0,
            axum::http::StatusCode::CONFLICT
        );
        let mut wrong_branch = query();
        wrong_branch.expected_branch = "other".into();
        assert_eq!(
            review_detail(
                State(state.clone()),
                Path((ws.id.clone(), 9)),
                Query(wrong_branch)
            )
            .await
            .unwrap_err()
            .0,
            axum::http::StatusCode::CONFLICT
        );
        assert_eq!(spy.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        let Json(result) = review_detail(State(state), Path((ws.id.clone(), 9)), Query(query()))
            .await
            .unwrap();
        assert_eq!(spy.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(result["ws_id"], ws.id);
        assert_eq!(result["host_id"], "local");
        assert_eq!(result["repository"]["owner"], "example");
        assert_eq!(result["atomic"], false);
    }
    #[tokio::test]
    async fn remote_review_requires_explicit_query() {
        let home = tempfile::tempdir().unwrap();
        let (project, repo) = committed();
        repo.remote("origin", "https://github.com/example/repo.git")
            .unwrap();
        let spy = Arc::new(ReviewSpy(std::sync::atomic::AtomicUsize::new(0)));
        let state = AppState::new(home.path().into(), rupu_config::PricingConfig::default())
            .with_repos(Some(spy.clone()));
        let ws = rupu_workspace::register(
            &rupu_workspace::WorkspaceStore {
                root: home.path().join("workspaces"),
            },
            project.path(),
        )
        .unwrap();
        let Json(value) = context(
            State(state.clone()),
            Path(ws.id.clone()),
            Query(ContextQuery::default()),
        )
        .await
        .unwrap();
        assert_eq!(value["pull_request"]["state"], "not_queried");
        assert_eq!(spy.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        let Json(value) = context(
            State(state),
            Path(ws.id),
            Query(ContextQuery {
                include_review: true,
                ..ContextQuery::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(value["pull_request"]["state"], "queried");
        assert_eq!(spy.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn endpoint_requires_registered_owner_and_refuses_remote() {
        let home = tempfile::tempdir().unwrap();
        let (project, _) = committed();
        let state = AppState::new(home.path().into(), rupu_config::PricingConfig::default());
        let store = rupu_workspace::WorkspaceStore {
            root: home.path().join("workspaces"),
        };
        let ws = rupu_workspace::register(&store, project.path()).unwrap();
        let Json(value) = context(
            State(state.clone()),
            Path(ws.id.clone()),
            Query(ContextQuery::default()),
        )
        .await
        .unwrap();
        assert_eq!(value["ws_id"], ws.id);
        assert_eq!(value["state"], "ready");
        let missing = context(
            State(state.clone()),
            Path("ws_missing".into()),
            Query(ContextQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(missing.0, axum::http::StatusCode::NOT_FOUND);
        assert!(context(
            State(state),
            Path(ws.id),
            Query(ContextQuery {
                host: Some("remote".into()),
                ..ContextQuery::default()
            })
        )
        .await
        .is_err());
    }
    fn committed() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("file.txt"), "original\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(FsPath::new("file.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
            .unwrap();
        drop(tree);
        (dir, repo)
    }
    #[test]
    fn working_staged_untracked_are_distinct_and_index_is_unchanged() {
        let (dir, repo) = committed();
        std::fs::write(dir.path().join("file.txt"), "staged\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(FsPath::new("file.txt")).unwrap();
        index.write().unwrap();
        std::fs::write(dir.path().join("file.txt"), "working\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "private untracked\n").unwrap();
        let before = std::fs::read(repo.path().join("index")).unwrap();
        let value = observe("ws_fixture", dir.path()).unwrap();
        assert_eq!(value["state"], "ready");
        assert!(value["staged_diff"]["text"]
            .as_str()
            .unwrap()
            .contains("+staged"));
        assert!(value["working_diff"]["text"]
            .as_str()
            .unwrap()
            .contains("+working"));
        assert!(!value["working_diff"]["text"]
            .as_str()
            .unwrap()
            .contains("private untracked"));
        assert_eq!(std::fs::read(repo.path().join("index")).unwrap(), before);
        assert!(value["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["path"] == "new.txt" && r["worktree_status"] == "untracked"));
    }
    #[test]
    fn credentials_and_configured_commands_are_not_exposed_or_executed() {
        let (dir, repo) = committed();
        repo.remote(
            "origin",
            "https://SECRET:SECRET@github.com/example/repo.git",
        )
        .unwrap();
        let marker = dir.path().join("executed");
        let attack = format!("touch {}", marker.display());
        let mut config = repo.config().unwrap();
        for key in [
            "filter.evil.clean",
            "filter.evil.process",
            "diff.external",
            "core.fsmonitor",
        ] {
            config.set_str(key, &attack).unwrap();
        }
        std::fs::write(
            dir.path().join(".gitattributes"),
            "*.txt filter=evil diff=evil\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("file.txt"), "changed\n").unwrap();
        let value = observe("ws_fixture", dir.path()).unwrap();
        assert!(!value.to_string().contains("SECRET"));
        assert!(!marker.exists());
        assert_eq!(value["repository"]["forge"], "github");
    }
    #[test]
    fn staged_rename_and_bounds() {
        let (dir, repo) = committed();
        std::fs::rename(dir.path().join("file.txt"), dir.path().join("renamed.txt")).unwrap();
        let mut index = repo.index().unwrap();
        index.remove_path(FsPath::new("file.txt")).unwrap();
        index.add_path(FsPath::new("renamed.txt")).unwrap();
        index.write().unwrap();
        let renamed = observe("ws_fixture", dir.path()).unwrap();
        assert!(renamed["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["index_status"] == "renamed"
                && r["original_path"] == "file.txt"
                && r["path"] == "renamed.txt"));
        for n in 0..205 {
            std::fs::write(dir.path().join(format!("new-{n}")), "new").unwrap();
        }
        let value = observe("ws_fixture", dir.path()).unwrap();
        assert_eq!(value["changes"].as_array().unwrap().len(), MAX_CHANGES);
        assert_eq!(value["changes_truncated"], true);
        // Test rename separately from a response in which it may be beyond the limit.
        let statuses = repo
            .statuses(Some(
                StatusOptions::new()
                    .include_untracked(false)
                    .renames_head_to_index(true),
            ))
            .unwrap();
        assert!(statuses.iter().any(|e| e.status().is_index_renamed()));
        let text = "large line\n".repeat(MAX_DIFF / 4);
        std::fs::write(dir.path().join("renamed.txt"), text).unwrap();
        let value = observe("ws_fixture", dir.path()).unwrap();
        assert_eq!(value["working_diff"]["truncated"], true);
        assert!(value["working_diff"]["text"].as_str().unwrap().len() <= MAX_DIFF);
    }
    #[test]
    fn no_repository_and_remote_refusal() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            observe("ws_fixture", dir.path()).unwrap()["state"],
            "not_repository"
        );
        assert!(super::super::fleet::require_local(Some("remote"), "project SCM context").is_err());
    }
}
