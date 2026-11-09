use super::*;
use std::fs;
struct Fixture {
    _tmp: tempfile::TempDir,
    reads: SessionReads,
    ctx: ToolContext,
}
fn prologue(run: &str, workspace: &str) -> Value {
    json!({"type":"run_start","data":{"run_id":run,"workspace_id":workspace,"agent":"rupu-session","provider":"fixture","model":"fixture-model","started_at":"2026-11-09T00:00:00Z","system_prompt":"private prompt must not be returned"}})
}
fn write_lines(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
}
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let workspace = tmp.path().join("project");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    let transcript = global.join("transcripts/run_owner.jsonl");
    let owner = Owner {
        session_id: "ses_owner".into(),
        run_id: "run_owner".into(),
        workspace_id: "ws_owner".into(),
        workspace_path: workspace.clone(),
        project_root: Some(workspace.clone()),
        transcript_path: transcript.clone(),
    };
    let snapshot = ContextSnapshot {
        allowed_reads: [
            "session.context",
            "session.workflows",
            "run.inspect",
            "run.netflows",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        permission_mode: "readonly".into(),
        declared_tools: Some(vec![]),
        selected_skills: vec!["global:example".into()],
        preloaded_skills: vec![],
        settings_revision: 3,
        orchestration_revision: 4,
        builtin_tools_preset: None,
    };
    // Constructor must not need a transcript which assembly has yet to create.
    let reads = SessionReads::new(global, owner, snapshot).unwrap();
    write_lines(&transcript, &[prologue("run_owner", "ws_owner")]);
    let mut ctx = ToolContext::in_workspace(workspace.canonicalize().unwrap());
    ctx.workspace.id = "ws_owner".into();
    ctx.identity_mut().run_id = "run_owner".into();
    ctx.identity_mut().surface = Surface::Session;
    ctx.identity_mut().provider = "fixture".into();
    ctx.identity_mut().model = "fixture-model".into();
    Fixture {
        _tmp: tmp,
        reads,
        ctx,
    }
}
#[test]
fn immutable_context_requires_exact_live_turn_and_redacts_prompts() {
    let f = fixture();
    let output = f.reads.read(SessionReadRequest::Context, &f.ctx).unwrap();
    assert_eq!(output["selection"]["declared_tools"], json!([]));
    assert_eq!(output["selection"]["settings_revision"], 3);
    assert!(!output.to_string().contains("private prompt"));
    let mut ctx = f.ctx.clone();
    ctx.identity_mut().run_id = "run_other".into();
    assert!(f.reads.read(SessionReadRequest::Context, &ctx).is_err());
    ctx = f.ctx.clone();
    ctx.workspace.id = "ws_other".into();
    assert!(f.reads.read(SessionReadRequest::Context, &ctx).is_err());
    ctx = f.ctx.clone();
    ctx.identity_mut().surface = Surface::Agent;
    assert!(f.reads.read(SessionReadRequest::Context, &ctx).is_err());
    ctx = f.ctx.clone();
    ctx.identity_mut().parent = Some(Default::default());
    assert!(f.reads.read(SessionReadRequest::Context, &ctx).is_err());
}
#[test]
fn local_descendant_requires_unique_chain_and_matching_prologue() {
    let f = fixture();
    let child = f
        .reads
        .global
        .path
        .join("runs/run_owner/sub/sub_child/transcript.jsonl");
    write_lines(&child, &[prologue("sub_child", "ws_owner")]);
    let request = || SessionReadRequest::Inspect {
        ancestry: vec!["sub_child".into()],
    };
    assert!(f.reads.read(request(), &f.ctx).is_err());
    let started = json!({"t":"started","run_id":"sub_child","transcript":child,"name":"child"});
    let ledger = f.reads.global.path.join("runs/run_owner/children.jsonl");
    write_lines(&ledger, &[started.clone()]);
    assert_eq!(
        f.reads.read(request(), &f.ctx).unwrap()["prologue"]["run_id"],
        "sub_child"
    );
    write_lines(&child, &[prologue("sub_child", "ws_other")]);
    assert!(f.reads.read(request(), &f.ctx).is_err());
    write_lines(&child, &[prologue("sub_child", "ws_owner")]);
    write_lines(&ledger, &[started.clone(), started]);
    assert!(f.reads.read(request(), &f.ctx).is_err());
    assert!(f
        .reads
        .read(
            SessionReadRequest::Inspect {
                ancestry: vec!["run_owner".into()]
            },
            &f.ctx
        )
        .is_err());
}
#[test]
fn recorded_paths_cannot_escape_artifacts_or_follow_symlinks() {
    use std::os::unix::fs::symlink;
    let f = fixture();
    let source = f.reads.workspace.path.join("source.jsonl");
    write_lines(&source, &[prologue("sub_child", "ws_owner")]);
    let ledger = f.reads.global.path.join("runs/run_owner/children.jsonl");
    write_lines(
        &ledger,
        &[json!({"t":"started","run_id":"sub_child","transcript":source})],
    );
    let request = || SessionReadRequest::Inspect {
        ancestry: vec!["sub_child".into()],
    };
    assert!(f.reads.read(request(), &f.ctx).is_err());
    let link = f.reads.global.path.join("transcripts/sub_child.jsonl");
    symlink(&source, &link).unwrap();
    write_lines(
        &ledger,
        &[json!({"t":"started","run_id":"sub_child","transcript":link})],
    );
    assert!(f.reads.read(request(), &f.ctx).is_err());
}
#[test]
fn fifo_read_refuses_before_blocking_and_replaced_root_is_revoked() {
    let f = fixture();
    fs::remove_file(&f.reads.owner.transcript_path).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .arg(&f.reads.owner.transcript_path)
        .status()
        .unwrap()
        .success());
    assert!(f.reads.read(SessionReadRequest::Context, &f.ctx).is_err());
    let g = fixture();
    let old = g.reads.global.path.with_extension("old");
    fs::rename(&g.reads.global.path, old).unwrap();
    fs::create_dir(&g.reads.global.path).unwrap();
    assert!(g.reads.read(SessionReadRequest::Context, &g.ctx).is_err());
}
#[test]
fn record_and_physical_line_bounds_are_visible_not_whole_file_reads() {
    let f = fixture();
    let mut rows = vec![prologue("run_owner", "ws_owner")];
    rows.extend(
        (0..MAX_RECORDS + 20)
            .map(|_| json!({"type":"assistant_delta","data":{"content":"not returned"}})),
    );
    write_lines(&f.reads.owner.transcript_path, &rows);
    let v = f
        .reads
        .read(SessionReadRequest::Inspect { ancestry: vec![] }, &f.ctx)
        .unwrap();
    assert_eq!(v["partial"], true);
    assert!(v["event_counts"]["assistant_delta"].as_u64().unwrap() < MAX_RECORDS as u64);
    assert!(!v.to_string().contains("not returned"));
    let mut bytes = format!("{}\n", prologue("run_owner", "ws_owner")).into_bytes();
    bytes.extend(vec![b'x'; MAX_LINE + 100]);
    fs::write(&f.reads.owner.transcript_path, bytes).unwrap();
    assert_eq!(
        f.reads
            .read(SessionReadRequest::Inspect { ancestry: vec![] }, &f.ctx)
            .unwrap()["partial"],
        true
    );
}
fn link(session: &str, run: &str) -> Value {
    json!({"association":{"session_id":session,"workspace_id":"ws_owner","host_id":"local","run_id":run,"workflow_name":"review","status":"running","launched_at":"2026-11-09T00:00:00Z"},"monitor_pid":0})
}
#[test]
fn workflow_links_have_exact_owner_and_bounded_live_pages() {
    let f = fixture();
    let root = f.reads.global.path.join("sessions/ses_owner/workflow-runs");
    fs::create_dir_all(&root).unwrap();
    for n in 0..55 {
        let run = format!("run_{n:04}");
        fs::write(
            root.join(format!("{run}.json")),
            link("ses_owner", &run).to_string(),
        )
        .unwrap();
    }
    let p = f
        .reads
        .read(
            SessionReadRequest::Workflows {
                offset: 0,
                limit: 25,
            },
            &f.ctx,
        )
        .unwrap();
    assert_eq!(p["rows"].as_array().unwrap().len(), 25);
    assert_eq!(p["next_offset"], 25);
    assert_eq!(p["status_evidence"], "stored_association_not_live");
    fs::write(
        root.join("run_0000.json"),
        link("ses_other", "run_0000").to_string(),
    )
    .unwrap();
    assert!(f
        .reads
        .read(
            SessionReadRequest::Workflows {
                offset: 0,
                limit: 25
            },
            &f.ctx
        )
        .is_err());
}
#[test]
fn thousand_link_inventory_stops_without_fabricated_continuation() {
    let f = fixture();
    let root = f.reads.global.path.join("sessions/ses_owner/workflow-runs");
    fs::create_dir_all(&root).unwrap();
    for n in 0..1002 {
        let run = format!("run_{n:04}");
        fs::write(
            root.join(format!("{run}.json")),
            link("ses_owner", &run).to_string(),
        )
        .unwrap();
    }
    let page = f
        .reads
        .read(
            SessionReadRequest::Workflows {
                offset: 0,
                limit: 10,
            },
            &f.ctx,
        )
        .unwrap();
    assert_eq!(page["partial"], true);
    assert!(page["next_offset"].is_null());
    assert_eq!(page["rows"].as_array().unwrap().len(), 10);
}
#[test]
fn netflow_summary_omits_paths_headers_payload_and_rejects_other_owner() {
    let f = fixture();
    let path = f.reads.netflow_root.join("run_owner.jsonl");
    let flow = json!({"type":"flow","ctx":{"run_id":"run_owner","workspace_id":"ws_owner"},"host":"example.invalid","port":443,"outcome":"ok","fidelity":"http","path":"/secret/path","headers":{"authorization":"fixture-secret"},"bytes_in":9000000,"body_complete":false});
    write_lines(
        &path,
        &[
            flow.clone(),
            flow.clone(),
            json!({"type":"capture","state":{"unavailable":{"reason":"fixture-private"}}}),
        ],
    );
    let request = || SessionReadRequest::Netflows { ancestry: vec![] };
    let value = f.reads.read(request(), &f.ctx).unwrap();
    assert_eq!(value["rows"][0]["count"], 2);
    let text = value.to_string();
    assert!(!text.contains("secret"));
    assert!(!text.contains("9000000"));
    assert!(!text.contains("fixture-private"));
    let mut wrong = flow;
    wrong["ctx"]["run_id"] = json!("run_other");
    write_lines(&path, &[wrong]);
    assert!(f.reads.read(request(), &f.ctx).is_err());
}
#[test]
fn absent_netflow_is_unavailable_not_zero_traffic() {
    let f = fixture();
    let value = f
        .reads
        .read(SessionReadRequest::Netflows { ancestry: vec![] }, &f.ctx)
        .unwrap();
    assert_eq!(value["state"], "unavailable");
    assert_eq!(value["partial"], true);
}

