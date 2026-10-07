//! The agentiflow operator loop, end to end, with real processes and the mock
//! provider: `run --detach` -> `send` -> `stop` / `stop --now` -> the orphan
//! reaper (`serve`) -> `attach`.
//!
//! Every test drives the real `rupu` binary and then looks at what the run left
//! on disk, or at the processes themselves: a command's exit code is never the
//! assertion, because a `send` that queues nothing and a `stop` that signals no
//! one both exit 0. Delivery is what is checked: the steering was consumed and
//! reached the lead's prompt, the record went terminal, the coordinator and the
//! unit's process group are gone, the log carries its terminal event.
//!
//! How the mock is made to take time. The mock provider has no delay turn, so a
//! round is slowed with the lead's own `bash` tool (`sleep N`), which runs for
//! real. A scripted `dispatch` makes the lead launch a real unit (a `rupu run`
//! child of its own, a process-group leader) whose model has its own script in
//! the `models` map: it holds a `sleep 300`.
//!
//! Every process a test starts is torn down by a guard on the way out, failing
//! assertions included, so nothing outlives the test.

use crate::ENV_LOCK;
use assert_cmd::Command as AssertCommand;
use rupu_agentiflow::{kill_group, pid_is_running, AgentiflowRecord};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const UNIT_MODEL: &str = "claude-unit-mock";

/// The lead's definition. The one goal ("record a finding") is never met, so a
/// run only stops when the ceiling, the operator or the reaper stops it.
fn def_yaml(ceiling_rounds: u32) -> String {
    format!(
        "name: acme\n\
         description: A tiny agentiflow for the operator-loop tests.\n\
         lead: lead\n\
         engagement_profiles: [network]\n\
         goals:\n  \
           - id: any-finding\n    \
             objective: \"Record at least one finding.\"\n    \
             target: {{ findings: {{}}, count_gte: 1 }}\n\
         scope: {{ authorized: true }}\n\
         pool: {{ agents: [lead, worker] }}\n\
         round: {{ lead_max_turns: 6, ceiling: {{ rounds: {ceiling_rounds} }} }}\n"
    )
}

/// An isolated rupu home with `acme`, its lead and a `worker` the lead may
/// dispatch, and a project directory (no `.rupu/`) to run from.
struct Fixture {
    _tmp: assert_fs::TempDir,
    global: PathBuf,
    project: PathBuf,
}

fn fixture(ceiling_rounds: u32) -> Fixture {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.path().join(".rupu");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(global.join("agents")).unwrap();
    std::fs::create_dir_all(global.join("agentiflows")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    // Both agents have `bash`: the lead and the unit each hold a round open
    // with a real `sleep`.
    std::fs::write(
        global.join("agents/lead.md"),
        "---\nname: lead\nprovider: anthropic\nmodel: claude-lead-mock\ntools: [bash]\n---\n\
         You are the lead.\n",
    )
    .unwrap();
    std::fs::write(
        global.join("agents/worker.md"),
        format!(
            "---\nname: worker\nprovider: anthropic\nmodel: {UNIT_MODEL}\ntools: [bash]\n---\n\
             You are a worker.\n"
        ),
    )
    .unwrap();
    std::fs::write(
        global.join("agentiflows/acme.yaml"),
        def_yaml(ceiling_rounds),
    )
    .unwrap();
    Fixture {
        _tmp: tmp,
        global,
        project,
    }
}

/// The variables a hermetic `rupu` must not inherit.
const AMBIENT: [&str; 5] = [
    "RUPU_AUTH_FILE",
    "RUPU_ANTHROPIC_API_KEY",
    "RUPU_OPENAI_API_KEY",
    "RUPU_GEMINI_API_KEY",
    "RUPU_COPILOT_API_KEY",
];

impl Fixture {
    /// `rupu <args>`, hermetic: this fixture's home and project, no ambient
    /// provider credentials, no stdin, and a bound so a hang is a failure and
    /// not a stuck test run.
    fn rupu(&self, mock_script: &str, args: &[&str]) -> AssertCommand {
        let mut cmd = AssertCommand::cargo_bin("rupu").unwrap();
        cmd.env("RUPU_HOME", &self.global)
            .env("RUPU_MOCK_PROVIDER_SCRIPT", mock_script)
            .current_dir(&self.project)
            .write_stdin("")
            .timeout(Duration::from_secs(90));
        for var in AMBIENT {
            cmd.env_remove(var);
        }
        cmd.args(args);
        cmd
    }

    /// The same, as a plain `Command` for a process the test keeps running.
    fn spawnable(&self, mock_script: &str, args: &[&str]) -> std::process::Command {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("rupu"));
        cmd.env("RUPU_HOME", &self.global)
            .env("RUPU_MOCK_PROVIDER_SCRIPT", mock_script)
            .current_dir(&self.project)
            .stdin(Stdio::null());
        for var in AMBIENT {
            cmd.env_remove(var);
        }
        cmd.args(args);
        cmd
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        self.global.join("agentiflows").join(id)
    }

    fn record(&self, id: &str) -> AgentiflowRecord {
        AgentiflowRecord::read(&self.run_dir(id)).unwrap()
    }
}

