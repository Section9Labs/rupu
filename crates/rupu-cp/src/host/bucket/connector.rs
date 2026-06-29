//! Bucket transport: dispatch/observe/control runs via the shared dead-drop bucket.
//!
//! The CP writes a job envelope (`jobs/<run_id>.json`) for each dispatched run;
//! a node agent polls `list_jobs`, claims one, and executes it.  Control messages
//! (cancel/approve/reject) are queued as `control/<run_id>/<seq:020>.json`
//! and read by the node agent during execution.  Observation delegates to the
//! shared `mirror_*` helpers — the same in-process [`NodeMirror`] / [`RunStore`]
//! that the tunnel connector uses.

use std::sync::Arc;

use rupu_orchestrator::runs::RunStore;
use ulid::Ulid;

use crate::{
    agent_launcher::AgentLaunchRequest,
    host::{
        bucket::{Bucket, BucketError, ControlEnvelope},
        connector::{
            mirror_get_run, mirror_list_runs, mirror_stream_run_events, read_transcript_file,
            EventByteStream, HostCapabilities, HostConnector, HostConnectorError, HostInfo,
            RunListQuery,
        },
    },
    launcher::LaunchRequest,
    node::{
        protocol::{RunSpec, RunSpecKind},
        NodeMirror,
    },
    session_sender::SendMessageRequest,
    session_starter::SessionStartRequest,
};

// ── BucketHostConnector ───────────────────────────────────────────────────────

/// [`HostConnector`] backed by the bucket dead-drop transport.
///
/// Dispatches workflow/agent runs by writing a [`RunSpec`] job envelope into
/// the bucket; a node agent polls the bucket, claims the job, and executes it.
/// Control operations (cancel/approve/reject) are queued as [`ControlEnvelope`]
/// objects in the bucket's control prefix.  Observation reads back from the
/// shared [`NodeMirror`] / [`RunStore`], identical to the tunnel connector.
pub(crate) struct BucketHostConnector {
    host_id: String,
    bucket: Arc<dyn Bucket>,
    mirror: Arc<NodeMirror>,
    run_store: Arc<RunStore>,
    pricing: rupu_config::PricingConfig,
}

impl BucketHostConnector {
    /// Construct a new connector.
    pub(crate) fn new(
        host_id: impl Into<String>,
        bucket: Arc<dyn Bucket>,
        mirror: Arc<NodeMirror>,
        run_store: Arc<RunStore>,
        pricing: rupu_config::PricingConfig,
    ) -> Self {
        Self {
            host_id: host_id.into(),
            bucket,
            mirror,
            run_store,
            pricing,
        }
    }

    /// Write a [`ControlEnvelope`] for `run_id` at the next available seq.
    ///
    /// Seq is derived from the current count of control messages — good enough
    /// for the CP-side write path since only the CP emits control envelopes.
    async fn put_control_envelope(
        &self,
        run_id: &str,
        envelope: ControlEnvelope,
    ) -> Result<(), HostConnectorError> {
        let existing = self
            .bucket
            .list_control(run_id)
            .await
            .map_err(bucket_err_to_unreachable)?;
        let seq = existing.len() as u64;
        let bytes =
            serde_json::to_vec(&envelope).map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        self.bucket
            .put_control(run_id, seq, &bytes)
            .await
            .map_err(bucket_err_to_unreachable)?;
        Ok(())
    }
}

fn bucket_err_to_unreachable(e: BucketError) -> HostConnectorError {
    HostConnectorError::Unreachable(e.to_string())
}

