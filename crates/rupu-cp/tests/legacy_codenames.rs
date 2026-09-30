//! Runs recorded before agent codenames existed get names derived on read
//! BELOW the run level (steps, fan-out units, panelists, fixers, parallel
//! sub-steps, sub-agents, and the events announcing them), flagged
//! `codename_derived: true`. A new-style run's stored names pass through
//! untouched and are never flagged derived.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use futures_util::TryStreamExt as _;
use rupu_orchestrator::{codenames::RunNaming, runs::RunStore, Workflow};
use serde_json::{json, Value};
use std::io::Write as _;
use std::path::Path;
use tokio::io::AsyncBufReadExt as _;

/// Crew `jade-reef`.
const LEGACY: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W";
const NEW: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2X";
const SUB: &str = "sub_01J9ZQ3K4M5N6P7Q8R9S0T1V01";

const WF: &str = r#"
name: legacy-fixture
steps:
  - id: alpha
    agent: ag
    actions: []
    prompt: "a"
  - id: fan
    agent: triage
    actions: []
    for_each: "{{ inputs.items }}"
    prompt: "c"
  - id: review
    actions: []
    panel:
      panelists: [sec, perf, sec]
      subject: "x"
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: fixer
        max_iterations: 3
  - id: par
    actions: []
    parallel:
      - id: p1
        agent: ag
        prompt: "p"
      - id: p2
        agent: triage
        prompt: "q"
"#;

fn record(id: &str, codename: Option<&str>) -> Value {
    let mut r = json!({
        "id": id,
        "workflow_name": "legacy-fixture",
        "status": "completed",
        "inputs": {},
        "workspace_id": "ws_test",
        "workspace_path": "/tmp/ws",
        "transcript_dir": "/tmp/ws/.rupu/transcripts",
        "started_at": "2026-09-10T19:34:14Z",
    });
    if let Some(c) = codename {
        r["codename"] = json!(c);
    }
    r
}

