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

    async fn reviews(
        &self,
        repo: &rupu_scm::RepoRef,
        cwd: &std::path::Path,
        branch: &str,
        origin_host: &str,
    ) -> Result<serde_json::Value, RepoListError> {
        use serde_json::json;
        let failed = || RepoListError::Backend("SCM review query unavailable".into());
        let (account, reviews, truncated) = match repo.platform {
            Platform::Github => {
                let (account, extras) = self
                    .registry
                    .github_extras_for(repo, Some(cwd), None)
                    .map_err(|_| failed())?;
                if !extras.matches_web_host(origin_host) {
                    return Err(failed());
                }
                // Fixed document; user/repository strings are variables, never
                // interpolated GraphQL. Only six records and one check rollup.
                let value = extras
                    .graphql_json(
                        BRANCH_REVIEWS,
                        json!({"owner":repo.owner,"name":repo.repo,"branch":branch}),
                    )
                    .await
                    .map_err(|_| failed())?;
                let page = &value["repository"]["pullRequests"];
                let rows = page["nodes"].as_array().ok_or_else(failed)?;
                let expected = format!("{}/{}", repo.owner, repo.repo);
                let reviews = rows
                    .iter()
                    .take(5)
                    .filter(|row| {
                        row["headRefName"].as_str() == Some(branch)
                            && row["headRepository"]["nameWithOwner"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(&expected))
                    })
                    .map(github_review)
                    .collect::<Vec<_>>();
                (
                    account,
                    reviews,
                    rows.len() > 5 || page["pageInfo"]["hasNextPage"] == true,
                )
            }
            Platform::Gitlab => {
                let (account, extras) = self
                    .registry
                    .gitlab_extras_for(repo, Some(cwd), None)
                    .map_err(|_| failed())?;
                if !extras.matches_web_host(origin_host) {
                    return Err(failed());
                }
                let (rows, truncated) = extras
                    .branch_merge_requests(repo, branch)
                    .await
                    .map_err(|_| failed())?;
                (account, rows.iter().map(gitlab_review).collect(), truncated)
            }
        };
        Ok(
            json!({"state":"queried","account":account.to_string(),"reviews":reviews,"truncated":truncated,
            "scope":"origin_repository_current_branch","freshness":"connector_cache_policy","queried_at":chrono::Utc::now().to_rfc3339()}),
        )
    }
}

const BRANCH_REVIEWS: &str = r#"query RupuBranchReviews($owner:String!,$name:String!,$branch:String!){
  repository(owner:$owner,name:$name){pullRequests(first:6,headRefName:$branch,orderBy:{field:UPDATED_AT,direction:DESC}){
    pageInfo{hasNextPage} nodes{number title url state isDraft headRefName headRefOid baseRefName baseRefOid
      headRepository{nameWithOwner} commits(last:1){nodes{commit{statusCheckRollup{state}}}}}
  }}
}"#;

fn github_review(row: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"number":row["number"],"title":row["title"],"url":row["url"],
        "state":row["state"].as_str().map(str::to_ascii_lowercase),"draft":row["isDraft"],
        "head_branch":row["headRefName"],"head_sha":row["headRefOid"],"base_branch":row["baseRefName"],"base_sha":row["baseRefOid"],
        "checks_state":row["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str().map(str::to_ascii_lowercase)})
}

fn gitlab_review(row: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"number":row["iid"],"title":row["title"],"url":row["web_url"],"state":row["state"],"draft":row["draft"],
        "head_branch":row["source_branch"],"head_sha":row["diff_refs"]["head_sha"],"base_branch":row["target_branch"],"base_sha":row["diff_refs"]["start_sha"],"merge_base_sha":row["diff_refs"]["base_sha"],
        "checks_state":row["head_pipeline"]["status"]})
}

#[cfg(test)]
mod tests {
    use super::to_entry;
    use rupu_cp::repos::RepoLister;
    use rupu_scm::{AccountId, Platform, RepoRef};