/// Tears down every process a test started, on every path out of it: SIGKILLs
/// the recorded process groups (a coordinator and its `bash` children, a unit)
/// and any plain children.
#[derive(Default)]
struct Cleanup {
    groups: Vec<u32>,
    children: Vec<std::process::Child>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for pgid in &self.groups {
            kill_group(*pgid);
        }
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---- mock scripts ---------------------------------------------------------------

fn bash(call_id: &str, command: &str) -> Value {
    json!({ "AssistantToolUse": {
        "text": null,
        "tool_id": call_id,
        "tool_name": "bash",
        "tool_input": { "command": command },
        "stop": "tool_use"
    } })
}

fn text(reply: &str) -> Value {
    json!({ "AssistantText": { "text": reply, "stop": "end_turn" } })
}

/// A lead whose every round holds open for `secs` seconds, then yields. (The
/// mock replays its script from the top each round: each round builds a new
/// provider.)
fn slow_rounds(secs: u32) -> String {
    json!([
        bash("call_hold", &format!("sleep {secs}")),
        text("Round done.")
    ])
    .to_string()
}

/// A lead that dispatches one real `worker` unit, then holds its round open; the
/// worker holds a `sleep 300` of its own.
fn lead_with_a_unit() -> String {
    json!({
        "turns": [
            { "AssistantToolUse": {
                "text": null,
                "tool_id": "call_dispatch",
                "tool_name": "dispatch",
                "tool_input": { "agent": "worker", "prompt": "hold until stopped" },
                "stop": "tool_use"
            } },
            bash("call_hold", "sleep 120"),
            text("Round done.")
        ],
        "models": { UNIT_MODEL: [bash("call_unit_hold", "sleep 300"), text("Unit done.")] }
    })
    .to_string()
}

// ---- waiting ----------------------------------------------------------------------

/// Poll `f` every 100ms until it yields a value or `secs` seconds pass.
async fn poll<T>(secs: u64, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// Process liveness and process-group membership, without procps. The musl CI
// image (Alpine busybox) has neither: its `ps` rejects `-o stat= -p`, and its
// `pgrep` has no `-g` (it prints "unrecognized option: g" and exits 1, which a
// naive reader mistakes for procps's "no match"). So on Linux these read
// `/proc` directly; off Linux (matt's macOS) they shell out to the real tools.

/// Dead for the purposes of a test: gone, or an exited process nothing has
/// reaped yet (a container's PID 1 may never collect one).
fn is_dead(pid: u32) -> bool {
    !pid_is_running(pid) || is_zombie(pid)
}

/// Whether `pid` has exited but not been reaped (state `Z`, or `X` mid-teardown).
#[cfg(target_os = "linux")]
fn is_zombie(pid: u32) -> bool {
    proc_stat_after_comm(pid)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|state| state == 'Z' || state == 'X')
}

/// Off Linux, ask `ps` for the process state.
#[cfg(not(target_os = "linux"))]
fn is_zombie(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| String::from_utf8_lossy(&out.stdout).trim().starts_with('Z'))
}

/// The fields of `/proc/<pid>/stat` after the parenthesised command name:
/// `state ppid pgrp session …`, already left-trimmed. The command name can
/// itself contain `)`, so the split is on the LAST one. `None` when gone.
#[cfg(target_os = "linux")]
fn proc_stat_after_comm(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    Some(rest.trim_start().to_string())
}

