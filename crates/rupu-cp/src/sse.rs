//! SSE bridge helpers — convert a [`FileTailRunSource`] stream into an axum
//! [`Sse`] response.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::{Stream, StreamExt as _};
use rupu_orchestrator::executor::{Event, FileTailRunSource};
use rupu_orchestrator::runs::{RunStatus, RunStore};
use tokio::sync::mpsc;

/// Tail run `run_id`'s `events.jsonl` in `store` as an SSE stream. Each rupu
/// [`Event`] becomes one SSE `data:` line of JSON. The stream is live — it
/// stays open and emits events as the run progresses, never terminating on
/// its own. A legacy (pre-codename) run's step/unit/dispatch events get a
/// derived codename (see [`sse_frame`]).
///
/// [`Event`]: rupu_orchestrator::executor::Event
pub async fn tail_events_sse(
    store: Arc<RunStore>,
    run_id: &str,
) -> std::io::Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    let source = FileTailRunSource::open(&store.events_path(run_id)).await?;
    // Classified once, at attach, off the executor: `None` for a
    // codename-era run, whose events then pass through untouched.
    let namers = crate::codename_legacy::namers_for_run(store, run_id).await;
    let stream = source.then(move |ev| {
        let namers = namers.clone();
        async move { Ok::<_, Infallible>(sse_frame(namers.as_ref(), ev).await) }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// One SSE frame for `ev`. An agent-announcing event without a codename of a
/// legacy run is re-serialized with a derived one + `codename_derived: true`
/// (any disk work on the blocking pool); every other event is serialized
/// exactly as before.
async fn sse_frame(
    namers: Option<&crate::codename_legacy::SharedEventNamers>,
    ev: Event,
) -> SseEvent {
    let row = match namers {
        Some(n) => crate::codename_legacy::name_event(n, &ev).await,
        None => None,
    };
    let framed = match row {
        Some(row) => SseEvent::default().json_data(&row),
        None => SseEvent::default().json_data(&ev),
    };
    framed.unwrap_or_else(|_| SseEvent::default().comment("event serialize error"))
}

/// An already-closed SSE stream for a run with no `events.jsonl` anywhere
/// (`RunLocation::Unpersisted` — the dispatch failed before/without ever
/// persisting a run directory). Returns a valid, empty 200 response rather
/// than erroring: there is genuinely nothing to tail, which is distinct from
/// a read failure.
pub fn empty_events_sse() -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = futures_util::stream::empty::<Result<SseEvent, Infallible>>();
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// A `Stream` over a plain mpsc receiver — lets us return the merged
/// multi-run event channel as a `Stream` without pulling in `tokio-stream`
/// (mirrors the wrapper [`FileTailRunSource`] uses internally).
struct MergedEvents {
    rx: mpsc::Receiver<Event>,
}

impl Stream for MergedEvents {
    type Item = Event;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

/// How long a run may sit in a terminal status before the coordinator stops
/// tailing its `events.jsonl` for this connection. A terminal run stops
/// appending, and its tail is drained within one 250 ms poll — the grace is
/// pure safety margin for the window where the final events land just after
/// `run.json` flips terminal. Without pruning, every forwarder lives for the
/// whole connection (a [`FileTailRunSource`] never ends on its own), so a
/// long-lived client accumulates one full-file 250 ms poller per run started
/// while it was connected — a day-scale connection against a busy server
/// degrades into hundreds of pointless file reads per second.
const TERMINAL_FORWARDER_GRACE: Duration = Duration::from_secs(30);

/// Tail **every** run's `events.jsonl` and merge them into a single live SSE
/// firehose — the global Live Events stream (no `?run` selector).
///
/// A coordinator task lists runs once per second and attaches a
/// [`FileTailRunSource`] to each run we haven't tailed yet:
///   - On the **first** pass it attaches only to currently-active
///     (Running / Pending / AwaitingApproval / Paused) runs, so the firehose
///     isn't flooded with the entire history of every terminal run on disk.
///   - On **later** passes it attaches to any run id not seen before — i.e. a
///     run created after the client connected — regardless of status, so a
///     short run launched live is shown start-to-finish.
///   - A tailed run that has sat in a *terminal* status for
///     [`TERMINAL_FORWARDER_GRACE`] is pruned (its forwarder aborted), so a
///     long-lived connection's per-run pollers don't accumulate forever.
///
/// This replaces the Phase-1 single-run tail: when many runs execute
/// concurrently, every run's events flow through the one stream, and a run
/// whose `events.jsonl` is missing or empty no longer blocks the whole feed.
pub async fn tail_all_events_sse(
    run_store: Arc<RunStore>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Event>(256);
    // Shared with the coordinator: it classifies each run from the record it
    // lists (codename-era runs never open a legacy namer) and evicts a run's
    // entry when it prunes that run's tail, so the cache stays bounded.
    let namers = crate::codename_legacy::EventNamers::shared(run_store.clone());
    let _coordinator = spawn_firehose_coordinator_named(
        run_store,
        tx,
        TERMINAL_FORWARDER_GRACE,
        Some(namers.clone()),
    );

    let stream = MergedEvents { rx }.then(move |ev| {
        let namers = namers.clone();
        async move { Ok::<_, Infallible>(sse_frame(Some(&namers), ev).await) }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// The coordinator behind [`tail_all_events_sse`], factored out (with an
/// injectable prune grace) so its attach / deliver / prune behavior is
/// testable without an axum server. Sends merged events into `tx` until the
/// receiving side is dropped, then aborts every forwarder and exits;
/// returns the coordinator task's handle so tests can observe that exit.
#[cfg(test)]
fn spawn_firehose_coordinator(
    run_store: Arc<RunStore>,
    tx: mpsc::Sender<Event>,
    terminal_grace: Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_firehose_coordinator_named(run_store, tx, terminal_grace, None)
}

/// [`spawn_firehose_coordinator`] plus the stream's shared codename cache:
/// each attached run is classified from its listed record, and its entry is
/// evicted when its tail is pruned.
fn spawn_firehose_coordinator_named(
    run_store: Arc<RunStore>,
    tx: mpsc::Sender<Event>,
    terminal_grace: Duration,
    namers: Option<crate::codename_legacy::SharedEventNamers>,
) -> tokio::task::JoinHandle<()> {
    let with_namers = move |f: &mut dyn FnMut(&mut crate::codename_legacy::EventNamers)| {
        if let Some(n) = &namers {
            f(&mut n.lock().unwrap_or_else(|p| p.into_inner()));
        }
    };
    tokio::spawn(async move {
        let mut seen: HashSet<String> = HashSet::new();
        // One forwarder task per tailed run, keyed by run id. A
        // `FileTailRunSource` never ends on its own (it polls forever), so
        // forwarders are aborted explicitly: individually once their run has
        // been terminal past `terminal_grace` (or vanished from the store),
        // and collectively when the client disconnects — otherwise each
        // would stay parked, keeping its 250 ms file-poll loop alive
        // indefinitely.
        let mut forwarders: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        // When each tailed run was *first observed* in a terminal status —
        // the prune clock. Terminal statuses never transition back, so an
        // entry only ever leaves this map when its forwarder is pruned.
        let mut terminal_since: HashMap<String, tokio::time::Instant> = HashMap::new();
        let mut first = true;
        loop {
            if tx.is_closed() {
                for h in forwarders.values() {
                    h.abort();
                }
                break;
            }
            match run_store.list() {
                Ok(runs) => {
                    for r in &runs {
                        let active = matches!(
                            r.status,
                            RunStatus::Running
                                | RunStatus::Pending
                                | RunStatus::AwaitingApproval
                                | RunStatus::Paused
                        );
                        // First pass: only currently-active runs. Later passes:
                        // any id we haven't seen (a run started after connect).
                        let take = if first { active } else { !seen.contains(&r.id) };
                        if take && seen.insert(r.id.clone()) {
                            with_namers(&mut |n| n.note_record(r));
                            let path = run_store.events_path(&r.id);
                            if let Ok(mut src) = FileTailRunSource::open(&path).await {
                                let tx_run = tx.clone();
                                tracing::debug!(run_id = %r.id, "events firehose: attached tail");
                                forwarders.insert(
                                    r.id.clone(),
                                    tokio::spawn(async move {
                                        while let Some(ev) = src.next().await {
                                            if tx_run.send(ev).await.is_err() {
                                                break;
                                            }
                                        }
                                    }),
                                );
                            }
                        }
                    }
                    // Mark every currently-known id as seen so subsequent passes
                    // treat only genuinely-new runs as new.
                    if first {
                        for r in &runs {
                            seen.insert(r.id.clone());
                        }
                    }

                    // ── prune ──
                    let now = tokio::time::Instant::now();
                    let listed: HashSet<&str> = runs.iter().map(|r| r.id.as_str()).collect();
                    for r in &runs {
                        if r.status.is_terminal() && forwarders.contains_key(&r.id) {
                            terminal_since.entry(r.id.clone()).or_insert(now);
                        }
                    }
                    let expired: Vec<String> = forwarders
                        .keys()
                        .filter(|id| {
                            // A run that vanished from the store entirely
                            // (deleted / archived) is pruned immediately; a
                            // terminal run after its grace.
                            !listed.contains(id.as_str())
                                || terminal_since
                                    .get(*id)
                                    .is_some_and(|t| now.duration_since(*t) >= terminal_grace)
                        })
                        .cloned()
                        .collect();
                    for id in expired {
                        with_namers(&mut |n| n.evict(&id));
                        if let Some(h) = forwarders.remove(&id) {
                            h.abort();
                        }
                        terminal_since.remove(&id);
                        tracing::debug!(run_id = %id, "events firehose: pruned terminal run tail");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "events firehose: failed to list runs");
                }
            }
            first = false;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use rupu_orchestrator::runs::RunRecord;
    use std::collections::BTreeMap;
    use std::io::Write as _;
    use std::path::PathBuf;

    fn seed_run(store: &RunStore, id: &str, status: RunStatus) -> RunRecord {
        let record = RunRecord {
            customer: None,
            id: id.into(),
            workflow_name: "test-workflow".into(),
            status,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_test".into(),
            workspace_path: PathBuf::from("/tmp/test-proj"),
            transcript_dir: PathBuf::from("/tmp/test-proj/.rupu/transcripts"),
            started_at: Utc.timestamp_opt(1_000, 0).unwrap(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            resume_rerequested_at: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            final_output: None,
            loop_progress: Default::default(),
            gate_decisions: Vec::new(),
            codename: None,
            cause: None,
        };
        store
            .create(record.clone(), "name: test\nsteps: []\n")
            .expect("create run");
        record
    }

    fn append_event(store: &RunStore, run_id: &str, step_id: &str) {
        let ev = Event::StepStarted {
            run_id: run_id.into(),
            step_id: step_id.into(),
            kind: rupu_orchestrator::runs::StepKind::Linear,
            agent: None,
            host: None,
            codename: None,
        };
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(store.events_path(run_id))
            .expect("open events.jsonl");
        writeln!(f, "{}", serde_json::to_string(&ev).expect("serialize")).expect("append");
    }

    async fn recv_step(rx: &mut mpsc::Receiver<Event>) -> Option<String> {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Some(Event::StepStarted { step_id, .. })) => Some(step_id),
            _ => None,
        }
    }

    /// End-to-end coordinator contract: a run created *after* connect is
    /// attached and its events delivered; once the run flips terminal and
    /// the grace elapses, its tail is pruned — late appends to a pruned
    /// run's `events.jsonl` are no longer delivered.
    ///
    /// `start_paused` keeps every internal sleep (the coordinator's 1 s
    /// pass, the tail's 250 ms poll, the grace window) virtual, so the test
    /// is fast and non-flaky while still exercising real file I/O.
    #[tokio::test(start_paused = true)]
    async fn firehose_attaches_new_runs_and_prunes_terminal_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(RunStore::new(tmp.path().join("runs")));

        let (tx, mut rx) = mpsc::channel::<Event>(256);
        spawn_firehose_coordinator(store.clone(), tx, Duration::from_secs(5));

        // Let the first pass complete on an empty store.
        tokio::time::sleep(Duration::from_secs(2)).await;

        // A run created after connect must be attached and delivered.
        let mut record = seed_run(&store, "run_live", RunStatus::Running);
        append_event(&store, "run_live", "step_one");
        assert_eq!(recv_step(&mut rx).await.as_deref(), Some("step_one"));

        // Still tailed while running: a later append is delivered too.
        append_event(&store, "run_live", "step_two");
        assert_eq!(recv_step(&mut rx).await.as_deref(), Some("step_two"));

        // Flip terminal and outlast the grace: the tail must be pruned.
        record.status = RunStatus::Completed;
        store.update(&record).expect("update status");
        tokio::time::sleep(Duration::from_secs(10)).await;

        // An append after pruning must NOT be delivered.
        append_event(&store, "run_live", "step_after_prune");
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Err(_) => {} // timed out with no event — pruned, as required
            Ok(ev) => panic!("expected no delivery after prune, got {ev:?}"),
        }
    }

    /// The firehose's codename cache is bounded by the tails it holds: an
    /// attached run is classified from its listed record, and its entry is
    /// evicted when its tail is pruned.
    #[tokio::test(start_paused = true)]
    async fn firehose_evicts_a_pruned_runs_codename_cache_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(RunStore::new(tmp.path().join("runs")));
        let namers = crate::codename_legacy::EventNamers::shared(store.clone());
        let (tx, mut rx) = mpsc::channel::<Event>(256);
        spawn_firehose_coordinator_named(
            store.clone(),
            tx,
            Duration::from_secs(5),
            Some(namers.clone()),
        );
        tokio::time::sleep(Duration::from_secs(2)).await;

        let mut record = seed_run(&store, "run_live", RunStatus::Running);
        record.codename = Some("jade-reef".into());
        store.update(&record).expect("update");
        append_event(&store, "run_live", "step_one");
        assert_eq!(recv_step(&mut rx).await.as_deref(), Some("step_one"));
        assert!(namers.lock().unwrap().known_modern("run_live"));

        record.status = RunStatus::Completed;
        store.update(&record).expect("update status");
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(namers.lock().unwrap().cached_runs(), 0);
    }

    /// Dropping the receiving side stops the coordinator: its task exits
    /// (aborting every forwarder on the way out) instead of polling the
    /// store forever for a client that is gone.
    #[tokio::test(start_paused = true)]
    async fn firehose_coordinator_exits_when_client_disconnects() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(RunStore::new(tmp.path().join("runs")));

        let (tx, mut rx) = mpsc::channel::<Event>(256);
        let handle = spawn_firehose_coordinator(store.clone(), tx, Duration::from_secs(5));

        tokio::time::sleep(Duration::from_secs(2)).await;
        seed_run(&store, "run_x", RunStatus::Running);
        append_event(&store, "run_x", "step_one");
        assert_eq!(recv_step(&mut rx).await.as_deref(), Some("step_one"));

        drop(rx);
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("coordinator must exit after the client disconnects")
            .expect("coordinator task must not panic");
    }
}
