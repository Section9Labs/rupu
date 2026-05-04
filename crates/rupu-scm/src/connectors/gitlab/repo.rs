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
    Branch, Comment, CreatePr, Diff, FileContent, Pr, PrFilter, PrRef, PrState, Repo, RepoRef,
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

fn translate_note_to_comment(v: &serde_json::Value) -> Result<Comment, ScmError> {
    let id = v
        .get("id")
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
        .to_string();
    let body = v
        .get("body")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let author = v
        .get("author")
        .and_then(|a| a.get("username"))
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let created_at = v
        .get("created_at")
        .and_then(|x| x.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    Ok(Comment {
        id,
        author,
        body,
        created_at,
    })
}

/// URL-encode a value for query parameter use (handles `/`, `+`, etc.).
fn urlencode_value(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn translate_mr_to_pr(repo: RepoRef, v: &serde_json::Value) -> Result<Pr, ScmError> {
    let iid = v
        .get("iid")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| ScmError::BadRequest {
            message: "merge request missing iid".into(),
        })?;
    let state_str = v.get("state").and_then(|x| x.as_str()).unwrap_or("opened");
    let state = match state_str {
        "opened" => PrState::Open,
        "merged" => PrState::Merged,
        _ => PrState::Closed,
    };
    let title = v
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let body = v
        .get("description")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let head_branch = v
        .get("source_branch")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let base_branch = v
        .get("target_branch")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let author = v
        .get("author")
        .and_then(|a| a.get("username"))
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let created_at = v
        .get("created_at")
        .and_then(|x| x.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let updated_at = v
        .get("updated_at")
        .and_then(|x| x.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    Ok(Pr {
        r: PrRef {
            repo,
            number: iid as u32,
        },
        title,
        body,
        state,
        head_branch,
        base_branch,
        author,
        created_at,
        updated_at,
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
        r: &RepoRef,
        name: &str,
        from_sha: &str,
    ) -> Result<Branch, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&r.owner, &r.repo);
        let path = format!(
            "/projects/{id}/repository/branches?branch={}&ref={}",
            urlencode_value(name),
            urlencode_value(from_sha),
        );
        let resp = self
            .client
            .write_json(reqwest::Method::POST, &path, serde_json::Value::Null)
            .await?;
        translate_branch(&resp)
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
    async fn list_prs(&self, r: &RepoRef, filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&r.owner, &r.repo);
        let mut path = format!("/projects/{id}/merge_requests?per_page=100");
        if let Some(state) = filter.state {
            let s = match state {
                PrState::Open => "opened",
                PrState::Closed | PrState::Merged => "closed",
            };
            path.push_str(&format!("&state={s}"));
        }
        if let Some(author) = filter.author.as_ref() {
            path.push_str(&format!("&author_username={author}"));
        }
        let body = self.client.get_json(&path).await?;
        let arr = body.as_array().ok_or_else(|| ScmError::BadRequest {
            message: "expected array from /merge_requests".into(),
        })?;
        let repo_ref = r.clone();
        arr.iter()
            .map(|v| translate_mr_to_pr(repo_ref.clone(), v))
            .collect()
    }
    async fn get_pr(&self, p: &PrRef) -> Result<Pr, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&p.repo.owner, &p.repo.repo);
        let body = self
            .client
            .get_json(&format!("/projects/{id}/merge_requests/{}", p.number))
            .await?;
        translate_mr_to_pr(p.repo.clone(), &body)
    }
    async fn diff_pr(&self, p: &PrRef) -> Result<Diff, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&p.repo.owner, &p.repo.repo);
        let body = self
            .client
            .get_json(&format!(
                "/projects/{id}/merge_requests/{}/changes",
                p.number
            ))
            .await?;
        let changes = body
            .get("changes")
            .and_then(|c| c.as_array())
            .ok_or_else(|| ScmError::BadRequest {
                message: "expected `changes` array on MR".into(),
            })?;
        let files_changed = changes.len() as u32;
        let mut additions = 0u32;
        let mut deletions = 0u32;
        let mut patch = String::new();
        for ch in changes {
            let new_path = ch
                .get("new_path")
                .and_then(|x| x.as_str())
                .unwrap_or_default();
            let old_path = ch
                .get("old_path")
                .and_then(|x| x.as_str())
                .unwrap_or_default();
            let raw_diff = ch.get("diff").and_then(|x| x.as_str()).unwrap_or_default();
            if !patch.is_empty() {
                patch.push('\n');
            }
            patch.push_str(&format!("diff --git a/{old_path} b/{new_path}\n"));
            patch.push_str(raw_diff);
            for line in raw_diff.lines() {
                if line.starts_with("+++") || line.starts_with("---") {
                    continue;
                }
                if line.starts_with('+') {
                    additions += 1;
                } else if line.starts_with('-') {
                    deletions += 1;
                }
            }
        }
        Ok(Diff {
            patch,
            files_changed,
            additions,
            deletions,
        })
    }
    async fn comment_pr(&self, p: &PrRef, body: &str) -> Result<Comment, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&p.repo.owner, &p.repo.repo);
        let payload = serde_json::json!({ "body": body });
        let resp = self
            .client
            .write_json(
                reqwest::Method::POST,
                &format!("/projects/{id}/merge_requests/{}/notes", p.number),
                payload,
            )
            .await?;
        translate_note_to_comment(&resp)
    }
    async fn create_pr(&self, r: &RepoRef, opts: CreatePr) -> Result<Pr, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&r.owner, &r.repo);
        let payload = serde_json::json!({
            "source_branch": opts.head,
            "target_branch": opts.base,
            "title": opts.title,
            "description": opts.body,
            "draft": opts.draft,
        });
        let resp = self
            .client
            .write_json(
                reqwest::Method::POST,
                &format!("/projects/{id}/merge_requests"),
                payload,
            )
            .await?;
        translate_mr_to_pr(r.clone(), &resp)
    }
    async fn clone_to(&self, r: &RepoRef, dir: &Path) -> Result<(), ScmError> {
        // GitLab PAT-as-password convention with username "oauth2".
        let url = format!(
            "https://oauth2:{}@gitlab.com/{}/{}.git",
            self.client.token, r.owner, r.repo
        );
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(), ScmError> {
            git2::Repository::clone(&url, &dir)
                .map_err(|e| ScmError::Network(anyhow::anyhow!("git clone failed: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| ScmError::Transient(anyhow::anyhow!("join: {e}")))??;
        Ok(())
    }
}
