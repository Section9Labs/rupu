//! `FleetUnitDispatcher` — dispatches remote fan-out units through the
//! [`rupu_cp::host::registry::HostRegistry`], polling the mirrored run until
//! a terminal status is reached.

#![deny(clippy::all)]

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use rupu_agent::RunError;
use rupu_cp::{
    agent_launcher::AgentLaunchRequest,
    host::{
        connector::{
            decode_delta, encode_delta, encode_payload, HostConnector, HostConnectorError,
        },
        registry::HostRegistry,
    },
};
use rupu_orchestrator::runner::{
    UnitDispatch, UnitDispatcher, UnitOutcome, WorkspaceConflict, WorkspaceDelta,
};

// ── Poll constants ─────────────────────────────────────────────────────────────

/// How long a placed run may take before the coordinator stops waiting.
///
/// This is a WALL-CLOCK budget, not a poll count. It replaces a
/// `POLL_MAX_ATTEMPTS: u32 = 120` that, at a flat 500 ms interval, gave a
/// placed run 60 SECONDS total to reach a terminal status.
///
/// Sixty seconds cannot accommodate a placed run doing real work. Measured on
/// a live host: an enumeration agent started at 20:21:33 and finished
/// `completed` at 20:23:16 — 103 s. The coordinator abandoned it at 60 s,
/// reported the step failed, and DISCARDED its workspace delta. The run had
/// succeeded; only the result was thrown away.
///
/// Four hours is chosen to be longer than the work, not to be a guess at it.
/// An offensive campaign step, a long build, or an agent that pages through a
/// large surface all legitimately run for minutes to hours; a bound below the
/// work turns a healthy run into a false failure, which is the same defect at
/// the other end of the run as the startup race this loop already handles.
const POLL_MAX_WALL: std::time::Duration = std::time::Duration::from_secs(4 * 60 * 60);

/// How long a placed run may take to be OBSERVED AT ALL before the
/// coordinator stops waiting.
///
/// [`POLL_MAX_WALL`] is the right budget for a run that is demonstrably
/// alive, and the wrong budget for a run that never started. The two are
/// different failures and this loop must not conflate them:
///
/// * observed-and-slow — the host answered `get_run` at least once, so the
///   run exists and is doing work. Four hours.
/// * never-observed — `get_run` has never once succeeded. Either the remote
///   `rupu` has not registered the run YET (the startup race
///   [`POLL_MAX_WALL`]'s predecessor got wrong, see the `observed` flag
///   below), or it never will because the launch died. Those are
///   indistinguishable from here, so the bound must be long enough to lose
///   the race honestly and short enough to fail fast when there is no race
///   to win.
///
/// Measured in production: a remote agent died in `launch_agent`'s detached
/// process with `save worker record: io rename …: No such file or directory`
/// and never wrote `run.json`. The coordinator polled it for over 90 minutes
/// and would have gone to four hours — and because the fan-out step had
/// `max_parallel: 2`, two such units held both semaphore slots and the whole
/// workflow stopped making progress while still looking healthy.
///
/// Five minutes is chosen against the real startup cost, not as a guess:
/// staging a ~50 MB workspace over SSH has been measured at ~60 s on this
/// fleet, and the remote process start (config read, provider resolution,
/// run registration) is seconds on top. Five minutes is ~5× that, so a
/// genuinely slow launch still wins; it is also ~48× shorter than the wall
/// budget, so a launch that never happened surfaces while an operator is
/// still looking at the run rather than after it has blocked a fan-out for
/// half a working day. Anything much below ~3 minutes would start racing
/// large-workspace staging again — the exact #645 regression — and anything
/// much above ~10 minutes stops being a fast failure.
const POLL_MAX_STARTUP: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Poll interval for the first [`POLL_FAST_ATTEMPTS`] polls.
///
/// Kept tight so a short run — the common case for a probe or a precondition
/// step — still returns promptly.
const POLL_INTERVAL_FAST: std::time::Duration = std::time::Duration::from_millis(500);
/// Poll interval after the fast phase.
///
/// A four-hour budget at 500 ms would be ~28,800 ssh invocations per placed
/// unit. Backing off to 5 s keeps a long run's cost proportionate (~2,880) and
/// leaves the host's ssh capacity for the units actually doing work — this
/// codebase has already tripped a lasting connection block by hammering a host.
const POLL_INTERVAL_SLOW: std::time::Duration = std::time::Duration::from_secs(5);
/// Number of polls served at [`POLL_INTERVAL_FAST`] before backing off. 60 ×
/// 500 ms = the first 30 s, which covers launch, workspace staging and startup.
const POLL_FAST_ATTEMPTS: u32 = 60;

/// The interval to wait before poll number `attempt`.
fn poll_interval_for(attempt: u32) -> std::time::Duration {
    if attempt < POLL_FAST_ATTEMPTS {
        POLL_INTERVAL_FAST
    } else {
        POLL_INTERVAL_SLOW
    }
}
// Why a `get_run` failure is NOT believed until the run has been seen once.
//
// `launch_agent` returns as soon as the DETACHED remote command has been
// spawned — `(setsid <argv> </dev/null >/dev/null 2>>…/launch.log &)` — so the
// run record on the host is created asynchronously, after the remote `rupu`
// has started, read its config and registered the run. Until that lands, `get_run` legitimately
// answers "unknown run", and "the host does not know this run" is
// indistinguishable from "the host has not started it yet".
//
// That window is not a constant we can pick: a step with `workspace: sync`
// stages the workspace before the remote `rupu` starts, so it scales with the
// workspace. Measured — a placed step from a small scratch project registered
// before the first poll and passed, while the SAME step from a 57 MB project
// lost the race every time and was reported FAILED even though it went on to
// complete on the host.
//
// So a pre-observation error retries for the whole poll budget instead. See the
// `observed` flag in the poll loop below.

// ── Helpers ───────────────────────────────────────────────────────────────────

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "rejected")
}

fn host_err_to_run_err(e: HostConnectorError) -> RunError {
    RunError::Provider(e.to_string())
}