fn write_lines(path: &Path, lines: &[Value]) {
    let mut f = std::fs::File::create(path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
}

fn step_rec(step_id: &str, kind: &str, run_id: &str, extra: Value) -> Value {
    let mut v = json!({
        "step_id": step_id, "run_id": run_id,
        "transcript_path": format!("/t/{run_id}.jsonl"),
        "output": "", "success": true, "skipped": false, "rendered_prompt": "",
        "kind": kind, "finished_at": "2026-09-10T19:34:35Z"
    });
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}

fn item(index: usize, sub_id: &str, run_id: &str, output: &str) -> Value {
    json!({"index": index, "item": null, "sub_id": sub_id, "rendered_prompt": "",
           "run_id": run_id, "transcript_path": format!("/t/{run_id}.jsonl"),
           "output": output, "success": true})
}

/// A legacy run dir exactly as a pre-codename binary wrote it: no
/// `codename` key anywhere.
fn seed_legacy(runs: &Path) {
    let dir = runs.join(LEGACY);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("run.json"), record(LEGACY, None).to_string()).unwrap();
    std::fs::write(dir.join("workflow.yaml"), WF).unwrap();
    write_lines(
        &dir.join("step_results.jsonl"),
        &[
            step_rec("alpha", "linear", "run_A", json!({})),
            step_rec(
                "fan",
                "for_each",
                "",
                json!({"items": [
                    item(0, "", "run_F0", ""), item(1, "", "run_F1", "")
                ]}),
            ),
            step_rec(
                "review",
                "panel",
                "",
                json!({
                    "items": [item(0, "sec", "run_R0", "SQLi"), item(1, "perf", "run_R1", ""),
                              item(2, "sec", "run_R2", "XSS")],
                    "findings": [{"source": "perf", "severity": "low", "title": "slow", "body": ""},
                                 {"source": "sec", "severity": "high", "title": "XSS", "body": ""}]
                }),
            ),
            step_rec(
                "par",
                "parallel",
                "",
                json!({"items": [
                    item(0, "p1", "run_P1", ""), item(1, "p2", "run_P2", "")
                ]}),
            ),
        ],
    );
    write_lines(
        &dir.join("unit_checkpoints.jsonl"),
        &[
            json!({"step_id": "fan", "index": 0, "item": "a.rs", "run_id": "run_F0",
                   "transcript_path": "/t/run_F0.jsonl", "output": "", "success": true,
                   "finished_at": "2026-09-10T19:34:35Z"}),
            json!({"step_id": "fan", "index": 1, "item": "b.rs", "run_id": "run_F1",
                   "transcript_path": "/t/run_F1.jsonl", "output": "", "success": true,
                   "finished_at": "2026-09-10T19:34:35Z"}),
        ],
    );
    write_lines(
        &dir.join("events.jsonl"),
        &[
            json!({"type": "run_started", "event_version": 1, "run_id": LEGACY,
                   "workflow_path": "/w", "started_at": "2026-09-10T19:34:14Z"}),
            json!({"type": "step_started", "run_id": LEGACY, "step_id": "alpha",
                   "kind": "linear", "agent": "ag"}),
            json!({"type": "step_working", "run_id": LEGACY, "step_id": "alpha",
                   "note": null, "transcript_path": "/t/run_A.jsonl"}),
            json!({"type": "dispatch_started", "run_id": LEGACY, "sub_run_id": SUB,
                   "agent": "scout", "transcript_path": "/t/sub.jsonl"}),
            json!({"type": "unit_started", "run_id": LEGACY, "step_id": "fan", "index": 0,
                   "unit_key": "a.rs", "agent": "triage", "transcript_path": "/t/run_F0.jsonl"}),
            json!({"type": "unit_started", "run_id": LEGACY, "step_id": "review", "index": 0,
                   "unit_key": "iter1:sec", "agent": "sec", "transcript_path": "/t/run_R0.jsonl"}),
            json!({"type": "unit_started", "run_id": LEGACY, "step_id": "review", "index": 1,
                   "unit_key": "iter1:perf", "agent": "perf", "transcript_path": "/t/run_R1.jsonl"}),
            json!({"type": "unit_started", "run_id": LEGACY, "step_id": "review", "index": 2,
                   "unit_key": "iter1:sec", "agent": "sec", "transcript_path": "/t/run_R2.jsonl"}),
            json!({"type": "unit_started", "run_id": LEGACY, "step_id": "review", "index": 3,
                   "unit_key": "iter1:fix:fixer", "agent": "fixer",
                   "transcript_path": "/t/run_X.jsonl"}),
            json!({"type": "run_completed", "run_id": LEGACY, "status": "completed",
                   "finished_at": "2026-09-10T19:34:35Z"}),
        ],
    );
    // Step alpha's agent (run_A) dispatched one `scout` sub-agent.
    let sub_t = runs
        .join("run_A")
        .join("sub")
        .join(SUB)
        .join("transcript.jsonl");
    std::fs::create_dir_all(sub_t.parent().unwrap()).unwrap();
    std::fs::write(
        &sub_t,
        format!(
            "{}\n",
            json!({"type": "run_start", "data": {"run_id": SUB, "workspace_id": "ws",
                "agent": "scout", "provider": "anthropic", "model": "claude-x",
                "started_at": "2026-09-10T19:34:20Z", "mode": "bypass"}})
        ),
    )
    .unwrap();
}

/// A new-style run: every name stored.
fn seed_new(runs: &Path) {
    let dir = runs.join(NEW);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("run.json"),
        record(NEW, Some("peach-prairie")).to_string(),
    )
    .unwrap();
    std::fs::write(dir.join("workflow.yaml"), WF).unwrap();
    write_lines(
        &dir.join("step_results.jsonl"),
        &[step_rec(
            "alpha",
            "linear",
            "run_NA",
            json!({"codename": "peach-prairie/stored"}),
        )],
    );
    write_lines(
        &dir.join("events.jsonl"),
        &[
            json!({"type": "step_started", "run_id": NEW, "step_id": "alpha",
                 "kind": "linear", "agent": "ag", "codename": "peach-prairie/stored"}),
        ],
    );
}

