use super::*;
use httpmock::prelude::*;
use serde_json::json;
fn transport(platform: Platform, base: &str) -> ReviewTransport {
    crate::install_default_crypto_provider();
    ReviewTransport::new(
        platform,
        base,
        Duration::from_secs(2),
        Arc::new(rupu_netflow::NullSink),
    )
}
fn h(name: &'static str, value: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(name, value.parse().unwrap());
    h
}
#[test]
fn origin_matches_full_scheme_authority_port_and_configured_web_prefix() {
    let gh = transport(Platform::Github, "https://api.github.com");
    assert!(gh.matches_origin("https://github.com/org/repo"));
    for origin in [
        "http://github.com/org/repo",
        "https://github.com:8443/org/repo",
        "https://api.github.com/org/repo",
        "https://user@github.com/org/repo",
        "https://github.com/org/repo?token=x",
        "https://github.com/org/repo/extra",
    ] {
        assert!(!gh.matches_origin(origin), "{origin}");
    }
    let gl = transport(Platform::Gitlab, "https://forge.test:8443/team/api/v4");
    assert!(gl.matches_origin("https://forge.test:8443/team/group/repo"));
    assert!(!gl.matches_origin("https://forge.test/team/group/repo"));
    assert!(!gl.matches_origin("https://forge.test:8443/group/repo"));
    let invalid = transport(Platform::Gitlab, "https://forge.test/wrong-api");
    assert!(!invalid.matches_origin("https://forge.test/group/repo"));
    assert!(invalid.get_request("/projects/1").is_err());
}
#[test]
fn connector_clone_retains_epoch_and_new_connector_changes_it_without_secret_derivation() {
    let one = transport(Platform::Github, "https://api.github.com");
    let cloned = one.clone();
    let other = transport(Platform::Github, "https://api.github.com");
    assert_eq!(one.identity_epoch(0), cloned.identity_epoch(0));
    assert_ne!(one.identity_epoch(0), other.identity_epoch(0));
    assert_ne!(one.identity_epoch(0), one.identity_epoch(1));
    assert_eq!(one.api_profile(), other.api_profile());
    assert!(one.api_profile().contains("2022-11-28"));
}
#[test]
fn fixed_paths_reject_absolute_traversal_encoded_aliases_and_unknown_queries() {
    for path in [
        "https://evil.test",
        "//evil.test/x",
        "/projects/../private",
        "/projects/x%2f..",
        "/projects/x%252f../merge_requests/1",
        "/projects/1/merge_requests/1?redirect=evil",
        "/projects/1/merge_requests/1?per_page=1&per_page=2",
        "/projects/1/merge_requests/1?page=513",
    ] {
        assert!(validate_path(Platform::Gitlab, path).is_err(), "{path}");
    }
    assert!(validate_path(
        Platform::Gitlab,
        "/projects/group%2Frepo/merge_requests/1/discussions?per_page=20&page=2"
    )
    .is_ok());
    assert!(validate_path(
        Platform::Gitlab,
        "/projects/1/pipelines/2/jobs?per_page=40&page=1&include_retried=false"
    )
    .is_ok());
    assert!(validate_path(
        Platform::Github,
        "/repos/org/repo/pulls/1/files?per_page=20&page=1"
    )
    .is_ok());
}
#[test]
fn link_scope_and_exact_forward_page_are_checked_never_followed() {
    let url = Url::parse(
        "https://forge.test/api/v4/projects/1/merge_requests/2/discussions?per_page=20&page=1",
    )
    .unwrap();
    let link="<https://forge.test/api/v4/projects/1/merge_requests/2/discussions?per_page=20&page=2>; rel=\"next\"";
    assert_eq!(pagination(&url, &h("link", link)).unwrap(), (Some(2), true));
    for wrong in [
        link.replace("forge.test", "evil.test"),
        link.replace("https:", "http:"),
        link.replace("projects/1", "projects/9"),
        link.replace("per_page=20", "per_page=10"),
        link.replace("page=2>", "page=1>"),
        link.replace("page=2>", "page=3>"),
    ] {
        assert!(pagination(&url, &h("link", &wrong)).is_err(), "{wrong}");
    }
}
#[test]
fn missing_pagination_is_distinct_from_explicit_exhausted_and_conflicts_refuse() {
    let url = Url::parse(
        "https://forge.test/api/v4/projects/1/merge_requests/2/discussions?page=1&per_page=20",
    )
    .unwrap();
    assert_eq!(pagination(&url, &HeaderMap::new()).unwrap(), (None, false));
    assert_eq!(
        pagination(&url, &h("x-next-page", "")).unwrap(),
        (None, true)
    );
    let mut headers = h("x-next-page", "2");
    headers.append("x-next-page", "3".parse().unwrap());
    assert!(pagination(&url, &headers).is_err());
    assert!(pagination(&url, &h("x-page", "2")).is_err());
    assert!(pagination(&url, &h("x-per-page", "40")).is_err());
    let mut headers=h("link","<https://forge.test/api/v4/projects/1/merge_requests/2/discussions?page=2&per_page=20>; rel=\"next\"");
    headers.insert("x-next-page", "".parse().unwrap());
    assert!(pagination(&url, &headers).is_err());
}
#[test]
fn bounded_queries_only_and_version_markers_preserve_unreported() {
    let gh = transport(Platform::Github, "https://api.github.com");
    assert!(gh
        .graphql_request("query Review { viewer { login } }", json!({}))
        .is_ok());
    assert!(gh
        .graphql_request("mutation Review { thing }", json!({}))
        .is_err());
    assert!(gh
        .graphql_request("query Review { x } mutation Write { y }", json!({}))
        .is_err());
    assert!(gh
        .graphql_request("query Review { x }", json!({"huge":"x".repeat(130*1024)}))
        .is_err());
    assert!(version(Platform::Github, &HeaderMap::new())
        .unwrap()
        .is_none());
    assert_eq!(
        version(
            Platform::Github,
            &h("x-github-api-version-selected", "2022-11-28")
        )
        .unwrap()
        .as_deref(),
        Some("x-github-api-version-selected:2022-11-28")
    );
    assert!(version(Platform::Gitlab, &h("x-gitlab-version", "")).is_err());
}
#[tokio::test]
async fn redirects_never_receive_credentials_and_errors_never_include_body() {
    let server = MockServer::start_async().await;
    let trap = server
        .mock_async(|when, then| {
            when.path("/trap");
            then.status(200).body("{}");
        })
        .await;
    let redirect = server
        .mock_async(|when, then| {
            when.path("/api/v4/projects/1");
            then.status(302)
                .header("location", format!("{}/trap", server.base_url()))
                .body("private-error-secret");
        })
        .await;
    let t = transport(Platform::Gitlab, &format!("{}/api/v4", server.base_url()));
    let (request, url) = t.get_request("/projects/1").unwrap();
    let response = request.bearer_auth("fixture-secret").send().await.unwrap();
    let err = t.finish(response, &url, MAX_REVIEW_BODY).await.unwrap_err();
    assert!(!err.to_string().contains("private-error-secret"));
    assert!(!err.to_string().contains("fixture-secret"));
    redirect.assert_hits_async(1).await;
    trap.assert_hits_async(0).await;
}
#[tokio::test]
async fn supplied_input_budget_is_enforced_before_json_decoding() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.path("/api/v4/projects/1");
            then.status(200).body(format!("\"{}\"", "x".repeat(2048)));
        })
        .await;
    let t = transport(Platform::Gitlab, &format!("{}/api/v4", server.base_url()));
    let (request, url) = t.get_request("/projects/1").unwrap();
    let response = request.send().await.unwrap();
    let error = t.finish(response, &url, 1024).await.unwrap_err();
    assert!(error.to_string().contains("byte limit"));
    mock.assert_hits_async(1).await;
}
#[tokio::test]
async fn fresh_reads_do_not_cache_and_full_page_without_metadata_refuses() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.path("/api/v4/projects/1/merge_requests/2/discussions")
                .header("accept-encoding", "identity");
            then.status(200).json_body(json!([{"id":"one"}]));
        })
        .await;
    let t = transport(Platform::Gitlab, &format!("{}/api/v4", server.base_url()));
    for _ in 0..2 {
        let (request, url) = t
            .get_request("/projects/1/merge_requests/2/discussions?per_page=1&page=1")
            .unwrap();
        let response = request.send().await.unwrap();
        assert!(t.finish(response, &url, MAX_REVIEW_BODY).await.is_err());
    }
    mock.assert_hits_async(2).await;
}
#[tokio::test]
async fn response_bytes_and_pagination_are_preserved_without_error_body_reads() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.path("/api/v4/projects/1/merge_requests/2/discussions");
            then.status(200)
                .header("x-next-page", "2")
                .header("x-page", "1")
                .header("x-per-page", "1")
                .body("[{\"id\":\"one\"}]");
        })
        .await;
    let t = transport(Platform::Gitlab, &format!("{}/api/v4", server.base_url()));
    let (request, url) = t
        .get_request("/projects/1/merge_requests/2/discussions?per_page=1&page=1")
        .unwrap();
    let response = request.send().await.unwrap();
    let page = t.finish(response, &url, MAX_REVIEW_BODY).await.unwrap();
    assert_eq!(page.body[0]["id"], "one");
    assert_eq!(page.next_page, Some(2));
    assert!(page.pagination_known);
    assert_eq!(page.bytes, 14);
    assert!(page.version.is_none());
    mock.assert_hits_async(1).await;
}