// ── Workspace-delta bridge ──────────────────────────────────────────────────
//
// The orchestrator's `WorkspaceDelta` is opaque: `payload` is whatever the
// dispatcher chooses, and `changed`/`deleted` are mirrored for observability.
// We define the bridge here so the orchestrator never sees the codec types.
// The chosen payload encoding IS the connector wire encoding (`encode_delta` /
// `decode_delta`), so the same self-describing bytes flow coordinator→host and
// host→coordinator without a second format.

/// Convert a codec [`rupu_workspace::Delta`] into the orchestrator's opaque
/// [`WorkspaceDelta`]: mirror `changed`/`deleted` for logging, and stash the
/// wire-encoded delta as the opaque `payload`.
fn to_orchestrator_delta(d: &rupu_workspace::Delta) -> WorkspaceDelta {
    WorkspaceDelta {
        changed: d.changed.clone(),
        deleted: d.deleted.clone(),
        payload: encode_delta(d),
    }
}

/// Decode an orchestrator [`WorkspaceDelta`] back into a codec
/// [`rupu_workspace::Delta`] for `apply_deltas`.
fn from_orchestrator_delta(
    wd: &WorkspaceDelta,
) -> Result<rupu_workspace::Delta, HostConnectorError> {
    decode_delta(&wd.payload)
}

// ── Resolver — production vs. test seam ───────────────────────────────────────

/// Internal enum so tests can inject a fixed connector without a registry.
enum Resolver {
    Registry(Arc<HostRegistry>),
    Fixed(Arc<dyn HostConnector>),
}

impl Resolver {
    fn resolve(&self, host: &str) -> Result<Arc<dyn HostConnector>, RunError> {
        match self {
            Resolver::Registry(reg) => reg
                .resolve(host)
                .map_err(|e| RunError::Provider(e.to_string())),
            Resolver::Fixed(conn) => Ok(Arc::clone(conn)),
        }
    }
}

// ── FleetUnitDispatcher ───────────────────────────────────────────────────────

/// Dispatches remote fan-out units through the [`HostRegistry`].
///
/// Production path: `new(registry)` — resolves the connector from the registry
/// on every `dispatch_unit` call.
/// Test/seam path: `from_connector(conn)` — bypasses registry resolution.
pub struct FleetUnitDispatcher {
    resolver: Resolver,
}

impl FleetUnitDispatcher {
    /// Production constructor: resolves the connector via `registry` per call.
    pub fn new(registry: Arc<HostRegistry>) -> Self {
        Self {
            resolver: Resolver::Registry(registry),
        }
    }

    /// Seam constructor for tests: always uses `conn`, skipping registry lookup.
    pub fn from_connector(conn: Arc<dyn HostConnector>) -> Self {
        Self {
            resolver: Resolver::Fixed(conn),
        }
    }
}

#[async_trait]
impl UnitDispatcher for FleetUnitDispatcher {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        let conn = self.resolver.resolve(host)?;

        // When the unit runs in `Sync` mode, pack the coordinator workspace and
        // stage it on the host BEFORE launching, so the agent runs against the
        // staged tree. `None` ⇒ self-contained: byte-for-byte the prior path.
        let working_dir = match &unit.workspace_path {
            Some(ws) => {
                let payload =
                    rupu_workspace::pack(ws).map_err(|e| RunError::Provider(e.to_string()))?;
                let encoded = encode_payload(&payload);
                let dir = conn
                    .stage_workspace(encoded)
                    .await
                    .map_err(host_err_to_run_err)?;
                Some(dir)
            }
            None => None,
        };

        // Launch the agent run on the remote host (against the staged dir, when
        // workspace sync is active).
        let run_id = match conn
            .launch_agent(AgentLaunchRequest {
                agent: unit.agent,
                prompt: Some(unit.rendered_prompt),
                mode: None,
                target: None,
                working_dir: working_dir.clone(),
            })
            .await
        {
            Ok(id) => id,
            Err(e) => {
                // Staging succeeded but the launch never happened, so
                // `collect_workspace_delta` will never run for this unit —
                // discard the remote scratch now or it leaks forever.
                // Best-effort: the launch error is what the caller sees.
                if let Some(dir) = &working_dir {
                    if let Err(discard_err) = conn.discard_workspace(dir).await {
                        tracing::warn!(
                            host,
                            dir,
                            error = %discard_err,
                            "best-effort workspace discard failed after launch_agent error"
                        );
                    }
                }
                return Err(host_err_to_run_err(e));
            }
        };

