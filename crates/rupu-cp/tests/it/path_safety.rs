//! Path-parameter safety (`rupu_cp::path_guard` plus the per-handler checks).
//!
//! Axum percent-decodes path parameters, so `..%2F`, `%2Fetc%2Fpasswd` and a
//! raw `..` segment would otherwise reach the handlers that join them onto a
//! store directory. Requests go through `oneshot` on the router rather than
//! an HTTP client, which would normalize `..` segments before sending.

use http_body_util::BodyExt;
use rupu_config::PricingConfig;
use tower::ServiceExt;

const AGENT_MD: &str = "---\nname: foo\ndescription: \"a test agent\"\n---\nYou are foo.\n";
const WORKFLOW_YAML: &str =
    "name: wf\nsteps:\n  - id: step1\n    agent: foo\n    prompt: do stuff\n";

/// A global dir with one real agent + workflow, and a decoy of each one
/// level up (outside `agents/` / `workflows/`) that a traversal would read.
fn seeded() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    std::fs::create_dir_all(g.join("agents")).unwrap();
    std::fs::create_dir_all(g.join("workflows")).unwrap();
    std::fs::create_dir_all(g.join("sessions")).unwrap();
    std::fs::write(g.join("agents/foo.md"), AGENT_MD).unwrap();
    std::fs::write(g.join("workflows/wf.yaml"), WORKFLOW_YAML).unwrap();
    std::fs::write(g.join("decoy.md"), AGENT_MD.replace("foo", "decoy")).unwrap();
    std::fs::write(g.join("decoy.yaml"), WORKFLOW_YAML.replace("wf", "decoy")).unwrap();
    tmp
}

async fn get(global: &std::path::Path, uri: &str) -> (u16, serde_json::Value) {
    let state = rupu_cp::state::AppState::new(global.to_path_buf(), PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let resp = app
        .oneshot(
            http::Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn real_definitions_still_resolve() {
    let tmp = seeded();
    assert_eq!(get(tmp.path(), "/api/agents/foo").await.0, 200);
    assert_eq!(get(tmp.path(), "/api/workflows/wf").await.0, 200);
}

#[tokio::test]
async fn traversal_in_any_path_parameter_is_a_400() {
    let tmp = seeded();
    for uri in [
        "/api/agents/..%2Fdecoy",
        "/api/agents/..%2F..%2Fdecoy",
        "/api/agents/%2Fetc%2Fpasswd",
        "/api/workflows/..%2Fdecoy",
        "/api/workflows/%2Fetc%2Fhosts",
        "/api/sessions/..",
        "/api/sessions/%2E%2E",
        "/api/sessions/..%2Fworkflows",
        "/api/sessions/..%2Fsessions/runs",
        "/api/projects/..%2F..%2Fx",
        "/api/projects/%2Ftmp%2Fx/tree",
        "/api/runs/..%2F..%2Ftmp/cancel",
        "/api/runs/..%2Fx",
        "/api/coverage/..%2F..%2Fx/catalog",
        "/api/runs/x%3Fy",
        "/api/runs/-oProxyCommand",
    ] {
        let (status, body) = get(tmp.path(), uri).await;
        assert_eq!(status, 400, "{uri} → {status} {body}");
        assert!(body["error"].is_string(), "{uri}: {body}");
    }
}

#[tokio::test]
async fn fs_browse_is_scoped_to_home_and_projects() {
    let tmp = seeded();
    // `/` is outside $HOME and every registered project (none here).
    let (status, body) = get(tmp.path(), "/api/fs/browse?path=%2F").await;
    assert_eq!(status, 403, "{body}");
    assert!(body["error"].as_str().unwrap().contains("outside"));
}
