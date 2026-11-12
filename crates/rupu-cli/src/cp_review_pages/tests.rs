use super::*;
use httpmock::{
    Method::{GET, POST},
    MockServer,
};
use rupu_scm::AccountId;
fn repo(platform: Platform) -> RepoRef {
    RepoRef {
        platform,
        owner: "fixture".into(),
        repo: "repo".into(),
    }
}
fn selection() -> ReviewSelection {
    ReviewSelection {
        account: "fixture".into(),
        branch: "topic".into(),
        head: "abc".into(),
        include_diff: false,
    }
}
fn gh_identity() -> Value {
    json!({"data":{"repository":{"id":"repo_fixture","nameWithOwner":"fixture/repo","pullRequest":{"number":9,"headRefName":"topic","headRefOid":"abc","baseRefOid":"def","changedFiles":4,"headRepository":{"id":"repo_fixture"}}}}})
}
fn registry(server: &MockServer, platform: Platform) -> Registry {
    rupu_scm::install_default_crypto_provider();
    let mut r = Registry::empty();
    match platform {
        Platform::Github => r.insert_github_extras_account(
            AccountId::new("fixture"),
            Arc::new(GithubExtras::new(
                rupu_scm::connectors::github::GithubClient::new(
                    "fixture-only".into(),
                    Some(server.base_url()),
                    Some(2),
                    Arc::new(rupu_netflow::NullSink),
                ),
            )),
        ),
        Platform::Gitlab => r.insert_gitlab_extras_account(
            AccountId::new("fixture"),
            Arc::new(GitlabExtras::new(
                rupu_scm::connectors::gitlab::GitlabClient::new(
                    "fixture-only".into(),
                    Some(format!("{}/api/v4", server.base_url())),
                    Some(2),
                    Arc::new(rupu_netflow::NullSink),
                ),
            )),
        ),
    };
    r
}
#[tokio::test]
async fn github_check_page_two_uses_exact_server_cursor_and_head() {
    let server = MockServer::start();
    let reg = registry(&server, Platform::Github);
    let identity = server.mock(|when, then| {
        when.method(POST).path("/api/graphql").json_body(
            json!({"query":GH_ID,"variables":{"owner":"fixture","name":"repo","number":9}}),
        );
        then.status(200).json_body(gh_identity());
    });
    let page=server.mock(|when,then|{when.method(POST).path("/api/graphql").json_body(json!({"query":GH_CHECKS,"variables":{"owner":"fixture","name":"repo","head":"abc","after":"cursor-one"}}));then.status(200).json_body(json!({"data":{"repository":{"id":"repo_fixture","object":{"oid":"abc","statusCheckRollup":{"contexts":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"__typename":"CheckRun","id":"check41","name":"fixture check","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":null}]}}}}}}));});
    let p = read(
        &reg,
        &repo(Platform::Github),
        Path::new("/fixture"),
        &format!("{}/fixture/repo", server.base_url()),
        9,
        &selection(),
        None,
        &Request {
            section: "checks".into(),
            after: Some("cursor-one".into()),
            parent: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(p.items[0].id, "check41");
    assert!(p.provider_exhausted);
    assert!(p.next.is_none());
    identity.assert_hits(2);
    page.assert_hits(1);
}
#[tokio::test]
async fn nested_comment_parent_cannot_cross_pull_request() {
    let server = MockServer::start();
    let reg = registry(&server, Platform::Github);
    server.mock(|when, then| {
        when.method(POST).json_body(
            json!({"query":GH_ID,"variables":{"owner":"fixture","name":"repo","number":9}}),
        );
        then.status(200).json_body(gh_identity());
    });
    let comments=server.mock(|when,then|{when.method(POST).json_body(json!({"query":GH_COMMENTS,"variables":{"id":"thread_fixture","after":null}}));then.status(200).json_body(json!({"data":{"node":{"id":"thread_fixture","pullRequest":{"number":10,"repository":{"id":"repo_fixture"}},"comments":{"pageInfo":{"hasNextPage":false},"nodes":[]}}}}));});
    let p = read(
        &reg,
        &repo(Platform::Github),
        Path::new("/fixture"),
        &format!("{}/fixture/repo", server.base_url()),
        9,
        &selection(),
        None,
        &Request {
            section: "comments".into(),
            after: None,
            parent: Some("thread_fixture".into()),
        },
    )
    .await;
    assert!(matches!(p, Err(RepoListError::SelectionChanged)));
    comments.assert_hits(1);
}
#[tokio::test]
async fn changed_connector_lifetime_refuses_before_page_fetch() {
    let server = MockServer::start();
    let reg = registry(&server, Platform::Github);
    let mock = server.mock(|when, then| {
        when.method(POST).json_body(
            json!({"query":GH_ID,"variables":{"owner":"fixture","name":"repo","number":9}}),
        );
        then.status(200).json_body(gh_identity());
    });
    let origin = format!("{}/fixture/repo", server.base_url());
    let old = read_identity(
        &reg,
        &repo(Platform::Github),
        Path::new("/fixture"),
        &origin,
        9,
        &selection(),
    )
    .await
    .unwrap();
    let replaced = registry(&server, Platform::Github);
    let result = read(
        &replaced,
        &repo(Platform::Github),
        Path::new("/fixture"),
        &origin,
        9,
        &selection(),
        Some(&old),
        &Request {
            section: "checks".into(),
            after: None,
            parent: None,
        },
    )
    .await;
    assert!(matches!(result, Err(RepoListError::SelectionChanged)));
    mock.assert_hits(2);
}
#[tokio::test]
async fn gitlab_exact_head_pipeline_page_preserves_pagination() {
    let server = MockServer::start();
    let reg = registry(&server, Platform::Gitlab);
    let detail=server.mock(|when,then|{when.method(GET).path("/api/v4/projects/fixture%2Frepo/merge_requests/9");then.status(200).json_body(json!({"iid":9,"project_id":7,"source_project_id":7,"target_project_id":7,"source_branch":"topic","diff_refs":{"head_sha":"abc","start_sha":"def","base_sha":"merge"},"head_pipeline":{"id":12,"sha":"abc"}}));});
    let jobs=server.mock(|when,then|{when.method(GET).path("/api/v4/projects/7/pipelines/12/jobs").query_param("page","2").query_param("per_page","40").query_param("include_retried","false");then.status(200).header("x-page","2").header("x-next-page","").json_body(json!([{"id":41,"name":"fixture","status":"success","pipeline":{"id":12,"sha":"abc"},"web_url":null}]));});
    let p = read(
        &reg,
        &repo(Platform::Gitlab),
        Path::new("/fixture"),
        &format!("{}/fixture/repo", server.base_url()),
        9,
        &selection(),
        None,
        &Request {
            section: "checks".into(),
            after: Some("2".into()),
            parent: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(p.items[0].id, "41");
    assert!(p.provider_exhausted);
    detail.assert_hits(2);
    jobs.assert_hits(1);
}
#[test]
fn provider_connections_require_real_forward_cursors() {
    let request = Request {
        section: "checks".into(),
        after: Some("same".into()),
        parent: None,
    };
    assert!(graph_next(
        &json!({"pageInfo":{"hasNextPage":true,"endCursor":"same"}}),
        &request
    )
    .is_err());
    assert!(graph_next(&json!({"pageInfo":{"hasNextPage":true}}), &request).is_err());
    assert!(
        graph_next(&json!({"pageInfo":{"hasNextPage":false}}), &request)
            .unwrap()
            .is_none()
    );
    assert!(rows(&json!([1, 2]), 1).is_err());
}
#[test]
fn same_hostname_different_port_and_scheme_do_not_become_links() {
    let origin = "https://forge.invalid:8443/fixture/repo";
    assert!(link(&json!("https://forge.invalid/fixture/repo/pull/9"), origin).is_null());
    assert!(link(
        &json!("http://forge.invalid:8443/fixture/repo/pull/9"),
        origin
    )
    .is_null());
    assert!(link(
        &json!("https://forge.invalid:8443/fixture/repo/pull/9#discussion"),
        origin
    )
    .is_string());
}

#[test]
fn malformed_optional_provider_fields_never_become_nested_primary_json() {
    assert!(nullable_text(&json!({"unexpected":"object"}), 1024).is_err());
    assert!(nullable_bool(&json!("false")).is_err());
    assert!(nullable_line(&json!(-1)).is_err());
    assert_eq!(nullable_line(&Value::Null).unwrap(), Value::Null);
    assert_eq!(nullable_bool(&json!(false)).unwrap(), json!(false));
}