        // Poll the mirrored run until a terminal status is reached.
        //
        // `observed` gates how a poll ERROR is treated. Before the first
        // successful read the run may simply not exist yet — the launch is
        // detached, so registration is asynchronous — and we retry. Once the
        // run has been read even once the host demonstrably knows it, so any
        // later error is real and propagates immediately, exactly as before.
        //
        // `observed` also selects WHICH deadline governs. A run that has been
        // seen alive gets the full `POLL_MAX_WALL`; a run that has never been
        // seen at all gets only `POLL_MAX_STARTUP`, because "launched but
        // never registered" is a launch failure, not a long run, and waiting
        // four hours for it holds a fan-out semaphore slot the whole time.
        // The deadline only ever LENGTHENS: once `observed` flips true the
        // wall budget applies and is never shortened again.
        let mut observed = false;
        let mut last_startup_err: Option<String> = None;
        let started = tokio::time::Instant::now();
        let deadline = started + POLL_MAX_WALL;
        let startup_deadline = started + POLL_MAX_STARTUP;
        let mut attempt: u32 = 0;
        while tokio::time::Instant::now() < if observed { deadline } else { startup_deadline } {
            if attempt > 0 {
                tokio::time::sleep(poll_interval_for(attempt)).await;
            }
            attempt = attempt.saturating_add(1);

            let rec = match conn.get_run(&run_id).await {
                Ok(rec) => {
                    observed = true;
                    rec
                }
                Err(e) if !observed => {
                    last_startup_err = Some(e.to_string());
                    tracing::debug!(
                        run_id = %run_id,
                        attempt,
                        error = %last_startup_err.as_deref().unwrap_or(""),
                        "placed run not registered on the host yet; retrying"
                    );
                    continue;
                }
                Err(e) => return Err(host_err_to_run_err(e)),
            };

            // All HostConnector::get_run impls return the query_run_detail
            // envelope: {"run": <RunRecord>, "steps": [...], "usage": {...}}.
            // Read from the nested "run" object, not the top-level envelope.
            let run = &rec["run"];
            let status = run["status"].as_str().unwrap_or("").to_string();

            if is_terminal_status(&status) {
                // The host says terminal, but the coordinator-side mirror
                // (SSH's tail pump is a spawned task) may still be mid-flight
                // on its terminal work — final transcript catch-up above all.
                // This process is `rupu workflow run`: it exits the moment
                // the workflow completes, and a spawned task dies with it.
                // Join the mirror BEFORE reporting the unit terminal, or the
                // mirrored transcript is silently truncated at whatever the
                // tail happened to deliver (measured on a real host: the
                // closing assistant_message / turn_end / run_complete lines
                // lost). No-op for transports that don't mirror this way.
                conn.await_run_mirror(&run_id).await;

                let output = run["final_output"].as_str().unwrap_or("").to_string();
                let success = status == "completed";
                let error = (!success).then(|| {
                    run["error_message"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| status.clone())
                });

                // Collect the workspace delta when staging was active. On a
                // successful unit, surface collect/decode failures (losing the
                // delta would silently drop the unit's work). On a failed unit,
                // still collect best-effort so the host scratch is cleaned up,
                // but carry no delta.
                let workspace_delta = match (&working_dir, success) {
                    (Some(dir), true) => {
                        let bytes = conn
                            .collect_workspace_delta(dir)
                            .await
                            .map_err(host_err_to_run_err)?;
                        let delta = decode_delta(&bytes).map_err(host_err_to_run_err)?;
                        Some(to_orchestrator_delta(&delta))
                    }
                    (Some(dir), false) => {
                        let _ = conn.collect_workspace_delta(dir).await;
                        None
                    }
                    (None, _) => None,
                };

                return Ok(UnitOutcome {
                    output,
                    success,
                    error,
                    workspace_delta,
                });
            }
        }

        // The poll loop exhausted its attempts without seeing a terminal
        // status, so `collect_workspace_delta` above never ran for this unit
        // either — discard the remote scratch here for the same reason as the
        // `launch_agent` error arm above.
        if let Some(dir) = &working_dir {
            if let Err(discard_err) = conn.discard_workspace(dir).await {
                tracing::warn!(
                    host,
                    dir,
                    error = %discard_err,
                    "best-effort workspace discard failed after poll timeout"
                );
            }
        }

        Err(RunError::Provider(match last_startup_err {
            // Never observed: the run was launched but never registered on
            // the host. Say that, rather than "timed out polling" — the
            // distinction is the difference between "still running, we gave
            // up watching" and "it never started", and the connector's own
            // message for this case blames `rupu run show` support. Then ask
            // the connector what the remote process itself said: a launch
            // that dies instantly (wrong cwd, missing agent, bad flag,
            // unresolvable provider) explains itself on stderr, and the
            // connector may have kept that. Without it, this error is the
            // ONLY trace the failure leaves — measured: hours spent on a
            // "does not support `rupu run show`" that was a dead process.
            Some(err) => {
                let mut msg = format!(
                    "remote unit run {run_id} never registered on host {host}: \
                     gave up after {attempt} polls over {}s (the startup \
                     deadline, not the {}h run budget) WITHOUT EVER OBSERVING \
                     THE RUN. The launch was accepted but the host never \
                     reported the run even once, so the remote process almost \
                     certainly died before registering it — check that host's \
                     launch log for this run. Last poll error: {err}",
                    POLL_MAX_STARTUP.as_secs(),
                    POLL_MAX_WALL.as_secs() / 3600,
                );
                if let Some(diag) = conn.launch_diagnostics(&run_id).await {
                    msg.push_str(&format!(
                        "\nremote launch stderr (host {host}, run {run_id}):\n{diag}"
                    ));
                }
                msg
            }
            None => format!(
                "timed out waiting for remote unit run {run_id} on host {host} \
                 after {attempt} polls over {}s; the run may STILL BE RUNNING \
                 on the host — its work is not lost, only unobserved from here",
                POLL_MAX_WALL.as_secs()
            ),
        }))
    }

    /// Bridge the orchestrator's opaque deltas to the `rupu-workspace` codec and
    /// apply them to the coordinator workspace. Conflicts (overlapping tar files
    /// or conflicting git hunks) become [`WorkspaceConflict`]; any other codec
    /// failure is surfaced as a conflict-class step failure too.
    async fn apply_workspace_deltas(
        &self,
        workspace_path: &Path,
        deltas: &[WorkspaceDelta],
    ) -> Result<(), WorkspaceConflict> {
        let mut codec = Vec::with_capacity(deltas.len());
        for wd in deltas {
            match from_orchestrator_delta(wd) {
                Ok(d) => codec.push(d),
                Err(e) => return Err(WorkspaceConflict(vec![e.to_string()])),
            }
        }
        match rupu_workspace::apply_deltas(workspace_path, &codec) {
            Ok(()) => Ok(()),
            Err(rupu_workspace::SyncError::Conflict(paths)) => Err(WorkspaceConflict(paths)),
            Err(e) => Err(WorkspaceConflict(vec![e.to_string()])),
        }
    }
}

// ── Registry builder ──────────────────────────────────────────────────────────

