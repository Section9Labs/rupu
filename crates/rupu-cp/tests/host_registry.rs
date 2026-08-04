//! Integration tests for `HostRegistry`.
//!
//! - resolve("local") → the local connector passed at construction.
//! - resolve(unknown) → HostConnectorError::NotFound.
//! - add_host then resolve → an Http connector aimed at the right base_url.
//! - remove_host("local") → HostConnectorError::Invalid.
//! - list_hosts puts "local" first.

use rupu_cp::host::{
    connector::{
        EventByteStream, HostConnector, HostConnectorError, HostInfo, RunListQuery,
    },
    registry::HostRegistry,
};
use rupu_cp::{
    agent_launcher::AgentLaunchRequest,
    launcher::LaunchRequest,
    session_sender::SendMessageRequest,
    session_starter::SessionStartRequest,
};
use rupu_workspace::HostStore;
use std::sync::Arc;

// ── Stub local connector ──────────────────────────────────────────────────────

/// Minimal stub that satisfies `HostConnector` so we can verify identity
/// comparisons without pulling in the full `LocalHostConnector`.
struct StubLocal;

#[async_trait::async_trait]
impl HostConnector for StubLocal {
    async fn info(&self) -> Result<HostInfo, HostConnectorError> {
        Ok(HostInfo {
            reachable: true,
            version: Some("stub".into()),
            capabilities: Default::default(),
        })
    }
    async fn launch_run(&self, _: LaunchRequest) -> Result<String, HostConnectorError> {
        unimplemented!()
    }
    async fn launch_agent(&self, _: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        unimplemented!()
    }
    async fn start_session(&self, _: SessionStartRequest) -> Result<String, HostConnectorError> {
        unimplemented!()
    }
    async fn send_session_turn(
        &self,
        _: SendMessageRequest,
    ) -> Result<String, HostConnectorError> {
        unimplemented!()
    }
    async fn list_runs(
        &self,
        _: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        unimplemented!()
    }
    async fn get_run(&self, _: &str) -> Result<serde_json::Value, HostConnectorError> {
        unimplemented!()
    }
    async fn approve_run(&self, _: &str, _: &str) -> Result<(), HostConnectorError> {
        unimplemented!()
    }
    async fn reject_run(&self, _: &str, _: Option<&str>) -> Result<(), HostConnectorError> {
        unimplemented!()
    }
    async fn cancel_run(&self, _: &str) -> Result<(), HostConnectorError> {
        unimplemented!()
    }
    async fn stream_run_events(&self, _: &str) -> Result<EventByteStream, HostConnectorError> {
        unimplemented!()
    }
    async fn get_transcript(&self, _: &str) -> Result<serde_json::Value, HostConnectorError> {
        unimplemented!()
    }
    async fn proxy_get_json(
        &self,
        _: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "local host is served in-process".into(),
        ))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_registry(tmp: &tempfile::TempDir) -> (HostRegistry, Arc<StubLocal>) {
    let store = HostStore { root: tmp.path().join("hosts") };
    let local: Arc<StubLocal> = Arc::new(StubLocal);
    let reg = HostRegistry::new(store, local.clone() as Arc<dyn HostConnector>);
    (reg, local)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `resolve("local")` returns the same connector object passed to `new`.
#[tokio::test]
async fn resolve_local_returns_local_connector() {
    let tmp = tempfile::tempdir().unwrap();
    let (reg, local) = make_registry(&tmp);

    let conn = reg.resolve("local").expect("resolve local should succeed");

    // The local connector should report reachable=true from our stub.
    let info = conn.info().await.unwrap();
    assert!(info.reachable);
    assert_eq!(info.version.as_deref(), Some("stub"));

    // Verify it IS the local stub (not some other connector) by checking via
    // Arc pointer equality.
    let local_any = local as Arc<dyn HostConnector>;
    assert!(
        Arc::ptr_eq(&conn, &local_any),
        "resolve('local') should return the exact local Arc"
    );
}

/// `resolve` on an unknown id returns `HostConnectorError::NotFound`.
#[test]
fn resolve_unknown_returns_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let (reg, _) = make_registry(&tmp);

    let result = reg.resolve("host_no_such");
    assert!(
        matches!(result, Err(HostConnectorError::NotFound(_))),
        "expected NotFound"
    );
}