/// The LIVE pids currently in process group `pgid`, read from `/proc`. The
/// fields after the command name are `state ppid pgrp …`, so the state is
/// first and the pgrp third. A zombie (`Z`) or dying (`X`) process is excluded:
/// it has exited and will never act, so the group "goes away" once only
/// zombies remain — matching a reaping init's view on macOS, where the killed
/// unit and coordinator are collected rather than left defunct (the musl CI
/// container's PID 1 is `cargo`, which reaps nothing).
#[cfg(target_os = "linux")]
fn pids_in_group(pgid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
        .filter(|&pid| {
            let Some(rest) = proc_stat_after_comm(pid) else {
                return false;
            };
            let mut fields = rest.split_whitespace();
            let live = fields.next().is_some_and(|state| state != "Z" && state != "X");
            let pgrp = fields.nth(1).and_then(|f| f.parse::<u32>().ok());
            live && pgrp == Some(pgid)
        })
        .collect()
}

/// `/proc/<pid>/cmdline` as a space-joined string (argv is NUL-separated).
#[cfg(target_os = "linux")]
fn proc_cmdline(pid: u32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|raw| {
            raw.split(|&b| b == 0)
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// Whether any process is still in group `pgid`. `None` where the probe is
/// unavailable (the check is then skipped, loudly).
#[cfg(target_os = "linux")]
fn group_is_empty(pgid: u32) -> Option<bool> {
    Some(pids_in_group(pgid).is_empty())
}

#[cfg(not(target_os = "linux"))]
fn group_is_empty(pgid: u32) -> Option<bool> {
    let out = std::process::Command::new("pgrep")
        .args(["-g", &pgid.to_string()])
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    }
}

/// Whether a process whose command line matches `pattern` is in group `pgid`.
/// `None` where the probe is unavailable.
#[cfg(target_os = "linux")]
fn group_runs(pgid: u32, pattern: &str) -> Option<bool> {
    Some(
        pids_in_group(pgid)
            .into_iter()
            .any(|pid| proc_cmdline(pid).contains(pattern)),
    )
}

#[cfg(not(target_os = "linux"))]
fn group_runs(pgid: u32, pattern: &str) -> Option<bool> {
    let out = std::process::Command::new("pgrep")
        .args(["-g", &pgid.to_string(), "-f", pattern])
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Wait for group `pgid` to be empty. Passes where the probe cannot tell.
async fn group_goes_away(pgid: u32, secs: u64) -> bool {
    if group_is_empty(pgid).is_none() {
        eprintln!("the process-group probe is unavailable here: skipping the check");
        return true;
    }
    poll(secs, || group_is_empty(pgid).unwrap_or(true).then_some(()))
        .await
        .is_some()
}

// ---- reading what a run left ------------------------------------------------------

fn json_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        // A line the run is writing right now may be torn.
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn events(run_dir: &Path) -> Vec<Value> {
    json_lines(&run_dir.join("events.jsonl"))
}

fn lead_transcript(run_dir: &Path, round: u32) -> Vec<Value> {
    json_lines(&run_dir.join(format!("lead/transcript.r{round}.jsonl")))
}

fn event_of_type<'a>(transcript: &'a [Value], ty: &str) -> Option<&'a Value> {
    transcript.iter().find(|e| e["type"] == ty)
}

fn steering_files(run_dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(run_dir.join("steering"))
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default()
}

