//! Remote netflow transport, end to end (spec
//! `2026-10-14-rupu-remote-netflow-transport-design.md`, plan Task 10): a
//! `host:` workflow step runs through the real orchestrator and the real
//! `FleetUnitDispatcher`, over a fake connector standing in for a transport
//! whose mirror lands the unit's stream. The stream carries the unit's own
//! ledger plus a sub-agent's. Both must show in the CP's run-scoped netflow
//! read with no gap; a copy that lands `partial` must still show its flows,
//! raise exactly one `StepWarning`, and be listed as one gap.
//!
//! Nothing here touches the environment: the coordinator's global dir is a
//! temp dir handed to every seam directly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rupu_cli::fleet_unit_dispatcher::FleetUnitDispatcher;
use rupu_cp::agent_launcher::AgentLaunchRequest;
use rupu_cp::api::netflow::{netflow_ids, run_scoped_flows_and_dropped};
use rupu_cp::host::connector::{
    CoverageRead, EventByteStream, HostConnector, HostConnectorError, HostInfo, NetflowLanding,
    OperatorMessageRequest, RunListQuery,
};
use rupu_cp::host::skill_ship::{ShipOutcome, SkillBundle};
use rupu_cp::launcher::LaunchRequest;
use rupu_cp::netflow_index::DirectReader;
use rupu_cp::session_sender::SendMessageRequest;
use rupu_cp::session_starter::SessionStartRequest;
use rupu_netflow::land::{land_and_record, read_landing_record, LandingStatus};
use rupu_netflow::ledger::TimeRange;
use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, LedgerLine, Origin, Outcome};
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::{RunStore, Workflow};
use rupu_runtime::services::operator::{MessagesView, OperatorSendReply, ViewQuery};

const HOST: &str = "host_a";
/// The sub-agent ledger the unit's stream carries alongside its own.
const SUB_LEDGER: &str = "sub_k";
const PARTIAL_REASON: &str = "the mirror copy was pulled before the run ended";

const YAML: &str = r#"
name: w
steps:
  - id: s
    agent: sec
    prompt: p
    host: host_a
"#;

/// A placed step never reaches the step factory: the dispatcher owns it.
struct PanicFactory;

#[async_trait]
impl StepFactory for PanicFactory {
    async fn launch_for_step(
        &self,
        _request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        panic!("a placed unit must not be built locally");
    }
}

#[derive(Default)]
struct CollectingSink {
    events: Mutex<Vec<Event>>,
}

impl EventSink for CollectingSink {
    fn emit(&self, _run_id: &str, ev: &Event) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

impl CollectingSink {
    fn step_warnings(&self) -> Vec<(String, Option<usize>, String)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::StepWarning {
                    step_id,
                    index,
                    message,
                    ..
                } => Some((step_id.clone(), *index, message.clone())),
                _ => None,
            })
            .collect()
    }
}

