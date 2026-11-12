use super::*;
use crate::IssueConnector;
use httpmock::prelude::*;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
const TIME: &str = "2026-11-09T12:00:00Z";
fn reference(tracker: IssueTracker, project: &str) -> IssueRef {
    IssueRef {
        tracker,
        project: project.into(),
        number: 7,
    }
}
fn session(server: &MockServer, provider: IssueTracker, epoch: Arc<AtomicUsize>) -> Session {
    crate::install_default_crypto_provider();
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture"),
    );
    Session::new(
        provider,
        server.base_url(),
        server.url("/graphql"),
        "1".into(),
        http("discussion-test", Arc::new(rupu_netflow::NullSink)).unwrap(),
        headers,
        Arc::new(move || epoch.load(Ordering::SeqCst).to_string()),
    )
    .unwrap()
}
fn gh_identity() -> Value {
    json!({"data":{"repository":{"id":"repo-1","nameWithOwner":"org/repo","issue":{"id":"issue-7","number":7}}}})
}
fn gh_comment(server: &MockServer, id: u64, body: &str) -> Value {
    json!({"id":id,"url":server.url(format!("/repos/org/repo/issues/comments/{id}")),"issue_url":server.url("/repos/org/repo/issues/7"),"body":body,"user":{"login":"alice"},"created_at":TIME,"updated_at":TIME})
}

