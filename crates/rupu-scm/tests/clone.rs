//! clone_repo_ref against a fixture connector.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use rupu_scm::{
    clone_repo_ref,
    types::{Branch, Comment, CreatePr, Diff, FileContent, Pr, PrFilter, PrRef, Repo},
    Platform, Registry, RepoConnector, RepoRef, ScmError,
};

struct FakeConnector;

#[async_trait]
impl RepoConnector for FakeConnector {
    fn platform(&self) -> Platform {
        Platform::Github
    }

    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        unimplemented!()
    }
    async fn get_repo(&self, _r: &RepoRef) -> Result<Repo, ScmError> {
        unimplemented!()
    }
    async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        unimplemented!()
    }
    async fn create_branch(
        &self,
        _r: &RepoRef,
        _name: &str,
        _from_sha: &str,
    ) -> Result<Branch, ScmError> {
        unimplemented!()
    }
    async fn read_file(
        &self,
        _r: &RepoRef,
        _path: &str,
        _ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        unimplemented!()
    }
    async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        unimplemented!()
    }
    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
        unimplemented!()
    }
    async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
        unimplemented!()
    }
    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn clone_to(&self, r: &RepoRef, dir: &Path) -> Result<(), ScmError> {
        std::fs::create_dir_all(dir).map_err(|e| ScmError::Transient(anyhow::anyhow!("{e}")))?;
        std::fs::write(dir.join("README.md"), format!("{}/{}\n", r.owner, r.repo))
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("{e}")))?;
        Ok(())
    }
}

#[tokio::test]
async fn clone_repo_ref_creates_target_dir_with_content() {
    let mut registry = Registry::default();
    registry.insert_repo_connector(Platform::Github, Arc::new(FakeConnector));

    let r = RepoRef {
        platform: Platform::Github,
        owner: "foo".into(),
        repo: "bar".into(),
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("clone");
    clone_repo_ref(&registry, &r, &target).await.expect("clone");
    assert!(target.join("README.md").exists());
}

#[tokio::test]
async fn clone_repo_ref_missing_connector_returns_error() {
    let registry = Registry::default();
    let r = RepoRef {
        platform: Platform::Github,
        owner: "foo".into(),
        repo: "bar".into(),
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("clone");
    let err = clone_repo_ref(&registry, &r, &target)
        .await
        .expect_err("should fail without connector");
    let msg = err.to_string();
    assert!(msg.contains("github"), "error mentions platform: {msg}");
    assert!(
        msg.contains("rupu auth login"),
        "error hints at login: {msg}"
    );
}