/// One flow, distinguishable by the host it was made to.
fn flow(ledger: &str, host: &str, seq: u64) -> FlowRecord {
    FlowRecord {
        id: FlowId::from_parts(1_700_000_000_000 + seq, u128::from(seq)),
        ts: Utc.timestamp_opt(1_700_000_000 + seq as i64, 0).unwrap(),
        ctx: FlowCtx {
            run_id: Some(ledger.to_string()),
            step_id: None,
            agent: Some("sec".into()),
            workspace_id: None,
            tool_call_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        fidelity: Fidelity::Http,
        method: "POST".into(),
        scheme: "https".into(),
        host: host.to_string(),
        port: 443,
        path: "/v1/messages".into(),
        peer_ip: None,
        resolved_ips: Vec::new(),
        process: None,
        local_addr: None,
        direction: None,
        http_version: Some("HTTP/1.1".into()),
        status: Some(200),
        outcome: Outcome::Ok,
        error: None,
        bytes_out: Some(10),
        bytes_in: Some(20),
        body_complete: true,
        ttfb_ms: Some(1),
        duration_ms: Some(2),
    }
}

/// The host's stream for `unit_run_id`: a `begin` line, flow A under the
/// unit's own ledger and flow B under a sub-agent's — exactly what the
/// host's tee writes (`rupu_netflow::stream`), scripted here.
fn scripted_stream(unit_run_id: &str) -> Vec<u8> {
    let mut out = String::new();
    out.push_str(&serde_json::json!({"kind": "begin", "v": 1, "run_id": unit_run_id}).to_string());
    out.push('\n');
    for (ledger, host, seq) in [
        (unit_run_id, "flow-a.example", 1),
        (SUB_LEDGER, "flow-b.example", 2),
    ] {
        let line = LedgerLine::Flow(Box::new(flow(ledger, host, seq)));
        out.push_str(
            &serde_json::json!({"kind": "line", "ledger": ledger, "line": line}).to_string(),
        );
        out.push('\n');
    }
    out.into_bytes()
}

/// A connector for a mirroring transport: the launch is accepted under the
/// coordinator-minted id, the run is terminal on the first poll, its
/// coverage stream is whole (so coverage raises no warning of its own), and
/// `land_run_netflow` lands the scripted stream the way the transport's
/// mirror would before answering `ByMirror`.
struct MirroringConnector {
    /// Whether the mirror's copy is confirmed whole.
    complete: bool,
    /// Every unit run id `launch_agent` was handed, in order.
    launched: Mutex<Vec<String>>,
    /// The `terminal` flag of every `land_run_netflow` call, in order.
    landings: Mutex<Vec<bool>>,
}

impl MirroringConnector {
    fn new(complete: bool) -> Self {
        Self {
            complete,
            launched: Mutex::default(),
            landings: Mutex::default(),
        }
    }
}

#[async_trait]
impl HostConnector for MirroringConnector {
    async fn info(&self) -> Result<HostInfo, HostConnectorError> {
        unimplemented!()
    }
    async fn launch_run(&self, _req: LaunchRequest) -> Result<String, HostConnectorError> {
        unimplemented!()
    }
    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        let id = req
            .run_id
            .expect("the dispatcher launches under the coordinator-minted id");
        self.launched.lock().unwrap().push(id.clone());
        Ok(id)
    }
    async fn ship_skills(&self, _bundle: &SkillBundle) -> Result<ShipOutcome, HostConnectorError> {
        Ok(ShipOutcome::NotNeeded)
    }
    async fn start_session(&self, _req: SessionStartRequest) -> Result<String, HostConnectorError> {
        unimplemented!()
    }
    async fn send_session_turn(
        &self,
        _req: SendMessageRequest,
    ) -> Result<String, HostConnectorError> {
        unimplemented!()
    }
    async fn list_runs(
        &self,
        _params: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        unimplemented!()
    }
    async fn get_run(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        Ok(serde_json::json!({
            "run": { "status": "completed", "final_output": "done" }
        }))
    }
    async fn approve_run(&self, _run_id: &str, _mode: &str) -> Result<(), HostConnectorError> {
        unimplemented!()
    }
    async fn reject_run(
        &self,
        _run_id: &str,
        _reason: Option<&str>,
    ) -> Result<(), HostConnectorError> {
        unimplemented!()
    }
    async fn cancel_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
        unimplemented!()
    }
    async fn stream_run_events(
        &self,
        _run_id: &str,
    ) -> Result<EventByteStream, HostConnectorError> {
        unimplemented!()
    }
    async fn get_transcript(&self, _path: &str) -> Result<serde_json::Value, HostConnectorError> {
        unimplemented!()
    }
    fn serves_runs_from_local_mirror(&self) -> bool {
        true
    }
    fn honours_supplied_run_id(&self) -> bool {
        true
    }
    async fn unit_coverage(&self, run_id: &str) -> Result<CoverageRead, HostConnectorError> {
        let begin = serde_json::json!({"ledger": "begin", "v": 1, "run_id": run_id});
        Ok(CoverageRead {
            bytes: format!("{begin}\n").into_bytes(),
            complete: true,
        })
    }
    async fn land_run_netflow(
        &self,
        run_id: &str,
        terminal: bool,
        runs_root: &Path,
    ) -> Result<NetflowLanding, HostConnectorError> {
        self.landings.lock().unwrap().push(terminal);
        let reason = (!self.complete).then(|| PARTIAL_REASON.to_string());
        land_and_record(
            runs_root,
            run_id,
            HOST,
            Some(&scripted_stream(run_id)),
            self.complete,
            reason,
        )
        .map_err(|e| HostConnectorError::Internal(e.to_string()))?;
        Ok(NetflowLanding::ByMirror)
    }
    async fn pull_finding_artifact(
        &self,
        _sha256: &str,
        _dest: &Path,
        _max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("test double".into()))
    }
    async fn list_messages(
        &self,
        _id: &str,
        _q: ViewQuery,
    ) -> Result<MessagesView, HostConnectorError> {
        Err(HostConnectorError::Unsupported("test double".into()))
    }
    async fn send_operator_message(
        &self,
        _id: &str,
        _msg: OperatorMessageRequest,
    ) -> Result<OperatorSendReply, HostConnectorError> {
        Err(HostConnectorError::Unsupported("test double".into()))
    }
    async fn run_board(
        &self,
        _id: &str,
    ) -> Result<rupu_runtime::services::board::BoardView, HostConnectorError> {
        Err(HostConnectorError::Unsupported("test double".into()))
    }
    async fn proxy_get_json(
        &self,
        _path_and_query: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        unimplemented!()
    }
}

