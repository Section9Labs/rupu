use httpmock::prelude::*;
use httpmock::Method::POST;
use rupu_scm::Platform;

mod common;

#[tokio::test]
async fn get_repo_translates() {
    rupu_scm::install_default_crypto_provider();
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
    rupu_scm::install_default_crypto_provider();
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
    assert_eq!(p.head_sha, "deadbeef");
    assert!(!p.draft);
    assert_eq!(p.author, "matias");
}

#[tokio::test]
async fn diff_pr_returns_unified_diff() {
    rupu_scm::install_default_crypto_provider();
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
    rupu_scm::install_default_crypto_provider();
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
    rupu_scm::install_default_crypto_provider();
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
    rupu_scm::install_default_crypto_provider();
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
    rupu_scm::install_default_crypto_provider();
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
    rupu_scm::install_default_crypto_provider();
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
    rupu_scm::install_default_crypto_provider();
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

#[tokio::test]
async fn comment_pr_posts_body() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/comment_create_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/section9labs/rupu/issues/42/comments");
        then.status(201)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_connector_against(&server);
    let comment = c
        .comment_pr(
            &rupu_scm::PrRef {
                repo: rupu_scm::RepoRef {
                    platform: Platform::Github,
                    owner: "section9labs".into(),
                    repo: "rupu".into(),
                },
                number: 42,
            },
            "LGTM, ship it.",
        )
        .await
        .unwrap();
    m.assert();
    assert_eq!(comment.id, "555");
    assert_eq!(comment.author, "matias");
    assert_eq!(comment.body, "LGTM, ship it.");
}

#[tokio::test]
async fn is_collaborator_204_is_true() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/collaborators/octocat");
        then.status(204);
    });
    let c = common::github_connector_against(&server);
    let is_collab = c
        .is_collaborator(
            &rupu_scm::RepoRef {
                platform: Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            "octocat",
        )
        .await
        .unwrap();
    m.assert();
    assert!(is_collab);
}

#[tokio::test]
async fn is_collaborator_404_is_false() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/collaborators/octocat");
        then.status(404)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({ "message": "Not Found" }));
    });
    let c = common::github_connector_against(&server);
    let is_collab = c
        .is_collaborator(
            &rupu_scm::RepoRef {
                platform: Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            "octocat",
        )
        .await
        .unwrap();
    m.assert();
    assert!(!is_collab);
}

#[tokio::test]
async fn create_pr_posts_payload() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/pr_create_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(POST).path("/repos/section9labs/rupu/pulls");
        then.status(201)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_connector_against(&server);
    let pr = c
        .create_pr(
            &rupu_scm::RepoRef {
                platform: Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            rupu_scm::CreatePr {
                title: "feat: new write paths".into(),
                body: "Adds create_branch, comment_pr, create_pr, clone_to.".into(),
                head: "feat/write-paths".into(),
                base: "main".into(),
                draft: false,
            },
        )
        .await
        .unwrap();
    m.assert();
    assert_eq!(pr.r.number, 99);
    assert_eq!(pr.title, "feat: new write paths");
    assert_eq!(pr.head_branch, "feat/write-paths");
    assert_eq!(pr.base_branch, "main");
}

#[tokio::test]
async fn list_comments_translates() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/issue_comments_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/issues/42/comments");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_issue_connector_against(&server);
    let comments = c
        .list_comments(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Github,
                project: "section9labs/rupu".into(),
                number: 42,
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(comments.len(), 2);
    // Oldest-first: GitHub's native order is preserved, which is what a
    // "what changed since my last iteration?" reader depends on.
    assert_eq!(comments[0].id, "1001");
    assert_eq!(comments[0].author, "mrbrutti");
    assert_eq!(comments[0].body, "first comment body");
    assert_eq!(comments[1].author, "octocat");
    assert_eq!(comments[1].body, "second comment body");
    assert!(comments[0].created_at < comments[1].created_at);
}