/// Build a `FleetUnitDispatcher` only when the workflow needs one.
///
/// Returns `None` when the workflow has no `distribute:` or `host:` step
/// (fast path — avoids constructing the registry).  When a dispatcher is
/// returned, it is wired to `run_store` so mirrored unit runs appear in the
/// same store the coordinator reads.
pub fn build_dispatcher_if_needed(
    workflow: &rupu_orchestrator::Workflow,
    global: &Path,
    run_store: Arc<rupu_orchestrator::runs::RunStore>,
    pricing: rupu_config::PricingConfig,
) -> Option<Arc<dyn UnitDispatcher>> {
    if !workflow
        .steps
        .iter()
        .any(|s| s.distribute.is_some() || s.host.is_some())
    {
        return None;
    }

    let node_registry = Arc::new(rupu_cp::node::NodeRegistry::new());
    let node_mirror = Arc::new(rupu_cp::node::NodeMirror::new(Arc::clone(&run_store)));
    let local = rupu_cp::host::local::LocalHostConnector::new(
        None,
        None,
        None,
        None,
        Arc::clone(&run_store),
        global.to_path_buf(),
    )
    .with_pricing(pricing.clone());
    let host_store = rupu_workspace::HostStore {
        root: global.join("hosts"),
    };
    let registry = HostRegistry::new(host_store, Arc::new(local)).with_tunnel_deps(
        node_registry,
        node_mirror,
        run_store,
        pricing,
    );

    Some(Arc::new(FleetUnitDispatcher::new(Arc::new(registry))))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use rupu_cp::{
        host::connector::{
            EventByteStream, HostConnector, HostConnectorError, HostInfo, RunListQuery,
        },
        launcher::LaunchRequest,
        session_sender::SendMessageRequest,
        session_starter::SessionStartRequest,
    };

    // ── Fake connector — success path ─────────────────────────────────────────

    struct FakeConnector {
        run_id: &'static str,
        get_run_response: serde_json::Value,
        /// How many `get_run` calls fail with `Unsupported` before the run
        /// "registers". Models the real ssh connector: `launch_agent` returns
        /// once the DETACHED remote command is spawned, so the run record does
        /// not exist until the remote `rupu` has actually started.
        get_run_failures_before_ready: std::sync::atomic::AtomicU32,
        /// What `launch_diagnostics` answers — the remote process's captured
        /// stderr. `None` models a transport that keeps none (the trait
        /// default) or a launch that wrote nothing.
        launch_stderr: Option<&'static str>,
        /// Ordered log of the connector calls the dispatcher made.
        calls: std::sync::Mutex<Vec<&'static str>>,
        /// How many polls report `running` before the run goes terminal.
        /// Models a placed run doing real work — the case the old fixed
        /// 120-poll budget could not express.
        non_terminal_polls: std::sync::atomic::AtomicU32,
    }

    impl FakeConnector {
        fn completed() -> Self {
            Self {
                run_id: "run_x",
                // Real envelope shape: {"run": <RunRecord>, "steps": [...], "usage": {...}}
                get_run_response: serde_json::json!({
                    "run": {
                        "status": "completed",
                        "final_output": "fake-out"
                    }
                }),
                get_run_failures_before_ready: std::sync::atomic::AtomicU32::new(0),
                launch_stderr: None,
                calls: Default::default(),
                non_terminal_polls: std::sync::atomic::AtomicU32::new(0),
            }
        }

        /// A run that is not registered on the host for its first `n` polls —
        /// the workspace-sync case, where staging delays the remote start.
        fn completed_after_startup_delay(n: u32) -> Self {
            let mut c = Self::completed();
            c.get_run_failures_before_ready = std::sync::atomic::AtomicU32::new(n);
            c
        }

        /// A run that stays non-terminal for `n` polls before completing —
        /// a placed run doing real work for longer than the old fixed budget.
        fn completed_after_non_terminal_polls(n: u32) -> Self {
            let mut c = Self::completed();
            c.non_terminal_polls = std::sync::atomic::AtomicU32::new(n);
            c
        }

        fn failed() -> Self {
            Self {
                run_id: "run_y",
                get_run_response: serde_json::json!({
                    "run": {
                        "status": "failed",
                        "error_message": "boom"
                    }
                }),
                get_run_failures_before_ready: std::sync::atomic::AtomicU32::new(0),
                launch_stderr: None,
                calls: Default::default(),
                non_terminal_polls: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl HostConnector for FakeConnector {
        async fn info(&self) -> Result<HostInfo, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_run(&self, _req: LaunchRequest) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_agent(
            &self,
            _req: AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            Ok(self.run_id.to_string())
        }
        async fn start_session(
            &self,
            _req: SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
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
            use std::sync::atomic::Ordering;
            self.calls.lock().unwrap().push("get_run");
            if self.get_run_failures_before_ready.load(Ordering::SeqCst) > 0 {
                self.get_run_failures_before_ready
                    .fetch_sub(1, Ordering::SeqCst);
                // The exact shape the ssh connector produces for a run the
                // host does not know yet.
                return Err(HostConnectorError::Unsupported(
                    "remote host h1 does not support `rupu run show`: \
                     host unreachable: [error] unknown run: run_x"
                        .into(),
                ));
            }
            if self.non_terminal_polls.load(Ordering::SeqCst) > 0 {
                self.non_terminal_polls.fetch_sub(1, Ordering::SeqCst);
                return Ok(serde_json::json!({ "run": { "status": "running" } }));
            }
            Ok(self.get_run_response.clone())
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
        async fn await_run_mirror(&self, _run_id: &str) {
            self.calls.lock().unwrap().push("await_run_mirror");
        }
        async fn launch_diagnostics(&self, _run_id: &str) -> Option<String> {
            self.calls.lock().unwrap().push("launch_diagnostics");
            self.launch_stderr.map(str::to_string)
        }
        // `pause_run`/`resume_run` are intentionally NOT overridden on any
        // fake `HostConnector` in this module — the real fleet dispatch path
        // (`FleetUnitDispatcher`) only ever launches + polls + collects a
        // unit's output, never pausing/resuming a unit run, so every fake
        // here inherits `HostConnector`'s default `Unsupported` impl, same as
        // the unoverridden `stage_workspace`/`collect_workspace_delta` below
        // (see `fleet_pause_resume_are_unsupported_by_default`).
        async fn stream_run_events(
            &self,
            _run_id: &str,
        ) -> Result<EventByteStream, HostConnectorError> {
            unimplemented!()
        }
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
    }

    // ── Fake connector — Unreachable on launch ────────────────────────────────

    struct UnreachableConnector;

    #[async_trait]
    impl HostConnector for UnreachableConnector {
        async fn info(&self) -> Result<HostInfo, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_run(&self, _req: LaunchRequest) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_agent(
            &self,
            _req: AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            Err(HostConnectorError::Unreachable("h1 is down".to_string()))
        }
        async fn start_session(
            &self,
            _req: SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
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
            unimplemented!()
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
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_unit() -> UnitDispatch {
        UnitDispatch {
            step_id: "s".to_string(),
            agent: "a".to_string(),
            rendered_prompt: "p".to_string(),
            index: 0,
            run_id: "r".to_string(),
            workspace_path: None,
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// `from_connector` with a fake that returns the real envelope
    /// `{"run":{"status":"completed","final_output":"fake-out"}}` on the
    /// first `get_run` poll → output "fake-out", success true.
    /// A placed run that is not registered on the host for its first polls
    /// still completes.
    ///
    /// `launch_agent` returns as soon as the DETACHED remote command is
    /// spawned, so the run record on the host is created asynchronously. The
    /// first poll therefore races registration and previously aborted the
    /// whole step: any `get_run` error propagated immediately, and the ssh
    /// connector reports a not-yet-known run as
    /// "does not support `rupu run show`" — which named the wrong subsystem
    /// and made a healthy remote run look like a transport capability gap.
    ///
    /// Measured before the fix: a placed step from a small workspace won the
    /// race and passed, while the same step from a 57 MB workspace lost it
    /// every time — because `workspace: sync` stages files before the remote
    /// `rupu` starts. The delay scales with the workspace, so it cannot be
    /// waited out with a fixed constant.
    #[tokio::test]
    async fn placed_run_survives_a_slow_remote_startup() {
        // Enough polls to fail the old code (which aborted on the very first
        // error) without making the test itself slow.
        let conn = Arc::new(FakeConnector::completed_after_startup_delay(5));
        let d = FleetUnitDispatcher::from_connector(conn);
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert!(
            out.success,
            "a run that registers late must still be reported as completed"
        );
        assert_eq!(out.output, "fake-out");
    }

    #[tokio::test]
    async fn fleet_dispatch_reads_final_output_from_mirror() {
        let conn = Arc::new(FakeConnector::completed());
        let d = FleetUnitDispatcher::from_connector(conn);
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert_eq!(out.output, "fake-out");
        assert!(out.success);
        assert!(out.error.is_none());
    }

    /// The dispatcher must join the coordinator-side mirror AFTER it observes
    /// terminal and BEFORE it returns the outcome. This process is the
    /// `rupu workflow run` CLI, which exits as soon as the workflow completes;
    /// the SSH connector's tail pump is a spawned task that dies with it. If
    /// the join is missing (or happens before the terminal poll), the
    /// mirrored transcript is truncated at whatever the tail delivered.
    #[tokio::test]
    async fn dispatch_joins_mirror_after_terminal_and_before_returning() {
        let conn = Arc::new(FakeConnector::completed());
        let d = FleetUnitDispatcher::from_connector(Arc::clone(&conn) as Arc<dyn HostConnector>);
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert!(out.success);

        let calls = conn.calls.lock().unwrap().clone();
        let last_get_run = calls
            .iter()
            .rposition(|c| *c == "get_run")
            .expect("dispatcher polled get_run");
        let join = calls
            .iter()
            .position(|c| *c == "await_run_mirror")
            .expect("dispatcher must call await_run_mirror before returning a terminal outcome");
        assert!(
            join > last_get_run,
            "await_run_mirror must follow the terminal get_run poll (calls: {calls:?})"
        );
    }

    /// `from_connector` with a fake that returns a failed envelope
    /// `{"run":{"status":"failed","error_message":"boom"}}` → success false,
    /// error contains "boom" (prefers error_message over status literal).
    #[tokio::test]
    async fn fleet_dispatch_failed_run_surfaces_error_message() {
        let conn = Arc::new(FakeConnector::failed());
        let d = FleetUnitDispatcher::from_connector(conn);
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert!(!out.success);
        let err = out.error.expect("failed run must have an error field");
        assert!(err.contains("boom"), "expected 'boom' in error, got: {err}");
    }

    /// A fake fleet `HostConnector` (never overrides `pause_run`/`resume_run`)
    /// surfaces `HostConnector`'s default `Unsupported` for both — the fleet
    /// dispatch path has no pause/resume routing (T5's SSH/HttpCp remote
    /// reach is a `HostConnector`-level concern; fan-out unit control is a
    /// separate follow-up), so a caller reaching a fleet-dispatched unit
    /// through this connector gets a clear error, never a silent no-op.
    #[tokio::test]
    async fn fleet_pause_resume_are_unsupported_by_default() {
        let conn = FakeConnector::completed();
        assert!(matches!(
            conn.pause_run("run_x").await,
            Err(HostConnectorError::Unsupported(_))
        ));
        assert!(matches!(
            conn.resume_run("run_x").await,
            Err(HostConnectorError::Unsupported(_))
        ));
    }

    /// `from_connector` with a fake whose `launch_agent` returns `Unreachable`
    /// → `dispatch_unit` returns `Err`.
    #[tokio::test]
    async fn fleet_dispatch_unreachable_host_errors() {
        let conn = Arc::new(UnreachableConnector);
        let d = FleetUnitDispatcher::from_connector(conn);
        let result = d.dispatch_unit(make_unit(), "h1").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unreachable") || msg.contains("h1 is down"));
    }

    /// A plain workflow (no `distribute:` / `host:` step) needs no fleet
    /// dispatcher — the fast path returns `None` without building a registry.
    #[test]
    fn build_dispatcher_none_for_plain_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let wf = rupu_orchestrator::Workflow::parse(
            r#"
name: plain
steps:
  - id: a
    agent: x
    prompt: "p"
"#,
        )
        .unwrap();
        let store = Arc::new(rupu_orchestrator::runs::RunStore::new(
            dir.path().join("runs"),
        ));
        let got = build_dispatcher_if_needed(
            &wf,
            dir.path(),
            store,
            rupu_config::PricingConfig::default(),
        );
        assert!(got.is_none(), "plain workflow must not get a dispatcher");
    }

    /// A workflow with a host-placed linear step needs a fleet dispatcher.
    #[test]
    fn build_dispatcher_some_for_host_placed_step() {
        let dir = tempfile::tempdir().unwrap();
        let wf = rupu_orchestrator::Workflow::parse(
            r#"
name: placed
steps:
  - id: a
    agent: x
    prompt: "p"
    host: worker-1
"#,
        )
        .unwrap();
        let store = Arc::new(rupu_orchestrator::runs::RunStore::new(
            dir.path().join("runs"),
        ));
        let got = build_dispatcher_if_needed(
            &wf,
            dir.path(),
            store,
            rupu_config::PricingConfig::default(),
        );
        assert!(got.is_some(), "host-placed workflow must get a dispatcher");
    }

    // ── Workspace-sync tests ──────────────────────────────────────────────────

    /// Build a one-file tar and wrap it in the dispatcher's wire encoding — the
    /// SAME `encode_delta` path `apply_workspace_deltas` decodes. The resulting
    /// bytes go into the orchestrator `WorkspaceDelta.payload`.
    fn tar_one(path: &str, body: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            b.append_data(&mut header, path, body.as_bytes()).unwrap();
            b.finish().unwrap();
        }
        let delta = rupu_workspace::Delta {
            mode: rupu_workspace::SyncMode::Tar,
            changed: vec![path.to_string()],
            deleted: vec![],
            bytes: buf,
        };
        rupu_cp::host::connector::encode_delta(&delta)
    }

    /// A transport that does not support workspace sync (default trait impls)
    /// surfaces a clear Unsupported error through the dispatcher.
    #[tokio::test]
    async fn workspace_sync_on_unsupported_transport_errors() {
        // UnreachableConnector inherits the default stage_workspace = Unsupported.
        let conn = Arc::new(UnreachableConnector);
        let d = FleetUnitDispatcher::from_connector(conn);
        let mut unit = make_unit();
        // Use a real dir so `pack` succeeds and `stage_workspace` (the default
        // Unsupported impl) is the genuine failure point.
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("f.txt"), "hi").unwrap();
        unit.workspace_path = Some(ws.path().to_path_buf());
        let err = d.dispatch_unit(unit, "h1").await.unwrap_err();
        let msg = err.to_string();
        // `UnreachableConnector` inherits the default `stage_workspace` impl
        // which returns `HostConnectorError::Unsupported("workspace sync")`.
        // The dispatcher must surface that — NOT silently fall through to the
        // launch step before staging is complete.
        assert!(
            msg.contains("workspace sync") || msg.contains("unsupported"),
            "expected unsupported-workspace-sync error, got: {msg}"
        );
    }

    /// apply_workspace_deltas bridges to the rupu-workspace tar codec: two
    /// disjoint tar deltas apply cleanly.
    #[tokio::test]
    async fn apply_bridges_to_workspace_codec() {
        let conn = Arc::new(FakeConnector::completed());
        let d = FleetUnitDispatcher::from_connector(conn);
        let ws = tempfile::tempdir().unwrap();
        // Build two disjoint tar-mode orchestrator deltas via the same encode
        // path the dispatcher uses (payload = wire-encoded one-file tar delta).
        let a = rupu_orchestrator::runner::WorkspaceDelta {
            changed: vec!["a.txt".into()],
            deleted: vec![],
            payload: tar_one("a.txt", "A"),
        };
        let b = rupu_orchestrator::runner::WorkspaceDelta {
            changed: vec!["b.txt".into()],
            deleted: vec![],
            payload: tar_one("b.txt", "B"),
        };
        d.apply_workspace_deltas(ws.path(), &[a, b]).await.unwrap();
        assert!(ws.path().join("a.txt").exists());
        assert!(ws.path().join("b.txt").exists());
    }

    /// Two deltas that both touch the same path → `WorkspaceConflict` whose
    /// `paths` name the conflicting file.  Validates the bridge's
    /// `SyncError::Conflict → WorkspaceConflict` mapping.
    #[tokio::test]
    async fn apply_workspace_deltas_conflict_on_overlapping_paths() {
        let conn = Arc::new(FakeConnector::completed());
        let d = FleetUnitDispatcher::from_connector(conn);
        let ws = tempfile::tempdir().unwrap();
        // Two deltas that both claim to change "shared.txt".
        let a = rupu_orchestrator::runner::WorkspaceDelta {
            changed: vec!["shared.txt".into()],
            deleted: vec![],
            payload: tar_one("shared.txt", "from-unit-A"),
        };
        let b = rupu_orchestrator::runner::WorkspaceDelta {
            changed: vec!["shared.txt".into()],
            deleted: vec![],
            payload: tar_one("shared.txt", "from-unit-B"),
        };
        let err = d
            .apply_workspace_deltas(ws.path(), &[a, b])
            .await
            .expect_err("overlapping deltas must conflict");
        // The conflict error must name the colliding path.
        let WorkspaceConflict(paths) = err;
        assert!(
            paths.iter().any(|p| p.contains("shared.txt")),
            "conflict paths must include 'shared.txt', got: {paths:?}"
        );
    }

    // ── Workspace-sync scratch-cleanup tests (T4, deferred from T2) ──────────
    //
    // `stage_workspace` succeeded (the unit has remote scratch on disk) but
    // the unit never reaches `collect_workspace_delta` — either because
    // `launch_agent` errors right after staging, or because the poll loop
    // times out. Both are leak paths unless the dispatcher calls
    // `discard_workspace` before returning the error.

    /// Fake connector: `stage_workspace` succeeds (returns a fixed dir),
    /// `launch_agent` always fails, and `discard_workspace` records whether
    /// it was called and with which dir — the launch-failure leak path.
    struct StageOkLaunchFailsConnector {
        staged_dir: &'static str,
        discard_called_with: std::sync::Mutex<Option<String>>,
    }

    impl StageOkLaunchFailsConnector {
        fn new(staged_dir: &'static str) -> Self {
            Self {
                staged_dir,
                discard_called_with: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl HostConnector for StageOkLaunchFailsConnector {
        async fn info(&self) -> Result<HostInfo, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_run(&self, _req: LaunchRequest) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_agent(
            &self,
            _req: AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            Err(HostConnectorError::Unreachable(
                "launch failed after staging".to_string(),
            ))
        }
        async fn start_session(
            &self,
            _req: SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
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
            unimplemented!()
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
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn stage_workspace(&self, _payload: Vec<u8>) -> Result<String, HostConnectorError> {
            Ok(self.staged_dir.to_string())
        }
        async fn collect_workspace_delta(
            &self,
            _working_dir: &str,
        ) -> Result<Vec<u8>, HostConnectorError> {
            unimplemented!("collect must never run when launch_agent fails before it")
        }
        async fn discard_workspace(&self, working_dir: &str) -> Result<(), HostConnectorError> {
            *self.discard_called_with.lock().unwrap() = Some(working_dir.to_string());
            Ok(())
        }
    }

    /// After a successful `stage_workspace`, a `launch_agent` failure must
    /// trigger `discard_workspace(&staged_dir)` before the dispatcher returns
    /// its error — otherwise the remote scratch dir leaks forever since
    /// `collect_workspace_delta` never runs.
    #[tokio::test]
    async fn launch_failure_after_stage_discards_scratch() {
        let conn = Arc::new(StageOkLaunchFailsConnector::new(
            "/cache/workspace-sync/leak-1/work",
        ));
        let d = FleetUnitDispatcher::from_connector(Arc::clone(&conn) as Arc<dyn HostConnector>);

        let mut unit = make_unit();
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("f.txt"), "hi").unwrap();
        unit.workspace_path = Some(ws.path().to_path_buf());

        let err = d.dispatch_unit(unit, "h1").await.unwrap_err();
        assert!(err.to_string().contains("launch failed after staging"));

        assert_eq!(
            conn.discard_called_with.lock().unwrap().as_deref(),
            Some("/cache/workspace-sync/leak-1/work"),
            "discard_workspace must be called with the staged dir after launch_agent fails"
        );
    }

    /// Fake connector: `stage_workspace` and `launch_agent` succeed, but
    /// `get_run` always reports a non-terminal status — forcing the poll
    /// loop to exhaust `POLL_MAX_ATTEMPTS` and time out. `discard_workspace`
    /// records whether it was called — the poll-timeout leak path.
    struct StageOkPollNeverTerminalConnector {
        staged_dir: &'static str,
        discard_called_with: std::sync::Mutex<Option<String>>,
    }

    impl StageOkPollNeverTerminalConnector {
        fn new(staged_dir: &'static str) -> Self {
            Self {
                staged_dir,
                discard_called_with: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl HostConnector for StageOkPollNeverTerminalConnector {
        async fn info(&self) -> Result<HostInfo, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_run(&self, _req: LaunchRequest) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_agent(
            &self,
            _req: AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            Ok("run_timeout".to_string())
        }
        async fn start_session(
            &self,
            _req: SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
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
            Ok(serde_json::json!({ "run": { "status": "running" } }))
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
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn stage_workspace(&self, _payload: Vec<u8>) -> Result<String, HostConnectorError> {
            Ok(self.staged_dir.to_string())
        }
        async fn collect_workspace_delta(
            &self,
            _working_dir: &str,
        ) -> Result<Vec<u8>, HostConnectorError> {
            unimplemented!("collect must never run when the poll loop times out")
        }
        async fn discard_workspace(&self, working_dir: &str) -> Result<(), HostConnectorError> {
            *self.discard_called_with.lock().unwrap() = Some(working_dir.to_string());
            Ok(())
        }
    }

    /// After a successful stage + launch, a poll loop that never observes a
    /// terminal status must discard the staged scratch before surfacing the
    /// timeout error — same leak-prevention contract as the launch-failure
    /// arm above. Runs with tokio's paused virtual clock so the 120 ×
    /// 500 ms poll budget (60 s of real time) resolves instantly: with no
    /// other work pending, tokio auto-advances virtual time past each
    /// `sleep`.
    /// A placed run that outlives the OLD fixed budget still completes.
    ///
    /// The previous loop was `for attempt in 0..120` at a flat 500 ms
    /// interval — 60 SECONDS total for a placed run to reach terminal.
    /// Measured on a live host, an enumeration agent ran 103 s and finished
    /// `completed`; the coordinator abandoned it at 60 s, reported the step
    /// failed, and discarded its workspace delta. The work had succeeded and
    /// only the result was thrown away.
    ///
    /// 400 non-terminal polls is comfortably past 120, so this fails against
    /// the old bound and passes against a wall-clock budget.
    #[tokio::test(start_paused = true)]
    async fn placed_run_outliving_the_old_poll_budget_still_completes() {
        let conn = Arc::new(FakeConnector::completed_after_non_terminal_polls(400));
        let d = FleetUnitDispatcher::from_connector(conn);
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert!(
            out.success,
            "a long-running placed run must complete, not time out"
        );
        assert_eq!(out.output, "fake-out");
    }

    #[tokio::test(start_paused = true)]
    async fn poll_timeout_after_stage_discards_scratch() {
        let conn = Arc::new(StageOkPollNeverTerminalConnector::new(
            "/cache/workspace-sync/leak-2/work",
        ));
        let d = FleetUnitDispatcher::from_connector(Arc::clone(&conn) as Arc<dyn HostConnector>);

        let mut unit = make_unit();
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("f.txt"), "hi").unwrap();
        unit.workspace_path = Some(ws.path().to_path_buf());

        let err = d.dispatch_unit(unit, "h1").await.unwrap_err();
        assert!(err.to_string().contains("timed out"));

        assert_eq!(
            conn.discard_called_with.lock().unwrap().as_deref(),
            Some("/cache/workspace-sync/leak-2/work"),
            "discard_workspace must be called with the staged dir after a poll timeout"
        );
    }

    /// A launch the host ACCEPTED but whose process died before registering
    /// the run (wrong cwd, missing agent, bad flag …) exhausts the poll
    /// budget. The dispatcher's error must then carry what the remote
    /// process wrote to stderr — the connector kept it — not just "never
    /// registered … does not support `rupu run show`", which is what an
    /// operator spent hours on when the real cause was a dead process.
    /// Paused virtual clock, as in `poll_timeout_after_stage_discards_scratch`.
    #[tokio::test(start_paused = true)]
    async fn never_registered_error_carries_remote_launch_stderr() {
        // Never registers: more startup failures than the poll budget.
        let mut fake = FakeConnector::completed_after_startup_delay(u32::MAX);
        fake.launch_stderr = Some("Error: agent `a` not found under ~/.rupu/agents");
        let conn = Arc::new(fake);
        let d = FleetUnitDispatcher::from_connector(Arc::clone(&conn) as Arc<dyn HostConnector>);

        let err = d
            .dispatch_unit(make_unit(), "h1")
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("never registered"), "{err}");
        assert!(
            err.contains("remote launch stderr"),
            "the diagnostics section must be labelled: {err}"
        );
        assert!(
            err.contains("Error: agent `a` not found under ~/.rupu/agents"),
            "the remote process's own explanation must reach the operator: {err}"
        );
        assert!(
            conn.calls.lock().unwrap().contains(&"launch_diagnostics"),
            "the dispatcher must ask the connector for the launch log"
        );
    }

    // ── Startup-deadline tests ────────────────────────────────────────────────
    //
    // `POLL_MAX_WALL` is the budget for a run that is ALIVE. Applying it to a
    // run that was never observed is what let a dead remote launch hold a
    // fan-out semaphore slot for hours. These three tests pin all three sides
    // of the split: never-observed fails fast, a realistically-slow startup
    // still wins the race (#645), and an observed long run keeps the full
    // wall budget (#649).

    /// A placed run whose `get_run` NEVER succeeds must fail at
    /// [`POLL_MAX_STARTUP`], not at [`POLL_MAX_WALL`].
    ///
    /// Measured in production: a remote agent died at launch
    /// (`save worker record: io rename …: No such file or directory`) and
    /// never wrote `run.json`. The coordinator polled it for 90+ minutes and
    /// was on course for four hours; with `max_parallel: 2`, two such units
    /// held both slots and the workflow silently stopped progressing.
    ///
    /// Paused virtual clock, so the whole budget resolves instantly and the
    /// elapsed VIRTUAL time is an exact assertion about which deadline fired.
    #[tokio::test(start_paused = true)]
    async fn never_observed_run_fails_at_the_startup_deadline_not_the_wall_budget() {
        let mut fake = FakeConnector::completed_after_startup_delay(u32::MAX);
        fake.launch_stderr = Some("[error] save worker record: io rename ...: No such file");
        let conn = Arc::new(fake);
        let d = FleetUnitDispatcher::from_connector(Arc::clone(&conn) as Arc<dyn HostConnector>);

        let t0 = tokio::time::Instant::now();
        let err = d
            .dispatch_unit(make_unit(), "h1")
            .await
            .unwrap_err()
            .to_string();
        let elapsed = t0.elapsed();

        // The load-bearing assertion: which deadline fired. The loop can only
        // overshoot by the interval it was sleeping when the deadline passed,
        // so allow one slow interval of slack — and nothing like four hours.
        assert!(
            elapsed >= POLL_MAX_STARTUP && elapsed < POLL_MAX_STARTUP + POLL_INTERVAL_SLOW * 2,
            "a never-observed run must give up at the {}s startup deadline, \
             not the {}s wall budget; gave up after {}s",
            POLL_MAX_STARTUP.as_secs(),
            POLL_MAX_WALL.as_secs(),
            elapsed.as_secs()
        );

        // The operator must be told the cause, not just that time ran out.
        assert!(err.contains("never registered"), "{err}");
        assert!(
            err.contains("WITHOUT EVER OBSERVING THE RUN"),
            "the error must say plainly that the run was never observed: {err}"
        );
        // The underlying `get_run` error — the thing that actually explains it.
        assert!(
            err.contains("unknown run"),
            "the error must carry the underlying get_run error: {err}"
        );
        assert!(
            err.contains("launch log"),
            "the error must point at the operator's next move: {err}"
        );
        // And the remote process's own stderr when the connector kept it.
        assert!(
            err.contains("save worker record"),
            "the remote launch stderr must reach the operator: {err}"
        );
    }

    /// A REALISTIC slow startup is still won, not cut off — the #645
    /// regression must not come back through the new deadline.
    ///
    /// 66 failing polls is ~60 s of virtual time under `poll_interval_for`
    /// (59 fast polls = 29.5 s, then 5 s each), which is the measured cost of
    /// staging a ~50 MB workspace over SSH before the remote `rupu` starts.
    /// That is comfortably inside [`POLL_MAX_STARTUP`] — the whole point of
    /// choosing five minutes rather than the 60 s the pre-#649 bound gave.
    #[tokio::test(start_paused = true)]
    async fn run_observed_after_a_realistic_startup_delay_still_completes() {
        let conn = Arc::new(FakeConnector::completed_after_startup_delay(66));
        let d = FleetUnitDispatcher::from_connector(conn);

        let t0 = tokio::time::Instant::now();
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        let elapsed = t0.elapsed();

        assert!(
            out.success,
            "a run that registers after a realistic staging delay must complete"
        );
        assert_eq!(out.output, "fake-out");
        assert!(
            elapsed < POLL_MAX_STARTUP,
            "sanity: this delay must sit inside the startup deadline, took {}s",
            elapsed.as_secs()
        );
    }

    /// Once the run HAS been observed, the full [`POLL_MAX_WALL`] budget
    /// governs — the startup deadline must never shorten a run that is
    /// demonstrably alive (#649).
    ///
    /// The fake first loses the startup race for 5 polls, then reports
    /// `running` for 400 more — ~29 minutes of virtual time, far past
    /// [`POLL_MAX_STARTUP`] — before completing. If the startup deadline
    /// leaked into the observed path this would fail instead of completing.
    #[tokio::test(start_paused = true)]
    async fn observed_long_run_keeps_the_full_wall_clock_budget() {
        let mut fake = FakeConnector::completed_after_startup_delay(5);
        fake.non_terminal_polls = std::sync::atomic::AtomicU32::new(400);
        let conn = Arc::new(fake);
        let d = FleetUnitDispatcher::from_connector(conn);

        let t0 = tokio::time::Instant::now();
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        let elapsed = t0.elapsed();

        assert!(
            out.success,
            "an observed long-running placed run must complete, not time out"
        );
        assert_eq!(out.output, "fake-out");
        assert!(
            elapsed > POLL_MAX_STARTUP,
            "this run must outlive the startup deadline for the test to mean \
             anything; it only took {}s",
            elapsed.as_secs()
        );
        assert!(elapsed < POLL_MAX_WALL, "and stay inside the wall budget");
    }
}