#[tokio::test]
async fn github_exact_connector_reads_full_text_between_lightweight_identity_queries() {
    crate::install_default_crypto_provider();
    let server = MockServer::start_async().await;
    let identity = server.mock(|w, t| {
        w.method(POST).path("/api/graphql").json_body(
            json!({"query":providers::GH_ID,"variables":{"owner":"org","name":"repo","number":7}}),
        );
        t.status(200).json_body(gh_identity());
    });
    let body = "é".repeat(12_000);
    let rows = server.mock(|w, t| {
        w.method(GET)
            .path("/repos/org/repo/issues/7/comments")
            .query_param("per_page", "20")
            .query_param("page", "1");
        t.status(200)
            .json_body(json!([gh_comment(&server, 9, &body)]));
    });
    let client = crate::connectors::github::GithubClient::new(
        "fixture".into(),
        Some(server.base_url()),
        None,
        Arc::new(rupu_netflow::NullSink),
    );
    let connector = crate::connectors::github::GithubIssueConnector::new(client);
    assert!(connector.discussion_supported());
    let page = connector
        .discussion_page(&reference(IssueTracker::Github, "org/repo"), None)
        .await
        .unwrap();
    assert_eq!(page.items[0].body, body);
    assert!(page.provider_exhausted);
    assert_eq!(
        page.identity.connector_epoch,
        connector.discussion_epoch().unwrap()
    );
    identity.assert_hits(2);
    rows.assert_hits(1);
}
#[tokio::test]
async fn github_foreign_comment_parent_is_never_published() {
    let server = MockServer::start_async().await;
    let s = session(&server, IssueTracker::Github, Arc::new(AtomicUsize::new(1)));
    server.mock(|w, t| {
        w.method(POST);
        t.status(200).json_body(gh_identity());
    });
    let mut row = gh_comment(&server, 9, "text");
    row["issue_url"] = json!(server.url("/repos/org/repo/issues/8"));
    server.mock(|w, t| {
        w.method(GET);
        t.status(200).json_body(json!([row]));
    });
    assert!(s
        .page(&reference(IssueTracker::Github, "org/repo"), None)
        .await
        .is_err());
}
#[tokio::test]
async fn gitlab_notes_bind_gid_parent_and_preserve_system_events() {
    crate::install_default_crypto_provider();
    let server = MockServer::start_async().await;
    let identity=server.mock(|w,t|{w.method(POST).path("/api/graphql").json_body(json!({"query":providers::GL_ID,"variables":{"path":"group/repo","iid":"7"}}));t.status(200).json_body(json!({"data":{"project":{"id":"gid://gitlab/Project/3","fullPath":"group/repo","issue":{"id":"gid://gitlab/Issue/11","iid":"7"}}}}));});
    let rows=server.mock(|w,t|{w.method(GET).path("/api/v4/projects/3/issues/7/notes").query_param("sort","asc");t.status(200).header("X-Next-Page","").json_body(json!([{"id":4,"noteable_type":"Issue","noteable_id":11,"system":true,"body":"recorded event","author":{"username":"alice"},"created_at":TIME,"updated_at":TIME}]));});
    let connector = crate::connectors::gitlab::GitlabIssueConnector::new(
        crate::connectors::gitlab::GitlabClient::new(
            "fixture".into(),
            Some(server.url("/api/v4")),
            None,
            Arc::new(rupu_netflow::NullSink),
        ),
    );
    let page = connector
        .discussion_page(&reference(IssueTracker::Gitlab, "group/repo"), None)
        .await
        .unwrap();
    assert_eq!(page.items[0].kind, "system");
    assert!(page.provider_exhausted);
    identity.assert_hits(2);
    rows.assert_hits(1);
}
#[tokio::test]
async fn linear_relay_continuation_keeps_exact_issue_and_provider_order() {
    crate::install_default_crypto_provider();
    let server = MockServer::start_async().await;
    let team = server.mock(|w, t| {
        w.method(POST)
            .json_body(json!({"query":providers::LINEAR_TEAM,"variables":{"id":"ENG"}}));
        t.status(200)
            .json_body(json!({"data":{"team":{"id":"team-1","key":"ENG"}}}));
    });
    let proof=server.mock(|w,t|{w.method(POST).json_body(json!({"query":providers::LINEAR_ID,"variables":{"id":"ENG-7"}}));t.status(200).json_body(json!({"data":{"issue":{"id":"issue-7","identifier":"ENG-7","team":{"id":"team-1","key":"ENG"}}}}));});
    let rows=server.mock(|w,t|{w.method(POST).json_body(json!({"query":providers::LINEAR_PAGE,"variables":{"id":"issue-7","after":"previous"}}));t.status(200).json_body(json!({"data":{"issue":{"id":"issue-7","identifier":"ENG-7","team":{"id":"team-1","key":"ENG"},"comments":{"nodes":[{"id":"c-1","body":"complete","createdAt":TIME,"updatedAt":TIME,"user":null,"issue":{"id":"issue-7"}}],"pageInfo":{"hasNextPage":true,"endCursor":"next"}}}}}));});
    let connector = crate::connectors::linear::LinearIssueConnector::new(
        "fixture".into(),
        Some(server.base_url()),
        Arc::new(rupu_netflow::NullSink),
    );
    let page = connector
        .discussion_page(&reference(IssueTracker::Linear, "ENG"), Some("previous"))
        .await
        .unwrap();
    assert_eq!(page.next.as_deref(), Some("next"));
    assert_eq!(page.order, "provider_created_order");
    assert_eq!(page.items[0].author, None);
    team.assert_hits(2);
    proof.assert_hits(2);
    rows.assert_hits(1);
}
#[tokio::test]
async fn jira_scope_context_path_and_adf_projection_are_explicit() {
    crate::install_default_crypto_provider();
    let server = MockServer::start_async().await;
    let proof=server.mock(|w,t|{w.method(GET).path("/jira/rest/api/3/issue/ENG-7").query_param("fields","project");t.status(200).json_body(json!({"id":"11","key":"ENG-7","self":server.url("/jira/rest/api/3/issue/11"),"fields":{"project":{"id":"3","key":"ENG"}}}));});
    let rows=server.mock(|w,t|{w.method(GET).path("/jira/rest/api/3/issue/11/comment").query_param("startAt","0").query_param("maxResults","20");t.status(200).json_body(json!({"startAt":0,"maxResults":20,"total":1,"comments":[{"id":"5","self":server.url("/jira/rest/api/3/issue/11/comment/5"),"author":{"displayName":"Alice"},"visibility":{"type":"role","value":"Developers"},"created":TIME,"updated":TIME,"body":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Hello","marks":[{"type":"strong"}]},{"type":"mention","attrs":{"text":"@Alice"}}]}]}}]}));});
    let connector = crate::connectors::jira::JiraIssueConnector::new(
        rupu_providers::auth::AuthCredentials::ApiKey {
            key: "fixture:token".into(),
        },
        Some(server.url("/jira")),
        Arc::new(rupu_netflow::NullSink),
    )
    .unwrap();
    let page = connector
        .discussion_page(&reference(IssueTracker::Jira, "ENG"), None)
        .await
        .unwrap();
    let c = &page.items[0];
    assert_eq!(c.body, "Hello@Alice");
    assert_eq!(c.format, "plain_text");
    assert!(c.metadata_truncated);
    assert!(!c.projection_complete);
    assert_eq!((c.unsupported_nodes, c.unsupported_marks), (1, 1));
    proof.assert_hits(2);
    rows.assert_hits(1);
    assert!(connector
        .discussion_page(
            &reference(IssueTracker::Jira, "https://foreign.example/ENG"),
            None
        )
        .await
        .is_err());
    rows.assert_hits(1);
}
#[tokio::test]
async fn transport_never_retries_refusal_or_follows_authenticated_redirect() {
    for status in [401, 429, 500, 307] {
        let server = MockServer::start_async().await;
        let other = MockServer::start_async().await;
        let target = other.mock(|w, t| {
            w.method(GET);
            t.status(200).json_body(json!({}));
        });
        let refused = server.mock(|w, t| {
            w.method(GET).header("authorization", "Bearer fixture");
            t.status(status)
                .header("Location", other.url("/foreign"))
                .json_body(json!({"private":"credential-should-not-leak"}));
        });
        let s = session(&server, IssueTracker::Github, Arc::new(AtomicUsize::new(1)));
        let error = s.get("/fixed", MAX_IDENTITY_BYTES).await.err().unwrap();
        assert!(!error.to_string().contains("credential-should-not-leak"));
        refused.assert_hits(1);
        target.assert_hits(0);
    }
}
#[tokio::test]
async fn transport_bounds_error_success_and_header_bytes_before_decode() {
    let response = |status, body: String| {
        reqwest::Response::from(
            ::http::Response::builder()
                .status(status)
                .body(body)
                .unwrap(),
        )
    };
    assert!(transport::bounded(response(200, " ".repeat(65)), 64)
        .await
        .is_err());
    assert!(transport::bounded(response(403, "secret".repeat(20)), 64)
        .await
        .is_err());
    let r = reqwest::Response::from(
        ::http::Response::builder()
            .status(200)
            .header("x-huge", "a".repeat(128 * 1024 + 1))
            .body("{}")
            .unwrap(),
    );
    assert!(transport::bounded(r, 64).await.is_err());
}
#[tokio::test]
async fn credential_epoch_drift_prevents_any_provider_request() {
    let server = MockServer::start_async().await;
    let mock = server.mock(|w, t| {
        w.method(GET);
        t.status(200).json_body(json!({}));
    });
    let epoch = Arc::new(AtomicUsize::new(1));
    let s = session(&server, IssueTracker::Github, epoch.clone());
    epoch.store(2, Ordering::SeqCst);
    assert!(s.get("/fixed", 64).await.is_err());
    mock.assert_hits(0);
}
#[test]
fn pagination_metadata_never_changes_authority_path_or_filters() {
    let origin = "https://gitlab.example/api/v4";
    let path = "/projects/3/issues/7/notes?per_page=20&page=1&order_by=created_at&sort=asc";
    let mut h = HeaderMap::new();
    assert!(transport::rest_next(&h, origin, path, 1, 20, true).is_err());
    assert_eq!(
        transport::rest_next(&h, origin, path, 1, 19, true).unwrap(),
        None
    );
    h.insert("x-next-page", HeaderValue::from_static("2"));
    assert_eq!(
        transport::rest_next(&h, origin, path, 1, 20, true).unwrap(),
        Some("2".into())
    );
    for url in ["https://other.example/api/v4/projects/3/issues/7/notes?per_page=20&page=2&order_by=created_at&sort=asc","https://gitlab.example/api/v4/projects/3/issues/8/notes?per_page=20&page=2&order_by=created_at&sort=asc","https://gitlab.example/api/v4/projects/3/issues/7/notes?per_page=20&page=2&order_by=created_at&sort=desc"]{
        h.insert(reqwest::header::LINK,HeaderValue::from_str(&format!("<{url}>; rel=\"next\"")).unwrap());assert!(transport::rest_next(&h,origin,path,1,20,true).is_err());
    }
}
#[test]
fn adf_depth_and_node_work_are_bounded_and_unknown_nodes_remain_visible() {
    let mut deep = json!({"type":"text","text":"deep"});
    for _ in 0..65 {
        deep = json!({"type":"paragraph","content":[deep]});
    }
    let deep = json!({"type":"doc","version":1,"content":[deep]});
    assert!(adf::render(&deep).is_err());
    let many =
        json!({"type":"doc","version":1,"content":vec![json!({"type":"text","text":"x"});10001]});
    assert!(adf::render(&many).is_err());
    let rendered = adf::render(
        &json!({"type":"doc","version":1,"content":[{"type":"media","attrs":{"id":"private"}}]}),
    )
    .unwrap();
    assert_eq!(rendered.nodes, 1);
    assert!(rendered.text.is_empty());
}
#[test]
fn private_cursors_and_ids_are_bounded_canonical_values() {
    assert!(transport::rest_next(
        &HeaderMap::new(),
        "https://api.github.com",
        "/repos/o/r/issues/7/comments?per_page=20&page=1000000",
        1_000_000,
        20,
        false
    )
    .is_err());
    assert!(page_number(Some("01"), 1).is_err());
    assert!(page_number(Some("0"), 1).is_err());
    assert!(page_number(Some("1000001"), 0).is_err());
    assert!(decimal(&json!("0007")).is_err());
    assert!(id(&json!("secret?url")).is_err());
}

#[tokio::test]
async fn gitlab_foreign_note_parent_and_linear_foreign_comment_issue_refuse() {
    let server = MockServer::start_async().await;
    let gl = session(&server, IssueTracker::Gitlab, Arc::new(AtomicUsize::new(1)));
    server.mock(|w,t|{w.method(POST).json_body(json!({"query":providers::GL_ID,"variables":{"path":"group/repo","iid":"7"}}));t.status(200).json_body(json!({"data":{"project":{"id":"gid://gitlab/Project/3","fullPath":"group/repo","issue":{"id":"gid://gitlab/Issue/11","iid":"7"}}}}));});
    server.mock(|w,t|{w.method(GET);t.status(200).json_body(json!([{"id":4,"noteable_type":"Issue","noteable_id":12,"system":false,"body":"foreign","created_at":TIME}]));});
    assert!(gl
        .page(&reference(IssueTracker::Gitlab, "group/repo"), None)
        .await
        .is_err());
    let server = MockServer::start_async().await;
    let linear = session(&server, IssueTracker::Linear, Arc::new(AtomicUsize::new(1)));
    server.mock(|w, t| {
        w.method(POST)
            .json_body(json!({"query":providers::LINEAR_TEAM,"variables":{"id":"ENG"}}));
        t.status(200)
            .json_body(json!({"data":{"team":{"id":"team-1","key":"ENG"}}}));
    });
    server.mock(|w,t|{w.method(POST).json_body(json!({"query":providers::LINEAR_ID,"variables":{"id":"ENG-7"}}));t.status(200).json_body(json!({"data":{"issue":{"id":"issue-7","identifier":"ENG-7","team":{"id":"team-1","key":"ENG"}}}}));});
    server.mock(|w,t|{w.method(POST).json_body(json!({"query":providers::LINEAR_PAGE,"variables":{"id":"issue-7","after":null}}));t.status(200).json_body(json!({"data":{"issue":{"id":"issue-7","identifier":"ENG-7","team":{"id":"team-1"},"comments":{"nodes":[{"id":"c-1","body":"foreign","createdAt":TIME,"issue":{"id":"other-issue"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}));});
    assert!(linear
        .page(&reference(IssueTracker::Linear, "ENG"), None)
        .await
        .is_err());
}
#[tokio::test]
async fn jira_foreign_comment_location_and_nonprogressing_total_refuse() {
    for foreign in [true, false] {
        let server = MockServer::start_async().await;
        let s = session(&server, IssueTracker::Jira, Arc::new(AtomicUsize::new(1)));
        server.mock(|w,t|{w.method(GET).path("/rest/api/3/issue/ENG-7");t.status(200).json_body(json!({"id":"11","key":"ENG-7","self":server.url("/rest/api/3/issue/11"),"fields":{"project":{"id":"3","key":"ENG"}}}));});
        server.mock(|w,t|{w.method(GET).path("/rest/api/3/issue/11/comment");t.status(200).json_body(if foreign {json!({"startAt":0,"maxResults":20,"total":1,"comments":[{"id":"4","self":server.url("/rest/api/3/issue/12/comment/4"),"created":TIME,"body":{"type":"doc","version":1,"content":[]}}]})}else{json!({"startAt":0,"maxResults":20,"total":2,"comments":[]})});});
        assert!(s
            .page(&reference(IssueTracker::Jira, "ENG"), None)
            .await
            .is_err());
    }
}
#[tokio::test]
async fn graphql_partial_errors_do_not_publish_identity() {
    let server = MockServer::start_async().await;
    let s = session(&server, IssueTracker::Github, Arc::new(AtomicUsize::new(1)));
    let mut value = gh_identity();
    value["errors"] = json!([{"message":"private provider diagnostic"}]);
    let proof = server.mock(|w, t| {
        w.method(POST);
        t.status(200).json_body(value);
    });
    let page = server.mock(|w, t| {
        w.method(GET);
        t.status(200).json_body(json!([]));
    });
    let error = s
        .page(&reference(IssueTracker::Github, "org/repo"), None)
        .await
        .err()
        .unwrap();
    assert!(!error.to_string().contains("private provider"));
    proof.assert_hits(1);
    page.assert_hits(0);
}

async fn read_test_request(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .unwrap()
            .unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        assert!(bytes.len() < 64 * 1024);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = std::str::from_utf8(&bytes[..end]).unwrap();
            let len = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + len {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
}
async fn reply_test_json(stream: &mut tokio::net::TcpStream, value: Value) {
    use tokio::io::AsyncWriteExt;
    let body = serde_json::to_vec(&value).unwrap();
    let header=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
    stream.write_all(header.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
    stream.shutdown().await.unwrap();
}
fn direct_session(origin: String, epoch: Arc<AtomicUsize>) -> Session {
    crate::install_default_crypto_provider();
    Session::new(
        IssueTracker::Github,
        origin.clone(),
        format!("{origin}/graphql"),
        "1".into(),
        http("discussion-fixture", Arc::new(rupu_netflow::NullSink)).unwrap(),
        HeaderMap::new(),
        Arc::new(move || epoch.load(Ordering::SeqCst).to_string()),
    )
    .unwrap()
}
#[tokio::test]
async fn response_arriving_after_credential_epoch_change_is_refused() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (arrived_tx, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) =
            tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let request = read_test_request(&mut stream).await;
        assert!(request.starts_with("GET /fixed "));
        arrived_tx.send(()).unwrap();
        release_rx.await.unwrap();
        reply_test_json(&mut stream, json!({"ok":true})).await;
    });
    let epoch = Arc::new(AtomicUsize::new(1));
    let s = direct_session(origin, epoch.clone());
    let read = tokio::spawn(async move { s.get("/fixed", MAX_IDENTITY_BYTES).await.map(|_| ()) });
    arrived_rx.await.unwrap();
    epoch.store(2, Ordering::SeqCst);
    release_tx.send(()).unwrap();
    let error = read.await.unwrap().err().unwrap();
    assert!(
        matches!(error,ScmError::BadRequest{message} if message=="discussion_selection_changed")
    );
    server.await.unwrap();
}
#[tokio::test]
async fn identity_changed_after_comment_read_never_publishes_a_page() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server_origin = origin.clone();
    let server = tokio::spawn(async move {
        for step in 0..3 {
            let (mut stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let request = read_test_request(&mut stream).await;
            let value = if step == 1 {
                assert!(request.starts_with("GET /repos/org/repo/issues/7/comments?"));
                json!([{"id":9,"url":format!("{server_origin}/repos/org/repo/issues/comments/9"),"issue_url":format!("{server_origin}/repos/org/repo/issues/7"),"body":"captured","user":{"login":"alice"},"created_at":TIME,"updated_at":TIME}])
            } else {
                assert!(request.starts_with("POST /graphql "));
                let mut proof = gh_identity();
                if step == 2 {
                    proof["data"]["repository"]["issue"]["id"] = json!("replaced-issue");
                }
                proof
            };
            reply_test_json(&mut stream, value).await;
        }
    });
    let s = direct_session(origin, Arc::new(AtomicUsize::new(1)));
    let error = s
        .page(&reference(IssueTracker::Github, "org/repo"), None)
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error,ScmError::BadRequest{message} if message=="discussion_selection_changed")
    );
    server.await.unwrap();
}
