//! HTTP-level wiring for `GET /api/models` and `POST /api/models/refresh`:
//! the routes are mounted behind the bearer guard, 501 with no adapter, and
//! the refresh body extractor tolerates `{}` and a bodyless POST (the web
//! sends `{}` for "refresh everything").

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use rupu_config::PricingConfig;
use rupu_cp::model_catalog::{
    CatalogModel, CatalogProvider, ModelCatalog, ModelCatalogError, RefreshOutcome,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fake {
    asked: Mutex<Vec<Option<String>>>,
}

#[async_trait::async_trait]
impl ModelCatalog for Fake {
    async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError> {
        Ok(vec![CatalogProvider {
            provider: "anthropic".into(),
            fetched_at: None,
            stale: false,
            models: vec![CatalogModel {
                id: "claude-a".into(),
                input_tokens: Some(1000),
                output_tokens: None,
                source: "live".into(),
            }],
        }])
    }

    async fn refresh(
        &self,
        provider: Option<String>,
    ) -> Result<Vec<RefreshOutcome>, ModelCatalogError> {
        if provider.as_deref() == Some("nope") {
            return Err(ModelCatalogError::UnknownProvider(
                "unknown provider 'nope'".into(),
            ));
        }
        self.asked.lock().unwrap().push(provider.clone());
        Ok(vec![RefreshOutcome {
            provider: provider.unwrap_or_else(|| "anthropic".into()),
            ok: true,
            count: 1,
            error: None,
        }])
    }
}

async fn spawn(
    catalog: Option<Arc<dyn ModelCatalog>>,
    token: Option<String>,
) -> std::net::SocketAddr {
    let dir = tempfile::tempdir().unwrap();
    // Leak the tempdir so it outlives the spawned server for the test.
    let path = dir.keep();
    let state =
        rupu_cp::state::AppState::new(path, PricingConfig::default()).with_model_catalog(catalog);
    let app = rupu_cp::server::router(state, token);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn list_serves_the_catalog_as_json() {
    let addr = spawn(Some(Arc::new(Fake::default())), None).await;
    let resp = reqwest::get(format!("http://{addr}/api/models"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body[0]["provider"], "anthropic");
    assert_eq!(body[0]["models"][0]["id"], "claude-a");
    assert_eq!(body[0]["models"][0]["input_tokens"], 1000);
    assert!(body[0]["models"][0]["output_tokens"].is_null());
}

#[tokio::test]
async fn refresh_accepts_empty_object_named_provider_and_no_body() {
    let fake = Arc::new(Fake::default());
    let addr = spawn(Some(fake.clone()), None).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/models/refresh");

    // The web's "refresh all": a JSON `{}` body.
    let resp = client
        .post(&url)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // A named provider.
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "provider": "openai" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body[0]["provider"], "openai");
    assert_eq!(body[0]["ok"], true);

    // No body at all (curl -X POST).
    let resp = client.post(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    assert_eq!(
        fake.asked.lock().unwrap().as_slice(),
        [None, Some("openai".to_string()), None]
    );
}

/// A request meant to narrow the refresh to one provider must never fan out
/// to every vendor because its body was malformed: it is a 400 and the port
/// is not called.
#[tokio::test]
async fn refresh_rejects_a_malformed_body_without_calling_the_port() {
    let fake = Arc::new(Fake::default());
    let addr = spawn(Some(fake.clone()), None).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/models/refresh");

    for (label, raw) in [
        ("truncated json", r#"{"provider":"openai""#),
        ("wrong field type", r#"{"provider": 5}"#),
        ("misspelled field", r#"{"provder":"openai"}"#),
        ("not an object", r#""openai""#),
    ] {
        let resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(raw)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "{label}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .starts_with("invalid refresh body: "),
            "{label}: {body}"
        );
    }
    assert!(
        fake.asked.lock().unwrap().is_empty(),
        "a rejected body must not reach the port"
    );
}

/// Content-Type is not required: `curl -d '{"provider":"openai"}'` style
/// callers (and wrong-header clients) still narrow to the named provider.
#[tokio::test]
async fn refresh_honours_a_provider_sent_without_a_content_type() {
    let fake = Arc::new(Fake::default());
    let addr = spawn(Some(fake.clone()), None).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/models/refresh"))
        .body(r#"{"provider":"openai"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        fake.asked.lock().unwrap().as_slice(),
        [Some("openai".to_string())]
    );
}

/// An all-whitespace body is "no body": refresh everything.
#[tokio::test]
async fn refresh_treats_a_whitespace_body_as_refresh_all() {
    let fake = Arc::new(Fake::default());
    let addr = spawn(Some(fake.clone()), None).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/models/refresh"))
        .body("  \n\t ")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(fake.asked.lock().unwrap().as_slice(), [None]);
}

#[tokio::test]
async fn refresh_unknown_provider_is_400_with_the_message() {
    let addr = spawn(Some(Arc::new(Fake::default())), None).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/models/refresh"))
        .json(&serde_json::json!({ "provider": "nope" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "unknown provider 'nope'");
}

#[tokio::test]
async fn without_an_adapter_both_routes_are_501() {
    let addr = spawn(None, None).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 501);
    let resp = client
        .post(format!("http://{addr}/api/models/refresh"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 501);
}

#[tokio::test]
async fn both_routes_require_the_bearer_token() {
    let addr = spawn(Some(Arc::new(Fake::default())), Some("secret123".into())).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let resp = client
        .post(format!("http://{addr}/api/models/refresh"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let resp = client
        .get(format!("http://{addr}/api/models"))
        .header("Authorization", "Bearer secret123")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
