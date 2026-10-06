//! `rupu transcript list` / `rupu run list` report each row's customer, and
//! degrade — never fail — on a workspace whose assignment can't be read: that
//! workspace's rows omit the customer keys (a coordinator then reads the host
//! as unable to report a customer for every run, never as "no customer"), one
//! warning per workspace goes to stderr, and stdout stays valid JSON.

use assert_cmd::Command;
use chrono::Utc;
use rupu_transcript::{Event, JsonlWriter, RunMode, RunStatus};
use serde_json::Value;

/// A workspace sidecar that exists but can't be read as a file.
fn break_assignment(home: &std::path::Path, ws: &str) {
    std::fs::create_dir_all(home.join("workspaces").join(format!("{ws}.customer"))).unwrap();
}

fn rupu_json(home: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> (Value, String) {
    let out = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home)
        .env_remove("RUPU_LOG")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (json, String::from_utf8_lossy(&out.stderr).into_owned())
}

fn write_transcript(dir: &std::path::Path, run_id: &str, ws: &str, customer: Option<&str>) {
    // `None` = a transcript that predates customers (no key).
    write_transcript_rec(dir, run_id, ws, customer.map(|c| Some(c.to_string())));
}

fn write_transcript_rec(
    dir: &std::path::Path,
    run_id: &str,
    ws: &str,
    customer: rupu_transcript::RecordedField,
) {
    std::fs::create_dir_all(dir).unwrap();
    let mut w = JsonlWriter::create(dir.join(format!("{run_id}.jsonl"))).unwrap();
    w.write(&Event::RunStart {
        run_id: run_id.into(),
        workspace_id: ws.into(),
        agent: "reviewer".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        started_at: Utc::now(),
        mode: RunMode::Bypass,
        schema: None,
        system_prompt: None,
        codename: None,
        customer,
    })
    .unwrap();
    w.write(&Event::RunComplete {
        run_id: run_id.into(),
        status: RunStatus::Ok,
        total_tokens: 1,
        duration_ms: 1,
        error: None,
        outcome: None,
    })
    .unwrap();
    w.flush().unwrap();
}

fn row<'a>(rows: &'a Value, key: &str, id: &str) -> &'a serde_json::Map<String, Value> {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r[key] == id)
        .unwrap_or_else(|| panic!("no row {id} in {rows}"))
        .as_object()
        .unwrap()
}

#[test]
fn transcript_list_degrades_on_an_unreadable_assignment() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let tx = home.join("transcripts");
    write_transcript(&tx, "run_broken_a", "ws_broken", None);
    write_transcript(&tx, "run_broken_b", "ws_broken", None);
    write_transcript(&tx, "run_acme", "ws_fine", Some("acme"));
    write_transcript(&tx, "run_plain", "ws_fine", None);
    break_assignment(&home, "ws_broken");

    let (report, stderr) = rupu_json(
        &home,
        tmp.path(),
        &["--format", "json", "transcript", "list"],
    );
    let rows = &report["rows"];
    assert_eq!(rows.as_array().unwrap().len(), 4, "every run is listed");
    for id in ["run_broken_a", "run_broken_b"] {
        let r = row(rows, "run_id", id);
        assert!(!r.contains_key("customer"), "{id}: {r:?}");
        assert!(!r.contains_key("customer_derived"), "{id}: {r:?}");
    }
    assert_eq!(row(rows, "run_id", "run_acme")["customer"], "acme");
    let plain = row(rows, "run_id", "run_plain");
    assert!(plain["customer"].is_null());
    assert_eq!(plain["customer_derived"], false);
    assert_eq!(
        stderr.matches("listed without a customer").count(),
        1,
        "one warning: {stderr}"
    );
    assert!(
        stderr.contains("ws_broken"),
        "names the workspace: {stderr}"
    );
}

