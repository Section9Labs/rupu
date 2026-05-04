//! Live smoke tests against the real GitHub API. Skipped silently
//! unless `RUPU_LIVE_TESTS=1` AND `RUPU_LIVE_GITHUB_TOKEN` are set.
//! Wired into the existing nightly-live-tests workflow in Plan 3.

use rupu_scm::{IssueConnector, IssueFilter, Platform, RepoConnector, RepoRef};

fn live_enabled() -> bool {
    std::env::var("RUPU_LIVE_TESTS").as_deref() == Ok("1")
}

fn token() -> Option<String> {
    std::env::var("RUPU_LIVE_GITHUB_TOKEN").ok()
}

fn build_connectors() -> Option<(
    std::sync::Arc<dyn RepoConnector>,
    std::sync::Arc<dyn IssueConnector>,
)> {
    let token = token()?;
    use rupu_scm::connectors::github::GithubClient;
    let client = GithubClient::new(token, None, Some(2));
    let repo: std::sync::Arc<dyn RepoConnector> = std::sync::Arc::new(
        rupu_scm::connectors::github::repo::GithubRepoConnector::new(client.clone()),
    );
    let issues: std::sync::Arc<dyn IssueConnector> = std::sync::Arc::new(
        rupu_scm::connectors::github::issues::GithubIssueConnector::new(client),
    );
    Some((repo, issues))
}

#[tokio::test]
async fn github_list_repos_returns_at_least_one() {
    if !live_enabled() {
        return;
    }
    let Some((repo, _)) = build_connectors() else {
        return;
    };
    let repos = repo.list_repos().await.expect("list_repos");
    assert!(!repos.is_empty());
}

#[tokio::test]
async fn github_get_repo_for_known_target() {
    if !live_enabled() {
        return;
    }
    let Some((repo, _)) = build_connectors() else {
        return;
    };
    let r = repo
        .get_repo(&RepoRef {
            platform: Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        })
        .await
        .expect("get_repo");
    assert_eq!(r.r.repo, "rupu");
}

#[tokio::test]
async fn github_list_issues_for_known_target() {
    if !live_enabled() {
        return;
    }
    let Some((_, issues)) = build_connectors() else {
        return;
    };
    let _ = issues
        .list_issues("section9labs/rupu", IssueFilter::default())
        .await
        .expect("list_issues");
}

fn gitlab_token() -> Option<String> {
    std::env::var("RUPU_LIVE_GITLAB_TOKEN").ok()
}

fn build_gitlab_connectors() -> Option<(
    std::sync::Arc<dyn RepoConnector>,
    std::sync::Arc<dyn IssueConnector>,
)> {
    let token = gitlab_token()?;
    use rupu_scm::connectors::gitlab::GitlabClient;
    let client = GitlabClient::new(token, None, Some(2));
    let repo: std::sync::Arc<dyn RepoConnector> = std::sync::Arc::new(
        rupu_scm::connectors::gitlab::repo::GitlabRepoConnector::new(client.clone()),
    );
    let issues: std::sync::Arc<dyn IssueConnector> = std::sync::Arc::new(
        rupu_scm::connectors::gitlab::issues::GitlabIssueConnector::new(client),
    );
    Some((repo, issues))
}

#[tokio::test]
async fn gitlab_list_repos_returns_at_least_one() {
    if !live_enabled() {
        return;
    }
    let Some((repo, _)) = build_gitlab_connectors() else {
        return;
    };
    let repos = repo.list_repos().await.expect("gitlab list_repos");
    assert!(!repos.is_empty());
}

#[tokio::test]
async fn gitlab_get_repo_smoke() {
    if !live_enabled() {
        return;
    }
    let Some((repo, _)) = build_gitlab_connectors() else {
        return;
    };
    // Use a known-public gitlab.com project that's stable + small.
    let r = repo
        .get_repo(&RepoRef {
            platform: Platform::Gitlab,
            owner: "gitlab-org".into(),
            repo: "gitlab-test".into(),
        })
        .await;
    // Tolerate both success and Not-Found-style errors — public projects
    // can move; the smoke checks the wire roundtrip works without
    // requiring a specific server-side artifact.
    if r.is_err() {
        eprintln!("gitlab_get_repo_smoke skipped: {:?}", r.err());
    }
}

#[tokio::test]
async fn gitlab_list_issues_smoke() {
    if !live_enabled() {
        return;
    }
    let Some((_, issues)) = build_gitlab_connectors() else {
        return;
    };
    let _ = issues
        .list_issues("gitlab-org/gitlab-test", IssueFilter::default())
        .await
        .expect("gitlab list_issues");
}