fn minted() -> RunNaming {
    RunNaming::open(&Workflow::parse(WF).unwrap(), LEGACY, None)
}

async fn spawn_server(dir: &Path) -> std::net::SocketAddr {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn get(addr: std::net::SocketAddr, path: &str) -> Value {
    let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    assert_eq!(resp.status(), 200, "{path}");
    resp.json().await.unwrap()
}

fn assert_derived(v: &Value, want: &str, what: &str) {
    assert_eq!(v["codename"], want, "{what}: {v}");
    assert_eq!(v["codename_derived"], true, "{what}: {v}");
}

fn assert_no_derived_flag_anywhere(v: &Value) {
    match v {
        Value::Object(m) => {
            assert_ne!(m.get("codename_derived"), Some(&json!(true)), "{v}");
            m.values().for_each(assert_no_derived_flag_anywhere);
        }
        Value::Array(a) => a.iter().for_each(assert_no_derived_flag_anywhere),
        _ => {}
    }
}

fn setup() -> (tempfile::TempDir, RunNaming) {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    seed_legacy(&runs);
    seed_new(&runs);
    (tmp, minted())
}

#[tokio::test]
async fn run_detail_derives_step_unit_and_finding_names_for_a_legacy_run() {
    let (tmp, m) = setup();
    let addr = spawn_server(tmp.path()).await;

    let d = get(addr, &format!("/api/runs/{LEGACY}")).await;
    let steps = d["steps"].as_array().unwrap();
    assert_derived(&steps[0], &m.step("alpha", "ag").to_string(), "linear step");
    // Fan-out / panel / parallel step records name no single member.
    assert!(steps[1].get("codename").is_none());
    assert_derived(
        &steps[1]["items"][1],
        &m.unit("fan", "triage", 1).to_string(),
        "unit",
    );
    let r = &steps[2];
    assert_derived(
        &r["items"][0],
        &m.panelist("review", "sec", Some(1)).to_string(),
        "sec#1",
    );
    assert_derived(
        &r["items"][1],
        &m.panelist("review", "perf", None).to_string(),
        "perf",
    );
    assert_derived(
        &r["items"][2],
        &m.panelist("review", "sec", Some(2)).to_string(),
        "sec#2",
    );
    assert_derived(
        &r["findings"][0],
        &m.panelist("review", "perf", None).to_string(),
        "finding",
    );
    assert_derived(
        &r["findings"][1],
        &m.panelist("review", "sec", Some(2)).to_string(),
        "finding",
    );
    assert_derived(
        &steps[3]["items"][1],
        &m.sub("par", "p2", "triage").to_string(),
        "par sub",
    );

    // New-style run: stored names pass through, never flagged derived.
    let n = get(addr, &format!("/api/runs/{NEW}")).await;
    assert_eq!(n["steps"][0]["codename"], "peach-prairie/stored");
    assert_no_derived_flag_anywhere(&n);
}

#[tokio::test]
async fn run_graph_derives_units_and_identities_for_a_legacy_run() {
    let (tmp, m) = setup();
    let addr = spawn_server(tmp.path()).await;

    let g = get(addr, &format!("/api/runs/{LEGACY}/graph")).await;
    assert_derived(
        &g["step_results"][0],
        &m.step("alpha", "ag").to_string(),
        "graph step",
    );
    let units = g["units"].as_array().unwrap();
    let unit = |step: &str, idx: u64| {
        units
            .iter()
            .find(|u| u["step_id"] == step && u["index"] == idx)
            .unwrap_or_else(|| panic!("unit {step}/{idx}"))
    };
    assert_derived(
        unit("fan", 0),
        &m.unit("fan", "triage", 0).to_string(),
        "checkpoint",
    );
    assert_derived(
        unit("review", 0),
        &m.panelist("review", "sec", Some(1)).to_string(),
        "p0",
    );
    assert_derived(
        unit("review", 1),
        &m.panelist("review", "perf", None).to_string(),
        "p1",
    );
    assert_derived(
        unit("review", 2),
        &m.panelist("review", "sec", Some(2)).to_string(),
        "p2",
    );
    assert_derived(
        unit("review", 3),
        &m.fixer("review", "fixer").to_string(),
        "fixer",
    );

    let alpha = m.step("alpha", "ag");
    assert_derived(
        &g["step_identities"]["alpha"],
        &alpha.to_string(),
        "step identity",
    );
    assert_eq!(g["step_identities"]["alpha"]["agent"], "ag");
    assert_derived(
        &g["unit_identities"]["par"]["1"],
        &m.sub("par", "p2", "triage").to_string(),
        "parallel identity",
    );
    let sub = &g["subrun_identities"][SUB];
    let role = m.namer().with(|n| n.clone().canonical_role("scout"));
    assert_derived(sub, &alpha.child(&role, Some(1)).to_string(), "sub-agent");
    assert_eq!(sub["agent"], "scout");
    assert_eq!(sub["provider"], "anthropic");
    assert_eq!(sub["model"], "claude-x");

    let n = get(addr, &format!("/api/runs/{NEW}/graph")).await;
    assert_eq!(n["step_results"][0]["codename"], "peach-prairie/stored");
    assert_no_derived_flag_anywhere(&n);
}

#[tokio::test]
async fn recent_events_derive_names_for_legacy_events_only() {
    let (tmp, m) = setup();
    let addr = spawn_server(tmp.path()).await;

    let rows = get(addr, "/api/events?limit=100").await;
    let rows = rows.as_array().unwrap();
    let find = |run: &str, ty: &str, pred: &dyn Fn(&Value) -> bool| {
        rows.iter()
            .find(|r| r["run_id"] == run && r["type"] == ty && pred(r))
            .unwrap_or_else(|| panic!("{run} {ty}"))
            .clone()
    };
    let alpha = m.step("alpha", "ag");
    assert_derived(
        &find(LEGACY, "step_started", &|_| true),
        &alpha.to_string(),
        "step",
    );
    assert_derived(
        &find(LEGACY, "unit_started", &|r| {
            r["step_id"] == "review" && r["index"] == 2
        }),
        &m.panelist("review", "sec", Some(2)).to_string(),
        "panel unit",
    );
    assert_derived(
        &find(LEGACY, "unit_started", &|r| r["step_id"] == "fan"),
        &m.unit("fan", "triage", 0).to_string(),
        "fan unit",
    );
    let role = m.namer().with(|n| n.clone().canonical_role("scout"));
    assert_derived(
        &find(LEGACY, "dispatch_started", &|_| true),
        &alpha.child(&role, Some(1)).to_string(),
        "dispatch",
    );
    // Run-level events stay unnamed.
    assert!(find(LEGACY, "run_completed", &|_| true)
        .get("codename")
        .is_none());

    let new = find(NEW, "step_started", &|_| true);
    assert_eq!(new["codename"], "peach-prairie/stored");
    assert!(new.get("codename_derived").is_none());
}

#[tokio::test]
async fn run_log_stream_derives_names_for_a_legacy_run() {
    let (tmp, m) = setup();
    let addr = spawn_server(tmp.path()).await;

    for (run, want) in [
        (LEGACY, Some(m.step("alpha", "ag").to_string())),
        (NEW, None),
    ] {
        let resp = reqwest::get(format!("http://{addr}/api/runs/{run}/log"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let reader =
            tokio_util::io::StreamReader::new(resp.bytes_stream().map_err(std::io::Error::other));
        let mut lines = tokio::io::BufReader::new(reader).lines();
        let step = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(line) = lines.next_line().await.unwrap() {
                let Some(data) = line.strip_prefix("data: ").or(line.strip_prefix("data:")) else {
                    continue;
                };
                let v: Value = serde_json::from_str(data.trim()).unwrap();
                if v["type"] == "step_started" {
                    return v;
                }
            }
            panic!("stream ended");
        })
        .await
        .expect("step_started within timeout");
        match want {
            Some(w) => assert_derived(&step, &w, "stream step"),
            None => {
                assert_eq!(step["codename"], "peach-prairie/stored");
                assert!(step.get("codename_derived").is_none());
            }
        }
    }
}

/// First SSE `data:` frame of `path` whose JSON satisfies `pred`.
async fn first_frame(
    addr: std::net::SocketAddr,
    path: &str,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let reader =
        tokio_util::io::StreamReader::new(resp.bytes_stream().map_err(std::io::Error::other));
    let mut lines = tokio::io::BufReader::new(reader).lines();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let v: Value = serde_json::from_str(data.trim()).unwrap();
            if pred(&v) {
                return v;
            }
        }
        panic!("stream ended");
    })
    .await
    .expect("frame within timeout")
}

