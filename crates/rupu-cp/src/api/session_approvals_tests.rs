use super::*;
use axum::{body::Body, http::Request};
use tower::ServiceExt;

struct Starter(Arc<Broker>);
#[async_trait::async_trait]
impl crate::session_starter::SessionStarter for Starter {
    fn supports_session_approvals(&self) -> bool {
        true
    }
    fn approval_broker(&self) -> Option<Arc<Broker>> {
        Some(self.0.clone())
    }
    async fn start(
        &self,
        _: crate::session_starter::SessionStartRequest,
    ) -> Result<String, crate::session_starter::SessionStartError> {
        panic!("approval reads/decisions never start a session")
    }
}
fn fixture(auth: bool) -> (tempfile::TempDir, AppState) {
    let root = tempfile::tempdir().unwrap();
    let broker = Broker::open(root.path().into()).unwrap();
    let state = AppState::new(root.path().into(), rupu_config::PricingConfig::default())
        .with_token_set(auth)
        .with_session_starter(Some(Arc::new(Starter(broker))));
    (root, state)
}
#[tokio::test]
async fn tokenless_cp_does_not_advertise_or_expose_approvals() {
    let (_root, state) = fixture(false);
    assert!(!available(&state));
    assert_eq!(status(&state)["reason"], "authentication_required");
    let response = routes()
        .with_state(state)
        .oneshot(
            Request::builder()
                .uri("/api/sessions/ses_fixture/approvals?host=local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
}
#[tokio::test]
async fn actual_router_requires_authentication_even_with_live_broker() {
    let (_root, state) = fixture(true);
    assert!(available(&state));
    let app = crate::server::router(state, Some("fixture-only-token".into()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/ses_fixture/approvals?host=local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn remote_and_unbounded_pages_are_refused_without_starting_work() {
    let (_root, state) = fixture(true);
    for (uri, expected) in [
        (
            "/api/sessions/ses_fixture/approvals?host=remote",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/api/sessions/ses_fixture/approvals?host=local&limit=51",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = routes()
            .with_state(state.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}
#[test]
fn approval_routes_are_not_model_callable_operator_tools() {
    for operation in crate::mcp::ops::ops() {
        assert!(!operation.path.contains("/approvals"));
    }
    assert_eq!(
        crate::mcp::ops::EXCLUDED
            .iter()
            .filter(|(_, path, _)| path.contains("/approvals"))
            .count(),
        4
    );
}