/// `rupu --format json agentiflow status <id>`.
fn status_json(fx: &Fixture, script: &str, id: &str) -> Value {
    let out = fx
        .rupu(script, &["--format", "json", "agentiflow", "status", id])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `run acme --detach`: returns the run's id once it is under way, having
/// checked that the parent did not wait on the run.
fn detach(fx: &Fixture, script: &str) -> String {
    let started = Instant::now();
    let out = fx
        .rupu(script, &["agentiflow", "run", "acme", "--detach"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "detach failed: {}\n{}",
        stderr_of(&out),
        stdout_of(&out)
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the parent waited on the run ({:?})",
        started.elapsed()
    );
    let stdout = stdout_of(&out);
    let id = stdout
        .trim_end()
        .strip_prefix("agentiflow acme: run ")
        .and_then(|rest| rest.strip_suffix(" (detached)"))
        .unwrap_or_else(|| panic!("unexpected detach output: {stdout:?}"))
        .to_string();
    assert!(id.starts_with("af_"), "{id}");
    id
}

/// A detached run's live coordinator pid, once it records one. Registered with
/// `cleanup` so the coordinator and its `bash` children never outlive the test.
async fn coordinator_pid(fx: &Fixture, id: &str, cleanup: &mut Cleanup) -> u32 {
    let dir = fx.run_dir(id);
    let record = poll(30, || {
        let r = AgentiflowRecord::read(&dir).ok()?;
        (r.status == "running" && r.runner_pid.is_some()).then_some(r)
    })
    .await
    .unwrap_or_else(|| panic!("no running record with a runner_pid under {dir:?}"));
    let pid = record.runner_pid.unwrap();
    cleanup.groups.push(pid);
    assert!(pid_is_running(pid), "coordinator {pid} is not live");
    pid
}

/// The unit a run's lead dispatched: its id, and the process group it leads,
/// once it is alive and has begun running its (mock) bash tool.
struct LiveUnit {
    id: String,
    pgid: u32,
}

async fn live_unit(fx: &Fixture, run_id: &str, cleanup: &mut Cleanup) -> LiveUnit {
    let units = fx.run_dir(run_id).join("units");
    let unit = poll(60, || {
        for entry in std::fs::read_dir(&units).ok()?.filter_map(Result::ok) {
            let unit_id = entry.file_name().to_string_lossy().into_owned();
            let doc: Value =
                serde_json::from_slice(&std::fs::read(entry.path().join("unit.json")).ok()?)
                    .ok()?;
            let pgid = u32::try_from(doc["pgid"].as_u64()?).ok()?;
            // The unit is really running its script (not merely spawned) once
            // its bash tool's `sleep 300` is in its group. (Its transcript
            // cannot say so: a tool call is flushed after the tool returns.)
            // Without `pgrep`, a live group leader has to do.
            let running = group_runs(pgid, "sleep 300").unwrap_or(true);
            if running && pid_is_running(pgid) {
                return Some(LiveUnit { id: unit_id, pgid });
            }
        }
        None
    })
    .await
    .unwrap_or_else(|| panic!("the lead's unit never came up under {units:?}"));
    cleanup.groups.push(unit.pgid);
    unit
}

// ---- tests ------------------------------------------------------------------------

/// The core loop: a detached run keeps going on its own; a steering message
/// reaches its lead and is consumed; a graceful stop ends it at a round
/// boundary with the operator's reason.
#[tokio::test(flavor = "multi_thread")]
async fn detach_then_send_then_graceful_stop() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(10_000); // never reached: only the operator ends this run
    let script = slow_rounds(1);
    let mut cleanup = Cleanup::default();

    let id = detach(&fx, &script);
    let dir = fx.run_dir(&id);
    let pid = coordinator_pid(&fx, &id, &mut cleanup).await;
    let status = poll(30, || {
        let s = status_json(&fx, &script, &id);
        (s["status"] == "running").then_some(s)
    })
    .await
    .expect("status never read `running`");
    assert_eq!(status["id"], json!(id));

    // ---- send: queued, then taken by the envelope and put in front of the lead
    let send = fx
        .rupu(&script, &["agentiflow", "send", &id, "focus on auth"])
        .output()
        .unwrap();
    assert!(send.status.success(), "{}", stderr_of(&send));
    assert_eq!(
        stdout_of(&send).trim(),
        format!("queued steering for {id}"),
        "{}",
        stderr_of(&send)
    );
    assert!(
        poll(60, || steering_files(&dir).is_empty().then_some(()))
            .await
            .is_some(),
        "the queued steering was never consumed: {:?}",
        steering_files(&dir)
    );
    // Consumed is not delivered: the message has to be in a round's prompt.
    let delivered = poll(60, || {
        (0..32).find_map(|round| {
            let t = lead_transcript(&dir, round);
            let prompt = t.iter().find(|e| {
                e["type"] == "user_message"
                    && e["data"]["content"]
                        .as_str()
                        .is_some_and(|c| c.contains("focus on auth"))
            })?;
            Some((round, prompt["data"]["content"].as_str()?.to_string()))
        })
    })
    .await
    .expect("no lead round was ever told `focus on auth`");
    assert!(
        delivered.1.contains("Operator steering"),
        "the steering was not framed as the operator's: {}",
        delivered.1
    );
    // The round event is written when the round that was told FINISHES.
    assert!(
        poll(60, || events(&dir)
            .iter()
            .any(|e| e["kind"] == "round" && e["steering"] == 1)
            .then_some(()))
        .await
        .is_some(),
        "no round event reports the delivery: {:?}",
        events(&dir)
    );
    assert_eq!(
        status_json(&fx, &script, &id)["status"],
        "running",
        "steering is not a stop"
    );

    // ---- stop (graceful): the run ends itself, saying the operator did it ----
    let stop = fx
        .rupu(&script, &["agentiflow", "stop", &id])
        .output()
        .unwrap();
    assert!(stop.status.success(), "{}", stderr_of(&stop));
    assert_eq!(
        stdout_of(&stop).trim(),
        format!("requested graceful stop of {id}")
    );
    let stopped = poll(60, || {
        let s = status_json(&fx, &script, &id);
        (s["status"] != "running").then_some(s)
    })
    .await
    .expect("the run never stopped after `stop`");
    assert_eq!(stopped["status"], "completed", "{stopped}");
    assert_eq!(stopped["stop_reason"], "operator_stop", "{stopped}");

    let record = fx.record(&id);
    assert_eq!(record.status, "completed");
    assert_eq!(record.stop_reason.as_deref(), Some("operator_stop"));
    assert_eq!(record.runner_pid, None);
    let last = events(&dir).pop().expect("a terminal event");
    assert_eq!(last["kind"], "run_stopped", "{last}");
    assert_eq!(last["stop_reason"], "operator_stop");
    // A graceful stop lets the coordinator exit by itself.
    assert!(
        poll(30, || is_dead(pid).then_some(())).await.is_some(),
        "the coordinator {pid} outlived its own run"
    );
}

/// `stop --now` does not wait for the round: it kills the coordinator and the
/// unit its lead launched, and closes the record and the log itself (the
/// coordinator dies without writing a word).
#[tokio::test(flavor = "multi_thread")]
async fn hard_stop_now_kills_the_coordinator_and_units() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(10_000);
    let script = lead_with_a_unit();
    let mut cleanup = Cleanup::default();

    let id = detach(&fx, &script);
    let dir = fx.run_dir(&id);
    let pid = coordinator_pid(&fx, &id, &mut cleanup).await;
    let unit = live_unit(&fx, &id, &mut cleanup).await;
    assert_ne!(unit.pgid, pid, "the unit shares the coordinator's group");
    assert!(
        group_is_empty(unit.pgid) != Some(true),
        "the unit's group is empty before the stop"
    );

    let stop = fx
        .rupu(&script, &["agentiflow", "stop", &id, "--now"])
        .output()
        .unwrap();
    assert!(stop.status.success(), "{}", stderr_of(&stop));
    assert_eq!(stdout_of(&stop).trim(), format!("hard-stopped {id}"));

    // The record: terminal, and says the operator did it.
    let record = fx.record(&id);
    assert_eq!(record.status, "failed", "{record:?}");
    assert!(
        record
            .stop_reason
            .as_deref()
            .is_some_and(|r| r.starts_with("operator_stop")),
        "{record:?}"
    );
    assert_eq!(record.runner_pid, None);
    assert!(record.ended_at.is_some());
    // The log: the coordinator died without a word, so the stop wrote the
    // terminal line, or the log would end mid-run on a failed run.
    let last = events(&dir).pop().expect("a terminal event");
    assert_eq!(last["kind"], "run_stopped", "{last}");
    assert!(
        last["stop_reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("operator_stop")),
        "{last}"
    );

    // The processes: the coordinator, and everything in the unit's group (the
    // `rupu run` and the `sleep 300` its bash tool started).
    assert!(
        poll(15, || is_dead(pid).then_some(())).await.is_some(),
        "the coordinator {pid} outlived `stop --now`"
    );
    assert!(
        group_goes_away(unit.pgid, 15).await,
        "the unit {} (group {}) outlived `stop --now`",
        unit.id,
        unit.pgid
    );
    // The coordinator led its own group, and the `sleep 120` its lead's bash
    // tool was running is in it: left behind, it would be work nobody can stop.
    assert!(
        group_goes_away(pid, 15).await,
        "what the lead's tool started (group {pid}) outlived `stop --now`"
    );

    // And the run is over for good: the status the operator sees agrees.
    let after = status_json(&fx, &script, &id);
    assert_eq!(after["status"], "failed", "{after}");
}

/// The orphan reaper, through `agentiflow serve`: a coordinator that is
/// SIGKILLed (so no cleanup runs, and its record keeps saying `running`) is
/// found dead by the sweep, its run recorded as failed, and the unit it left
/// behind stopped.
#[tokio::test(flavor = "multi_thread")]
async fn serve_reaps_a_dead_coordinator_and_stops_its_unit() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(10_000);
    let script = lead_with_a_unit();
    let mut cleanup = Cleanup::default();

    let id = detach(&fx, &script);
    let dir = fx.run_dir(&id);
    let pid = coordinator_pid(&fx, &id, &mut cleanup).await;
    let unit = live_unit(&fx, &id, &mut cleanup).await;

    // SIGKILL the coordinator alone: its Drop never runs, so nothing winds the
    // unit down and the record is left `running` with a dead pid.
    let kill = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(kill.success(), "could not SIGKILL the coordinator {pid}");
    assert!(
        poll(30, || is_dead(pid).then_some(())).await.is_some(),
        "the coordinator {pid} survived SIGKILL"
    );
    let record = fx.record(&id);
    assert_eq!(record.status, "running", "nothing may have closed it yet");
    assert!(
        pid_is_running(unit.pgid),
        "the unit died with its coordinator, so there is nothing to reap"
    );

    // `serve` sweeps at startup and then every second.
    std::fs::write(
        fx.global.join("config.toml"),
        "[agentiflow]\nserve_interval_secs = 1\n",
    )
    .unwrap();
    let stderr_path = fx.global.join("serve.stderr");
    let mut serve = fx
        .spawnable("[]", &["agentiflow", "serve"])
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .spawn()
        .unwrap();
    let serve_pid = serve.id();
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        use std::io::BufRead as _;
        let (out, lines) = (serve.stdout.take().unwrap(), lines.clone());
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                lines.lock().unwrap().push(line);
            }
        });
    }
    cleanup.children.push(serve);

    let reaped_line = format!("agentiflow serve: reaped orphaned run {id}");
    let seen = poll(60, || {
        lines.lock().unwrap().contains(&reaped_line).then_some(())
    })
    .await;
    assert!(
        seen.is_some(),
        "`serve` never reported reaping {id}: stdout {:?}, stderr {:?}",
        lines.lock().unwrap(),
        std::fs::read_to_string(&stderr_path).unwrap_or_default()
    );

    // SIGTERM ends `serve` cleanly (it takes the signal itself, so it exits 0).
    let term = std::process::Command::new("kill")
        .args(["-TERM", &serve_pid.to_string()])
        .status()
        .unwrap();
    assert!(term.success());
    let exit = poll(30, || cleanup.children[0].try_wait().unwrap())
        .await
        .expect("`serve` did not exit on SIGTERM");
    assert_eq!(exit.code(), Some(0), "serve exited {exit:?}");
    let startup = lines.lock().unwrap().first().cloned();
    assert_eq!(
        startup.as_deref(),
        Some("agentiflow serve: reaping orphans every 1s"),
        "{:?}",
        lines.lock().unwrap()
    );

    // The record: failed, as an orphan, with the dead pid named.
    let record = fx.record(&id);
    assert_eq!(record.status, "failed", "{record:?}");
    let reason = record.stop_reason.as_deref().unwrap_or_default();
    assert!(reason.starts_with("orphaned:"), "{record:?}");
    assert!(reason.contains(&pid.to_string()), "{reason}");
    assert_eq!(record.runner_pid, None);
    // The log was closed for the dead coordinator.
    let last = events(&dir).pop().expect("a terminal event");
    assert_eq!(last["kind"], "run_stopped", "{last}");
    assert!(
        last["stop_reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("orphaned:")),
        "{last}"
    );
    // The unit the dead coordinator left running is stopped.
    assert!(
        group_goes_away(unit.pgid, 15).await,
        "the orphaned unit {} (group {}) was left running",
        unit.id,
        unit.pgid
    );
    // The unit is dead, and nothing about the run is still being watched.
    assert_eq!(status_json(&fx, "[]", &id)["status"], "failed");
}