#[tokio::test]
async fn firehose_derives_names_for_an_active_legacy_run() {
    let (tmp, m) = setup();
    // The firehose's first pass attaches only to ACTIVE runs.
    let dir = tmp.path().join("runs").join(LEGACY);
    let mut rec: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap();
    rec["status"] = json!("running");
    std::fs::write(dir.join("run.json"), rec.to_string()).unwrap();
    let addr = spawn_server(tmp.path()).await;

    let step = first_frame(addr, "/api/events/stream", |v| {
        v["run_id"] == LEGACY && v["type"] == "step_started"
    })
    .await;
    assert_derived(&step, &m.step("alpha", "ag").to_string(), "firehose step");
    let unit = first_frame(addr, "/api/events/stream", |v| {
        v["run_id"] == LEGACY
            && v["type"] == "unit_started"
            && v["step_id"] == "review"
            && v["index"] == 2
    })
    .await;
    assert_derived(
        &unit,
        &m.panelist("review", "sec", Some(2)).to_string(),
        "firehose unit",
    );
}

/// A codename-era run (its run.json carries a codename).
const MODERN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2Y";
const MODERN_SUB: &str = "sub_01J9ZQ3K4M5N6P7Q8R9S0T1V09";

/// A codename-era run whose runner deliberately wrote codename-less records
/// and events: a SKIPPED linear agent step, an agent-less gate step, and a
/// `dispatch_started` without a codename (with a sub transcript on disk). The
/// snapshot names agents for all of them, so ANY legacy derivation — opening
/// a `LegacyNamer` for this run — would put derived names on the wire.
fn seed_modern(runs: &Path, status: &str) {
    const WF_MODERN: &str = r#"
name: modern-fixture
steps:
  - id: alpha
    agent: ag
    actions: []
    prompt: "a"
  - id: skipme
    agent: ag
    actions: []
    when: "false"
    prompt: "s"
  - id: gate
    approval:
      prompt: "ok?"
"#;
    let dir = runs.join(MODERN);
    std::fs::create_dir_all(&dir).unwrap();
    let mut rec = record(MODERN, Some("olive-pine"));
    rec["status"] = json!(status);
    std::fs::write(dir.join("run.json"), rec.to_string()).unwrap();
    std::fs::write(dir.join("workflow.yaml"), WF_MODERN).unwrap();
    write_lines(
        &dir.join("step_results.jsonl"),
        &[
            step_rec(
                "alpha",
                "linear",
                "run_MA",
                json!({"codename": "olive-pine/stored"}),
            ),
            step_rec(
                "skipme",
                "linear",
                "",
                json!({"skipped": true, "success": true}),
            ),
            step_rec("gate", "linear", "", json!({})),
        ],
    );
    write_lines(
        &dir.join("events.jsonl"),
        &[
            json!({"type": "step_started", "run_id": MODERN, "step_id": "alpha",
                   "kind": "linear", "agent": "ag", "codename": "olive-pine/stored"}),
            json!({"type": "dispatch_started", "run_id": MODERN, "sub_run_id": MODERN_SUB,
                   "agent": "scout", "transcript_path": "/t/msub.jsonl"}),
            json!({"type": "step_skipped", "run_id": MODERN, "step_id": "skipme",
                   "reason": "when false"}),
            json!({"type": "step_started", "run_id": MODERN, "step_id": "skipme",
                   "kind": "linear", "agent": "ag"}),
            json!({"type": "step_started", "run_id": MODERN, "step_id": "gate",
                   "kind": "linear"}),
        ],
    );
    let sub_t = runs
        .join("run_MA")
        .join("sub")
        .join(MODERN_SUB)
        .join("transcript.jsonl");
    std::fs::create_dir_all(sub_t.parent().unwrap()).unwrap();
    std::fs::write(
        &sub_t,
        format!(
            "{}\n",
            json!({"type": "run_start", "data": {"run_id": MODERN_SUB, "workspace_id": "ws",
                "agent": "scout", "provider": "anthropic", "model": "claude-x",
                "started_at": "2026-09-10T19:34:20Z", "mode": "bypass"}})
        ),
    )
    .unwrap();
}

