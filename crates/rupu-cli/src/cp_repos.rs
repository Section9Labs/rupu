//! `cp serve` adapter for rupu-cp's `RepoLister` port. Lists repos across the
//! logged-in platforms via the SCM registry (same path as `rupu repos list`).
use rupu_cp::repos::{RepoEntry, RepoListError, RepoLister};
use rupu_scm::{AccountId, Platform, Registry};
use std::sync::Arc;

pub struct CpRepoLister {
    pub registry: Arc<Registry>,
}

pub(crate) fn to_entry(p: Platform, account: &AccountId, r: &rupu_scm::Repo) -> RepoEntry {
    RepoEntry {
        platform: p.to_string(),
        account: account.to_string(),
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
            // Account-scoped, no repo to key on (spec §6.2): fan out
            // across every configured account of this platform, tagging
            // each row — the old `registry.repo(p)` shim silently picked
            // one account and dropped the rest.
            for (account, conn) in self.registry.all_repo_connectors(p) {
                match conn.list_repos().await {
                    Ok(repos) => out.extend(repos.iter().map(|r| to_entry(p, &account, r))),
                    Err(e) => {
                        tracing::warn!(platform = %p, account = %account, error = %e, "list_repos failed; skipping")
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::to_entry;
    use rupu_scm::{AccountId, Platform, RepoRef};

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
        let e = to_entry(Platform::Github, &AccountId::new("gh-work"), &repo);
        assert_eq!(e.platform, "github");
        assert_eq!(e.account, "gh-work");
        assert_eq!(e.repo, "o/r");
        assert_eq!(e.default_branch, "main");
        assert!(e.private);
    }
}