#[test]
fn canonical_completion_records_require_the_exact_run() {
    let f = fixture();
    let complete = rupu_transcript::Event::RunComplete {
        run_id: "run_owner".into(),
        status: rupu_transcript::RunStatus::Ok,
        total_tokens: 12,
        duration_ms: 30,
        error: None,
        outcome: None,
    };
    let row = serde_json::to_value(complete).unwrap();
    write_lines(
        &f.reads.owner.transcript_path,
        &[prologue("run_owner", "ws_owner"), row.clone()],
    );
    let request = || SessionReadRequest::Inspect { ancestry: vec![] };
    assert_eq!(
        f.reads.read(request(), &f.ctx).unwrap()["observed_terminal_status"],
        "ok"
    );
    let mut wrong = row;
    wrong["data"]["run_id"] = json!("run_other");
    write_lines(
        &f.reads.owner.transcript_path,
        &[prologue("run_owner", "ws_owner"), wrong],
    );
    assert!(f.reads.read(request(), &f.ctx).is_err());
}

#[test]
fn canonical_flow_envelope_is_projected_without_payload() {
    use rupu_netflow::{Fidelity, FlowRecord, LedgerLine, Outcome};
    let f = fixture();
    let mut ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Provider("fixture".into()));
    ctx.run_id = Some("run_owner".into());
    ctx.workspace_id = Some("ws_owner".into());
    let flow = FlowRecord {
        id: ulid::Ulid::new(),
        ts: chrono::Utc::now(),
        ctx,
        fidelity: Fidelity::Http,
        method: "GET".into(),
        scheme: "https".into(),
        host: "example.invalid".into(),
        port: 443,
        path: "/private-path".into(),
        peer_ip: None,
        resolved_ips: vec![],
        process: None,
        local_addr: None,
        direction: None,
        http_version: None,
        status: Some(200),
        outcome: Outcome::Ok,
        error: None,
        bytes_out: Some(0),
        bytes_in: Some(5),
        body_complete: false,
        ttfb_ms: None,
        duration_ms: None,
    };
    write_lines(
        &f.reads.netflow_root.join("run_owner.jsonl"),
        &[serde_json::to_value(LedgerLine::Flow(Box::new(flow.clone()))).unwrap()],
    );
    let value = f
        .reads
        .read(SessionReadRequest::Netflows { ancestry: vec![] }, &f.ctx)
        .unwrap();
    assert_eq!(value["rows"][0]["host"], "example.invalid");
    assert_eq!(value["rows"][0]["count"], 1);
    assert!(!value.to_string().contains("private-path"));
    assert_eq!(value["legacy_owner_rows"], 0);
    assert_eq!(value["ownership_evidence"], "record_annotations");
    assert_eq!(value["partial"], false);

    // Canonical FlowCtx serialization omits None identity fields. The file
    // path remains scoped, but that is weaker evidence than row annotations.
    let annotated = serde_json::to_value(LedgerLine::Flow(Box::new(flow.clone()))).unwrap();
    let mut legacy = flow;
    legacy.ctx.run_id = None;
    legacy.ctx.workspace_id = None;
    let legacy = serde_json::to_value(LedgerLine::Flow(Box::new(legacy))).unwrap();
    assert!(legacy["ctx"].get("run_id").is_none());
    let path = f.reads.netflow_root.join("run_owner.jsonl");
    write_lines(&path, &[legacy.clone()]);
    let read = || {
        f.reads
            .read(SessionReadRequest::Netflows { ancestry: vec![] }, &f.ctx)
            .unwrap()
    };
    let only_legacy = read();
    assert_eq!(only_legacy["legacy_owner_rows"], 1);
    assert_eq!(only_legacy["ownership_evidence"], "artifact_path_only");
    assert_eq!(only_legacy["partial"], true);
    let mut explicit_null = legacy.clone();
    explicit_null["ctx"]["run_id"] = Value::Null;
    explicit_null["ctx"]["workspace_id"] = json!("ws_owner");
    write_lines(&path, &[annotated, legacy, explicit_null]);
    let mixed = read();
    assert_eq!(mixed["legacy_owner_rows"], 2);
    assert_eq!(mixed["ownership_evidence"], "mixed");
    assert_eq!(mixed["partial"], true);
}

