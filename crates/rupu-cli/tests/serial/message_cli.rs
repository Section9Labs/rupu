//! `rupu message` and the hidden `rupu __message` against the real binary
//! (spec F1b §5 test 7): a message the operator sends into a running workflow
//! run's space reaches the participant it names, reads back through `list`,
//! and `list --follow` returns once the run stops.

use crate::ENV_LOCK;
use assert_cmd::Command as AssertCommand;
use rupu_fleet::{Bus, Member};
use rupu_orchestrator::{RunRecord, RunStatus, RunStore};
use rupu_runtime::services::operator::{MessagesView, OperatorSendReply};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RUN: &str = "run_01MSGCLIFOLLOW0000000000";

struct Fixture {
    _tmp: assert_fs::TempDir,
    global: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn rupu(&self, args: &[&str]) -> AssertCommand {
        let mut cmd = AssertCommand::cargo_bin("rupu").unwrap();
        cmd.env("RUPU_HOME", &self.global)
            .current_dir(&self.project)
            .args(args);
        cmd
    }

    fn store(&self) -> RunStore {
        RunStore::new(self.global.join("runs"))
    }

    fn set_status(&self, status: RunStatus) {
        let mut rec = self.store().load(RUN).unwrap();
        rec.status = status;
        self.store().update(&rec).unwrap();
    }
}

/// A workflow run `RUN`, running under this test process, with `lynx#1`
/// joined in its message space.
fn fixture() -> Fixture {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.path().join(".rupu");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let rec: RunRecord = serde_json::from_value(json!({
        "id": RUN,
        "workflow_name": "review",
        "status": "running",
        "inputs": {},
        "workspace_id": "",
        "workspace_path": ".",
        "transcript_dir": global.join("runs").join(RUN),
        "started_at": "2026-10-13T00:00:00Z",
        "runner_pid": std::process::id(),
    }))
    .unwrap();
    RunStore::new(global.join("runs")).create(rec, "").unwrap();
    Bus::open(&global.join("runs").join(RUN).join("messages"), "lynx#1")
        .join(Member {
            agent: "recon".into(),
            run_id: format!("{RUN}_scan"),
            step_id: Some("scan".into()),
            kind: "workflow_step".into(),
            ..Default::default()
        })
        .unwrap();
    Fixture {
        _tmp: tmp,
        global,
        project,
    }
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Kills the child on the way out, failing assertions included.
struct Reap(std::process::Child);

impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_exit(child: &mut std::process::Child, within: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "`list --follow` did not return");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn read_to_string(r: Option<impl std::io::Read>) -> String {
    let mut s = String::new();
    r.unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn message_cli_follow() {
    let _guard = ENV_LOCK.blocking_lock();
    let fx = fixture();

    let out = fx
        .rupu(&["message", "send", RUN, "--to", "lynx#1", "hello", "there"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let sent = stdout(&out);

    let out = fx
        .rupu(&["message", "list", RUN, "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let view: MessagesView = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(view.messages.len(), 1);
    let m = &view.messages[0].message;
    assert_eq!(sent.trim(), format!("sent {} to lynx#1", m.id));
    assert_eq!(
        (m.kind.as_str(), m.from.as_str(), m.body.as_str()),
        ("operator", "operator", "hello there")
    );
    assert_eq!(m.to.as_deref(), Some("lynx#1"));
    assert_eq!(m.delivery.resolved, ["lynx#1"]);
    assert_eq!(view.participants[0].address, "lynx#1");
    let first = m.id.clone();

    // The SSH transport's command: JSON in, JSON out.
    let out = fx
        .rupu(&[
            "__message",
            "send",
            RUN,
            "--body=-urgent: stop",
            "--to",
            "step:scan",
            "--urgent",
            "--viewer",
            "matt",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let reply: OperatorSendReply = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply.delivery.resolved, ["lynx#1"]);
    let out = fx
        .rupu(&["__message", "list", RUN, "--after", &first, "--limit", "5"])
        .output()
        .unwrap();
    let page: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(page["messages"].as_array().unwrap().len(), 1, "{page}");
    let m = &page["messages"][0];
    assert_eq!(m["id"], json!(reply.id));
    assert_eq!(
        (&m["body"], &m["urgent"], &m["via"], &m["viewer"]),
        (
            &json!("-urgent: stop"),
            &json!(true),
            &json!("cp"),
            &json!("matt")
        )
    );

    // `--follow` prints what is there, then returns once the run stops.
    let mut follow = Reap(
        Command::new(assert_cmd::cargo::cargo_bin("rupu"))
            .env("RUPU_HOME", &fx.global)
            .current_dir(&fx.project)
            .args(["message", "list", RUN, "--follow", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        follow.0.try_wait().unwrap().is_none(),
        "it follows a live run"
    );
    let out = fx
        .rupu(&["message", "send", RUN, "while", "following"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    std::thread::sleep(Duration::from_millis(600));
    fx.set_status(RunStatus::Completed);
    let status = wait_exit(&mut follow.0, Duration::from_secs(10));
    assert!(status.success(), "{status:?}");
    let lines = read_to_string(follow.0.stdout.take());
    let bodies: Vec<Value> = lines
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["body"].clone())
        .collect();
    assert_eq!(
        bodies,
        [
            json!("hello there"),
            json!("-urgent: stop"),
            json!("while following")
        ]
    );
    let why = read_to_string(follow.0.stderr.take());
    assert!(
        why.contains(&format!("workflow run `{RUN}` is not running (completed)")),
        "{why}"
    );

    // A run that stopped takes no message.
    let out = fx.rupu(&["message", "send", RUN, "late"]).output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("it takes no message"),
        "{out:?}"
    );
    let out = fx
        .rupu(&["__message", "send", RUN, "--body=late"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let err: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(err["error"]["code"], json!("not_live"), "{err}");
    let out = fx
        .rupu(&["__message", "list", "run_nope"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let err: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(err["error"]["code"], json!("not_found"), "{err}");
    assert_eq!(count_messages(&fx.global), 3);
}

fn count_messages(global: &Path) -> usize {
    let log = global.join("runs").join(RUN).join("messages/log.jsonl");
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .filter(|l| l.contains(r#""t":"msg""#))
        .count()
}
