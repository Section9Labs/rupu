//! `rupu agentiflow run` → `list` → `status`, end to end, with the mock
//! provider.
//!
//! The launch path hands `run_agentiflow` two provider closures built in
//! `cmd/agentiflow.rs`, and each runs in a different context:
//!
//! * `make_provider` (the lead, one provider per round) runs on
//!   `run_agentiflow`'s plain worker thread and owns a dedicated runtime that
//!   it `block_on`s;
//! * `generation.factory` (a provider for each `generate_workflow` call) runs
//!   INSIDE the lead driver's current-thread runtime, where `block_on` and
//!   `block_in_place` panic, so it must be synchronous.
//!
//! A wrong closure only shows up when the real launch calls it, so these tests
//! run the real `rupu` binary (`RUPU_MOCK_PROVIDER_SCRIPT` replaces every
//! provider the factory builds) and look at what the run left on disk. The
//! lead never dispatches a unit: a unit is a `rupu run` child process, which a
//! hermetic test has no business starting.

use crate::ENV_LOCK;
use assert_cmd::Command as AssertCommand;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// One lead round: a text reply, so the round yields.
const LEAD_ROUND: &str = r#"
[
  { "AssistantText": { "text": "Round done.", "stop": "end_turn" } }
]
"#;

/// The lead's definition. The one goal ("record a finding") is never met,
/// because the lead records nothing, so the run is stopped by its ceiling.
fn def_yaml(ceiling_rounds: u32) -> String {
    format!(
        "name: acme\n\
         description: A tiny agentiflow for the end-to-end test.\n\
         lead: lead\n\
         engagement_profiles: [network]\n\
         goals:\n  \
           - id: any-finding\n    \
             objective: \"Record at least one finding.\"\n    \
             target: {{ findings: {{}}, count_gte: 1 }}\n\
         scope: {{ authorized: true }}\n\
         pool: {{ agents: [lead] }}\n\
         round: {{ lead_max_turns: 4, ceiling: {{ rounds: {ceiling_rounds} }} }}\n"
    )
}

/// An isolated rupu home with `acme` and its lead agent installed, and a
/// project directory (no `.rupu/`) to run from.
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
    // The lead's model is not the generation model
    // (`claude-sonnet-4-6`, the first of `DEFAULT_GEN_MODELS`), so a mock
    // script can answer each differently.
    std::fs::write(
        global.join("agents/lead.md"),
        "---\nname: lead\nprovider: anthropic\nmodel: claude-lead-mock\n---\nYou are the lead.\n",
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

impl Fixture {
    /// `rupu --format json <args>`, hermetic: this fixture's home and project,
    /// no ambient provider credentials, and no stdin.
    fn rupu(&self, mock_script: &str, args: &[&str]) -> AssertCommand {
        let mut cmd = AssertCommand::cargo_bin("rupu").unwrap();
        cmd.env("RUPU_HOME", &self.global)
            .env("RUPU_MOCK_PROVIDER_SCRIPT", mock_script)
            .current_dir(&self.project)
            .write_stdin("");
        for var in [
            "RUPU_AUTH_FILE",
            "RUPU_ANTHROPIC_API_KEY",
            "RUPU_OPENAI_API_KEY",
            "RUPU_GEMINI_API_KEY",
            "RUPU_COPILOT_API_KEY",
        ] {
            cmd.env_remove(var);
        }
        cmd.args(args);
        cmd
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        self.global.join("agentiflows").join(id)
    }
}

/// stdout of a command that must have succeeded, parsed as the JSON report it
/// prints; the failure message carries both streams.
fn json_stdout(cmd: &mut AssertCommand) -> Value {
    let out = cmd.output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(out.status.success(), "command failed: {stderr}\n{stdout}");
    serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}): {stdout}\n{stderr}"))
}