/// One coordinator: a temp global dir with its run store under `runs/` and
/// a workspace beside it (no `.rupu/netflow/` of its own, so the CP's read
/// falls back to `<global>/netflow/`, where landings go).
struct Coordinator {
    _tmp: tempfile::TempDir,
    global: PathBuf,
    workspace: PathBuf,
    store: Arc<RunStore>,
}

impl Coordinator {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let store = Arc::new(RunStore::new(global.join("runs")));
        Self {
            _tmp: tmp,
            global,
            workspace,
            store,
        }
    }

    /// Run the one-step `host: host_a` workflow through the real runner and
    /// the real dispatcher over `conn`; the workflow run id and the events.
    async fn run(&self, conn: Arc<MirroringConnector>) -> (String, Arc<CollectingSink>) {
        let dispatcher = FleetUnitDispatcher::from_connector(conn, self.global.clone(), None)
            .with_runs_root(self.store.root.clone());
        let sink = Arc::new(CollectingSink::default());
        let opts = OrchestratorRunOpts {
            run_step: Default::default(),
            workflow: Workflow::parse(YAML).unwrap(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_e2e".into(),
            workspace_path: self.workspace.clone(),
            transcript_dir: self.global.join("transcripts"),
            factory: Arc::new(PanicFactory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&self.store)),
            workflow_yaml: Some(YAML.to_string()),
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: Some(sink.clone() as Arc<dyn EventSink>),
            unit_dispatcher: Some(Arc::new(dispatcher)),
            action_services: None,
            pause: None,
            naming: None,
        };
        let result = run_workflow(opts).await.expect("the workflow runs");
        assert_eq!(result.step_results.len(), 1, "{:?}", result.step_results);
        assert!(
            result.step_results[0].success,
            "the placed step succeeded: {:?}",
            result.step_results[0]
        );
        (result.run_id, sink)
    }

    fn flows(&self, run_id: &str) -> Vec<FlowRecord> {
        let (flows, dropped) = run_scoped_flows_and_dropped(
            &DirectReader,
            &self.store,
            run_id,
            &self.workspace,
            &self.global,
            &TimeRange::unbounded(),
        );
        assert_eq!(dropped, 0);
        flows
    }
}

fn hosts(flows: &[FlowRecord]) -> Vec<&str> {
    let mut hosts: Vec<&str> = flows.iter().map(|f| f.host.as_str()).collect();
    hosts.sort_unstable();
    hosts
}