/// After `add_host`, `resolve` returns an Http connector that points to the
/// registered base_url (verified by calling `info()` against a mock server
/// that returns 200 with a version string).
#[tokio::test]
async fn add_host_then_resolve_returns_http_connector_for_right_base_url() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(serde_json::json!({"version": "1.2.3"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let (reg, _) = make_registry(&tmp);

    let host = reg
        .add_host("test-remote", &server.base_url(), None)
        .expect("add_host should succeed");

    assert!(host.id.starts_with("host_"), "id should have 'host_' prefix");

    let conn = reg
        .resolve(&host.id)
        .expect("resolve after add_host should succeed");

    // The connector should reach our mock server and report the version.
    let info = conn.info().await.expect("info() should succeed");
    assert!(info.reachable, "connector should reach the mock server");
    assert_eq!(
        info.version.as_deref(),
        Some("1.2.3"),
        "version should match what the mock server returns"
    );
}

/// `remove_host("local")` is rejected with `HostConnectorError::Invalid`.
#[test]
fn remove_host_local_returns_invalid_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (reg, _) = make_registry(&tmp);

    let result = reg.remove_host("local");
    assert!(
        matches!(result, Err(HostConnectorError::Invalid(_))),
        "expected Invalid, got {result:?}"
    );
}

/// `list_hosts` always starts with the local host (id = "local"), followed by
/// any persisted hosts in sorted order.
#[test]
fn list_hosts_local_is_first() {
    let tmp = tempfile::tempdir().unwrap();
    let (reg, _) = make_registry(&tmp);

    // Without any persisted hosts, list should just be [local].
    let hosts = reg.list_hosts();
    assert!(!hosts.is_empty(), "list_hosts should return at least one entry");
    assert_eq!(hosts[0].id, "local", "first entry must always be 'local'");

    // Add two remote hosts and verify local is still first.
    reg.add_host("alpha", "https://alpha.example.com", None).unwrap();
    reg.add_host("beta", "https://beta.example.com", None).unwrap();

    let hosts = reg.list_hosts();
    assert!(hosts.len() >= 3, "should have local + 2 remotes");
    assert_eq!(hosts[0].id, "local", "local must remain first after add_host");
}

/// `resolve` after `remove_host` returns `NotFound`.
#[test]
fn resolve_after_remove_returns_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let (reg, _) = make_registry(&tmp);

    let host = reg
        .add_host("removable", "https://gone.example.com", None)
        .unwrap();

    // Should be resolvable.
    assert!(reg.resolve(&host.id).is_ok());

    // After removal, should be gone.
    reg.remove_host(&host.id).expect("remove_host should succeed");

    let result = reg.resolve(&host.id);
    assert!(
        matches!(result, Err(HostConnectorError::NotFound(_))),
        "expected NotFound after remove"
    );
}

/// Task 7 Step-1 acceptance test: a `Tunnel` host enrolled in the store
/// resolves via `HostRegistry` (when tunnel deps are wired via
/// `with_tunnel_deps`) and reports `reachable = false` because no node
/// has connected to the `NodeRegistry`.
///
/// Resolution path under test:
///   `enroll_node` → `HostStore` → `HostRegistry::resolve` → `TunnelHostConnector`
#[tokio::test]
async fn tunnel_host_resolves_reachable_false_when_no_node_connected() {
    use rupu_cp::node::{NodeMirror, NodeRegistry};
    use rupu_orchestrator::runs::RunStore;
    use rupu_workspace::enroll_node;

    let tmp = tempfile::tempdir().unwrap();

    // Enroll a tunnel node — this writes a Tunnel Host record to HostStore.
    let host_store = HostStore { root: tmp.path().join("hosts") };
    let (host, _token) = enroll_node(&host_store, "t7-test-node").unwrap();

    // Build the shared deps required by TunnelHostConnector.
    let run_store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let node_registry = Arc::new(NodeRegistry::new());
    let node_mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));

    // Build a HostRegistry with tunnel deps wired.
    let reg = HostRegistry::new(host_store, Arc::new(StubLocal) as Arc<dyn HostConnector>)
        .with_tunnel_deps(
            node_registry,
            node_mirror,
            run_store,
            rupu_config::PricingConfig::default(),
        );

    // resolve() must succeed — the Tunnel host record exists and deps are wired.
    let conn = reg
        .resolve(&host.id)
        .expect("resolve() must succeed when tunnel deps are wired");

    // info() must report reachable=false: no node has connected to NodeRegistry.
    let info = conn
        .info()
        .await
        .expect("info() must not error even when the node is offline");
    assert!(
        !info.reachable,
        "TunnelHostConnector must report reachable=false when no node is connected"
    );
}