#[tokio::test]
async fn list_comments_respects_limit() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/issue_comments_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/issues/42/comments");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_issue_connector_against(&server);
    let comments = c
        .list_comments(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Github,
                project: "section9labs/rupu".into(),
                number: 42,
            },
            Some(1),
        )
        .await
        .unwrap();

    // The server returned two; `limit` truncates client-side so the
    // contract holds regardless of what the API page actually contained.
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, "1001");
}

/// Build a synthetic GitHub issue-comment JSON object with just the
/// fields the octocrab model requires (plus `body`, which the assertions
/// use to identify which page an item came from).
fn synth_comment(id: u64, login: &str, created_at: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "node_id": format!("IC_synth_{id}"),
        "url": format!("https://api.github.com/repos/section9labs/rupu/issues/42/comments/{id}"),
        "html_url": format!("https://github.com/section9labs/rupu/issues/42#issuecomment-{id}"),
        "issue_url": "https://api.github.com/repos/section9labs/rupu/issues/42",
        "body": format!("comment {id}"),
        "created_at": created_at,
        "updated_at": created_at,
        "user": {
            "login": login,
            "id": id + 900_000,
            "node_id": format!("U_synth_{id}"),
            "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4",
            "gravatar_id": "",
            "url": format!("https://api.github.com/users/{login}"),
            "html_url": format!("https://github.com/{login}"),
            "followers_url": format!("https://api.github.com/users/{login}/followers"),
            "following_url": format!("https://api.github.com/users/{login}/following{{/other_user}}"),
            "gists_url": format!("https://api.github.com/users/{login}/gists{{/gist_id}}"),
            "starred_url": format!("https://api.github.com/users/{login}/starred{{/owner}}{{/repo}}"),
            "subscriptions_url": format!("https://api.github.com/users/{login}/subscriptions"),
            "organizations_url": format!("https://api.github.com/users/{login}/orgs"),
            "repos_url": format!("https://api.github.com/users/{login}/repos"),
            "events_url": format!("https://api.github.com/users/{login}/events{{/privacy}}"),
            "received_events_url": format!("https://api.github.com/users/{login}/received_events"),
            "type": "User",
            "site_admin": false
        },
        "author_association": "NONE"
    })
}

#[tokio::test]
async fn list_comments_paginates_across_pages() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();

    // Page 1: a full page (100 items) — per_page is fixed at 100, so a
    // full page never signals "last page" on its own and the connector
    // must fetch page 2 to find out.
    let page1: Vec<serde_json::Value> = (1..=100)
        .map(|n| synth_comment(2000 + n, "page1-user", "2026-08-01T00:00:00Z"))
        .collect();
    // Page 2: a short page (10 items, < per_page) — this is the signal
    // that tells the connector to stop walking pages.
    let page2: Vec<serde_json::Value> = (1..=10)
        .map(|n| synth_comment(3000 + n, "page2-user", "2026-08-02T00:00:00Z"))
        .collect();

    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/issues/42/comments")
            .query_param("page", "1");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!(page1));
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/issues/42/comments")
            .query_param("page", "2");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!(page2));
    });

    let c = common::github_issue_connector_against(&server);
    let comments = c
        .list_comments(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Github,
                project: "section9labs/rupu".into(),
                number: 42,
            },
            // Spans the page boundary: page 1 alone (100) can't satisfy
            // this, so the real multi-page path must run. Also spans
            // into page 2 far enough that the post-pagination truncate
            // still has work to do (110 fetched -> 105 kept).
            Some(105),
        )
        .await
        .unwrap();

    assert_eq!(comments.len(), 105);
    // Last item of page 1, in place just before the boundary.
    assert_eq!(comments[99].id, "2100");
    assert_eq!(comments[99].author, "page1-user");
    // First item of page 2, proving the walk actually crossed pages
    // rather than stopping at page 1's (full) 100 items.
    assert_eq!(comments[100].id, "3001");
    assert_eq!(comments[100].author, "page2-user");
    // Last kept item: truncation to `limit` applies after pagination,
    // not instead of it.
    assert_eq!(comments[104].id, "3005");
}