fn events(run_dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(run_dir.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn kinds(evs: &[Value]) -> Vec<&str> {
    evs.iter().map(|e| e["kind"].as_str().unwrap()).collect()
}

/// The primary guarantee: a real launch builds the lead's provider through
/// `make_provider` on `run_agentiflow`'s worker thread, runs every round, and
/// leaves a run that `list` and `status` report.
#[tokio::test(flavor = "multi_thread")]
async fn run_then_list_then_status_with_a_mock_provider() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(2);

    // ---- run ---------------------------------------------------------------
    let run = json_stdout(&mut fx.rupu(
        LEAD_ROUND,
        &["--format", "json", "agentiflow", "run", "acme"],
    ));
    assert_eq!(run["kind"], "agentiflow_run");
    assert_eq!(run["name"], "acme");
    assert_eq!(run["stop_reason"], "ceiling", "{run}");
    assert_eq!(run["rounds"], 2, "{run}");
    assert_eq!(run["goals"][0]["id"], "any-finding");
    assert_eq!(run["goals"][0]["met"], false);
    let id = run["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("af_"), "{id}");

    // ---- what the run left on disk ------------------------------------------
    let dir = fx.run_dir(&id);
    for file in [
        "agentiflow.json",
        "agentiflow.yaml",
        "events.jsonl",
        "lead/transcript.r0.jsonl",
        "lead/transcript.r1.jsonl",
    ] {
        assert!(dir.join(file).is_file(), "{file} missing under {dir:?}");
    }
    assert!(!dir.join("lead/transcript.r2.jsonl").exists());

    // The record: completed, stopped by the ceiling, two rounds.
    let record = rupu_agentiflow::AgentiflowRecord::read(&dir).unwrap();
    assert_eq!(record.id, id);
    assert_eq!(record.status, "completed");
    assert_eq!(record.stop_reason.as_deref(), Some("ceiling"));
    assert_eq!(record.rounds, 2);
    assert_eq!(record.goals.len(), 1);
    assert!(!record.goals[0].met);
    assert!(record.ended_at.is_some());

    // The snapshot is the definition the run started from.
    let snapshot = std::fs::read_to_string(dir.join("agentiflow.yaml")).unwrap();
    let snapshot = rupu_agentiflow::AgentiflowDef::parse_str(&snapshot).unwrap();
    assert_eq!(snapshot.name, "acme");
    assert_eq!(snapshot.lead, "lead");
    assert_eq!(snapshot.goals[0].id, "any-finding");

    // Every round's provider built and answered: a round whose `make_provider`
    // could not build would have ended in `error` (or taken the run down).
    let evs = events(&dir);
    assert_eq!(
        kinds(&evs),
        ["run_started", "round", "round", "run_stopped"],
        "{evs:?}"
    );
    assert_eq!(evs[1]["outcome"], "yielded", "{evs:?}");
    assert_eq!(evs[2]["outcome"], "yielded", "{evs:?}");
    assert_eq!(evs[3]["stop_reason"], "ceiling");
    let transcript = std::fs::read_to_string(dir.join("lead/transcript.r0.jsonl")).unwrap();
    assert!(
        transcript.contains("Round done."),
        "the mock lead's reply is not in its transcript: {transcript}"
    );

    // The definition file sits beside the run directory in `agentiflows/`,
    // and is not a run.
    assert!(fx.global.join("agentiflows/acme.yaml").is_file());

    // ---- list ---------------------------------------------------------------
    let list = json_stdout(&mut fx.rupu(LEAD_ROUND, &["--format", "json", "agentiflow", "list"]));
    assert_eq!(list["kind"], "agentiflow_list");
    let rows = list["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "exactly the one run is listed: {list}");
    assert_eq!(rows[0]["id"], json!(id));
    assert_eq!(rows[0]["name"], "acme");
    assert_eq!(rows[0]["status"], "completed");
    assert_eq!(rows[0]["stop_reason"], "ceiling");
    assert_eq!(rows[0]["rounds"], 2);
    assert_eq!(rows[0]["goals_met"], 0);
    assert_eq!(rows[0]["goals_total"], 1);

    // ---- status -------------------------------------------------------------
    let status = json_stdout(&mut fx.rupu(
        LEAD_ROUND,
        &["--format", "json", "agentiflow", "status", &id],
    ));
    assert_eq!(status["kind"], "agentiflow_status");
    assert_eq!(status["id"], json!(id));
    assert_eq!(status["name"], "acme");
    assert_eq!(status["status"], "completed");
    assert_eq!(status["stop_reason"], "ceiling");
    assert_eq!(status["rounds"], 2);
    assert_eq!(status["engagement_profiles"], json!(["network"]));
    assert_eq!(status["goals"][0]["id"], "any-finding");
    assert_eq!(status["goals"][0]["met"], false);
    assert_eq!(
        status["goals"][0]["objective"],
        "Record at least one finding."
    );
    assert_eq!(status["goals"][0]["predicate"], "findings, count >= 1");
    assert_eq!(status["budget"]["state"], "ok", "{status}");

    // The human form, addressed by a unique suffix of the id.
    let suffix = &id[id.len() - 8..];
    let human = fx
        .rupu(LEAD_ROUND, &["agentiflow", "status", suffix])
        .output()
        .unwrap();
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains(&id), "{human}");
    assert!(human.contains("(stopped: ceiling)"), "{human}");
    assert!(human.contains("goals: 0/1 met"), "{human}");
}

/// The bridge's safety net: `generation.factory` runs inside the lead's
/// current-thread runtime, where a closure that `block_on`s (or blocks in
/// place) PANICS. An API key makes generation available (the mock seam answers
/// before any OAuth check), the scripted lead calls `generate_workflow`, and
/// the tool builds its provider through the factory. That provider is scripted
/// to produce text that is not a workflow, so the tool fails closed after its
/// repair attempts, never reaching a unit launch (which would start a child
/// process). A result the lead can read is the proof the factory returned a
/// working provider rather than panicking.
#[tokio::test(flavor = "multi_thread")]
async fn generate_workflow_builds_its_provider_inside_the_lead_runtime() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(1);

    // `claude-sonnet-4-6` is the generating model (first of
    // `DEFAULT_GEN_MODELS`, and the API key below authenticates anthropic);
    // every other build (the lead's) replays `turns`.
    let not_a_workflow =
        json!({ "AssistantText": { "text": "this is not a workflow", "stop": "end_turn" } });
    let script = json!({
        "turns": [
            { "AssistantToolUse": {
                "text": null,
                "tool_id": "call_gen",
                "tool_name": "generate_workflow",
                "tool_input": { "description": "list the files in the workspace" },
                "stop": "tool_use"
            } },
            { "AssistantText": { "text": "Round done.", "stop": "end_turn" } }
        ],
        "models": {
            "claude-sonnet-4-6": [not_a_workflow, not_a_workflow, not_a_workflow]
        }
    })
    .to_string();

    let mut cmd = fx.rupu(&script, &["--format", "json", "agentiflow", "run", "acme"]);
    cmd.env("RUPU_ANTHROPIC_API_KEY", "sk-test");
    let out = cmd.output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(out.status.success(), "run failed: {stderr}\n{stdout}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    let run: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(run["stop_reason"], "ceiling", "{run}");
    assert_eq!(run["rounds"], 1, "{run}");
    let id = run["id"].as_str().unwrap();
    let dir = fx.run_dir(id);

    // The tool was offered (an unknown tool would read "unknown tool"), ran its
    // factory without panicking, drove the provider the factory returned
    // through all three attempts, and reported the failure to the lead.
    let transcript = std::fs::read_to_string(dir.join("lead/transcript.r0.jsonl")).unwrap();
    assert!(
        transcript.contains("workflow generation failed")
            && transcript.contains("did not parse after 3 attempt(s)"),
        "generate_workflow did not reach its generator: {transcript}"
    );
    // Nothing was authored or launched.
    assert!(!dir.join("generated").exists());
    let evs = events(&dir);
    assert_eq!(evs[1]["outcome"], "yielded", "{evs:?}");
}

// ---- `run --detach` -----------------------------------------------------------

/// Poll `f` every 100ms until it yields a value or `secs` seconds pass.
async fn poll<T>(secs: u64, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// One `ps -o <field>= -p <pid>` value, or `None` when `ps` is unavailable or
/// the process is gone.
fn ps_field(pid: u32, field: &str) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !value.is_empty()).then_some(value)
}

/// SIGKILLs a process group on drop, so a failing assertion cannot leave the
/// detached coordinator (and the `sleep` its bash tool started) running.
struct GroupGuard(Option<u32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.0 {
            rupu_agentiflow::kill_group(pgid);
        }
    }
}