// ── HostConnector impl ────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl HostConnector for BucketHostConnector {
    async fn info(&self) -> Result<HostInfo, HostConnectorError> {
        let reachable = self.bucket.probe().await.is_ok();
        Ok(HostInfo {
            reachable,
            version: None,
            capabilities: HostCapabilities::default(),
        })
    }

    async fn launch_run(&self, req: LaunchRequest) -> Result<String, HostConnectorError> {
        let run_id = format!("run_{}", Ulid::new());

        let spec = RunSpec {
            kind: RunSpecKind::Workflow,
            name: req.workflow.clone(),
            inputs: req.inputs.clone(),
            prompt: None,
            mode: req.mode.clone(),
            target: req.target.clone(),
        };

        self.mirror
            .create_run(&run_id, &self.host_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        let bytes =
            serde_json::to_vec(&spec).map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        self.bucket
            .put_job(&run_id, &bytes)
            .await
            .map_err(bucket_err_to_unreachable)?;

        Ok(run_id)
    }

    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        let run_id = format!("run_{}", Ulid::new());

        let spec = RunSpec {
            kind: RunSpecKind::Agent,
            name: req.agent.clone(),
            inputs: std::collections::BTreeMap::new(),
            prompt: req.prompt.clone(),
            mode: req.mode.clone(),
            target: req.target.clone(),
        };

        self.mirror
            .create_run(&run_id, &self.host_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        let bytes =
            serde_json::to_vec(&spec).map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        self.bucket
            .put_job(&run_id, &bytes)
            .await
            .map_err(bucket_err_to_unreachable)?;

        Ok(run_id)
    }

    async fn start_session(
        &self,
        _req: SessionStartRequest,
    ) -> Result<String, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "sessions not supported over bucket (slice 2b)".into(),
        ))
    }

    async fn send_session_turn(
        &self,
        _req: SendMessageRequest,
    ) -> Result<String, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "sessions not supported over bucket (slice 2b)".into(),
        ))
    }

    async fn list_runs(
        &self,
        params: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        mirror_list_runs(&self.run_store, &self.host_id, &params, &self.pricing)
    }

    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        mirror_get_run(&self.run_store, &self.host_id, run_id, &self.pricing)
    }

    async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError> {
        let mode_val = if mode.is_empty() {
            None
        } else {
            Some(mode.to_string())
        };
        self.put_control_envelope(
            run_id,
            ControlEnvelope {
                kind: "approve".to_string(),
                mode: mode_val,
                reason: None,
            },
        )
        .await
    }

    async fn reject_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<(), HostConnectorError> {
        self.put_control_envelope(
            run_id,
            ControlEnvelope {
                kind: "reject".to_string(),
                mode: None,
                reason: reason.map(|r| r.to_string()),
            },
        )
        .await
    }

    async fn cancel_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.put_control_envelope(
            run_id,
            ControlEnvelope {
                kind: "cancel".to_string(),
                mode: None,
                reason: None,
            },
        )
        .await
    }

    async fn stream_run_events(
        &self,
        run_id: &str,
    ) -> Result<EventByteStream, HostConnectorError> {
        mirror_stream_run_events(&self.run_store, &self.host_id, run_id).await
    }

    async fn get_transcript(
        &self,
        path: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        read_transcript_file(path)
    }

    async fn proxy_get_json(
        &self,
        _path_and_query: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "proxy_get_json is not supported for bucket hosts".into(),
        ))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use object_store::memory::InMemory;

    use super::*;
    use crate::host::bucket::ObjectStoreBucket;

    fn make_conn() -> (
        BucketHostConnector,
        Arc<RunStore>,
        Arc<dyn Bucket>,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let run_store = Arc::new(RunStore::new(tmp.path().join("runs")));
        let mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));
        let bucket: Arc<dyn Bucket> = Arc::new(ObjectStoreBucket::new(
            Arc::new(InMemory::new()),
            "test/host_bucket_1",
        ));
        let conn = BucketHostConnector::new(
            "host_bucket_1",
            Arc::clone(&bucket),
            mirror,
            Arc::clone(&run_store),
            rupu_config::PricingConfig::default(),
        );
        (conn, run_store, bucket, tmp)
    }

    #[tokio::test]
    async fn launch_run_mints_id_creates_mirror_and_puts_job() {
        let (conn, run_store, bucket, _tmp) = make_conn();

        let run_id = conn
            .launch_run(LaunchRequest {
                workflow: "deploy".into(),
                inputs: Default::default(),
                mode: Some("bypass".into()),
                target: None,
                working_dir: None,
            })
            .await
            .unwrap();

        // run_id starts with run_
        assert!(run_id.starts_with("run_"), "run_id must start with run_");

        // Mirror run was created, attributed to host_bucket_1.
        let rec = run_store.load(&run_id).unwrap();
        assert_eq!(
            rec.worker_id.as_deref(),
            Some("host_bucket_1"),
            "worker_id must equal host_id"
        );

        // A job envelope is in the bucket containing the workflow name.
        let job_bytes = bucket.get_job(&run_id).await.unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&job_bytes).unwrap();
        assert_eq!(
            spec.get("name").and_then(|v| v.as_str()),
            Some("deploy"),
            "job envelope must contain the workflow name"
        );
        assert_eq!(
            spec.get("kind").and_then(|v| v.as_str()),
            Some("workflow"),
            "job envelope kind must be 'workflow'"
        );
    }

    #[tokio::test]
    async fn launch_agent_puts_agent_kind_job() {
        let (conn, _run_store, bucket, _tmp) = make_conn();

        let run_id = conn
            .launch_agent(AgentLaunchRequest {
                agent: "my-agent".into(),
                prompt: Some("do something".into()),
                mode: None,
                target: None,
                working_dir: None,
            })
            .await
            .unwrap();

        let job_bytes = bucket.get_job(&run_id).await.unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&job_bytes).unwrap();
        assert_eq!(
            spec.get("kind").and_then(|v| v.as_str()),
            Some("agent"),
            "job envelope kind must be 'agent'"
        );
        assert_eq!(
            spec.get("name").and_then(|v| v.as_str()),
            Some("my-agent")
        );
    }

    #[tokio::test]
    async fn cancel_approve_reject_write_control_envelopes() {
        let (conn, _run_store, bucket, _tmp) = make_conn();

        // Dispatch a run first so the run_id is valid for mirror.
        let run_id = conn
            .launch_run(LaunchRequest {
                workflow: "wf".into(),
                inputs: Default::default(),
                mode: None,
                target: None,
                working_dir: None,
            })
            .await
            .unwrap();

        conn.cancel_run(&run_id).await.unwrap();
        conn.approve_run(&run_id, "bypass").await.unwrap();
        conn.reject_run(&run_id, Some("nope")).await.unwrap();

        let controls = bucket.list_control(&run_id).await.unwrap();
        assert_eq!(controls.len(), 3, "expected 3 control envelopes");

        // seq 0 — cancel
        let env0: ControlEnvelope =
            serde_json::from_slice(&controls[0].1).expect("seq 0 must be valid JSON");
        assert_eq!(env0.kind, "cancel");
        assert!(env0.mode.is_none());
        assert!(env0.reason.is_none());

        // seq 1 — approve with mode=bypass
        let env1: ControlEnvelope =
            serde_json::from_slice(&controls[1].1).expect("seq 1 must be valid JSON");
        assert_eq!(env1.kind, "approve");
        assert_eq!(env1.mode.as_deref(), Some("bypass"));
        assert!(env1.reason.is_none());

        // seq 2 — reject with reason
        let env2: ControlEnvelope =
            serde_json::from_slice(&controls[2].1).expect("seq 2 must be valid JSON");
        assert_eq!(env2.kind, "reject");
        assert!(env2.mode.is_none());
        assert_eq!(env2.reason.as_deref(), Some("nope"));
    }

    #[tokio::test]
    async fn approve_with_empty_mode_omits_mode_field() {
        let (conn, _run_store, bucket, _tmp) = make_conn();
        let run_id = conn
            .launch_run(LaunchRequest {
                workflow: "wf".into(),
                inputs: Default::default(),
                mode: None,
                target: None,
                working_dir: None,
            })
            .await
            .unwrap();

        conn.approve_run(&run_id, "").await.unwrap();

        let controls = bucket.list_control(&run_id).await.unwrap();
        assert_eq!(controls.len(), 1);
        let env: ControlEnvelope = serde_json::from_slice(&controls[0].1).unwrap();
        assert_eq!(env.kind, "approve");
        assert!(env.mode.is_none(), "empty mode string must be stored as None");
    }

    #[tokio::test]
    async fn info_reachable_true_for_in_memory_bucket() {
        let (conn, _run_store, _bucket, _tmp) = make_conn();
        let info = conn.info().await.unwrap();
        assert!(info.reachable, "in-memory bucket must be reachable");
    }

    #[tokio::test]
    async fn sessions_return_invalid_error() {
        let (conn, _run_store, _bucket, _tmp) = make_conn();
        let err = conn
            .start_session(SessionStartRequest {
                agent: "a".into(),
                prompt: None,
                mode: None,
                target: None,
                working_dir: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Invalid(_)),
            "start_session must return Invalid"
        );
    }

    #[tokio::test]
    async fn proxy_get_json_returns_invalid() {
        let (conn, _run_store, _bucket, _tmp) = make_conn();
        let err = conn.proxy_get_json("/api/anything").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Invalid(_)),
            "proxy_get_json must return Invalid"
        );
    }
}
