use super::*;
use rupu_launch::ledger::{LedgerLine, Started};
use serde_json::json;
use std::{fs, path::Path as FsPath};

const FLOW: &str = "af_observation_test";
fn fixture() -> (tempfile::TempDir, AppState, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(dir.path().into(), rupu_config::PricingConfig::default());
    let root = dir.path().join("agentiflows").join(FLOW);
    let record = AgentiflowRecord {
        id: FLOW.into(),
        name: "test".into(),
        engagement_profiles: vec![],
        trigger: rupu_runtime::RunTriggerSource::Agentiflow,
        status: "completed".into(),
        stop_reason: Some("goals_met".into()),
        rounds: 1,
        goals: vec![],
        started_at: Utc::now(),
        ended_at: Some(Utc::now()),
        codename: None,
        spent_usd: None,
        spent_tokens: 0,
        runner_pid: None,
    };
    record.write(&root).unwrap();
    (dir, state, root)
}
fn write(path: impl AsRef<FsPath>, bytes: impl AsRef<[u8]>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
fn query(token: &str, section: &str, cursor: Option<String>) -> PageQuery {
    PageQuery {
        host: None,
        snapshot: token.into(),
        section: section.into(),
        cursor,
        limit: Some(100),
    }
}

#[test]
fn thousand_units_have_finite_pages_and_compact_summary() {
    let (_dir, state, root) = fixture();
    let mut ledger = String::new();
    for i in 0..1501 {
        let started = Started::new(
            &format!("h_{i}"),
            rupu_tools::ChildKind::Agent,
            "worker",
            &format!("run_{i:05}"),
            0,
            1,
        );
        ledger.push_str(&serde_json::to_string(&LedgerLine::Started(started)).unwrap());
        ledger.push('\n');
    }
    write(root.join("children.jsonl"), ledger);
    let captured = capture::capture(&state, FLOW).unwrap();
    assert_eq!(captured.summary["sections"]["units"]["total"], 1501);
    assert!(captured.summary.get("units").is_none());
    assert!(serde_json::to_vec(&captured.summary).unwrap().len() < 8192);
    let mut seen = std::collections::BTreeSet::new();
    let mut cursor = None;
    loop {
        let page = captured.page("units", cursor.as_deref(), 100).unwrap();
        let rows = page["records"].as_array().unwrap();
        assert!(rows.len() <= 100);
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_PAGE);
        for row in rows {
            assert!(seen.insert(row["id"].as_str().unwrap().to_owned()));
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 1501);
}

#[test]
fn pages_remain_frozen_after_source_is_removed() {
    let (_dir, state, root) = fixture();
    write(
        root.join("events.jsonl"),
        b"{\"kind\":\"round\",\"budget\":\"ok\"}\n",
    );
    let captured = capture::capture(&state, FLOW).unwrap();
    fs::remove_dir_all(root).unwrap();
    let page = captured.page("events", None, 100).unwrap();
    assert_eq!(page["records"][0]["source"], "coordinator");
    assert_eq!(captured.summary["budget_state"], "ok");
}

#[test]
fn cursor_is_bound_to_snapshot_section_and_offset() {
    let (_dir, state, root) = fixture();
    write(
        root.join("events.jsonl"),
        b"{\"kind\":\"a\"}\n{\"kind\":\"b\"}\n",
    );
    let captured = capture::capture(&state, FLOW).unwrap();
    let second = capture::capture(&state, FLOW).unwrap();
    let page = captured.page("events", None, 1).unwrap();
    let cursor = page["next_cursor"].as_str().unwrap();
    assert_eq!(
        captured.page("events", Some(cursor), 1).unwrap()["offset"],
        1
    );
    assert_eq!(
        captured.page("units", Some(cursor), 1).unwrap_err().0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        second.page("events", Some(cursor), 1).unwrap_err().0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        captured
            .page("events", Some(&cursor.replacen("1.", "2.", 1)), 1)
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
}

#[test]
fn malformed_and_partial_source_evidence_is_preserved() {
    let (_dir, state, root) = fixture();
    write(
        root.join("events.jsonl"),
        b"{\"kind\":\"ok\"}\nnot-json\n{\"kind\":\"partial\"}",
    );
    let captured = capture::capture(&state, FLOW).unwrap();
    assert_eq!(captured.summary["sections"]["events"]["total"], 1);
    assert_eq!(captured.summary["evidence"]["malformed_records"], 1);
    assert_eq!(captured.summary["evidence"]["incomplete_lines"], 1);
    assert_eq!(
        captured.page("events", None, 100).unwrap()["evidence"],
        captured.summary["evidence"]
    );
}

#[test]
fn sparse_oversized_inputs_refuse_without_loading() {
    let (_dir, state, root) = fixture();
    fs::File::create(root.join("children.jsonl"))
        .unwrap()
        .set_len(17 * 1024 * 1024)
        .unwrap();
    assert!(matches!(
        capture::capture(&state, FLOW),
        Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, _))
    ));
}