#[tokio::test]
async fn a_placed_units_landed_ledgers_are_read_whole_with_no_gap() {
    let coord = Coordinator::new();
    let conn = Arc::new(MirroringConnector::new(true));
    let (run_id, sink) = coord.run(Arc::clone(&conn)).await;

    // One unit, landed once, after the run was seen terminal.
    let launched = conn.launched.lock().unwrap().clone();
    assert_eq!(launched.len(), 1, "{launched:?}");
    let unit = launched[0].clone();
    assert_eq!(*conn.landings.lock().unwrap(), vec![true]);
    let record = read_landing_record(&coord.store.root, &unit).expect("a landing record");
    assert_eq!(record.status, LandingStatus::Complete);
    assert_eq!(record.host, HOST);
    assert_eq!(record.ledgers, vec![unit.clone(), SUB_LEDGER.to_string()]);

    // Both ledgers landed under the coordinator's global netflow dir, and the
    // roster names the unit on its host.
    let netflow_dir = rupu_netflow::global_netflow_dir(&coord.global);
    assert!(netflow_dir.join(format!("{unit}.jsonl")).exists());
    assert!(netflow_dir.join(format!("{SUB_LEDGER}.jsonl")).exists());
    let roster = rupu_netflow::land::read_roster(&coord.store.root, &run_id);
    assert_eq!(roster.len(), 1, "{roster:?}");
    assert_eq!(roster[0].unit_run_id, unit);
    assert_eq!(roster[0].host, HOST);
    assert_eq!(roster[0].step_id, "s");

    // The CP's id walk reads both ledgers for the workflow run and knows of
    // no gap.
    let ids = netflow_ids(&coord.store, &coord.global, &run_id);
    assert!(ids.ids.contains(&unit), "{ids:?}");
    assert!(ids.ids.contains(&SUB_LEDGER.to_string()), "{ids:?}");
    assert!(ids.gaps.is_empty(), "{ids:?}");

    // And the run-scoped read holds flow A (the unit's own) and flow B (its
    // sub-agent's), attributed through the files they landed in.
    let flows = coord.flows(&run_id);
    assert_eq!(hosts(&flows), vec!["flow-a.example", "flow-b.example"]);
    let by_ledger: Vec<(&str, &str)> = flows
        .iter()
        .map(|f| (f.ctx.run_id.as_deref().unwrap_or(""), f.host.as_str()))
        .collect();
    assert!(
        by_ledger.contains(&(unit.as_str(), "flow-a.example")),
        "{by_ledger:?}"
    );
    assert!(
        by_ledger.contains(&(SUB_LEDGER, "flow-b.example")),
        "{by_ledger:?}"
    );

    // A whole landing warns about nothing.
    assert!(
        sink.step_warnings().is_empty(),
        "{:?}",
        sink.step_warnings()
    );
}

#[tokio::test]
async fn a_partial_landing_is_one_step_warning_and_one_gap_and_still_shows_its_flows() {
    let coord = Coordinator::new();
    let conn = Arc::new(MirroringConnector::new(false));
    let (run_id, sink) = coord.run(Arc::clone(&conn)).await;

    let launched = conn.launched.lock().unwrap().clone();
    assert_eq!(launched.len(), 1, "{launched:?}");
    let unit = launched[0].clone();
    let record = read_landing_record(&coord.store.root, &unit).expect("a landing record");
    assert_eq!(record.status, LandingStatus::Partial);
    assert_eq!(record.reason.as_deref(), Some(PARTIAL_REASON));

    // Exactly one warning, on the placed step, naming the host and the
    // mirror's reason; it never failed the unit (the run succeeded above).
    let warnings = sink.step_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let (step_id, index, message) = &warnings[0];
    assert_eq!(step_id, "s");
    assert_eq!(*index, None);
    assert_eq!(
        message,
        &format!("host {HOST}: netflow from this unit may be incomplete: {PARTIAL_REASON}")
    );

    // Exactly one gap, on the host, naming the unit — and the ledgers that
    // did land are still read.
    let ids = netflow_ids(&coord.store, &coord.global, &run_id);
    assert_eq!(ids.gaps.len(), 1, "{ids:?}");
    assert_eq!(ids.gaps[0].host_id, HOST);
    assert_eq!(
        ids.gaps[0].reason,
        format!("netflow from unit run {unit} may be incomplete: {PARTIAL_REASON}")
    );
    assert!(ids.ids.contains(&unit), "{ids:?}");
    assert!(ids.ids.contains(&SUB_LEDGER.to_string()), "{ids:?}");
    let flows = coord.flows(&run_id);
    assert_eq!(hosts(&flows), vec!["flow-a.example", "flow-b.example"]);
}