/// Negative wiring guard: a `HostRegistry` built WITHOUT `with_tunnel_deps`
/// returns `HostConnectorError::Invalid` when resolving a `Tunnel` host.
///
/// This guards the requirement that `serve()` calls `with_tunnel_deps` before
/// any tunnel host can be resolved.
#[test]
fn tunnel_host_without_tunnel_deps_returns_invalid() {
    use rupu_workspace::enroll_node;

    let tmp = tempfile::tempdir().unwrap();

    let host_store = HostStore { root: tmp.path().join("hosts") };
    let (host, _token) = enroll_node(&host_store, "t7-unwired-node").unwrap();

    // Deliberately skip with_tunnel_deps.
    let reg = HostRegistry::new(host_store, Arc::new(StubLocal) as Arc<dyn HostConnector>);

    let result = reg.resolve(&host.id);
    assert!(
        matches!(result, Err(HostConnectorError::Invalid(_))),
        "expected Invalid when tunnel deps not wired"
    );
}

/// Task 5: an `Ssh` host persisted in the store resolves to an `SshHostConnector`
/// (when tunnel deps are wired via `with_tunnel_deps`) and `info()` returns
/// `reachable = false` because the SSH target (`nonexistent-rupu-test.invalid`)
/// is unreachable in the test environment.
///
/// Resolution path under test:
///   `add_ssh_host` → `HostStore` → `HostRegistry::resolve` → `SshHostConnector`
#[tokio::test]
async fn ssh_host_resolves_reachable_false_when_ssh_unreachable() {
    use rupu_cp::node::{NodeMirror, NodeRegistry};
    use rupu_orchestrator::runs::RunStore;
    use rupu_workspace::add_ssh_host;

    let tmp = tempfile::tempdir().unwrap();

    // Persist an SSH host record pointing at a host that will never respond.
    let host_store = HostStore { root: tmp.path().join("hosts") };
    let host = add_ssh_host(
        &host_store,
        "test-ssh-node",
        "nonexistent-rupu-test.invalid",
        None,
        None,
    )
    .unwrap();

    // Build the shared deps (same bundle used by TunnelHostConnector).
    let run_store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let node_registry = Arc::new(NodeRegistry::new());
    let node_mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));

    // Build a HostRegistry with tunnel deps wired — SSH uses mirror + run_store
    // from this same bundle.
    let reg = HostRegistry::new(host_store, Arc::new(StubLocal) as Arc<dyn HostConnector>)
        .with_tunnel_deps(
            node_registry,
            node_mirror,
            run_store,
            rupu_config::PricingConfig::default(),
        );

    // resolve() must succeed — the SSH host record exists and deps are wired.
    let conn = reg
        .resolve(&host.id)
        .expect("resolve() must succeed when tunnel deps are wired");

    // info() must return Ok with reachable=false: the remote SSH target is
    // unreachable in the test environment (BatchMode=yes + ConnectTimeout=10
    // ensures ssh exits fast rather than hanging).
    let info = conn
        .info()
        .await
        .expect("info() must not error even when ssh target is unreachable");
    assert!(
        !info.reachable,
        "SshHostConnector must report reachable=false when the remote ssh target is offline"
    );
}