/// `run --detach` hands the run to a process of its own: the command returns at
/// once with the run id, and the run keeps going as a separate process-group
/// leader that `agentiflow stop` can find through the record's `runner_pid`.
///
/// The lead's one round runs `sleep 20` through its bash tool, so the run is
/// `running` for far longer than the test needs; a parent that waited on its
/// child would not return inside the bound below.
#[tokio::test(flavor = "multi_thread")]
async fn run_detach_returns_at_once_and_leaves_a_live_coordinator_in_its_own_group() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(1);
    std::fs::write(
        fx.global.join("agents/lead.md"),
        "---\nname: lead\nprovider: anthropic\nmodel: claude-lead-mock\ntools: [bash]\n---\n\
         You are the lead.\n",
    )
    .unwrap();
    let hold = json!([
        { "AssistantToolUse": {
            "text": null,
            "tool_id": "call_hold",
            "tool_name": "bash",
            "tool_input": { "command": "sleep 20" },
            "stop": "tool_use"
        } },
        { "AssistantText": { "text": "Round done.", "stop": "end_turn" } }
    ])
    .to_string();
    let mut group = GroupGuard(None);

    // ---- the parent returns at once, with the id on stdout -------------------
    let started = std::time::Instant::now();
    let out = fx
        .rupu(&hold, &["agentiflow", "run", "acme", "--detach"])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(out.status.success(), "detach failed: {stderr}\n{stdout}");
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the parent waited on the run ({elapsed:?}): {stdout}"
    );
    let id = stdout
        .trim_end()
        .strip_prefix("agentiflow acme: run ")
        .and_then(|rest| rest.strip_suffix(" (detached)"))
        .unwrap_or_else(|| panic!("unexpected detach output: {stdout:?}"))
        .to_string();
    assert!(id.starts_with("af_"), "{id}");
    assert_eq!(stdout.lines().count(), 1, "{stdout}");

    // ---- the child records under that id, as a live coordinator --------------
    let dir = fx.run_dir(&id);
    let record = poll(30, || {
        let r = rupu_agentiflow::AgentiflowRecord::read(&dir).ok()?;
        (r.status == "running" && r.runner_pid.is_some()).then_some(r)
    })
    .await
    .unwrap_or_else(|| panic!("no running record with a runner_pid under {dir:?}"));
    assert_eq!(record.id, id);
    let pid = record.runner_pid.unwrap();
    group.0 = Some(pid);
    assert!(
        rupu_agentiflow::pid_is_running(pid),
        "pid {pid} is not live"
    );
    assert_ne!(
        pid,
        std::process::id(),
        "the run executed in the test's own process"
    );
    // The parent already exited, so the coordinator is another process, and a
    // group leader (`process_group(0)`): its pgid is its pid.
    match ps_field(pid, "pgid") {
        Some(pgid) => assert_eq!(
            pgid,
            pid.to_string(),
            "the child is not its own group leader"
        ),
        None => eprintln!("`ps` is unavailable here: skipping the process-group check"),
    }

    let status =
        json_stdout(&mut fx.rupu(&hold, &["--format", "json", "agentiflow", "status", &id]));
    assert_eq!(status["status"], "running", "{status}");

    // ---- a hard stop reaches the detached process -----------------------------
    let stop = fx
        .rupu(&hold, &["agentiflow", "stop", &id, "--now"])
        .output()
        .unwrap();
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    assert!(
        String::from_utf8_lossy(&stop.stdout).contains(&format!("hard-stopped {id}")),
        "{}",
        String::from_utf8_lossy(&stop.stdout)
    );
    let record = rupu_agentiflow::AgentiflowRecord::read(&dir).unwrap();
    assert_eq!(record.status, "failed");
    assert_eq!(record.stop_reason.as_deref(), Some("operator_stop:now"));

    // The coordinator is gone (a zombie that nothing has reaped yet counts: it
    // is dead, and a container's PID 1 may never collect it).
    let gone = poll(15, || {
        let dead = !rupu_agentiflow::pid_is_running(pid)
            || ps_field(pid, "stat").is_some_and(|s| s.starts_with('Z'));
        dead.then_some(())
    })
    .await;
    assert!(
        gone.is_some(),
        "the detached coordinator {pid} outlived `stop --now`"
    );
}