#[test]
fn netflow_context_must_be_an_object_with_exact_string_or_legacy_owner_fields() {
    let f = fixture();
    let path = f.reads.netflow_root.join("run_owner.jsonl");
    let base = json!({"type":"flow","ctx":{"run_id":"run_owner","workspace_id":"ws_owner"},"host":"example.invalid","port":443,"outcome":"ok","fidelity":"http"});
    for bad_context in [Value::Null, json!([]), json!("run_owner"), json!(42)] {
        let mut row = base.clone();
        row["ctx"] = bad_context;
        write_lines(&path, &[row]);
        assert!(f
            .reads
            .read(SessionReadRequest::Netflows { ancestry: vec![] }, &f.ctx)
            .is_err());
    }
    for field in ["run_id", "workspace_id"] {
        for bad_identity in [
            json!(42),
            json!(true),
            json!([]),
            json!({}),
            json!("other_owner"),
        ] {
            let mut row = base.clone();
            row["ctx"][field] = bad_identity;
            write_lines(&path, &[row]);
            assert!(f
                .reads
                .read(SessionReadRequest::Netflows { ancestry: vec![] }, &f.ctx)
                .is_err());
        }
    }
}

#[test]
fn netflow_drop_counts_require_unsigned_integer_evidence() {
    let f = fixture();
    let path = f.reads.netflow_root.join("run_owner.jsonl");
    let valid = serde_json::to_value(rupu_netflow::LedgerLine::Dropped {
        count: 7,
        ts: chrono::Utc::now(),
    })
    .unwrap();
    write_lines(&path, &[valid]);
    let observed = f
        .reads
        .read(SessionReadRequest::Netflows { ancestry: vec![] }, &f.ctx)
        .unwrap();
    assert_eq!(observed["recorded_drop_markers"], 7);
    for count in [Value::Null, json!("7"), json!(-1), json!(1.5), json!(true)] {
        write_lines(&path, &[json!({"type":"dropped","count":count})]);
        assert!(f
            .reads
            .read(SessionReadRequest::Netflows { ancestry: vec![] }, &f.ctx)
            .is_err());
    }
    write_lines(&path, &[json!({"type":"dropped"})]);
    assert!(f
        .reads
        .read(SessionReadRequest::Netflows { ancestry: vec![] }, &f.ctx)
        .is_err());
}