#[tokio::test]
async fn chunked_body_without_length_is_capped_before_decode() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        socket.read(&mut request).await.unwrap();
        socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n").await.unwrap();
        for _ in 0..3 {
            if socket.write_all(b"200\r\n").await.is_err() {
                break;
            }
            if socket.write_all(&[b'x'; 512]).await.is_err() {
                break;
            }
            if socket.write_all(b"\r\n").await.is_err() {
                break;
            }
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });
    let t = transport(Platform::Gitlab, &format!("http://{addr}/api/v4"));
    let (request, url) = t.get_request("/projects/1").unwrap();
    let response = request.send().await.unwrap();
    let error = t.finish(response, &url, 1024).await.unwrap_err();
    assert!(error.to_string().contains("byte limit"));
    server.await.unwrap();
}

#[tokio::test]
async fn transient_and_auth_errors_are_not_retried_or_body_echoed() {
    for status in [401, 429, 503] {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.path("/api/v4/projects/1");
                then.status(status)
                    .header("retry-after", "2")
                    .body("fixture-sensitive-error");
            })
            .await;
        let client = crate::connectors::gitlab::client::GitlabClient::new(
            "fixture-token".into(),
            Some(format!("{}/api/v4", server.base_url())),
            Some(1),
            Arc::new(rupu_netflow::NullSink),
        );
        let result = client.review_get("/projects/1").await.unwrap_err();
        assert!(!result.to_string().contains("fixture-sensitive-error"));
        mock.assert_hits_async(1).await;
    }
}

#[test]
fn graphql_profiles_preserve_public_and_enterprise_endpoint_derivation() {
    for (base, expected) in [
        ("https://api.github.com", "https://api.github.com/graphql"),
        (
            "https://enterprise.test",
            "https://enterprise.test/api/graphql",
        ),
        (
            "https://enterprise.test/api/v3",
            "https://enterprise.test/api/graphql",
        ),
        (
            "http://127.0.0.1:12345",
            "http://127.0.0.1:12345/api/graphql",
        ),
    ] {
        let transport = transport(Platform::Github, base);
        let (_, url) = transport
            .graphql_request("query Review { viewer { login } }", json!({}))
            .unwrap();
        assert_eq!(url.as_str(), expected);
    }
}