/// `attach` follows a run round by round and leaves by itself when the run ends.
#[tokio::test(flavor = "multi_thread")]
async fn attach_follows_until_the_run_ends() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(3);
    let script = slow_rounds(1);
    let mut cleanup = Cleanup::default();

    let id = detach(&fx, &script);
    coordinator_pid(&fx, &id, &mut cleanup).await;

    let started = Instant::now();
    let attach = fx
        .rupu(&script, &["agentiflow", "attach", &id])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    let (stdout, stderr) = (stdout_of(&attach), stderr_of(&attach));
    assert!(
        attach.status.success(),
        "attach failed (it is bounded at 90s: a timeout is a kill): {stderr}\n{stdout}"
    );

    // It ran until the run did, and no longer: three rounds of ~1s.
    let record = fx.record(&id);
    assert_eq!(record.status, "completed", "{record:?}");
    assert_eq!(record.stop_reason.as_deref(), Some("ceiling"));
    assert!(
        elapsed >= Duration::from_millis(1500),
        "attach returned before the run could have ended ({elapsed:?}): {stdout}"
    );

    // What it printed: the event log as it landed, the lead's transcript
    // following the rollover from round to round, and why the run stopped.
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        stdout.contains("[events]  run started: acme"),
        "no events line: {stdout}"
    );
    assert!(
        stdout.contains("[events]  round 0 finished: yielded"),
        "{stdout}"
    );
    let at = |needle: &str| lines.iter().position(|l| l.contains(needle));
    let (r1, r2) = (
        at("[lead r1]").unwrap_or_else(|| panic!("round 1 never followed: {stdout}")),
        at("[lead r2]").unwrap_or_else(|| panic!("round 2 never followed: {stdout}")),
    );
    assert!(r1 < r2, "rounds out of order: {stdout}");
    assert!(
        stdout.contains("[lead r2]  assistant: Round done."),
        "{stdout}"
    );
    assert!(
        stdout.contains("[events]  run stopped: ceiling"),
        "the terminal event was not shown: {stdout}"
    );
    assert_eq!(
        lines.last().copied(),
        Some(format!("{id} completed  (stopped: ceiling)").as_str()),
        "{stdout}"
    );

    // Attaching to a run that has finished shows its log and returns at once.
    let again_started = Instant::now();
    let again = fx
        .rupu(&script, &["agentiflow", "attach", &id])
        .output()
        .unwrap();
    assert!(again.status.success(), "{}", stderr_of(&again));
    assert!(
        again_started.elapsed() < Duration::from_secs(20),
        "attach to a finished run waited"
    );
    let again = stdout_of(&again);
    assert!(again.contains("[events]  run stopped: ceiling"), "{again}");
    assert!(
        again.contains("[lead r2]"),
        "the last round is shown: {again}"
    );
    assert!(
        !again.contains("[lead r1]"),
        "only the current round: {again}"
    );
}