fn seed_run(home: &std::path::Path, id: &str, ws: &str, customer: Option<&str>) {
    let store = rupu_orchestrator::RunStore::new(home.join("runs"));
    let mut v = serde_json::json!({
        "id": id,
        "workflow_name": "wf",
        "status": "completed",
        "inputs": {},
        "workspace_id": ws,
        "workspace_path": "/tmp/proj",
        "transcript_dir": "/tmp/proj/.rupu/transcripts",
        "started_at": Utc::now().to_rfc3339(),
    });
    if let Some(c) = customer {
        v["customer"] = serde_json::json!(c);
    }
    store
        .create(serde_json::from_value(v).unwrap(), "name: wf\n")
        .unwrap();
}

#[test]
fn run_list_degrades_on_an_unreadable_assignment() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    seed_run(&home, "run_broken", "ws_broken", None);
    seed_run(&home, "run_broken_recorded", "ws_broken", Some("acme"));
    seed_run(&home, "run_plain", "ws_fine", None);
    break_assignment(&home, "ws_broken");

    let (report, stderr) = rupu_json(&home, tmp.path(), &["--format", "json", "run", "list"]);
    let rows = &report["rows"];
    assert_eq!(rows.as_array().unwrap().len(), 3, "every run is listed");
    let broken = row(rows, "id", "run_broken");
    assert!(!broken.contains_key("customer"), "{broken:?}");
    assert!(!broken.contains_key("customer_derived"), "{broken:?}");
    // A RECORDED customer needs no assignment read.
    assert_eq!(row(rows, "id", "run_broken_recorded")["customer"], "acme");
    assert!(row(rows, "id", "run_plain")["customer"].is_null());
    assert_eq!(
        stderr.matches("listed without a customer").count(),
        1,
        "one warning: {stderr}"
    );
    assert!(
        stderr.contains("ws_broken"),
        "names the workspace: {stderr}"
    );
}

/// `transcript list` attributes by what each transcript recorded: an explicit
/// `null` stays none even in a workspace assigned since; a session turn whose
/// transcript predates customers inherits its session's customer (derived);
/// any other legacy transcript derives from its workspace's assignment.
#[test]
fn transcript_list_keeps_a_recorded_none_and_inherits_a_legacy_turns_session() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let tx = home.join("transcripts");
    rupu_workspace::CustomerStore::new(&home)
        .create(
            "acme",
            &rupu_workspace::NewCustomer {
                name: "Acme".into(),
                ..Default::default()
            },
        )
        .unwrap();
    rupu_workspace::CustomerStore::new(&home)
        .create(
            "globex",
            &rupu_workspace::NewCustomer {
                name: "Globex".into(),
                ..Default::default()
            },
        )
        .unwrap();
    std::fs::create_dir_all(home.join("workspaces")).unwrap();
    std::fs::write(home.join("workspaces/ws_acme.customer"), "acme\n").unwrap();

    write_transcript_rec(&tx, "run_none", "ws_acme", Some(None));
    write_transcript(&tx, "run_legacy", "ws_acme", None);
    // Session turns of `ses_globex` (whose record says globex): one legacy,
    // one that recorded none.
    for (id, field) in [("turn_legacy", None), ("turn_none", Some(None))] {
        write_transcript_rec(&tx, id, "ws_acme", field);
        std::fs::write(
            tx.join(format!("{id}.meta.json")),
            serde_json::json!({
                "version": 1,
                "run_id": id,
                "session_id": "ses_globex",
                "workspace_path": "/tmp/proj",
                "backend_id": "local",
                "trigger_source": "session_turn",
            })
            .to_string(),
        )
        .unwrap();
    }
    let sdir = home.join("sessions/ses_globex");
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(
        sdir.join("session.json"),
        serde_json::json!({"session_id": "ses_globex", "customer": "globex"}).to_string(),
    )
    .unwrap();

    let (report, _) = rupu_json(
        &home,
        tmp.path(),
        &["--format", "json", "transcript", "list"],
    );
    let rows = &report["rows"];
    let keys = |id: &str| {
        let r = row(rows, "run_id", id);
        (r["customer"].clone(), r["customer_derived"].clone())
    };
    use serde_json::json;
    assert_eq!(keys("run_none"), (json!(null), json!(false)));
    assert_eq!(keys("run_legacy"), (json!("acme"), json!(true)));
    assert_eq!(keys("turn_legacy"), (json!("globex"), json!(true)));
    assert_eq!(keys("turn_none"), (json!(null), json!(false)));
}
