//! Token auth for the `/api/*` surface (`rupu_cp::auth`).
//!
//! When a token is configured, `/api/*` requires `Authorization: Bearer
//! <token>` or the browser's token cookie (401 otherwise) while `/healthz` and
//! the static UI / SPA fallback stay open. The cookie is set by a page load
//! with `?token=`. With no token configured the API is a pass-through.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use rupu_config::PricingConfig;

async fn spawn(token: Option<String>) -> std::net::SocketAddr {
    let dir = tempfile::tempdir().unwrap();
    // Leak the tempdir so it outlives the spawned server for the test.
    let path = dir.keep();
    let state = rupu_cp::state::AppState::new(path, PricingConfig::default());
    let app = rupu_cp::server::router(state, token);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn api_requires_bearer_when_token_set() {
    let addr = spawn(Some("secret123".to_string())).await;
    let client = reqwest::Client::new();

    // No header → 401.
    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "missing token should be rejected");

    // Wrong token → 401.
    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .header("Authorization", "Bearer nope")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "wrong token should be rejected");

    // Correct token → 200.
    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .header("Authorization", "Bearer secret123")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "correct token should be accepted");
}

#[tokio::test]
async fn healthz_and_static_stay_open_with_token() {
    let addr = spawn(Some("secret123".to_string())).await;
    let client = reqwest::Client::new();

    // /healthz is open even with a token configured.
    let resp = client
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/healthz must stay open");

    // Static SPA fallback is open (the browser loads without a header).
    let resp = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "static UI must stay open");
}

#[tokio::test]
async fn api_open_when_no_token() {
    let addr = spawn(None).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "no token → API is a pass-through");
}

fn no_redirect() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Sign in the way a browser does (`/?token=`) and return the `name=value`
/// cookie pair the server set.
async fn signed_in_cookie(addr: std::net::SocketAddr, token: &str) -> String {
    let resp = no_redirect()
        .get(format!("http://{addr}/?token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let set = resp.headers()["set-cookie"].to_str().unwrap();
    set.split(';').next().unwrap().to_string()
}

#[tokio::test]
async fn unauthorized_uses_the_error_envelope() {
    let addr = spawn(Some("secret123".to_string())).await;
    let resp = reqwest::get(format!("http://{addr}/api/dashboard"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    let msg = body["error"]
        .as_str()
        .expect("401 carries {\"error\": ...}");
    assert!(msg.contains("?token="), "{msg}");
}

#[tokio::test]
async fn token_query_bootstraps_a_cookie_and_drops_the_token() {
    let addr = spawn(Some("secret123".to_string())).await;
    let resp = no_redirect()
        .get(format!("http://{addr}/runs/abc?token=secret123&tab=graph"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    assert_eq!(resp.headers()["location"], "/runs/abc?tab=graph");
    let cookie = resp.headers()["set-cookie"].to_str().unwrap().to_string();
    let port = addr.port();
    assert!(
        cookie.starts_with(&format!("rupu_cp_token_{port}=")),
        "{cookie}"
    );
    // The cookie carries a value derived from the token, never the token.
    assert!(!cookie.contains("secret123"), "{cookie}");
    for attr in ["HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(cookie.contains(attr), "{cookie} lacks {attr}");
    }

    // A bare `?token=` on `/` lands on `/`.
    let resp = no_redirect()
        .get(format!("http://{addr}/?token=secret123"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.headers()["location"], "/");

    // A protocol-relative path never becomes an off-site redirect.
    let resp = no_redirect()
        .get(format!("http://{addr}//evil.example/?token=secret123"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    assert_eq!(resp.headers()["location"], "/");
}

#[tokio::test]
async fn wrong_bootstrap_token_is_refused_without_a_cookie() {
    let addr = spawn(Some("secret123".to_string())).await;
    let resp = no_redirect()
        .get(format!("http://{addr}/?token=nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    assert!(resp.headers().get("set-cookie").is_none());
}

#[tokio::test]
async fn cookie_authenticates_api_and_sse_reads() {
    let addr = spawn(Some("secret123".to_string())).await;
    let cookie = signed_in_cookie(addr, "secret123").await;
    let value = cookie.split_once('=').unwrap().1.to_string();
    let client = no_redirect();
    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .header("Cookie", format!("other=1; {cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // EventSource sends only cookies — the firehose must accept them.
    let resp = client
        .get(format!("http://{addr}/api/events/stream"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

    // Wrong value, the raw token as a cookie, or the cookie of a control
    // plane on another port → 401.
    for bad in [
        format!("rupu_cp_token_{}=nope", addr.port()),
        format!("rupu_cp_token_{}=secret123", addr.port()),
        format!("rupu_cp_token_1={value}"),
    ] {
        let resp = client
            .get(format!("http://{addr}/api/dashboard"))
            .header("Cookie", bad)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    // The cookie value is no bearer token.
    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .header("Authorization", format!("Bearer {value}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn cookie_writes_need_a_same_origin_origin() {
    let addr = spawn(Some("secret123".to_string())).await;
    let cookie = signed_in_cookie(addr, "secret123").await;
    let url = format!("http://{addr}/api/runs/run_missing/cancel");
    let client = no_redirect();

    // No Origin, or another origin (another localhost port included) → 403.
    for origin in [None, Some("http://127.0.0.1:1".to_string())] {
        let mut req = client.post(&url).header("Cookie", &cookie);
        if let Some(o) = origin {
            req = req.header("Origin", o);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), 403);
    }

    // Same origin → past the guard (the handler's own answer, not 401/403).
    let resp = client
        .post(&url)
        .header("Cookie", &cookie)
        .header("Origin", format!("http://{addr}"))
        .send()
        .await
        .unwrap();
    assert!(
        ![401, 403].contains(&resp.status().as_u16()),
        "{}",
        resp.status()
    );

    // A bearer write needs no Origin.
    let resp = client
        .post(&url)
        .header("Authorization", "Bearer secret123")
        .send()
        .await
        .unwrap();
    assert!(![401, 403].contains(&resp.status().as_u16()));
}