/// `--format json` keeps stdout one JSON document for a detached run too, and
/// the child records under the id the parent printed.
#[tokio::test(flavor = "multi_thread")]
async fn run_detach_with_json_prints_one_document() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(1);

    let out = json_stdout(&mut fx.rupu(
        LEAD_ROUND,
        &["--format", "json", "agentiflow", "run", "acme", "--detach"],
    ));
    assert_eq!(out["kind"], "agentiflow_run_detached", "{out}");
    assert_eq!(out["name"], "acme");
    let id = out["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("af_"), "{id}");
    assert_eq!(out["run_dir"], json!(fx.run_dir(&id).display().to_string()));

    // The child runs the (instant) mock round to its ceiling under that id; it
    // finishes on its own, so nothing outlives the test.
    let dir = fx.run_dir(&id);
    let record = poll(30, || {
        let r = rupu_agentiflow::AgentiflowRecord::read(&dir).ok()?;
        (r.status != "running").then_some(r)
    })
    .await
    .unwrap_or_else(|| panic!("the detached run never finished under {dir:?}"));
    assert_eq!(record.id, id);
    assert_eq!(record.status, "completed", "{record:?}");
    assert_eq!(record.stop_reason.as_deref(), Some("ceiling"));
}

/// The run directories under a fixture's `agentiflows/` (definition files sit
/// beside them and are not runs).
fn run_dirs(fx: &Fixture) -> Vec<String> {
    std::fs::read_dir(fx.global.join("agentiflows"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("af_"))
        .collect()
}

/// A definition that does not load is refused by the PARENT, before anything is
/// spawned.
#[tokio::test(flavor = "multi_thread")]
async fn run_detach_refuses_a_missing_definition_up_front() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(1);
    let out = fx
        .rupu(LEAD_ROUND, &["agentiflow", "run", "nope", "--detach"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("agentiflow `nope` not found"), "{stderr}");
    assert_eq!(
        run_dirs(&fx),
        Vec::<String>::new(),
        "a refused detach left a run behind"
    );
}

/// A detached run that cannot start must not look like one that did. With no
/// provider script and no credential, the lead's provider cannot be built, which
/// the CHILD finds out (the parent only checks what it can before spawning).
/// The parent has to learn that from the child and print its error, exit
/// non-zero, and leave no run directory behind.
#[tokio::test(flavor = "multi_thread")]
async fn detach_with_no_credential_fails_loudly() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(1);

    let mut cmd = fx.rupu(LEAD_ROUND, &["agentiflow", "run", "acme", "--detach"]);
    // No mock provider: the real factory runs, and there is no credential.
    cmd.env_remove("RUPU_MOCK_PROVIDER_SCRIPT");
    let out = cmd.output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(
        !out.status.success(),
        "a failed start exited 0: {stdout}\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "an id was printed for a run that never started: {stdout}"
    );
    assert!(
        stderr.contains("the detached agentiflow exited"),
        "the parent did not report the failed start: {stderr}"
    );
    assert!(
        stderr.contains("build the lead's provider `anthropic`"),
        "the child's own error is missing: {stderr}"
    );
    assert_eq!(
        run_dirs(&fx),
        Vec::<String>::new(),
        "a failed start left a run directory behind"
    );
}

/// The same for a definition `AgentiflowDef::validate` accepts but
/// `run_agentiflow` refuses once it starts (`round.ceiling.wall_clock` is
/// parsed there): the child exits before it writes a record.
#[tokio::test(flavor = "multi_thread")]
async fn detach_with_bad_wall_clock_fails_loudly() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture(1);
    std::fs::write(
        fx.global.join("agentiflows/acme.yaml"),
        def_yaml(1).replace("rounds: 1", "wall_clock: \"six hours\""),
    )
    .unwrap();

    let out = fx
        .rupu(LEAD_ROUND, &["agentiflow", "run", "acme", "--detach"])
        .output()
        .unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(
        !out.status.success(),
        "a failed start exited 0: {stdout}\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "an id was printed for a run that never started: {stdout}"
    );
    assert!(stderr.contains("wall_clock"), "{stderr}");
    assert_eq!(
        run_dirs(&fx),
        Vec::<String>::new(),
        "a failed start left a run directory behind"
    );
}