/// `attach --no-follow` (`--once`) looks once and returns while the run is still
/// going; an id nothing matches is refused the way `status` / `stop` refuse it.
#[tokio::test(flavor = "multi_thread")]
async fn attach_once_returns_at_once_on_a_running_run() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(10_000);
    let script = slow_rounds(2);
    let mut cleanup = Cleanup::default();

    let id = detach(&fx, &script);
    coordinator_pid(&fx, &id, &mut cleanup).await;
    let dir = fx.run_dir(&id);
    // Something to show: round 0 has begun.
    poll(60, || {
        dir.join("lead/transcript.r0.jsonl").is_file().then_some(())
    })
    .await
    .expect("round 0 never started");

    for flag in ["--no-follow", "--once"] {
        let started = Instant::now();
        let out = fx
            .rupu(&script, &["agentiflow", "attach", &id, flag])
            .output()
            .unwrap();
        assert!(out.status.success(), "{flag}: {}", stderr_of(&out));
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{flag} followed the run"
        );
        let stdout = stdout_of(&out);
        assert!(
            stdout.contains("[events]  run started: acme"),
            "{flag}: {stdout}"
        );
        assert_eq!(
            stdout.lines().last(),
            Some(format!("{id} running").as_str()),
            "{flag}: {stdout}"
        );
    }
    // Looking did not disturb the run.
    assert_eq!(fx.record(&id).status, "running");

    let missing = fx
        .rupu(&script, &["agentiflow", "attach", "af_NOSUCHRUN"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(
        stderr_of(&missing).contains("no agentiflow run matches 'af_NOSUCHRUN'"),
        "{}",
        stderr_of(&missing)
    );

    // End the run (the cleanup guard would, too: this is the operator's way).
    let stop = fx
        .rupu(&script, &["agentiflow", "stop", &id, "--now"])
        .output()
        .unwrap();
    assert!(stop.status.success(), "{}", stderr_of(&stop));
}

/// `send --now` does not wait for the round boundary to be heard: it cuts the
/// lead's round short, and the next round's prompt carries the message.
#[tokio::test(flavor = "multi_thread")]
async fn send_now_interrupts_the_current_round() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(2);
    // Round 0 holds open (the marker is how the lead knows it is round 0);
    // round 1 finds the marker and is quick.
    let marker = fx.project.join("round0-ran");
    let script = json!([
        bash(
            "call_hold",
            &format!(
                "if [ ! -e {m} ]; then touch {m}; sleep 5; fi",
                m = marker.display()
            )
        ),
        text("Round done.")
    ])
    .to_string();
    let mut cleanup = Cleanup::default();

    let id = detach(&fx, &script);
    coordinator_pid(&fx, &id, &mut cleanup).await;
    let dir = fx.run_dir(&id);

    // Round 0 is mid-tool once its command has touched the marker (and is in
    // its `sleep 5`). The transcript cannot say so: a tool call is written
    // after the tool returns.
    poll(60, || marker.exists().then_some(()))
        .await
        .expect("round 0's command never started");
    assert!(
        event_of_type(&lead_transcript(&dir, 0), "run_complete").is_none(),
        "round 0 was already over"
    );

    let send = fx
        .rupu(
            &script,
            &["agentiflow", "send", &id, "stop poking auth", "--now"],
        )
        .output()
        .unwrap();
    assert!(send.status.success(), "{}", stderr_of(&send));
    assert_eq!(stdout_of(&send).trim(), format!("queued steering for {id}"));

    // The run carries on to its ceiling: round 1, then done.
    let finished = poll(90, || {
        let r = AgentiflowRecord::read(&dir).ok()?;
        (r.status != "running").then_some(r)
    })
    .await
    .expect("the run never finished");
    assert_eq!(finished.status, "completed", "{finished:?}");
    assert_eq!(finished.stop_reason.as_deref(), Some("ceiling"));
    assert_eq!(finished.rounds, 2);

    // Round 0 was CUT, not finished: the runner paused it at its safe boundary
    // (after the tool, before the next provider request), so its second turn
    // ("Round done.") never happened.
    let r0 = lead_transcript(&dir, 0);
    let complete = event_of_type(&r0, "run_complete").expect("round 0 never completed");
    assert_eq!(complete["data"]["status"], "aborted", "{complete}");
    assert_eq!(complete["data"]["error"], "paused", "{complete}");
    assert!(
        event_of_type(&r0, "tool_result").is_some(),
        "the tool was not allowed to finish: {r0:?}"
    );
    assert!(
        !r0.iter().any(|e| e["data"]["content"]
            .as_str()
            .is_some_and(|c| c.contains("Round done."))),
        "round 0 ran past the interrupt: {r0:?}"
    );

    // The message was delivered at the next boundary: round 1's prompt has it,
    // and the queue is empty.
    let r1 = lead_transcript(&dir, 1);
    let prompt = event_of_type(&r1, "user_message").expect("round 1 has no prompt");
    let prompt = prompt["data"]["content"].as_str().unwrap();
    assert!(prompt.contains("Operator steering"), "{prompt}");
    assert!(prompt.contains("stop poking auth"), "{prompt}");
    assert!(steering_files(&dir).is_empty());
    // Round 1, uninterrupted, ran to its own end.
    let complete = event_of_type(&r1, "run_complete").expect("round 1 never completed");
    assert_eq!(complete["data"]["status"], "ok", "{complete}");
    assert!(
        r1.iter().any(|e| e["data"]["content"]
            .as_str()
            .is_some_and(|c| c.contains("Round done."))),
        "{r1:?}"
    );

    // The log tells the same story: round 0 yielded with nothing delivered,
    // round 1 was told once.
    let rounds: Vec<Value> = events(&dir)
        .into_iter()
        .filter(|e| e["kind"] == "round")
        .collect();
    assert_eq!(rounds.len(), 2, "{rounds:?}");
    assert_eq!(rounds[0]["outcome"], "yielded");
    assert_eq!(rounds[0]["steering"], 0);
    assert_eq!(rounds[1]["steering"], 1);
}
