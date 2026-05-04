//! GitlabRepoConnector — implements rupu_scm::RepoConnector.
//!
//! Each method:
//! 1. Acquires the per-platform semaphore permit.
//! 2. Issues the request via [`GitlabClient`] (which handles ETag
//!    cache, retries, and classify_scm_error mapping).
//! 3. Deserializes the JSON via `serde_json::from_value` into the
//!    GitLab-flavored DTO struct, then translates to rupu_scm types.
//!
//! GitLab vs GitHub vocabulary:
//!   - "project" ↔ Repo
//!   - "merge request" (MR) ↔ Pr
//!   - "namespace/path" ↔ owner/repo (always full slash-joined for nested groups)

use std::path::Path;

use async_trait::async_trait;

use crate::connectors::RepoConnector;
use crate::error::ScmError;
use crate::platform::Platform;
use crate::types::{
    Branch, Comment, CreatePr, Diff, FileContent, Pr, PrFilter, PrRef, Repo, RepoRef,
};

use super::client::GitlabClient;

pub struct GitlabRepoConnector {
    client: GitlabClient,
}

impl GitlabRepoConnector {
    pub fn new(client: GitlabClient) -> Self {
        Self { client }
    }
}

/// Pure translation function — fixture-tested in
/// `crates/rupu-scm/tests/gitlab_translation.rs`.
///
/// Handles GitLab's nested-namespace quirk:
/// `group/subgroup/project` → owner=`group/subgroup`, repo=`project`.
pub fn translate_project_to_repo(p: &serde_json::Value) -> Result<Repo, ScmError> {
    let path_with_namespace = p
        .get("path_with_namespace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ScmError::BadRequest {
            message: "missing path_with_namespace".into(),
        })?;
    let (owner, repo_name) = split_namespace(path_with_namespace);
    Ok(Repo {
        r: RepoRef {
            platform: Platform::Gitlab,
            owner,
            repo: repo_name,
        },
        default_branch: p
            .get("default_branch")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .to_string(),
        clone_url_https: p
            .get("http_url_to_repo")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        clone_url_ssh: p
            .get("ssh_url_to_repo")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        private: p
            .get("visibility")
            .and_then(|v| v.as_str())
            .map(|s| s != "public")
            .unwrap_or(true),
        description: p
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

fn split_namespace(path: &str) -> (String, String) {
    // GitLab nested groups: "group/subgroup/project" → owner="group/subgroup", repo="project".
    if let Some((owner, name)) = path.rsplit_once('/') {
        (owner.to_string(), name.to_string())
    } else {
        (String::new(), path.to_string())
    }
}

/// Encode a GitLab project ID for use in URL paths.
///
/// GitLab requires URL-encoded path segments. The only meaningful character
/// in our project IDs is `/`, which encodes to `%2F`.
fn encode_project_id(owner: &str, repo: &str) -> String {
    let full = format!("{owner}/{repo}");
    full.replace('/', "%2F")
}

fn translate_branch(v: &serde_json::Value) -> Result<Branch, ScmError> {
    Ok(Branch {
        name: v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        sha: v
            .get("commit")
            .and_then(|c| c.get("id"))
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        protected: v
            .get("protected")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
    })
}

#[async_trait]
impl RepoConnector for GitlabRepoConnector {
    fn platform(&self) -> Platform {
        Platform::Gitlab
    }

    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        let _permit = self.client.permit().await;
        let mut out = Vec::new();
        let mut page: u32 = 1;
        loop {
            let path = format!("/projects?membership=true&per_page=100&page={page}");
            let body = self.client.get_json(&path).await?;
            let arr = body.as_array().ok_or_else(|| ScmError::BadRequest {
                message: "expected JSON array from /projects".into(),
            })?;
            if arr.is_empty() {
                break;
            }
            for item in arr {
                out.push(translate_project_to_repo(item)?);
            }
            if arr.len() < 100 {
                break;
            }
            page += 1;
            if page > 100 {
                // 10k repos cap — safety against runaway pagination.
                break;
            }
        }
        Ok(out)
    }

    async fn get_repo(&self, r: &RepoRef) -> Result<Repo, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&r.owner, &r.repo);
        let body = self.client.get_json(&format!("/projects/{id}")).await?;
        translate_project_to_repo(&body)
    }

    async fn list_branches(&self, r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&r.owner, &r.repo);
        let body = self
            .client
            .get_json(&format!("/projects/{id}/repository/branches"))
            .await?;
        let arr = body.as_array().ok_or_else(|| ScmError::BadRequest {
            message: "expected array from /repository/branches".into(),
        })?;
        arr.iter().map(translate_branch).collect()
    }
    async fn create_branch(
        &self,
        _r: &RepoRef,
        _name: &str,
        _from_sha: &str,
    ) -> Result<Branch, ScmError> {
        unimplemented!("subtask 4f")
    }
    async fn read_file(
        &self,
        r: &RepoRef,
        path: &str,
        ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&r.owner, &r.repo);
        let path_encoded = path.replace('/', "%2F");
        let resolved_ref = ref_.unwrap_or("HEAD").to_string();
        let url = match ref_ {
            Some(r) => format!("/projects/{id}/repository/files/{path_encoded}/raw?ref={r}"),
            None => format!("/projects/{id}/repository/files/{path_encoded}/raw"),
        };
        let content = self.client.get_text(&url).await?;
        Ok(FileContent {
            path: path.to_string(),
            ref_: resolved_ref,
            content,
            encoding: crate::types::FileEncoding::Utf8,
        })
    }
    async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        unimplemented!("subtask 4d")
    }
    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
        unimplemented!("subtask 4d")
    }
    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
        unimplemented!("subtask 4d")
    }
    async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
        unimplemented!("subtask 4f")
    }
    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
        unimplemented!("subtask 4f")
    }
    async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> {
        unimplemented!("subtask 4f")
    }
}