/// Task 4: a `Bucket` host persisted in the store resolves to a
/// `BucketHostConnector` (when tunnel deps are wired via `with_tunnel_deps`) and
/// `info()` returns `Ok` because the `file://` local-filesystem backend is
/// reachable in the test environment.
///
/// Resolution path under test:
///   `add_bucket_host` → `HostStore` → `HostRegistry::resolve` → `BucketHostConnector`
#[tokio::test]
async fn bucket_host_resolves_ok_with_deps() {
    use rupu_cp::node::{NodeMirror, NodeRegistry};
    use rupu_orchestrator::runs::RunStore;
    use rupu_workspace::add_bucket_host;

    let tmp = tempfile::tempdir().unwrap();

    // Create the bucket directory so the local filesystem backend can probe it.
    let bucket_dir = tmp.path().join("bucket");
    std::fs::create_dir_all(&bucket_dir).unwrap();
    let bucket_url = format!("file://{}", bucket_dir.display());

    let host_store = HostStore { root: tmp.path().join("hosts") };
    let host = add_bucket_host(&host_store, "test-bucket-node", &bucket_url, None).unwrap();

    // Build the shared deps required by BucketHostConnector (same bundle as Tunnel/SSH).
    let run_store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let node_registry = Arc::new(NodeRegistry::new());
    let node_mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));

    // Build a HostRegistry with tunnel deps wired — Bucket uses mirror + run_store
    // from this same bundle.
    let reg = HostRegistry::new(host_store, Arc::new(StubLocal) as Arc<dyn HostConnector>)
        .with_tunnel_deps(
            node_registry,
            node_mirror,
            run_store,
            rupu_config::PricingConfig::default(),
        );

    // resolve() must succeed — the Bucket host record exists and deps are wired.
    let conn = reg
        .resolve(&host.id)
        .expect("resolve() must succeed when tunnel deps are wired for a Bucket host");

    // info() must return Ok — the file:// backend is reachable.
    let info = conn
        .info()
        .await
        .expect("info() must not error for a local-filesystem bucket");
    assert!(
        info.reachable,
        "BucketHostConnector must report reachable=true for a reachable file:// bucket"
    );
}

/// Negative wiring guard (Bucket): a `HostRegistry` built WITHOUT
/// `with_tunnel_deps` returns `HostConnectorError::Invalid` when resolving a
/// `Bucket` host.
///
/// Bucket needs `node_mirror` + `run_store` from the `with_tunnel_deps` bundle;
/// skipping the call must produce `Invalid`.
#[test]
fn bucket_host_without_tunnel_deps_returns_invalid() {
    use rupu_workspace::add_bucket_host;

    let tmp = tempfile::tempdir().unwrap();

    let host_store = HostStore { root: tmp.path().join("hosts") };
    let host =
        add_bucket_host(&host_store, "test-bucket-unwired", "file:///tmp/rupu-test-unwired", None)
            .unwrap();

    // Deliberately skip with_tunnel_deps.
    let reg = HostRegistry::new(host_store, Arc::new(StubLocal) as Arc<dyn HostConnector>);

    let result = reg.resolve(&host.id);
    assert!(
        matches!(result, Err(HostConnectorError::Invalid(_))),
        "expected Invalid when tunnel deps not wired for Bucket host"
    );
}

/// Negative wiring guard (SSH): a `HostRegistry` built WITHOUT `with_tunnel_deps`
/// returns `HostConnectorError::Invalid` when resolving an `Ssh` host.
///
/// SSH needs `node_mirror` + `run_store` from the same `with_tunnel_deps` bundle
/// that Tunnel uses; skipping the call must produce `Invalid`.
#[test]
fn ssh_host_without_tunnel_deps_returns_invalid() {
    use rupu_workspace::add_ssh_host;

    let tmp = tempfile::tempdir().unwrap();

    let host_store = HostStore { root: tmp.path().join("hosts") };
    let host = add_ssh_host(
        &host_store,
        "test-ssh-unwired",
        "nonexistent-rupu-test.invalid",
        None,
        None,
    )
    .unwrap();

    // Deliberately skip with_tunnel_deps.
    let reg = HostRegistry::new(host_store, Arc::new(StubLocal) as Arc<dyn HostConnector>);

    let result = reg.resolve(&host.id);
    assert!(
        matches!(result, Err(HostConnectorError::Invalid(_))),
        "expected Invalid when tunnel deps not wired for SSH host"
    );
}
