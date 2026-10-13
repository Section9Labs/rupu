//! W7's cross-process acceptance tests (spec §6): a parent `rupu run`
//! dispatches `agent` and `workflow` children, which are real `rupu run` /
//! `rupu workflow run` processes of this binary (`CARGO_BIN_EXE_rupu`).
//! Each process reads the mock script afresh; the parent and its children
//! run on different models so the script can give each its own turns.

use crate::ENV_LOCK;
use assert_fs::prelude::*;
use rupu_launch::ledger;
use rupu_tools::ChildState;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const PARENT_MODEL: &str = "claude-sonnet-4-6";
const CHILD_MODEL: &str = "claude-haiku-4-5";

fn tool(id: &str, name: &str, input: &str) -> String {
    format!(
        r#"{{ "AssistantToolUse": {{ "text": null, "tool_id": "{id}", "tool_name": "{name}", "tool_input": {input}, "stop": "tool_use" }} }}"#
    )
}

fn text(t: &str) -> String {
    format!(r#"{{ "AssistantText": {{ "text": "{t}", "stop": "end_turn" }} }}"#)
}

/// One script for every process: the parent's turns on [`PARENT_MODEL`],
/// each child's on [`CHILD_MODEL`].
fn script(parent: &[String], child: &[String]) -> String {
    format!(
        r#"{{ "turns": [], "models": {{ "{PARENT_MODEL}": [{}], "{CHILD_MODEL}": [{}] }} }}"#,
        parent.join(","),
        child.join(",")
    )
}

struct Home {
    _tmp: assert_fs::TempDir,
    global: PathBuf,
    project: PathBuf,
}

impl Home {
    fn new() -> Self {
        let tmp = assert_fs::TempDir::new().unwrap();
        let global = tmp.child(".rupu");
        global.create_dir_all().unwrap();
        let project = tmp.child("proj");
        project.create_dir_all().unwrap();
        Self {
            global: global.path().to_path_buf(),
            project: project.path().to_path_buf(),
            _tmp: tmp,
        }
    }

    fn agent(&self, name: &str, model: &str, extra: &str) {
        let dir = self.global.join("agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.md")),
            format!("---\nname: {name}\nprovider: anthropic\nmodel: {model}\nmaxTurns: 6\n{extra}---\nyou are {name}.\n"),
        )
        .unwrap();
    }

    fn workflow(&self, name: &str, yaml: &str) {
        let dir = self.global.join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.yaml")), yaml).unwrap();
    }

    /// `rupu run <args…>` to its end; returns its stderr.
    fn rupu_run(&self, script: &str, args: &[&str]) -> String {
        let stderr = self.global.join("parent.stderr");
        let mut child = Command::new(env!("CARGO_BIN_EXE_rupu"))
            .arg("run")
            .args(args)
            .env("RUPU_HOME", &self.global)
            .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
            .current_dir(&self.project)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .expect("spawn rupu");
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                let err = std::fs::read_to_string(&stderr).unwrap_or_default();
                assert!(status.success(), "{status}: {err}");
                return err;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "rupu run did not finish; stderr: {}",
                    std::fs::read_to_string(&stderr).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// `rupu run <args…>`, started and left running.
    fn spawn_run(&self, script: &str, args: &[&str]) -> std::process::Child {
        Command::new(env!("CARGO_BIN_EXE_rupu"))
            .arg("run")
            .args(args)
            .env("RUPU_HOME", &self.global)
            .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
            .current_dir(&self.project)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn rupu")
    }

    /// The first child in `run_id`'s ledger once it has a pid.
    fn wait_for_child(&self, run_id: &str) -> ledger::ChildEntry {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(c) = self
                .children(run_id)
                .into_iter()
                .find(|c| c.started.pid.is_some())
            {
                return c;
            }
            assert!(Instant::now() < deadline, "{run_id} never started a child");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn children(&self, run_id: &str) -> Vec<ledger::ChildEntry> {
        ledger::read(&self.global.join("runs").join(run_id))
    }

    fn transcript(&self, run_id: &str) -> PathBuf {
        self.global
            .join("transcripts")
            .join(format!("{run_id}.jsonl"))
    }
}

fn events(path: &Path) -> Vec<rupu_transcript::Event> {
    rupu_transcript::JsonlReader::iter(path)
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

fn tool_results(path: &Path) -> Vec<(String, Option<String>)> {
    events(path)
        .into_iter()
        .filter_map(|e| match e {
            rupu_transcript::Event::ToolResult { output, error, .. } => Some((output, error)),
            _ => None,
        })
        .collect()
}

/// §6.1 across processes: an `agent` and a `workflow` child run as their own
/// processes, `dispatch {wait: true}` returns each one's answer, both are in
/// the parent's ledger as done, and each child's model calls are in its own
/// usage ledger and copied to the root's as rollup rows. The workflow child's
/// run is named by the catalog id it was dispatched as, not its file's
/// `name:`.
#[test]
fn process_kinds_dispatch_join_and_charge_the_root() {
    let _env = ENV_LOCK.blocking_lock();
    let home = Home::new();
    home.agent(
        "parent",
        PARENT_MODEL,
        "tools: [dispatch, join]\ndispatchableAgents: [child]\ndispatchableWorkflows: [sweep]\n",
    );
    home.agent("child", CHILD_MODEL, "");
    home.workflow(
        "sweep",
        "name: not-the-catalog-id\nsteps:\n  - id: scan\n    agent: child\n    prompt: scan it\n",
    );
    let run_id = "run_01W7KINDS000000000000000";
    let script = script(
        &[
            tool(
                "c1",
                "dispatch",
                r#"{"kind": "agent", "name": "child", "prompt": "find it", "wait": true, "timeout_secs": 150}"#,
            ),
            tool(
                "c2",
                "dispatch",
                r#"{"kind": "workflow", "name": "sweep", "wait": true, "timeout_secs": 150}"#,
            ),
            text("parent done"),
        ],
        &[text("the child answer")],
    );
    home.rupu_run(
        &script,
        &["parent", "--mode", "bypass", "--run-id", run_id, "go"],
    );

    let children = home.children(run_id);
    let kinds: Vec<_> = children
        .iter()
        .map(|c| (c.started.kind, c.state(), c.started.depth))
        .collect();
    assert_eq!(
        kinds,
        [
            (rupu_tools::ChildKind::Agent, ChildState::Done, 1),
            (rupu_tools::ChildKind::Workflow, ChildState::Done, 1),
        ]
    );
    // The workflow child runs under the codename its parent minted.
    let minted = children[1]
        .started
        .codename
        .clone()
        .expect("the parent minted one");
    let record: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            home.global
                .join("runs")
                .join(&children[1].started.run_id)
                .join("run.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(record["codename"].as_str(), Some(minted.as_str()));
    assert_eq!(record["workflow_name"].as_str(), Some("sweep"));
    let results = tool_results(&home.transcript(run_id));
    for (out, err) in &results[..2] {
        assert_eq!(err, &None, "{out}");
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        assert_eq!(v["result"]["status"], "done", "{v}");
        assert_eq!(v["result"]["output"], "the child answer", "{v}");
    }

    let ledger = std::fs::read_to_string(home.global.join("runs").join(run_id).join("usage.jsonl"))
        .expect("the root ledger");
    let rows: Vec<rupu_runtime::usage_ledger::LedgerRow> = ledger
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let child_rows: Vec<_> = rows.iter().filter(|r| r.agent == "child").collect();
    assert!(
        child_rows.len() >= 2,
        "the agent child's and the workflow child's calls: {rows:?}"
    );
    for r in &child_rows {
        assert!(
            r.rollup,
            "a child's row in its root's ledger is a rollup copy: {r:?}"
        );
        assert_eq!(r.step_id, None, "{r:?}");
    }
    assert!(
        rows.iter()
            .filter(|r| r.agent == "parent")
            .all(|r| !r.rollup),
        "the root's own calls: {rows:?}"
    );
    // Each child keeps its own ledger, which reports read.
    let own = |run: &str| -> Vec<rupu_runtime::usage_ledger::LedgerRow> {
        std::fs::read_to_string(home.global.join("runs").join(run).join("usage.jsonl"))
            .unwrap_or_else(|e| panic!("{run}'s own ledger: {e}"))
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let agent_rows = own(&children[0].started.run_id);
    assert_eq!(agent_rows.len(), 1, "{agent_rows:?}");
    assert!(!agent_rows[0].rollup);
    assert_eq!(agent_rows[0].parent_agent_run_id.as_deref(), Some(run_id));
    let workflow_rows = own(&children[1].started.run_id);
    assert_eq!(workflow_rows.len(), 1, "{workflow_rows:?}");
    assert!(!workflow_rows[0].rollup);
    assert_eq!(workflow_rows[0].step_id.as_deref(), Some("scan"));
    // The parent's ledger records each child's spend, read from its ledger.
    for (c, rows) in children.iter().zip([&agent_rows, &workflow_rows]) {
        let want = rupu_tools::ChildTokens {
            input: rows[0].input_tokens,
            output: rows[0].output_tokens,
        };
        assert_eq!(c.finished.as_ref().unwrap().tokens, Some(want), "{c:?}");
    }
}

/// §6.3: a run launched at depth 4 dispatches an `agent` child; that child
/// is at depth 5 and its own dispatch is refused (L2).
#[test]
fn depth_across_processes() {
    let _env = ENV_LOCK.blocking_lock();
    let home = Home::new();
    home.agent(
        "parent",
        PARENT_MODEL,
        "tools: [dispatch]\ndispatchableAgents: [child]\n",
    );
    home.agent(
        "child",
        CHILD_MODEL,
        "tools: [dispatch]\ndispatchableAgents: [child]\n",
    );
    let run_id = "run_01W7DEPTH000000000000000";
    let script = script(
        &[
            tool(
                "c1",
                "dispatch",
                r#"{"kind": "agent", "name": "child", "prompt": "go deeper", "wait": true, "timeout_secs": 150}"#,
            ),
            text("parent done"),
        ],
        &[
            tool(
                "d1",
                "dispatch",
                r#"{"kind": "subagent", "name": "child", "prompt": "deeper still", "wait": true}"#,
            ),
            text("child done"),
        ],
    );
    home.rupu_run(
        &script,
        &[
            "parent",
            "--mode",
            "bypass",
            "--run-id",
            run_id,
            "--launch-depth",
            "4",
            "--launch-ceiling",
            "bypass",
            "--parent-run",
            "run_01W7ROOT",
            "--root-run",
            "run_01W7ROOT",
            "--launch-agents=child",
            "--launch-workflows=",
            "go",
        ],
    );
    let children = home.children(run_id);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].started.depth, 5);
    let child_transcript = children[0].started.transcript.clone().unwrap();
    let results = tool_results(&child_transcript);
    let refused = results
        .iter()
        .find_map(|(_, e)| e.clone())
        .expect("the depth-5 child's dispatch was refused");
    assert!(refused.contains("max_dispatch_depth_exceeded"), "{refused}");
    assert!(home.children(&children[0].started.run_id).is_empty());
}

/// §6.4: a readonly parent's `agent` child, whose own mode is bypass, runs
/// readonly: its `write_file` is denied (D7 across the boundary).
#[test]
fn ceiling_across_processes() {
    let _env = ENV_LOCK.blocking_lock();
    let home = Home::new();
    home.agent(
        "parent",
        PARENT_MODEL,
        "tools: [dispatch]\ndispatchableAgents: [writer]\n",
    );
    home.agent(
        "writer",
        CHILD_MODEL,
        "permissionMode: bypass\ntools: [write_file]\n",
    );
    let run_id = "run_01W7CEILING0000000000000";
    let script = script(
        &[
            tool(
                "c1",
                "dispatch",
                r#"{"kind": "agent", "name": "writer", "prompt": "write it", "wait": true, "timeout_secs": 150}"#,
            ),
            text("parent done"),
        ],
        &[
            tool(
                "w1",
                "write_file",
                r#"{"path": "escaped.txt", "content": "x"}"#,
            ),
            text("child done"),
        ],
    );
    home.rupu_run(
        &script,
        &["parent", "--mode", "readonly", "--run-id", run_id, "go"],
    );
    assert!(
        !home.project.join("escaped.txt").exists(),
        "the child must not write"
    );
    let children = home.children(run_id);
    let child_transcript = children[0].started.transcript.clone().unwrap();
    let evs = events(&child_transcript);
    assert!(evs.iter().any(|e| matches!(
        e,
        rupu_transcript::Event::RunStart {
            mode: rupu_transcript::RunMode::Readonly,
            ..
        }
    )));
    assert!(tool_results(&child_transcript)
        .iter()
        .any(|(_, e)| e.as_deref() == Some("permission_denied")));
}

/// An `ask` parent's process child has no terminal to prompt at: it runs
/// `ask` unattended, as a workflow step does — its write is allowed with a
/// `permission_mode_degraded` notice — rather than refusing to start.
#[test]
fn an_ask_ceiling_child_runs_unattended() {
    let _env = ENV_LOCK.blocking_lock();
    let home = Home::new();
    home.agent(
        "writer",
        CHILD_MODEL,
        "permissionMode: bypass\ntools: [write_file]\n",
    );
    let script = script(
        &[],
        &[
            tool(
                "w1",
                "write_file",
                r#"{"path": "written.txt", "content": "x"}"#,
            ),
            text("child done"),
        ],
    );
    let run_id = "run_01W7ASKCHILD00000000000";
    home.rupu_run(
        &script,
        &[
            "writer",
            "--mode",
            "ask",
            "--run-id",
            run_id,
            "--launch-depth",
            "1",
            "--launch-ceiling",
            "ask",
            "--parent-run",
            "run_01W7ASKPARENT0000000000",
            "--root-run",
            "run_01W7ASKPARENT0000000000",
            "--launch-agents=",
            "--launch-workflows=",
            "go",
        ],
    );
    assert!(home.project.join("written.txt").exists());
    let evs = events(&home.transcript(run_id));
    assert!(evs.iter().any(|e| matches!(
        e,
        rupu_transcript::Event::RunStart {
            mode: rupu_transcript::RunMode::Ask,
            ..
        }
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        rupu_transcript::Event::Notice { kind, .. } if kind == "permission_mode_degraded"
    )));
}

/// §6.7 across processes: a parent that ends without joining its `agent`
/// child stops it — the process is gone and the ledger says `abandoned`.
#[test]
fn parent_end_stops_an_unjoined_process_child() {
    let _env = ENV_LOCK.blocking_lock();
    let home = Home::new();
    home.agent(
        "parent",
        PARENT_MODEL,
        "tools: [dispatch]\ndispatchableAgents: [sleeper]\n",
    );
    home.agent("sleeper", CHILD_MODEL, "tools: [bash]\n");
    let run_id = "run_01W7ABANDON0000000000000";
    let script = script(
        &[
            tool(
                "c1",
                "dispatch",
                r#"{"kind": "agent", "name": "sleeper", "prompt": "nap"}"#,
            ),
            text("parent done"),
        ],
        &[
            tool("b1", "bash", r#"{"command": "sleep 60"}"#),
            text("never"),
        ],
    );
    let t0 = Instant::now();
    home.rupu_run(
        &script,
        &["parent", "--mode", "bypass", "--run-id", run_id, "go"],
    );
    assert!(
        t0.elapsed() < Duration::from_secs(50),
        "the parent did not wait out the child"
    );
    let children = home.children(run_id);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].state(), ChildState::Abandoned);
    let pid = children[0]
        .started
        .pid
        .expect("a process child records its pid");
    let deadline = Instant::now() + Duration::from_secs(20);
    while rupu_launch::proc::pid_is_running(pid) && !rupu_launch::proc::is_zombie(pid) {
        assert!(
            Instant::now() < deadline,
            "the child process {pid} survived"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A process child stopped by SIGTERM (or a Ctrl-C's SIGINT) stops its own
/// children first: the grandchild's process is gone. Its parent, still
/// running, records it failed, and so what it left is `abandoned`.
#[test]
fn a_signalled_child_stops_its_own_children() {
    let _env = ENV_LOCK.blocking_lock();
    for signal in [rustix::process::Signal::TERM, rustix::process::Signal::INT] {
        signalled_child_stops_its_own_children(signal);
    }
}

fn signalled_child_stops_its_own_children(signal: rustix::process::Signal) {
    const GRAND_MODEL: &str = "claude-opus-4-7";
    let home = Home::new();
    home.agent(
        "parent",
        PARENT_MODEL,
        // A child's allowlist is capped by its parent's.
        "tools: [dispatch, join]\ndispatchableAgents: [child, grand]\n",
    );
    home.agent(
        "child",
        CHILD_MODEL,
        "tools: [dispatch, bash]\ndispatchableAgents: [grand]\n",
    );
    home.agent("grand", GRAND_MODEL, "tools: [bash]\n");
    let nap = || tool("b1", "bash", r#"{"command": "sleep 60"}"#);
    let script = format!(
        r#"{{ "turns": [], "models": {{ "{PARENT_MODEL}": [{}], "{CHILD_MODEL}": [{}], "{GRAND_MODEL}": [{}] }} }}"#,
        [
            tool(
                "c1",
                "dispatch",
                r#"{"kind": "agent", "name": "child", "prompt": "go", "wait": true, "timeout_secs": 150}"#,
            ),
            text("parent done"),
        ]
        .join(","),
        [
            tool(
                "c2",
                "dispatch",
                r#"{"kind": "agent", "name": "grand", "prompt": "go"}"#,
            ),
            nap(),
            text("never"),
        ]
        .join(","),
        [nap(), text("never")].join(","),
    );
    let run_id = "run_01W7SIGTERM0000000000000";
    let mut parent = home.spawn_run(
        &script,
        &["parent", "--mode", "bypass", "--run-id", run_id, "go"],
    );
    let child = home.wait_for_child(run_id);
    let grand = home.wait_for_child(&child.started.run_id);
    let grand_pid = grand.started.pid.unwrap();

    let group = child.started.pgid.unwrap();
    rustix::process::kill_process_group(
        rustix::process::Pid::from_raw(i32::try_from(group).unwrap()).unwrap(),
        signal,
    )
    .unwrap_or_else(|e| panic!("signal {signal:?} to group {group}: {e}"));

    let deadline = Instant::now() + Duration::from_secs(30);
    while rupu_launch::proc::pid_is_running(grand_pid) && !rupu_launch::proc::is_zombie(grand_pid) {
        if Instant::now() >= deadline {
            let _ = parent.kill();
            rupu_launch::proc::kill_group(grand_pid);
            panic!("the grandchild {grand_pid} survived its parent's {signal:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    while parent.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the parent never finished");
        std::thread::sleep(Duration::from_millis(100));
    }
    // The signalled child recorded its own child `interrupted`; its parent,
    // which nothing stopped, saw it die and records what it left abandoned.
    let entry = home
        .children(&child.started.run_id)
        .into_iter()
        .find(|c| c.started.handle == grand.started.handle)
        .unwrap();
    assert_eq!(entry.state(), ChildState::Abandoned);
}

/// A model call that answers `turn` after a minute: the run is mid-call
/// when the test kills or stops it.
fn slow(turn: &str) -> String {
    format!(r#"{{ "Slow": {{ "ms": 60000, "turn": {turn} }} }}"#)
}

/// Whether `path`'s transcript has a line of `kind`.
fn has_event(path: &Path, kind: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.contains(&format!("\"type\":\"{kind}\"")))
}

/// (h) A `rupu run` parent SIGKILLed while its agent child runs records
/// nothing. `rupu run --continue <parent>` stops the still-running child
/// (its own records and start time say the recorded pid is it), records it
/// `interrupted`, continues it as `rupu run --continue <child>` under the
/// same handle, and the continued parent's `join` gets the continued
/// child's answer.
#[test]
fn continuing_a_crashed_parent_continues_its_agent_child() {
    let _env = ENV_LOCK.blocking_lock();
    let home = Home::new();
    home.agent(
        "parent",
        PARENT_MODEL,
        "tools: [dispatch, join]\ndispatchableAgents: [child]\n",
    );
    home.agent("child", CHILD_MODEL, "");
    let crashed = "run_01W7CRASHED00000000000000";
    let first = script(
        &[
            tool(
                "c1",
                "dispatch",
                r#"{"kind": "agent", "name": "child", "prompt": "find it"}"#,
            ),
            slow(&text("never")),
        ],
        &[slow(&text("never"))],
    );
    let mut parent = home.spawn_run(
        &first,
        &["parent", "--mode", "bypass", "--run-id", crashed, "go"],
    );
    let child = home.wait_for_child(crashed);
    let child_pid = child.started.pid.unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !(has_event(&home.transcript(crashed), "tool_result")
        && has_event(&home.transcript(&child.started.run_id), "user_message"))
    {
        assert!(
            Instant::now() < deadline,
            "the runs never reached their model calls:\n{}\n---\n{}",
            std::fs::read_to_string(home.transcript(crashed)).unwrap_or_default(),
            std::fs::read_to_string(home.transcript(&child.started.run_id)).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    parent.kill().unwrap();
    parent.wait().unwrap();
    assert!(
        rupu_launch::proc::pid_is_running(child_pid),
        "the child outlived the crash"
    );

    let second = script(
        &[
            tool("c2", "join", r#"{"timeout_secs": 150}"#),
            text("parent done"),
        ],
        &[text("continued child answer")],
    );
    home.rupu_run(
        &second,
        &[
            "parent",
            "--mode",
            "bypass",
            "--run-id",
            "run_01W7CONTINUED0000000000000",
            "--continue",
            crashed,
        ],
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    while rupu_launch::proc::pid_is_running(child_pid) && !rupu_launch::proc::is_zombie(child_pid) {
        assert!(
            Instant::now() < deadline,
            "the crashed run's child {child_pid} survived"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(home.children(crashed)[0].state(), ChildState::Interrupted);
    let now = home.children("run_01W7CONTINUED0000000000000");
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].started.handle, child.started.handle);
    assert_ne!(now[0].started.run_id, child.started.run_id);
    assert_eq!(now[0].state(), ChildState::Done);
    let joined = tool_results(&home.transcript("run_01W7CONTINUED0000000000000"));
    let (output, _) = joined.last().expect("the join's result");
    assert!(output.contains("continued child answer"), "{output}");
    assert!(output.contains(&child.started.handle), "{output}");
}

/// (b) across processes: a `rupu run` parent SIGKILLed while its workflow
/// child's step is mid-model-call. `rupu run --continue <parent>` stops the
/// workflow run's runner (its `run.json` names the recorded pid), records
/// the child `interrupted`, and continues it as `rupu workflow resume <its
/// run>`, which continues the interrupted step; the parent's `join` gets the
/// workflow's answer, from the same workflow run.
#[test]
fn continuing_a_crashed_parent_resumes_its_workflow_child() {
    let _env = ENV_LOCK.blocking_lock();
    let home = Home::new();
    home.agent(
        "parent",
        PARENT_MODEL,
        "tools: [dispatch, join]\ndispatchableAgents: [child]\ndispatchableWorkflows: [sweep]\n",
    );
    home.agent("child", CHILD_MODEL, "");
    home.workflow(
        "sweep",
        "name: sweep\nsteps:\n  - id: scan\n    agent: child\n    prompt: scan it\n",
    );
    let crashed = "run_01W7CRASHEDWF000000000000";
    let first = script(
        &[
            tool("c1", "dispatch", r#"{"kind": "workflow", "name": "sweep"}"#),
            slow(&text("never")),
        ],
        &[slow(&text("never"))],
    );
    let mut parent = home.spawn_run(
        &first,
        &["parent", "--mode", "bypass", "--run-id", crashed, "go"],
    );
    let child = home.wait_for_child(crashed);
    let wf_run = child.started.run_id.clone();
    let deadline = Instant::now() + Duration::from_secs(60);
    let step_started = || {
        rupu_orchestrator::RunStore::new(home.global.join("runs"))
            .read_attempts(&wf_run)
            .ok()
            .and_then(|a| a.first().map(|a| a.transcript_path.clone()))
            .is_some_and(|t| has_event(&t, "user_message"))
    };
    while !(has_event(&home.transcript(crashed), "tool_result") && step_started()) {
        assert!(
            Instant::now() < deadline,
            "the runs never reached their model calls"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    parent.kill().unwrap();
    parent.wait().unwrap();

    let second = script(
        &[
            tool("c2", "join", r#"{"timeout_secs": 150}"#),
            text("parent done"),
        ],
        &[text("step answer")],
    );
    let continued = "run_01W7CONTINUEDWF0000000000";
    home.rupu_run(
        &second,
        &[
            "parent",
            "--mode",
            "bypass",
            "--run-id",
            continued,
            "--continue",
            crashed,
        ],
    );

    assert_eq!(home.children(crashed)[0].state(), ChildState::Interrupted);
    let now = home.children(continued);
    assert_eq!(now.len(), 1);
    assert_eq!(
        (
            now[0].started.handle.as_str(),
            now[0].started.run_id.as_str()
        ),
        (child.started.handle.as_str(), wf_run.as_str())
    );
    assert_eq!(now[0].state(), ChildState::Done, "{:?}", now[0].finished);
    let joined = tool_results(&home.transcript(continued));
    let (output, _) = joined.last().expect("the join's result");
    assert!(output.contains("step answer"), "{output}");
    let rec = rupu_orchestrator::RunStore::new(home.global.join("runs"))
        .load(&wf_run)
        .unwrap();
    assert_eq!(rec.status, rupu_orchestrator::RunStatus::Completed);
}

/// D2 across processes: a `rupu run` parent stopped by SIGTERM stops its
/// agent child, which stops its own child in turn, each recorded
/// `interrupted` rather than `abandoned`. `rupu run --continue <parent>`
/// continues the child under the same handle, and the continued child
/// continues the grandchild: the whole tree picks up where it stopped.
#[test]
fn a_signalled_tree_is_continued_at_every_level() {
    const GRAND_MODEL: &str = "claude-opus-4-7";
    let _env = ENV_LOCK.blocking_lock();
    let home = Home::new();
    home.agent(
        "parent",
        PARENT_MODEL,
        "tools: [dispatch, join]\ndispatchableAgents: [child, grand]\n",
    );
    home.agent(
        "child",
        CHILD_MODEL,
        "tools: [dispatch, join]\ndispatchableAgents: [grand]\n",
    );
    home.agent("grand", GRAND_MODEL, "");
    let models = |parent: &[String], child: &[String], grand: &[String]| {
        format!(
            r#"{{ "turns": [], "models": {{ "{PARENT_MODEL}": [{}], "{CHILD_MODEL}": [{}], "{GRAND_MODEL}": [{}] }} }}"#,
            parent.join(","),
            child.join(","),
            grand.join(",")
        )
    };
    let first = models(
        &[
            tool(
                "c1",
                "dispatch",
                r#"{"kind": "agent", "name": "child", "prompt": "go"}"#,
            ),
            slow(&text("never")),
        ],
        &[
            tool(
                "c2",
                "dispatch",
                r#"{"kind": "agent", "name": "grand", "prompt": "go"}"#,
            ),
            slow(&text("never")),
        ],
        &[slow(&text("never"))],
    );
    let stopped = "run_01W7SIGNALLEDTREE00000000";
    let mut parent = home.spawn_run(
        &first,
        &["parent", "--mode", "bypass", "--run-id", stopped, "go"],
    );
    let child = home.wait_for_child(stopped);
    let grand = home.wait_for_child(&child.started.run_id);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !has_event(&home.transcript(&grand.started.run_id), "user_message") {
        assert!(Instant::now() < deadline, "the grandchild never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    let sent = Command::new("kill")
        .args(["-s", "TERM", &parent.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let deadline = Instant::now() + Duration::from_secs(60);
    while parent.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the parent never exited");
        std::thread::sleep(Duration::from_millis(50));
    }
    let grand_pid = grand.started.pid.unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while rupu_launch::proc::pid_is_running(grand_pid) && !rupu_launch::proc::is_zombie(grand_pid) {
        assert!(Instant::now() < deadline, "the grandchild survived");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(home.children(stopped)[0].state(), ChildState::Interrupted);
    assert_eq!(
        home.children(&child.started.run_id)[0].state(),
        ChildState::Interrupted
    );

    let second = models(
        &[
            tool("c3", "join", r#"{"timeout_secs": 150}"#),
            text("parent done"),
        ],
        &[
            tool("c4", "join", r#"{"timeout_secs": 150}"#),
            text("child answer"),
        ],
        &[text("grand answer")],
    );
    let continued = "run_01W7SIGNALLEDTREECONT0000";
    home.rupu_run(
        &second,
        &[
            "parent",
            "--mode",
            "bypass",
            "--run-id",
            continued,
            "--continue",
            stopped,
        ],
    );

    let now = home.children(continued);
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].started.handle, child.started.handle);
    assert_eq!(now[0].state(), ChildState::Done);
    let below = home.children(&now[0].started.run_id);
    assert_eq!(below.len(), 1);
    assert_eq!(below[0].started.handle, grand.started.handle);
    assert_eq!(below[0].state(), ChildState::Done);
    let (output, _) = tool_results(&home.transcript(&now[0].started.run_id))
        .pop()
        .expect("the child's join");
    assert!(output.contains("grand answer"), "{output}");
    let (output, _) = tool_results(&home.transcript(continued))
        .pop()
        .expect("the parent's join");
    assert!(output.contains("child answer"), "{output}");
}