#[test]
fn duplicate_owner_keys_are_rejected_instead_of_last_value_wins() {
    let f = fixture();
    let bytes = b"{\"type\":\"run_start\",\"data\":{\"run_id\":\"run_other\",\"run_id\":\"run_owner\",\"workspace_id\":\"ws_owner\",\"agent\":\"x\",\"provider\":\"x\",\"model\":\"x\",\"started_at\":\"x\"}}\n";
    fs::write(&f.reads.owner.transcript_path, bytes).unwrap();
    assert!(f.reads.read(SessionReadRequest::Context, &f.ctx).is_err());
}

#[test]
fn exact_binding_authorization_is_validated_and_enforced_on_direct_calls() {
    let mut f = fixture();
    f.reads.snapshot.allowed_reads = vec!["session.context".into()];
    assert!(f.reads.permits("session.context"));
    assert!(!f.reads.permits("run.inspect"));
    assert!(f.reads.read(SessionReadRequest::Context, &f.ctx).is_ok());
    for request in [
        SessionReadRequest::Workflows {
            offset: 0,
            limit: 1,
        },
        SessionReadRequest::Inspect { ancestry: vec![] },
        SessionReadRequest::Netflows { ancestry: vec![] },
    ] {
        assert!(f
            .reads
            .read(request, &f.ctx)
            .unwrap_err()
            .to_string()
            .contains("not authorized"));
    }
    for allowed in [
        vec!["session.*".into()],
        vec!["session.context".into(), "session.context".into()],
        vec!["operator.admin".into()],
    ] {
        let mut snapshot = f.reads.snapshot.clone();
        snapshot.allowed_reads = allowed;
        assert!(
            SessionReads::new(f.reads.global.path.clone(), f.reads.owner.clone(), snapshot)
                .is_err()
        );
    }
}

