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
