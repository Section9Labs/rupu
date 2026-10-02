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
        bucket::{Bucket, BucketError, ControlEnvelope, WorkerInfo},
        connector::{
            blocking_host, mirror_get_run, mirror_list_runs, mirror_stream_run_events,
            read_transcript_file, EventByteStream, HostCapabilities, HostConnector,
            HostConnectorError, HostInfo, RunListQuery,
        },
    },
    launcher::LaunchRequest,
    node::{
        protocol::{RunSpec, RunSpecKind, CAP_AGENT_FINDINGS_PROFILE},
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
pub struct BucketHostConnector {
    host_id: String,
    bucket: Arc<dyn Bucket>,
    mirror: Arc<NodeMirror>,
    run_store: Arc<RunStore>,
    pricing: rupu_config::PricingConfig,
}

impl BucketHostConnector {
    /// Construct a new connector.
    pub fn new(
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
        // Use max(existing_seq) + 1 rather than len() so two concurrent
        // put_control_envelope calls cannot race to the same seq number
        // (len() would give both N; the second put would silently overwrite).
        let seq = existing.iter().map(|(s, _)| *s + 1).max().unwrap_or(0);
        let bytes =
            serde_json::to_vec(&envelope).map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        self.bucket
            .put_control(run_id, seq, &bytes)
            .await
            .map_err(bucket_err_to_unreachable)?;
        Ok(())
    }
}

impl BucketHostConnector {
    /// Refuse unless the pull workers on this bucket have advertised
    /// `capability` in their [`WorkerInfo`] markers: at least one marker, and
    /// every marker lists it. A worker predating the markers writes none and
    /// would silently drop a [`RunSpec`] field it doesn't know, so no marker
    /// ⇒ refuse.
    ///
    /// Residual gap, inherent to a handshake-free dead drop: an old worker
    /// polling the SAME bucket alongside an upgraded one is invisible here
    /// and can still claim the job.
    async fn require_worker_capability(
        &self,
        capability: &str,
        what: &str,
    ) -> Result<(), HostConnectorError> {
        let bodies = self
            .bucket
            .list_worker_info()
            .await
            .map_err(bucket_err_to_unreachable)?;
        let infos: Vec<Option<WorkerInfo>> = bodies
            .iter()
            .map(|b| serde_json::from_slice(b).ok())
            .collect();
        let lacking: Vec<String> = infos
            .iter()
            .filter_map(|i| match i {
                Some(i) if i.capabilities.iter().any(|c| c == capability) => None,
                Some(i) => Some(format!("{} (rupu {})", i.worker_id, i.rupu_version)),
                None => Some("an unreadable worker marker".to_string()),
            })
            .collect();
        if infos.is_empty() {
            return Err(HostConnectorError::Unsupported(format!(
                "{what}: no pull worker on bucket host {} has advertised support \
                 (none has written a nodes/<worker>.json marker); upgrade rupu on its \
                 workers (`rupu node pull`)",
                self.host_id
            )));
        }
        if !lacking.is_empty() {
            return Err(HostConnectorError::Unsupported(format!(
                "{what}: pull worker(s) on bucket host {} do not support it: {}; \
                 upgrade rupu on them",
                self.host_id,
                lacking.join(", ")
            )));
        }
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
            findings_profile: None,
        };

        self.mirror
            .create_run(&run_id, &self.host_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        let bytes = serde_json::to_vec(&spec).map_err(|e| {
            let _ = self.mirror.finish(&run_id, &self.host_id, "failed");
            HostConnectorError::Invalid(e.to_string())
        })?;
        self.bucket
            .put_job(&run_id, &bytes)
            .await
            .map_err(|e| {
                let _ = self.mirror.finish(&run_id, &self.host_id, "failed");
                bucket_err_to_unreachable(e)
            })?;

        Ok(run_id)
    }

    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        // Before the mirror run exists, so a refusal leaves nothing behind.
        if let Some(profile) = req.findings_profile {
            self.require_worker_capability(
                CAP_AGENT_FINDINGS_PROFILE,
                &format!("this run cannot be held to the `{profile}` findings profile"),
            )
            .await?;
        }

        let run_id = format!("run_{}", Ulid::new());

        let spec = RunSpec {
            kind: RunSpecKind::Agent,
            name: req.agent.clone(),
            inputs: std::collections::BTreeMap::new(),
            prompt: req.prompt.clone(),
            mode: req.mode.clone(),
            target: req.target.clone(),
            findings_profile: req.findings_profile,
        };

        self.mirror
            .create_run(&run_id, &self.host_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        let bytes = serde_json::to_vec(&spec).map_err(|e| {
            let _ = self.mirror.finish(&run_id, &self.host_id, "failed");
            HostConnectorError::Invalid(e.to_string())
        })?;
        self.bucket
            .put_job(&run_id, &bytes)
            .await
            .map_err(|e| {
                let _ = self.mirror.finish(&run_id, &self.host_id, "failed");
                bucket_err_to_unreachable(e)
            })?;

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
        let (store, id, pricing) = (
            Arc::clone(&self.run_store),
            self.host_id.clone(),
            self.pricing.clone(),
        );
        blocking_host(move || mirror_list_runs(&store, &id, &params, &pricing)).await
    }

    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        let (store, id, pricing) = (
            Arc::clone(&self.run_store),
            self.host_id.clone(),
            self.pricing.clone(),
        );
        let run_id = run_id.to_string();
        blocking_host(move || mirror_get_run(&store, &id, &run_id, &pricing)).await
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

    /// The mirrored stream. Complete once the run is terminal: the worker
    /// writes its finished marker only after every coverage upload landed,
    /// and the poller re-lists after seeing the marker before it finishes the
    /// run here.
    async fn unit_coverage(
        &self,
        run_id: &str,
    ) -> Result<crate::host::connector::CoverageRead, HostConnectorError> {
        let bytes = crate::host::connector::mirror_unit_coverage(&self.run_store, run_id).await?;
        Ok(crate::host::connector::CoverageRead {
            bytes,
            complete: true,
        })
    }
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &std::path::Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        crate::host::connector::validate_sha256(sha256)?;
        self.bucket
            .get_artifact_to_file(sha256, dest, max_bytes)
            .await
            .map_err(|e| match e {
                BucketError::NotFound(_) => HostConnectorError::NotFound(format!(
                    "artifact {sha256} was not uploaded to bucket host {} (the run may not \
                     have finished yet — a worker uploads its blobs when the run ends — its \
                     worker may predate artifact upload, or the upload failed)",
                    self.host_id
                )),
                other => bucket_err_to_unreachable(other),
            })
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

    /// SSH/Tunnel/Bucket runs are created in, and tailed into, the
    /// coordinator's own `RunStore` by `NodeMirror`, so run-scoped detail
    /// endpoints read that mirror instead of the wire.
    fn serves_runs_from_local_mirror(&self) -> bool {
        true
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
                codename: None,
                agent: "my-agent".into(),
                prompt: Some("do something".into()),
                mode: None,
                target: None,
                working_dir: None,
                run_id: None,
                findings_profile: None,
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
    async fn unit_coverage_reads_the_mirrored_stream() {
        let (conn, run_store, _bucket, _tmp) = make_conn();
        let run_id = conn.launch_agent(profile_req(None)).await.unwrap();
        let p = rupu_coverage::stream_path(&run_store.root, &run_id);
        std::fs::write(&p, b"{\"ledger\":\"begin\",\"v\":1,\"run_id\":\"x\"}\n").unwrap();
        let read = conn.unit_coverage(&run_id).await.unwrap();
        assert!(read.bytes.starts_with(b"{\"ledger\":\"begin\""));
        assert!(read.complete, "the marker follows the uploads: complete");
    }

    fn profile_req(profile: Option<rupu_coverage::FindingProfile>) -> AgentLaunchRequest {
        AgentLaunchRequest {
            agent: "sec".into(),
            prompt: Some("audit".into()),
            mode: None,
            target: None,
            working_dir: None,
            run_id: None,
            findings_profile: profile,
            codename: None,
        }
    }

    async fn put_worker(bucket: &Arc<dyn Bucket>, id: &str, caps: &[&str]) {
        let info = WorkerInfo {
            worker_id: id.into(),
            rupu_version: "9.9.9".into(),
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
        };
        bucket
            .put_worker_info(id, &serde_json::to_vec(&info).unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn launch_agent_carries_the_findings_profile_to_a_capable_worker() {
        let (conn, _run_store, bucket, _tmp) = make_conn();
        put_worker(&bucket, "node_a", &[CAP_AGENT_FINDINGS_PROFILE]).await;

        let run_id = conn
            .launch_agent(profile_req(Some(rupu_coverage::FindingProfile::Summary)))
            .await
            .unwrap();

        let spec: RunSpec =
            serde_json::from_slice(&bucket.get_job(&run_id).await.unwrap()).unwrap();
        assert_eq!(
            spec.findings_profile,
            Some(rupu_coverage::FindingProfile::Summary)
        );
    }

    #[tokio::test]
    async fn launch_agent_refuses_a_profile_when_no_worker_advertised_support() {
        let (conn, run_store, bucket, _tmp) = make_conn();

        let err = conn
            .launch_agent(profile_req(Some(rupu_coverage::FindingProfile::Full)))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Unsupported(m) if m.contains("`full`")),
            "{err:?}"
        );
        // Refused before anything was written: no job, no orphaned mirror run.
        assert!(bucket.list_jobs().await.unwrap().is_empty());
        assert!(run_store.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn launch_agent_refuses_a_profile_when_any_worker_lacks_support() {
        let (conn, run_store, bucket, _tmp) = make_conn();
        put_worker(&bucket, "node_new", &[CAP_AGENT_FINDINGS_PROFILE]).await;
        put_worker(&bucket, "node_other", &[]).await;

        let err = conn
            .launch_agent(profile_req(Some(rupu_coverage::FindingProfile::Summary)))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Unsupported(m) if m.contains("node_other")),
            "{err:?}"
        );
        assert!(bucket.list_jobs().await.unwrap().is_empty());
        assert!(run_store.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn launch_agent_without_a_profile_needs_no_worker_marker() {
        let (conn, _run_store, bucket, _tmp) = make_conn();
        let run_id = conn.launch_agent(profile_req(None)).await.unwrap();
        let raw: serde_json::Value =
            serde_json::from_slice(&bucket.get_job(&run_id).await.unwrap()).unwrap();
        assert!(
            raw.get("findings_profile").is_none(),
            "no override ⇒ the envelope is byte-compatible with older workers: {raw}"
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
    async fn bucket_pause_unsupported() {
        // Bucket has no wiring to reach a node agent's cooperative-pause
        // check (unlike cancel/approve/reject, which just queue a control
        // envelope the node polls for) — it inherits `HostConnector`'s
        // default `Unsupported` for both `pause_run` and `resume_run`
        // rather than faking a pause that never actually lands.
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

        assert!(matches!(
            conn.pause_run(&run_id).await,
            Err(HostConnectorError::Unsupported(_))
        ));
        assert!(matches!(
            conn.resume_run(&run_id).await,
            Err(HostConnectorError::Unsupported(_))
        ));
        // No control envelope was written for either call.
        let controls = bucket.list_control(&run_id).await.unwrap();
        assert!(
            controls.is_empty(),
            "unsupported pause/resume must not queue any control envelope"
        );
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

    // ── pull_finding_artifact ─────────────────────────────────────────────────

    #[tokio::test]
    async fn pull_finding_artifact_streams_a_workers_uploaded_blob() {
        let (conn, _run_store, bucket, tmp) = make_conn();
        let sha = "ab".repeat(32);
        let src = tmp.path().join("blob");
        std::fs::write(&src, b"poc bytes").unwrap();
        bucket.put_artifact_file(&sha, &src).await.unwrap();

        let dest = tmp.path().join("pulled");
        conn.pull_finding_artifact(&sha, &dest, 9).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"poc bytes");

        // More than the recorded size arriving is refused (the caller removes
        // `dest`).
        let err = conn
            .pull_finding_artifact(&sha, &tmp.path().join("capped"), 8)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("exceeds its recorded 8 bytes"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn pull_finding_artifact_not_uploaded_is_not_found_naming_the_host() {
        let (conn, _run_store, _bucket, tmp) = make_conn();
        let sha = "cd".repeat(32);
        let err = conn
            .pull_finding_artifact(&sha, &tmp.path().join("out"), 9)
            .await
            .unwrap_err();
        match err {
            HostConnectorError::NotFound(msg) => {
                assert!(msg.contains(&sha), "{msg}");
                assert!(msg.contains("host_bucket_1"), "{msg}");
                assert!(msg.contains("not uploaded"), "{msg}");
                // Blobs upload when the run ends, so "not there yet" is a cause.
                assert!(msg.contains("may not have finished"), "{msg}");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
        assert!(
            !tmp.path().join("out").exists(),
            "a missing blob must not create dest"
        );
    }

    #[tokio::test]
    async fn pull_finding_artifact_refuses_a_malformed_sha_before_the_bucket() {
        // FailingBucket panics (`unimplemented!`) on any call it isn't scripted
        // for: a malformed sha must be refused without reaching the bucket.
        let tmp = tempfile::tempdir().unwrap();
        let run_store = Arc::new(RunStore::new(tmp.path().join("runs")));
        let mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));
        let conn = BucketHostConnector::new(
            "host_failing",
            Arc::new(FailingBucket),
            mirror,
            run_store,
            rupu_config::PricingConfig::default(),
        );
        for bad in ["../../etc/passwd", "", "AB", &"AB".repeat(32)] {
            let err = conn
                .pull_finding_artifact(bad, &tmp.path().join("out"), 9)
                .await
                .unwrap_err();
            assert!(
                matches!(err, HostConnectorError::Invalid(_)),
                "{bad:?}: {err:?}"
            );
        }
    }

    // ── FailingBucket: test double whose put_job always fails ─────────────────

    struct FailingBucket;

    #[async_trait::async_trait]
    impl Bucket for FailingBucket {
        async fn put_job(&self, _run_id: &str, _envelope: &[u8]) -> Result<(), BucketError> {
            Err(BucketError::Io("boom".into()))
        }
        async fn list_jobs(&self) -> Result<Vec<String>, BucketError> {
            unimplemented!("FailingBucket::list_jobs")
        }
        async fn claim_job(&self, _run_id: &str, _worker: &str) -> Result<bool, BucketError> {
            unimplemented!("FailingBucket::claim_job")
        }
        async fn get_job(&self, _run_id: &str) -> Result<Vec<u8>, BucketError> {
            unimplemented!("FailingBucket::get_job")
        }
        async fn put_control(
            &self,
            _run_id: &str,
            _seq: u64,
            _envelope: &[u8],
        ) -> Result<(), BucketError> {
            unimplemented!("FailingBucket::put_control")
        }
        async fn list_control(&self, _run_id: &str) -> Result<Vec<(u64, Vec<u8>)>, BucketError> {
            unimplemented!("FailingBucket::list_control")
        }
        async fn put_result(
            &self,
            _run_id: &str,
            _key: &str,
            _body: &[u8],
        ) -> Result<(), BucketError> {
            unimplemented!("FailingBucket::put_result")
        }
        async fn list_results(
            &self,
            _run_id: &str,
        ) -> Result<Vec<(String, Vec<u8>)>, BucketError> {
            unimplemented!("FailingBucket::list_results")
        }
        async fn put_finished(&self, _run_id: &str, _status: &str) -> Result<(), BucketError> {
            unimplemented!("FailingBucket::put_finished")
        }
        async fn get_finished(&self, _run_id: &str) -> Result<Option<String>, BucketError> {
            unimplemented!("FailingBucket::get_finished")
        }
        async fn probe(&self) -> Result<(), BucketError> {
            unimplemented!("FailingBucket::probe")
        }
        async fn put_worker_info(&self, _worker_id: &str, _body: &[u8]) -> Result<(), BucketError> {
            unimplemented!("FailingBucket::put_worker_info")
        }
        async fn list_worker_info(&self) -> Result<Vec<Vec<u8>>, BucketError> {
            unimplemented!("FailingBucket::list_worker_info")
        }
        async fn artifact_exists(&self, _sha256: &str) -> Result<bool, BucketError> {
            unimplemented!("FailingBucket::artifact_exists")
        }
        async fn put_artifact_file(
            &self,
            _sha256: &str,
            _src: &std::path::Path,
        ) -> Result<(), BucketError> {
            unimplemented!("FailingBucket::put_artifact_file")
        }
        async fn get_artifact_to_file(
            &self,
            _sha256: &str,
            _dest: &std::path::Path,
            _max_bytes: u64,
        ) -> Result<(), BucketError> {
            unimplemented!("FailingBucket::get_artifact_to_file")
        }
    }

    #[tokio::test]
    async fn put_job_failure_cleans_up_mirror_run() {
        let tmp = tempfile::tempdir().unwrap();
        let run_store = Arc::new(rupu_orchestrator::runs::RunStore::new(
            tmp.path().join("runs"),
        ));
        let mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));
        let bucket: Arc<dyn Bucket> = Arc::new(FailingBucket);
        let conn = BucketHostConnector::new(
            "host_failing",
            bucket,
            mirror,
            Arc::clone(&run_store),
            rupu_config::PricingConfig::default(),
        );

        let err = conn
            .launch_run(LaunchRequest {
                workflow: "wf".into(),
                inputs: Default::default(),
                mode: None,
                target: None,
                working_dir: None,
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, HostConnectorError::Unreachable(_)),
            "put_job failure must map to Unreachable, got: {err:?}"
        );

        // The mirror run must NOT be left Running — cleanup must set it to Failed.
        let runs = run_store.list().unwrap();
        assert_eq!(runs.len(), 1, "exactly one run must exist in the store");
        assert_eq!(
            runs[0].status,
            rupu_orchestrator::RunStatus::Failed,
            "mirror run must be transitioned to Failed, not left Running"
        );
    }
}