#[test]
fn trusted_root_aliases_normalize_future_and_recorded_descendant_paths_only() {
    use std::os::unix::fs::symlink;
    let f = fixture();
    let alias = f._tmp.path().join("global-alias");
    symlink(&f.reads.global.path, &alias).unwrap();
    let mut owner = f.reads.owner.clone();
    owner.transcript_path = alias.join("transcripts/run_owner.jsonl");
    // The only source of alias authority is constructor input; the target
    // transcript is not canonicalized or opened during construction.
    let reads = SessionReads::new(alias.clone(), owner, f.reads.snapshot.clone()).unwrap();
    assert_eq!(reads.owner.transcript_path, f.reads.owner.transcript_path);
    assert!(reads.read(SessionReadRequest::Context, &f.ctx).is_ok());
    let child_relative = Path::new("runs/run_owner/sub/sub_child/transcript.jsonl");
    write_lines(
        &f.reads.global.path.join(child_relative),
        &[prologue("sub_child", "ws_owner")],
    );
    let ledger = f.reads.global.path.join("runs/run_owner/children.jsonl");
    write_lines(
        &ledger,
        &[json!({"t":"started","run_id":"sub_child","transcript":alias.join(child_relative)})],
    );
    let inspect = || SessionReadRequest::Inspect {
        ancestry: vec!["sub_child".into()],
    };
    assert_eq!(
        reads.read(inspect(), &f.ctx).unwrap()["prologue"]["run_id"],
        "sub_child"
    );
    let undeclared_alias = f._tmp.path().join("not-trusted");
    symlink(&f.reads.global.path, &undeclared_alias).unwrap();
    write_lines(
        &ledger,
        &[
            json!({"t":"started","run_id":"sub_child","transcript":undeclared_alias.join(child_relative)}),
        ],
    );
    assert!(reads.read(inspect(), &f.ctx).is_err());
    write_lines(
        &ledger,
        &[
            json!({"t":"started","run_id":"sub_child","transcript":alias.join("runs/../runs/run_owner/sub/sub_child/transcript.jsonl")}),
        ],
    );
    assert!(reads.read(inspect(), &f.ctx).is_err());
    let mut future = f.reads.owner.clone();
    future.transcript_path = alias.join("transcripts/run_not_created_yet.jsonl");
    assert!(SessionReads::new(alias, future, f.reads.snapshot.clone()).is_ok());
}