    #[tokio::test]
    async fn branch_reviews_use_selected_github_account_and_bounded_graphql() {
        use httpmock::{Method::POST, MockServer};
        use rupu_scm::connectors::github::{GithubClient, GithubExtras};
        use serde_json::json;
        use std::sync::Arc;
        rupu_scm::install_default_crypto_provider();
        let server = MockServer::start();
        let row = json!({"number":9,"title":"Review","url":"https://github.com/o/r/pull/9","state":"OPEN","headRefName":"feature/a","headRefOid":"abc","baseRefName":"main","baseRefOid":"def","headRepository":{"nameWithOwner":"o/r"},"commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS"}}}]}});
        let foreign = json!({"number":10,"headRefName":"feature/a","headRepository":{"nameWithOwner":"someone-else/r"}});
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/graphql").header("authorization", "Bearer fixture-only")
                .json_body(json!({"query":super::BRANCH_REVIEWS,"variables":{"owner":"o","name":"r","branch":"feature/a"}}));
            then.status(200).json_body(json!({"data":{"repository":{"pullRequests":{"pageInfo":{"hasNextPage":false},"nodes":[row,foreign]}}}}));
        });
        let mut registry = rupu_scm::Registry::empty();
        registry.insert_github_extras_account(
            AccountId::new("fixture"),
            Arc::new(GithubExtras::new(GithubClient::new(
                "fixture-only".into(),
                Some(server.base_url()),
                Some(2),
                Arc::new(rupu_netflow::NullSink),
            ))),
        );
        let adapter = super::CpRepoLister {
            registry: Arc::new(registry),
        };
        let result = adapter
            .reviews(
                &RepoRef {
                    platform: Platform::Github,
                    owner: "o".into(),
                    repo: "r".into(),
                },
                std::path::Path::new("/fixture"),
                "feature/a",
                "127.0.0.1",
            )
            .await
            .unwrap();
        let wrong_host = adapter
            .reviews(
                &RepoRef {
                    platform: Platform::Github,
                    owner: "o".into(),
                    repo: "r".into(),
                },
                std::path::Path::new("/fixture"),
                "feature/a",
                "github.com",
            )
            .await;
        assert!(wrong_host.is_err());
        mock.assert_hits(1);
        assert_eq!(result["account"], "fixture");
        assert_eq!(result["reviews"].as_array().unwrap().len(), 1);
        assert_eq!(result["reviews"][0]["checks_state"], "success");
        assert_eq!(result["reviews"][0]["head_sha"], "abc");
    }

    #[tokio::test]
    async fn branch_reviews_use_selected_gitlab_account_without_fork_aliasing() {
        use httpmock::{Method::GET, MockServer};
        use rupu_scm::connectors::gitlab::{GitlabClient, GitlabExtras};
        use serde_json::json;
        use std::sync::Arc;
        rupu_scm::install_default_crypto_provider();
        let server = MockServer::start();
        let list = server.mock(|when, then| {
            when.method(GET).path("/projects/group%2Fr/merge_requests").query_param("source_branch", "feature/a")
                .query_param("per_page", "6").header("authorization", "Bearer fixture-only");
            then.status(200).json_body(json!([
                {"iid":4,"source_branch":"feature/a","source_project_id":1,"target_project_id":1},
                {"iid":5,"source_branch":"feature/a","source_project_id":2,"target_project_id":1}]));
        });
        let detail = server.mock(|when, then| {
            when.method(GET).path("/projects/group%2Fr/merge_requests/4");
            then.status(200).json_body(json!({"iid":4,"title":"MR","source_branch":"feature/a","target_branch":"main","source_project_id":1,"target_project_id":1,"diff_refs":{"head_sha":"abc","start_sha":"def","base_sha":"ancestor"},"head_pipeline":{"status":"failed"}}));
        });
        let mut registry = rupu_scm::Registry::empty();
        registry.insert_gitlab_extras_account(
            AccountId::new("fixture"),
            Arc::new(GitlabExtras::new(GitlabClient::new(
                "fixture-only".into(),
                Some(server.base_url()),
                Some(2),
                Arc::new(rupu_netflow::NullSink),
            ))),
        );
        let adapter = super::CpRepoLister {
            registry: Arc::new(registry),
        };
        let result = adapter
            .reviews(
                &RepoRef {
                    platform: Platform::Gitlab,
                    owner: "group".into(),
                    repo: "r".into(),
                },
                std::path::Path::new("/fixture"),
                "feature/a",
                "127.0.0.1",
            )
            .await
            .unwrap();
        list.assert_hits(1);
        detail.assert_hits(1);
        assert_eq!(result["reviews"].as_array().unwrap().len(), 1);
        assert_eq!(result["reviews"][0]["checks_state"], "failed");
        assert_eq!(result["reviews"][0]["base_sha"], "def");
    }

    #[test]
    fn review_projection_keeps_exact_refs_and_absent_checks() {
        let github = super::github_review(
            &serde_json::json!({"number":9,"title":"Review","url":"https://github.com/o/r/pull/9","state":"OPEN","isDraft":true,"headRefName":"feature","headRefOid":"abc","baseRefName":"main","baseRefOid":"def"}),
        );
        assert_eq!(github["head_sha"], "abc");
        assert_eq!(github["base_sha"], "def");
        assert_eq!(github["state"], "open");
        assert!(github["checks_state"].is_null());
        let gitlab = super::gitlab_review(
            &serde_json::json!({"iid":4,"source_branch":"feature","target_branch":"main","diff_refs":{"head_sha":"abc","start_sha":"def","base_sha":"ancestor"},"head_pipeline":{"status":"failed"}}),
        );
        assert_eq!(gitlab["checks_state"], "failed");
        assert_eq!(gitlab["head_sha"], "abc");
        assert_eq!(gitlab["base_sha"], "def");
    }

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
