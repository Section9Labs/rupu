use httpmock::prelude::*;
use rupu_scm::Platform;

mod common;

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
