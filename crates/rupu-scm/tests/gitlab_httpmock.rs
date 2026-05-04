//! GitLab httpmock round-trip integration tests.

use httpmock::prelude::*;
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
