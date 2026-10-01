//! GitLab connector authentication: the header every request carries.
//!
//! GitLab takes an OAuth access token only as `Authorization: Bearer`
//! (docs.gitlab.com/api/rest/authentication); `PRIVATE-TOKEN` is for
//! personal / project / group access tokens, which also accept Bearer. So
//! every GitLab request sends Bearer, whatever kind of token it holds.

use httpmock::prelude::*;
use rupu_scm::connectors::gitlab::client::GitlabClient;
use rupu_scm::connectors::gitlab::repo::GitlabRepoConnector;
use rupu_scm::{PrRef, RepoConnector, RepoRef};

fn repo() -> RepoRef {
    RepoRef {
        platform: rupu_scm::Platform::Gitlab,
        owner: "section9labs".into(),
        repo: "rupu-mirror".into(),
    }
}

fn connector(server: &MockServer, token: &str) -> GitlabRepoConnector {
    GitlabRepoConnector::new(GitlabClient::new(
        token.into(),
        Some(server.base_url()),
        Some(2),
        std::sync::Arc::new(rupu_netflow::NullSink),
    ))
}

#[tokio::test]
async fn a_read_sends_the_token_as_a_bearer_token() {
    let server = MockServer::start_async().await;
    let body = std::fs::read_to_string("tests/fixtures/gitlab/project_get_happy.json").unwrap();
    let get = server.mock(|when, then| {
        when.method(GET)
            .path("/projects/section9labs%2Frupu-mirror")
            .header("authorization", "Bearer gl-oauth-access");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });

    connector(&server, "gl-oauth-access")
        .get_repo(&repo())
        .await
        .expect("the request carries the Bearer header");
    get.assert();
}

#[tokio::test]
async fn a_write_sends_the_token_as_a_bearer_token() {
    let server = MockServer::start_async().await;
    let post = server.mock(|when, then| {
        when.method(POST)
            .path("/projects/section9labs%2Frupu-mirror/merge_requests/7/notes")
            .header("authorization", "Bearer gl-oauth-access");
        then.status(201)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "id": 1,
                "body": "lgtm",
                "author": { "username": "matt" },
                "created_at": "2026-10-01T00:00:00Z",
            }));
    });

    connector(&server, "gl-oauth-access")
        .comment_pr(
            &PrRef {
                repo: repo(),
                number: 7,
            },
            "lgtm",
        )
        .await
        .expect("the request carries the Bearer header");
    post.assert();
}

#[tokio::test]
async fn a_raw_file_read_sends_the_token_as_a_bearer_token() {
    let server = MockServer::start_async().await;
    let raw = server.mock(|when, then| {
        when.method(GET)
            .path("/projects/section9labs%2Frupu-mirror/repository/files/README.md/raw")
            .header("authorization", "Bearer gl-oauth-access");
        then.status(200)
            .header("content-type", "text/plain")
            .body("# rupu");
    });

    let file = connector(&server, "gl-oauth-access")
        .read_file(&repo(), "README.md", None)
        .await
        .expect("the request carries the Bearer header");
    assert_eq!(file.content, "# rupu");
    raw.assert();
}