#[test]
fn lexical_worker_workspace_alias_matches_pinned_owner_but_retarget_fails() {
    use std::os::unix::fs::symlink;
    let f = fixture();
    let alias = f._tmp.path().join("worker-workspace-alias");
    symlink(&f.reads.owner.workspace_path, &alias).unwrap();
    let mut owner = f.reads.owner.clone();
    owner.workspace_path = alias.clone();
    let reads =
        SessionReads::new(f.reads.global.path.clone(), owner, f.reads.snapshot.clone()).unwrap();
    let mut ctx = f.ctx.clone();
    ctx.workspace.path = alias.clone();
    let value = reads.read(SessionReadRequest::Context, &ctx).unwrap();
    assert_eq!(value["session_id"], "ses_owner");
    assert_eq!(value["run_id"], "run_owner");
    assert_eq!(value["workspace_id"], "ws_owner");
    assert_eq!(value["host_id"], "local");

    // Same lexical binding, different physical owner: never reuse authority.
    let foreign = f._tmp.path().join("foreign-workspace");
    fs::create_dir(&foreign).unwrap();
    fs::remove_file(&alias).unwrap();
    symlink(&foreign, &alias).unwrap();
    assert!(reads.read(SessionReadRequest::Context, &ctx).is_err());
    fs::remove_file(&alias).unwrap();
    assert!(reads.read(SessionReadRequest::Context, &ctx).is_err());
}
