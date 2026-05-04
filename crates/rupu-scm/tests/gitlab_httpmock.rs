//! GitLab httpmock round-trip integration tests.

use httpmock::prelude::*;
use httpmock::Method::POST;
use rupu_scm::connectors::gitlab::client::GitlabClient;
use rupu_scm::connectors::gitlab::repo::GitlabRepoConnector;
use rupu_scm::RepoConnector;

mod common;

#[tokio::test]
async fn list_repos_paginates_until_empty() {
    let server = MockServer::start_async().await;
    let p1 = std::fs::read_to_string("tests/fixtures/gitlab/projects_list_paginated_page_1.json")
        .unwrap();
    let p2 = std::fs::read_to_string("tests/fixtures/gitlab/projects_list_paginated_page_2.json")
        .unwrap();

    server.mock(|when, then| {
        when.method(GET).path("/projects").query_param("page", "1");
        then.status(200)
            .header("content-type", "application/json")
            .body(&p1);
    });
    server.mock(|when, then| {
        when.method(GET).path("/projects").query_param("page", "2");
        then.status(200)
            .header("content-type", "application/json")
            .body(&p2);
    });
    server.mock(|when, then| {
        when.method(GET).path("/projects").query_param("page", "3");
        then.status(200)
            .header("content-type", "application/json")
            .body("[]");
    });

    let client = GitlabClient::new("fake-token".into(), Some(server.base_url()), Some(2));
    let conn = GitlabRepoConnector::new(client);
    let repos = conn.list_repos().await.unwrap();
    assert_eq!(repos.len(), 200, "two pages × 100 per page");
}

#[tokio::test]
async fn get_repo_translates() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/project_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/projects/section9labs%2Frupu-mirror");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let r = conn
        .get_repo(&rupu_scm::RepoRef {
            platform: rupu_scm::Platform::Gitlab,
            owner: "section9labs".into(),
            repo: "rupu-mirror".into(),
        })
        .await
        .unwrap();
    assert_eq!(r.r.repo, "rupu-mirror");
    assert!(r.private);
}

#[tokio::test]
async fn list_branches_translates() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/branches_list_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/projects/section9labs%2Frupu/repository/branches");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let bs = conn
        .list_branches(&rupu_scm::RepoRef {
            platform: rupu_scm::Platform::Gitlab,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        })
        .await
        .unwrap();
    assert_eq!(bs.len(), 2);
    assert_eq!(bs[0].name, "main");
    assert!(bs[0].protected);
    assert_eq!(bs[1].name, "feat/x");
    assert!(!bs[1].protected);
}

#[tokio::test]
async fn read_file_returns_raw_body() {
    let server = MockServer::start_async().await;
    let raw_body = "fn main() {\n    println!(\"hello rupu\");\n}\n";
    server.mock(|when, then| {
        when.method(GET)
            .path("/projects/section9labs%2Frupu/repository/files/src%2Fmain.rs/raw");
        then.status(200)
            .header("content-type", "text/plain")
            .body(raw_body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let f = conn
        .read_file(
            &rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Gitlab,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            "src/main.rs",
            None,
        )
        .await
        .unwrap();
    assert_eq!(f.path, "src/main.rs");
    assert_eq!(f.encoding, rupu_scm::types::FileEncoding::Utf8);
    assert!(f.content.contains("println!"));
}

#[tokio::test]
async fn get_pr_translates_mr() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/mr_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/projects/section9labs%2Frupu/merge_requests/42");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let p = conn
        .get_pr(&rupu_scm::PrRef {
            repo: rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Gitlab,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            number: 42,
        })
        .await
        .unwrap();
    assert_eq!(p.title, "feat: add streaming");
    assert_eq!(p.head_branch, "feat/stream");
    assert_eq!(p.base_branch, "main");
    assert_eq!(p.author, "matias");
}

#[tokio::test]
async fn list_prs_translates_with_state_filter() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/mrs_list_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/projects/section9labs%2Frupu/merge_requests");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let prs = conn
        .list_prs(
            &rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Gitlab,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            rupu_scm::PrFilter::default(),
        )
        .await
        .unwrap();
    assert_eq!(prs.len(), 2);
    assert_eq!(prs[0].state, rupu_scm::PrState::Open);
    assert_eq!(prs[1].state, rupu_scm::PrState::Merged);
}

#[tokio::test]
async fn diff_pr_aggregates_changes() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/mr_changes_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/projects/section9labs%2Frupu/merge_requests/42/changes");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let d = conn
        .diff_pr(&rupu_scm::PrRef {
            repo: rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Gitlab,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            number: 42,
        })
        .await
        .unwrap();
    assert_eq!(d.files_changed, 1);
    assert!(d.patch.contains("diff --git a/src/main.rs b/src/main.rs"));
    assert_eq!(d.additions, 1);
}

#[tokio::test]
async fn comment_pr_posts_note() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/note_create_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/projects/section9labs%2Frupu/merge_requests/42/notes");
        then.status(201)
            .header("content-type", "application/json")
            .body(&body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let comment = conn
        .comment_pr(
            &rupu_scm::PrRef {
                repo: rupu_scm::RepoRef {
                    platform: rupu_scm::Platform::Gitlab,
                    owner: "section9labs".into(),
                    repo: "rupu".into(),
                },
                number: 42,
            },
            "looks great",
        )
        .await
        .unwrap();
    m.assert();
    assert_eq!(comment.id, "7777");
    assert_eq!(comment.body, "looks great");
    assert_eq!(comment.author, "matias");
}

#[tokio::test]
async fn create_pr_posts_mr() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/mr_create_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/projects/section9labs%2Frupu/merge_requests");
        then.status(201)
            .header("content-type", "application/json")
            .body(&body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let pr = conn
        .create_pr(
            &rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Gitlab,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            rupu_scm::CreatePr {
                title: "feat: add foo".into(),
                body: "adds foo".into(),
                head: "feat/foo".into(),
                base: "main".into(),
                draft: false,
            },
        )
        .await
        .unwrap();
    m.assert();
    assert_eq!(pr.r.number, 200);
    assert_eq!(pr.title, "feat: add foo");
    assert_eq!(pr.head_branch, "feat/foo");
}

#[tokio::test]
async fn create_branch_posts_with_ref() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/branch_create_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/projects/section9labs%2Frupu/repository/branches")
            .query_param("branch", "feat/new-branch")
            .query_param("ref", "abc123");
        then.status(201)
            .header("content-type", "application/json")
            .body(&body);
    });
    let conn = common::gitlab_repo_connector_against(&server);
    let b = conn
        .create_branch(
            &rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Gitlab,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            "feat/new-branch",
            "abc123",
        )
        .await
        .unwrap();
    m.assert();
    assert_eq!(b.name, "feat/new-branch");
    assert!(!b.protected);
}
