use httpmock::prelude::*;
use rupu_scm::Platform;

mod common;

#[tokio::test]
async fn get_repo_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/repo_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_connector_against(&server);
    let r = c
        .get_repo(&rupu_scm::RepoRef {
            platform: rupu_scm::Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        })
        .await
        .unwrap();
    assert_eq!(r.r.repo, "rupu");
    assert!(r.private);
    assert_eq!(r.default_branch, "main");
}

#[tokio::test]
async fn get_pr_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/pr_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/pulls/42");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_connector_against(&server);
    let p = c
        .get_pr(&rupu_scm::PrRef {
            repo: rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            number: 42,
        })
        .await
        .unwrap();
    assert_eq!(p.title, "fix: streaming tokens");
    assert_eq!(p.head_branch, "feat/stream");
    assert_eq!(p.base_branch, "main");
    assert_eq!(p.author, "matias");
}

#[tokio::test]
async fn diff_pr_returns_unified_diff() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/pr_diff_happy.patch").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/pulls/42")
            .header("accept", "application/vnd.github.v3.diff");
        then.status(200).body(&body);
    });
    let c = common::github_connector_against(&server);
    let d = c
        .diff_pr(&rupu_scm::PrRef {
            repo: rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            number: 42,
        })
        .await
        .unwrap();
    assert!(d.patch.contains("diff --git a/src/main.rs b/src/main.rs"));
    assert_eq!(d.files_changed, 1);
}

#[tokio::test]
async fn read_file_decodes_base64() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/file_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/contents/README.md");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_connector_against(&server);
    let f = c
        .read_file(
            &rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            "README.md",
            None,
        )
        .await
        .unwrap();
    assert_eq!(f.path, "README.md");
    assert_eq!(f.encoding, rupu_scm::types::FileEncoding::Utf8);
    assert_eq!(f.content, "# hello\n");
}

#[tokio::test]
async fn list_repos_translates_octocrab_response() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/repos_list_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(GET).path("/user/repos");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });

    let connector = common::github_connector_against(&server);
    let repos = connector.list_repos().await.expect("list_repos");
    m.assert();

    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].r.platform, Platform::Github);
    assert_eq!(repos[0].r.owner, "section9labs");
    assert_eq!(repos[0].r.repo, "rupu");
    assert_eq!(repos[0].default_branch, "main");
    assert!(repos[0].private);
    assert_eq!(
        repos[0].clone_url_https,
        "https://github.com/section9labs/rupu.git"
    );
    assert_eq!(repos[0].description.as_deref(), Some("agentic coding CLI"));
    assert_eq!(repos[1].description, None);
}

#[tokio::test]
async fn list_prs_paginates_and_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/prs_list_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/pulls");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });

    let connector = common::github_connector_against(&server);
    let prs = connector
        .list_prs(
            &rupu_scm::RepoRef {
                platform: Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            rupu_scm::PrFilter::default(),
        )
        .await
        .expect("list_prs");
    m.assert();

    assert_eq!(prs.len(), 2);
    assert_eq!(prs[0].state, rupu_scm::PrState::Open);
    assert_eq!(prs[1].state, rupu_scm::PrState::Merged);
}

#[tokio::test]
async fn list_branches_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/branches_list_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/branches");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });

    let connector = common::github_connector_against(&server);
    let branches = connector
        .list_branches(&rupu_scm::RepoRef {
            platform: Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        })
        .await
        .expect("list_branches");
    m.assert();

    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0].name, "main");
    assert!(branches[0].protected);
    assert_eq!(branches[1].name, "feat/stream");
    assert!(!branches[1].protected);
}

#[tokio::test]
async fn get_issue_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/issue_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/issues/123");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_issue_connector_against(&server);
    let i = c
        .get_issue(&rupu_scm::IssueRef {
            tracker: rupu_scm::IssueTracker::Github,
            project: "section9labs/rupu".into(),
            number: 123,
        })
        .await
        .unwrap();
    assert_eq!(i.title, "Investigate flaky test");
    assert_eq!(i.state, rupu_scm::IssueState::Open);
    assert_eq!(i.labels, vec!["bug".to_string(), "ci".to_string()]);
}

#[tokio::test]
async fn list_issues_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/issues_list_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/issues");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_issue_connector_against(&server);
    let items = c
        .list_issues("section9labs/rupu", rupu_scm::IssueFilter::default())
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].labels, vec!["bug".to_string()]);
}
