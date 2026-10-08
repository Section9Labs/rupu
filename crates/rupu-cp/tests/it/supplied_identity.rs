//! W6 §3.3 / §6 test 4: a placed unit's coordinator-minted run id and
//! codename across the tunnel and bucket transports.
//!
//! - The run id needs no capability: it travels outside `RunSpec` (the
//!   tunnel `Run` frame, the bucket job key) and every node runs `rupu run
//!   --run-id <that id>`, so both connectors use a supplied id as is.
//! - The codename is best-effort: it rides `RunSpec.codename` only to a node
//!   that advertised `run.codename`, and a node that didn't still launches.

use std::sync::Arc;

use object_store::memory::InMemory;
use rupu_cp::agent_launcher::AgentLaunchRequest;
use rupu_cp::host::bucket::{Bucket, BucketHostConnector, ObjectStoreBucket, WorkerInfo};
use rupu_cp::host::connector::{HostConnector, HostConnectorError};
use rupu_cp::host::tunnel::TunnelHostConnector;
use rupu_cp::node::protocol::{Frame, RunSpec, CAP_RUN_CODENAME};
use rupu_cp::node::{NodeMirror, NodeRegistry};
use rupu_orchestrator::RunStore;

const CODENAME: &str = "cobalt-harbor/heron#412";

fn placed(run_id: &str, codename: Option<&str>) -> AgentLaunchRequest {
    AgentLaunchRequest {
        agent: "recon".into(),
        prompt: Some("go".into()),
        mode: None,
        target: None,
        working_dir: None,
        run_id: Some(run_id.into()),
        findings_profile: None,
        engagement_profiles: Vec::new(),
        codename: codename.map(str::to_string),
    }
}

fn pricing() -> Arc<rupu_cp::customers::CustomerPricing> {
    Arc::new(rupu_cp::customers::CustomerPricing::flat(
        rupu_config::PricingConfig::default(),
    ))
}

/// A tunnel connector to one connected node that advertised `caps`, and the
/// node's end of the tunnel.
fn tunnel(
    dir: &std::path::Path,
    caps: &[&str],
) -> (TunnelHostConnector, tokio::sync::mpsc::Receiver<Frame>) {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let registry = Arc::new(NodeRegistry::new());
    registry.register_with_hello(
        "node-x",
        tx,
        caps.iter().map(|c| c.to_string()).collect(),
        Some("0.0.1".into()),
    );
    let run_store = Arc::new(RunStore::new(dir.join("runs")));
    let connector = TunnelHostConnector::new(
        "node-x",
        registry,
        Arc::new(NodeMirror::new(Arc::clone(&run_store))),
        run_store,
        pricing(),
    );
    (connector, rx)
}

/// A bucket connector, its run store and the bucket, whose pull workers
/// advertised `workers` (none: no marker at all).
async fn bucket(
    dir: &std::path::Path,
    workers: &[(&str, &[&str])],
) -> (BucketHostConnector, Arc<RunStore>, Arc<dyn Bucket>) {
    let run_store = Arc::new(RunStore::new(dir.join("runs")));
    let bucket: Arc<dyn Bucket> = Arc::new(ObjectStoreBucket::new(
        Arc::new(InMemory::new()),
        "test/host_IDENTITY",
    ));
    for (id, caps) in workers {
        let info = WorkerInfo {
            worker_id: id.to_string(),
            rupu_version: "9.9.9".into(),
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
        };
        bucket
            .put_worker_info(id, &serde_json::to_vec(&info).unwrap())
            .await
            .unwrap();
    }
    let connector = BucketHostConnector::new(
        "host_IDENTITY",
        Arc::clone(&bucket),
        Arc::new(NodeMirror::new(Arc::clone(&run_store))),
        Arc::clone(&run_store),
        pricing(),
    );
    (connector, run_store, bucket)
}

#[tokio::test]
async fn tunnel_bucket_use_supplied_run_id() {
    // Tunnel: the id is the `Run` frame's, whatever the node's version.
    let dir = tempfile::tempdir().unwrap();
    let (connector, mut rx) = tunnel(dir.path(), &[]);
    assert!(connector.honours_supplied_run_id());
    let id = connector
        .launch_agent(placed("run_01COORD", None))
        .await
        .unwrap();
    assert_eq!(id, "run_01COORD");
    let Some(Frame::Run { run_id, .. }) = rx.recv().await else {
        panic!("expected a Run frame")
    };
    assert_eq!(run_id, "run_01COORD");
    // A malformed id is refused before anything is sent.
    let err = connector
        .launch_agent(placed("../run", None))
        .await
        .unwrap_err();
    assert!(matches!(err, HostConnectorError::Invalid(_)), "{err:?}");
    assert!(rx.try_recv().is_err());

    // Bucket: the id keys the job (and the mirror run).
    let dir = tempfile::tempdir().unwrap();
    let (connector, run_store, bucket) = bucket(dir.path(), &[]).await;
    assert!(connector.honours_supplied_run_id());
    let id = connector
        .launch_agent(placed("run_01COORD", None))
        .await
        .unwrap();
    assert_eq!(id, "run_01COORD");
    assert!(bucket.get_job("run_01COORD").await.is_ok());
    assert!(run_store.load("run_01COORD").is_ok());
    assert!(matches!(
        connector
            .launch_agent(placed("run_../x", None))
            .await
            .unwrap_err(),
        HostConnectorError::Invalid(_)
    ));
}

#[tokio::test]
async fn codename_best_effort() {
    // Tunnel: the node that advertised `run.codename` gets it; one that
    // didn't still runs the unit, under its own name.
    for (caps, want) in [(vec![CAP_RUN_CODENAME], Some(CODENAME)), (vec![], None)] {
        let dir = tempfile::tempdir().unwrap();
        let (connector, mut rx) = tunnel(dir.path(), &caps);
        connector
            .launch_agent(placed("run_01NAMED", Some(CODENAME)))
            .await
            .expect("a codename never blocks a launch");
        let Some(Frame::Run { spec, .. }) = rx.recv().await else {
            panic!("expected a Run frame")
        };
        assert_eq!(spec.codename.as_deref(), want, "{caps:?}");
    }

    // Bucket: only when every worker that may claim the job advertised it.
    let capable: &[&str] = &[CAP_RUN_CODENAME];
    let old: &[&str] = &[];
    for (workers, want) in [
        (vec![("node_a", capable)], Some(CODENAME)),
        (vec![("node_a", capable), ("node_old", old)], None),
        (vec![], None),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (connector, _run_store, bucket) = bucket(dir.path(), &workers).await;
        let run_id = connector
            .launch_agent(placed("run_01NAMED", Some(CODENAME)))
            .await
            .expect("a codename never blocks a launch");
        let spec: RunSpec =
            serde_json::from_slice(&bucket.get_job(&run_id).await.unwrap()).unwrap();
        assert_eq!(spec.codename.as_deref(), want, "{workers:?}");
    }
}
