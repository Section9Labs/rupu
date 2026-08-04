//! Proves rupu-scm's hand-rolled connectors go through the instrumented
//! netflow client, not a bare `reqwest::Client`.
//!
//! `connector_egress_carries_the_platform_as_origin` and
//! `private_token_in_a_query_never_reaches_the_record` exercise the netflow
//! factory directly (via `client_with`'s explicit-sink overload) with the
//! same tuning source (`ScmClientOptions::http_client_builder`) the migrated
//! connectors consume. On their own they would NOT catch a connector
//! reverting to a bare `reqwest::Client` — they never call into any
//! `rupu-scm` connector constructor.
//!
//! `real_connectors_reach_the_netflow_sink_with_scm_origin` is the test that
//! would catch that: it drives the actual migrated constructors —
//! `GitlabEventConnector`, `LinearEventConnector`, `JiraEventConnector`,
//! `JiraIssueConnector`, and `GithubClient::graphql_json` — against mock
//! servers and asserts the resulting flows land in the netflow sink under
//! `Origin::Scm(<platform>)`.
//!
//! `GithubClient::fetch_token_scopes` (the other ad-hoc GitHub client, in
//! `client.rs:145`) is migrated but NOT exercised here: it POSTs to a
//! hardcoded `https://api.github.com/user` (pre-existing, unrelated to this
//! migration) with no seam to redirect it to a mock server, so driving it
//! for real would require live network I/O against the real GitHub API.
//!
//! `rupu_netflow::http::init` is process-wide and first-call-wins. This file
//! has exactly ONE test that calls it (`real_connectors_...`); the other two
//! use `client_with`'s explicit-sink overload, which never touches the
//! global one — so there is no cross-test race within this binary.

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use rupu_scm::client_options::ScmClientOptions;
use std::sync::Arc;

#[tokio::test]
async fn connector_egress_carries_the_platform_as_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/api/v4/projects");
            then.status(200).body("[]");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Scm("gitlab".into())),
        ScmClientOptions::default().http_client_builder(),
        sink.clone(),
    )
    .unwrap();

    client
        .get(server.url("/api/v4/projects"))
        .send()
        .await
        .unwrap();

    let r = &sink.records()[0];
    assert_eq!(r.ctx.origin, Origin::Scm("gitlab".to_string()));
    assert_eq!(r.path, "/api/v4/projects");
    assert_eq!(r.fidelity, rupu_netflow::Fidelity::Http);
}

#[tokio::test]
async fn private_token_in_a_query_never_reaches_the_record() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/api/v4/user");
            then.status(200).body("{}");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Scm("gitlab".into())),
        ScmClientOptions::default().http_client_builder(),
        sink.clone(),
    )
    .unwrap();

    let url = format!("{}?private_token=glpat-LEAKME", server.url("/api/v4/user"));
    client.get(url).send().await.unwrap();

    let json = serde_json::to_string(&sink.records()[0]).unwrap();
    assert!(!json.contains("glpat-LEAKME"));
}