#[test]
fn lead_paths_are_paged_with_only_bounded_latest_prologue_read() {
    let (_dir, state, root) = fixture();
    write(
        root.join("lead/transcript.r1.jsonl"),
        b"not transcript bytes",
    );
    fs::File::create(root.join("lead/transcript.r2.jsonl"))
        .unwrap()
        .set_len(1024 * 1024 * 1024)
        .unwrap();
    let captured = capture::capture(&state, FLOW).unwrap();
    assert_eq!(captured.summary["latest_lead"]["round"], 2);
    assert_eq!(captured.page("leads", None, 100).unwrap()["total"], 2);
}

#[test]
fn legacy_unit_output_is_available_on_demand_not_repeated_in_pages() {
    let (_dir, state, root) = fixture();
    write(
        root.join("units/run_legacy/unit.json"),
        json!({"agent":"worker", "status":{"state":"done","success":true,"output":"answer"}})
            .to_string(),
    );
    let captured = capture::capture(&state, FLOW).unwrap();
    let page = captured.page("units", None, 100).unwrap();
    assert_eq!(
        page["records"][0]["value"]["output_recorded_in_capture"],
        true
    );
    assert!(page["records"][0]["value"]["status"]["output"].is_null());
}

#[cfg(unix)]
#[test]
fn symlinked_capture_subdirectories_are_refused() {
    let (_dir, state, root) = fixture();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("board")).unwrap();
    assert!(matches!(
        capture::capture(&state, FLOW),
        Err(ApiError(StatusCode::CONFLICT, _))
    ));
}