/// No derived field, and no codename on the deliberately unnamed rows.
fn assert_untouched_modern(v: &Value) {
    assert_no_derived_flag_anywhere(v);
    let s = v.to_string();
    assert!(
        !s.contains("olive-pine/hedgehog"),
        "derived step name leaked: {s}"
    );
    assert!(!s.contains(">"), "derived sub-agent name leaked: {s}");
}

#[tokio::test]
async fn codename_era_run_never_gets_derived_names() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    seed_modern(&runs, "running");
    let addr = spawn_server(tmp.path()).await;

    let d = get(addr, &format!("/api/runs/{MODERN}")).await;
    assert_untouched_modern(&d["steps"]);
    assert!(
        d["steps"][1].get("codename").is_none(),
        "skipped step: {}",
        d["steps"][1]
    );
    assert!(d["steps"][2].get("codename").is_none(), "gate step");

    let g = get(addr, &format!("/api/runs/{MODERN}/graph")).await;
    for key in [
        "step_results",
        "units",
        "step_identities",
        "unit_identities",
        "subrun_identities",
    ] {
        assert_untouched_modern(&g[key]);
    }
    assert!(g["subrun_identities"]
        .get(MODERN_SUB)
        .is_none_or(|s| s.get("codename").is_none()));

    let rows = get(addr, "/api/events?limit=100").await;
    let mine: Vec<&Value> = rows
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["run_id"] == MODERN)
        .collect();
    assert_eq!(mine.len(), 5);
    for r in &mine {
        assert_untouched_modern(r);
    }
    let unnamed = mine.iter().filter(|r| r.get("codename").is_none()).count();
    assert_eq!(
        unnamed, 4,
        "only alpha's step_started carries a (stored) name"
    );

    // Per-run tail and the firehose: the last event (gate's step_started)
    // arrives untouched, as does the codename-less dispatch before it.
    for path in [
        format!("/api/runs/{MODERN}/log"),
        "/api/events/stream".to_string(),
    ] {
        let dispatch = first_frame(addr, &path, |v| {
            v["run_id"] == MODERN && v["type"] == "dispatch_started"
        })
        .await;
        assert!(dispatch.get("codename").is_none(), "{path}: {dispatch}");
        let gate = first_frame(addr, &path, |v| {
            v["run_id"] == MODERN && v["type"] == "step_started" && v["step_id"] == "skipme"
        })
        .await;
        assert_untouched_modern(&gate);
        assert!(gate.get("codename").is_none(), "{path}: {gate}");
    }
}

#[test]
fn legacy_fixture_has_no_codename_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    seed_legacy(&runs);
    for f in [
        "run.json",
        "step_results.jsonl",
        "unit_checkpoints.jsonl",
        "events.jsonl",
    ] {
        let body = std::fs::read_to_string(runs.join(LEGACY).join(f)).unwrap();
        assert!(!body.contains("codename"), "{f}");
    }
    // The store reads it as a legacy run.
    let store = RunStore::new(runs);
    assert!(store.load(LEGACY).unwrap().codename.is_none());
}