/// Drives the real migrated connector constructors — not the netflow
/// factory in isolation — against mock servers, and asserts each one's
/// egress lands in the sink under `Origin::Scm(<platform>)`.
#[tokio::test]
async fn real_connectors_reach_the_netflow_sink_with_scm_origin() {
    rupu_scm::install_default_crypto_provider();
    let sink = Arc::new(MemorySink::default());
    rupu_netflow::http::init(sink.clone());

    // ── GitLab: GitlabEventConnector::poll_events ──────────────────────────
    {
        use rupu_scm::connectors::gitlab::GitlabEventConnector;
        use rupu_scm::event_connector::EventConnector;
        use rupu_scm::{EventSourceRef, Platform, RepoRef};

        let server = httpmock::MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/v4/projects/acme%2Frepo/events");
                then.status(200)
                    .header("content-type", "application/json")
                    .body("[]");
            })
            .await;

        let connector = GitlabEventConnector::new("glpat-fake".into(), Some(server.base_url()));
        let source = EventSourceRef::Repo {
            repo: RepoRef {
                platform: Platform::Gitlab,
                owner: "acme".into(),
                repo: "repo".into(),
            },
        };
        // A cursor with `since:` already set skips the warmup fast-path
        // (empty cursor ⇒ no HTTP call at all) and forces the real GET.
        connector
            .poll_events(&source, Some("since:2020-01-01T00:00:00+00:00"), 10)
            .await
            .expect("gitlab poll_events against the mock server");
    }

    // ── Linear: LinearEventConnector::poll_events ──────────────────────────
    {
        use rupu_scm::connectors::linear::LinearEventConnector;
        use rupu_scm::event_connector::EventConnector;
        use rupu_scm::{EventSourceRef, IssueTracker};

        let server = httpmock::MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/");
                then.status(200)
                    .header("content-type", "application/json")
                    .json_body(serde_json::json!({
                        "data": {
                            "team": {
                                "issues": {
                                    "nodes": [],
                                    "pageInfo": { "hasNextPage": false, "endCursor": null }
                                }
                            }
                        }
                    }));
            })
            .await;

        let temp = tempfile::tempdir().unwrap();
        let connector = LinearEventConnector::new(
            "lin_api_fake".into(),
            Some(server.url("/")),
            Some(temp.path().to_path_buf()),
        );
        let source = EventSourceRef::TrackerProject {
            tracker: IssueTracker::Linear,
            project: "team-1".into(),
        };
        connector
            .poll_events(&source, None, 10)
            .await
            .expect("linear poll_events against the mock server");
    }

    // ── Jira: JiraEventConnector::poll_events ──────────────────────────────
    {
        use rupu_scm::connectors::jira::events::{JiraAuth, JiraEventConnector};
        use rupu_scm::event_connector::EventConnector;
        use rupu_scm::{EventSourceRef, IssueTracker};

        let server = httpmock::MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET).path("/rest/api/3/field");
                then.status(200).json_body(serde_json::json!([]));
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST)
                    .path("/rest/api/3/search/jql");
                then.status(200)
                    .json_body(serde_json::json!({ "issues": [], "nextPageToken": null }));
            })
            .await;

        let temp = tempfile::tempdir().unwrap();
        let connector = JiraEventConnector::new(
            JiraAuth::Basic {
                email: "matt@example.com".into(),
                token: "tok".into(),
            },
            Some(server.base_url()),
            Some(temp.path().to_path_buf()),
        );
        let source = EventSourceRef::TrackerProject {
            tracker: IssueTracker::Jira,
            project: "ENG".into(),
        };
        connector
            .poll_events(&source, None, 10)
            .await
            .expect("jira poll_events against the mock server");
    }

    // ── Jira: JiraIssueConnector::get_issue ─────────────────────────────────
    {
        use rupu_scm::connectors::jira::JiraIssueConnector;
        use rupu_scm::{IssueConnector, IssueRef, IssueTracker};

        let server = httpmock::MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/rest/api/3/issue/ENG-5");
                then.status(200).json_body(serde_json::json!({
                    "id": "10005",
                    "key": "ENG-5",
                    "self": "https://acme.atlassian.net/rest/api/3/issue/10005",
                    "fields": {
                        "summary": "Investigate flaky test",
                        "description": null,
                        "labels": [],
                        "status": { "statusCategory": { "key": "new", "name": "To Do" } },
                        "reporter": { "displayName": "matias" },
                        "created": "2026-05-10T00:00:00.000+0000",
                        "updated": "2026-05-10T00:00:00.000+0000"
                    }
                }));
            })
            .await;

        let creds = rupu_providers::auth::AuthCredentials::ApiKey {
            key: "matt@example.com:tok".into(),
        };
        let connector = JiraIssueConnector::new(creds, Some(server.base_url())).unwrap();
        connector
            .get_issue(&IssueRef {
                tracker: IssueTracker::Jira,
                project: "ENG".into(),
                number: 5,
            })
            .await
            .expect("jira get_issue against the mock server");
    }

    // ── GitHub: GithubClient::graphql_json (the ad-hoc client, not octocrab) ─
    {
        use rupu_scm::connectors::github::GithubClient;

        let server = httpmock::MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST).path("/api/graphql");
                then.status(200)
                    .json_body(serde_json::json!({ "data": { "viewer": { "login": "matias" } } }));
            })
            .await;

        let client = GithubClient::new("ghp_fake".into(), Some(server.base_url()), Some(1));
        client
            .graphql_json("query { viewer { login } }", serde_json::json!({}))
            .await
            .expect("github graphql_json against the mock server");
    }

    let records = sink.records();
    let origins: Vec<_> = records.iter().map(|r| r.ctx.origin.clone()).collect();
    assert_eq!(
        origins
            .iter()
            .filter(|o| **o == Origin::Scm("gitlab".into()))
            .count(),
        1,
        "gitlab: {origins:?}"
    );
    assert_eq!(
        origins
            .iter()
            .filter(|o| **o == Origin::Scm("linear".into()))
            .count(),
        1,
        "linear: {origins:?}"
    );
    assert_eq!(
        origins
            .iter()
            .filter(|o| **o == Origin::Scm("jira".into()))
            .count(),
        3,
        "jira: 2 calls from JiraEventConnector::poll_events (field list + search) \
         + 1 from JiraIssueConnector::get_issue: {origins:?}"
    );
    assert_eq!(
        origins
            .iter()
            .filter(|o| **o == Origin::Scm("github".into()))
            .count(),
        1,
        "github: {origins:?}"
    );
    assert_eq!(
        records.len(),
        6,
        "one flow per connector HTTP call: {origins:?}"
    );
    for r in records.iter() {
        assert_eq!(r.fidelity, rupu_netflow::Fidelity::Http);
    }
}