#[tokio::test]
async fn endpoint_cache_refuses_other_flow_expiry_and_remote_scope() {
    let (_dir, state, _root) = fixture();
    let captured = capture::capture(&state, FLOW).unwrap();
    let token = captured.token.clone();
    state
        .agentiflow_observations
        .snapshots
        .lock()
        .unwrap()
        .push(Arc::new(captured));
    assert_eq!(
        page(
            State(state.clone()),
            Path("af_other".into()),
            Query(query(&token, "units", None))
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    let mut remote = query(&token, "units", None);
    remote.host = Some("remote".into());
    assert_eq!(
        page(State(state.clone()), Path(FLOW.into()), Query(remote))
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    Arc::get_mut(&mut state.agentiflow_observations.snapshots.lock().unwrap()[0])
        .unwrap()
        .born = Instant::now() - TTL - Duration::from_secs(1);
    assert_eq!(
        page(
            State(state),
            Path(FLOW.into()),
            Query(query(&token, "units", None))
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn capture_reuse_and_explicit_refresh_are_distinct() {
    let (_dir, state, root) = fixture();
    let first = snapshot(
        State(state.clone()),
        Path(FLOW.into()),
        Query(SnapshotQuery::default()),
    )
    .await
    .unwrap()
    .0;
    write(root.join("events.jsonl"), b"{\"kind\":\"later\"}\n");
    let cached = snapshot(
        State(state.clone()),
        Path(FLOW.into()),
        Query(SnapshotQuery::default()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(first["snapshot"], cached["snapshot"]);
    let fresh = snapshot(
        State(state),
        Path(FLOW.into()),
        Query(SnapshotQuery {
            host: None,
            refresh: true,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_ne!(first["snapshot"], fresh["snapshot"]);
    assert_eq!(fresh["sections"]["events"]["total"], 1);
}

#[tokio::test]
async fn page_limits_and_unknown_parameters_are_not_silently_ignored() {
    let (_dir, state, _) = fixture();
    let mut q = query("unknown", "units", None);
    q.limit = Some(101);
    assert_eq!(
        page(State(state), Path(FLOW.into()), Query(q))
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    assert!(serde_json::from_value::<PageQuery>(
        json!({"snapshot":"x","section":"units","after":"wrong"})
    )
    .is_err());
}

#[test]
fn invalid_utf8_and_partial_ledger_lines_do_not_erase_valid_units() {
    let (_dir, state, root) = fixture();
    let line = LedgerLine::Started(Started::new(
        "h_valid",
        rupu_tools::ChildKind::Agent,
        "worker",
        "run_valid",
        0,
        1,
    ));
    let mut bytes = serde_json::to_vec(&line).unwrap();
    bytes.extend_from_slice(b"\n\xff\n");
    let partial = LedgerLine::Started(Started::new(
        "h_partial",
        rupu_tools::ChildKind::Agent,
        "worker",
        "run_partial",
        0,
        1,
    ));
    bytes.extend_from_slice(&serde_json::to_vec(&partial).unwrap());
    write(root.join("children.jsonl"), bytes);
    let captured = capture::capture(&state, FLOW).unwrap();
    assert_eq!(captured.summary["sections"]["units"]["total"], 1);
    assert_eq!(captured.summary["evidence"]["malformed_records"], 1);
    assert_eq!(captured.summary["evidence"]["incomplete_lines"], 1);
}

#[tokio::test]
async fn busy_capture_does_not_block_existing_pages_or_queue_workers() {
    let (_dir, state, _root) = fixture();
    let c = capture::capture(&state, FLOW).unwrap();
    let token = c.token.clone();
    state
        .agentiflow_observations
        .snapshots
        .lock()
        .unwrap()
        .push(Arc::new(c));
    let _permit = state
        .agentiflow_observations
        .capture_gate
        .clone()
        .try_acquire_owned()
        .unwrap();
    assert_eq!(
        snapshot(
            State(state.clone()),
            Path(FLOW.into()),
            Query(SnapshotQuery {
                host: None,
                refresh: true
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        page(
            State(state),
            Path(FLOW.into()),
            Query(query(&token, "units", None))
        )
        .await
        .unwrap()
        .0["total"],
        0
    );
}

#[cfg(unix)]
#[test]
fn captured_root_anchor_survives_path_replacement_without_following_symlink() {
    let (_dir, state, root) = fixture();
    write(root.join("events.jsonl"), b"original\n");
    write(root.join("lead/transcript.r1.jsonl"), b"");
    let outside = tempfile::tempdir().unwrap();
    write(outside.path().join("events.jsonl"), b"outside\n");
    write(outside.path().join("lead/transcript.r99.jsonl"), b"");
    let mut input = super::input::Input::new(&state.global_dir, FLOW).unwrap();
    fs::rename(&root, root.with_file_name("retained_original")).unwrap();
    std::os::unix::fs::symlink(outside.path(), &root).unwrap();
    assert_eq!(
        input
            .copy(FsPath::new("events.jsonl"), 1024)
            .unwrap()
            .unwrap(),
        b"original\n"
    );
    let entries = input.entries(FsPath::new("lead")).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "transcript.r1.jsonl");
}

#[test]
fn remaining_input_budget_is_checked_before_reading_another_file() {
    let (_dir, state, root) = fixture();
    write(root.join("events.jsonl"), b"12345");
    let mut input = super::input::Input::new(&state.global_dir, FLOW).unwrap();
    input.bytes = 32 * 1024 * 1024 - 4;
    assert_eq!(
        input.copy(FsPath::new("events.jsonl"), 1024).unwrap_err().0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(input.bytes, 32 * 1024 * 1024 - 4);
}

#[test]
fn legacy_board_work_is_refused_before_quadratic_fold() {
    let (_dir, state, root) = fixture();
    write(
        root.join("board/claims.jsonl"),
        "{\"key\":\"test\"}\n".repeat(4001),
    );
    assert!(matches!(
        capture::capture(&state, FLOW),
        Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, _))
    ));
}

fn run_start() -> Value {
    json!({"type":"run_start","data":{"run_id":"run_actual_lead","workspace_id":"ws_actual", "agent":"lead","provider":"fixture","model":"fixture","started_at":"2026-11-08T00:00:00Z","mode":"readonly"}})
}
#[test]
fn latest_lead_proof_reads_first_record_of_huge_file_and_binds_exact_handle() {
    let (_dir, state, root) = fixture();
    let path = root.join("lead/transcript.r1.jsonl");
    let event = run_start();
    write(&path, format!("{}\n", event));
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(1024 * 1024 * 1024)
        .unwrap();
    let captured = capture::capture(&state, FLOW).unwrap();
    let proof = &captured.summary["latest_lead"]["prologue"];
    assert_eq!(proof["status"], "verified");
    assert_eq!(proof["run_start"], event);
    assert_eq!(proof["flow_id"], FLOW);
    assert_eq!(proof["host_id"], "local");
    assert_eq!(proof["path"], path.to_string_lossy().as_ref());
    assert!(
        captured.summary["evidence"]["input_bytes"]
            .as_u64()
            .unwrap()
            < 8192
    );
    assert_eq!(
        captured.page("leads", None, 100).unwrap()["records"][0]["value"]["prologue"],
        *proof
    );
}
#[test]
fn latest_lead_proof_does_not_skip_bad_first_record_or_infer_from_path() {
    let (_dir, state, root) = fixture();
    let path = root.join("lead/transcript.r1.jsonl");
    for (bytes, status) in [
        (format!("bad\n{}\n", run_start()).into_bytes(), "malformed"),
        (run_start().to_string().into_bytes(), "incomplete"),
        (vec![b'x'; 256 * 1024 + 1], "too_large"),
    ] {
        write(&path, bytes);
        let captured = capture::capture(&state, FLOW).unwrap();
        assert_eq!(
            captured.summary["latest_lead"]["prologue"]["status"],
            status
        );
        assert!(captured.summary["latest_lead"]["prologue"]["run_start"].is_null());
    }
}
#[test]
fn latest_lead_proof_changes_generation_and_refuses_conflicting_identity() {
    let (_dir, state, root) = fixture();
    let path = root.join("lead/transcript.r1.jsonl");
    write(&path, format!("{}\n", run_start()));
    let first = capture::capture(&state, FLOW).unwrap();
    let mut other = run_start();
    other["data"]["workspace_id"] = json!("ws_other");
    write(&path, format!("{}\n", other));
    let second = capture::capture(&state, FLOW).unwrap();
    assert_ne!(
        first.summary["source_generation"],
        second.summary["source_generation"]
    );
    other["host_id"] = json!("remote");
    write(&path, format!("{}\n", other));
    assert_eq!(
        capture::capture(&state, FLOW).unwrap().summary["latest_lead"]["prologue"]["status"],
        "malformed"
    );
}
