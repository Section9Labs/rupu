//! `cp serve` adapter for rupu-cp's `RepoLister` port. Lists repos across the
//! logged-in platforms via the SCM registry (same path as `rupu repos list`).
use rupu_cp::repos::{RepoEntry, RepoListError, RepoLister};
use rupu_scm::{Platform, Registry};
use std::sync::Arc;

pub struct CpRepoLister {
    pub registry: Arc<Registry>,
}

pub(crate) fn to_entry(p: Platform, r: &rupu_scm::Repo) -> RepoEntry {
    RepoEntry {
        platform: p.to_string(),
        repo: format!("{}/{}", r.r.owner, r.r.repo),
        default_branch: r.default_branch.clone(),
        private: r.private,
    }
}

#[async_trait::async_trait]
impl RepoLister for CpRepoLister {
    async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError> {
        let mut out = Vec::new();
        for p in [Platform::Github, Platform::Gitlab] {
            let Some(conn) = self.registry.repo(p) else {
                continue;
            };
            match conn.list_repos().await {
                Ok(repos) => out.extend(repos.iter().map(|r| to_entry(p, r))),
                Err(e) => {
                    tracing::warn!(platform = %p, error = %e, "list_repos failed; skipping")
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::to_entry;
    use rupu_scm::{Platform, RepoRef};

    #[test]
    fn maps_repo_to_entry() {
        let repo = rupu_scm::Repo {
            r: RepoRef {
                platform: Platform::Github,
                owner: "o".into(),
                repo: "r".into(),
            },
            default_branch: "main".into(),
            clone_url_https: String::new(),
            clone_url_ssh: String::new(),
            private: true,
            description: None,
        };
        let e = to_entry(Platform::Github, &repo);
        assert_eq!(e.platform, "github");
        assert_eq!(e.repo, "o/r");
        assert_eq!(e.default_branch, "main");
        assert!(e.private);
    }
}
