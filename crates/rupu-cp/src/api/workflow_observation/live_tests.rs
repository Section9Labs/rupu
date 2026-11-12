use super::*;
use std::io::Write;

#[test]
fn metadata_frontier_does_not_parse_or_read_large_log_bodies() {
    let (_tmp, s, path) = fixture();
    let file = fs::File::create(path.join("events.jsonl")).unwrap();
    file.set_len(128 * 1024 * 1024).unwrap();
    let first = frontier::read(&s, ID, WS, None).unwrap();
    assert_eq!(first["changed"], true);
    assert!(first["input_bytes"].as_u64().unwrap() < 64 * 1024);
    let second = frontier::read(&s, ID, WS, first["frontier"].as_str()).unwrap();
    assert_eq!(second["changed"], false);
    assert_eq!(second["frontier"], first["frontier"]);
}
#[test]
fn frontier_detects_new_files_status_resume_and_instance_change() {
    let (tmp, s, path) = fixture();
    let a = frontier::read(&s, ID, WS, None).unwrap();
    fs::write(
        path.join("events.jsonl"),
        lines(&[json!({"type":"run_paused","run_id":ID})]),
    )
    .unwrap();
    let b = frontier::read(&s, ID, WS, a["frontier"].as_str()).unwrap();
    assert_eq!(b["changed"], true);
    let mut run: Value = serde_json::from_slice(&fs::read(path.join("run.json")).unwrap()).unwrap();
    run["status"] = json!("running");
    fs::write(path.join("run.json"), serde_json::to_vec(&run).unwrap()).unwrap();
    let c = frontier::read(&s, ID, WS, b["frontier"].as_str()).unwrap();
    assert_eq!(c["changed"], true);
    assert_eq!(c["run"]["status"], "running");
    let other = AppState::new(
        tmp.path().canonicalize().unwrap(),
        rupu_config::PricingConfig::default(),
    );
    assert_ne!(
        frontier::read(&other, ID, WS, None).unwrap()["frontier"],
        c["frontier"]
    );
}
#[test]
fn capture_frontier_never_acknowledges_a_later_append() {
    let (_tmp, s, path) = fixture();
    let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
    assert_eq!(
        frontier::read(&s, ID, WS, c.header["frontier"].as_str()).unwrap()["changed"],
        false
    );
    fs::write(
        path.join("events.jsonl"),
        lines(&[json!({"type":"run_paused","run_id":ID})]),
    )
    .unwrap();
    assert_eq!(
        frontier::read(&s, ID, WS, c.header["frontier"].as_str()).unwrap()["changed"],
        true
    );
    assert_eq!(
        c.page("events", None, None, 50).unwrap()["captured_total"],
        0
    );
}
#[test]
fn latest_reaches_complete_activity_after_the_prefix_limit_with_absolute_offsets() {
    let (_tmp, s, path) = fixture();
    let mut file = fs::File::create(path.join("events.jsonl")).unwrap();
    file.write_all(&vec![b'x'; input::MAX_FILE]).unwrap();
    file.write_all(b"\n").unwrap();
    let row = lines(&[json!({"type":"run_paused","run_id":ID})]);
    file.write_all(&row).unwrap();
    drop(file);
    let prefix = capture::capture_window(&s, ID, WS, Window::Prefix).unwrap();
    assert_eq!(
        prefix.page("events", None, None, 50).unwrap()["records"][0]["state"],
        "partial"
    );
    let latest = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
    let page = latest.page("events", None, None, 50).unwrap();
    assert_eq!(page["window"], "latest");
    assert_eq!(page["records"].as_array().unwrap().len(), 1);
    assert_eq!(
        page["records"][0]["record_id"],
        format!("event:{}", input::MAX_FILE + 1)
    );
    assert_eq!(page["records"][0]["state"], "available");
    assert_eq!(latest.header["sections"]["events"]["head_truncated"], true);
    assert_eq!(latest.header["sections"]["events"]["tail_truncated"], false);
    assert!(page["source_total"].is_null());
    assert_eq!(page["source_complete"], false);
}
#[test]
fn latest_boundary_probe_never_decodes_a_cut_record_or_trailing_partial() {
    let (_tmp, _s, path) = fixture();
    fs::write(
        path.join("events.jsonl"),
        b"first-line\nsecond-line\nunfinished",
    )
    .unwrap();
    let mut input = input::Input::open(&path).unwrap();
    input.window = Window::Latest;
    let r = input.read("events.jsonl", 23, true).unwrap().unwrap();
    assert!(r.head_truncated);
    assert_eq!(r.read_bytes, 23);
    assert!(r.leading_skipped_bytes > 0);
    assert_eq!(&r.bytes, b"second-line\nunfinished");
    assert_eq!(r.start_offset, 11);
    assert!(!r.complete);
}
#[tokio::test]
async fn prefix_and_latest_caches_are_isolated_and_frontier_is_admission_bounded() {
    let (_tmp, s, _path) = fixture();
    let uri = format!("/api/runs/{ID}/observation?ws_id={WS}");
    let (_, prefix) = request(&s, &uri).await;
    let (_, latest) = request(&s, &format!("{uri}&window=latest")).await;
    assert_ne!(prefix["snapshot"], latest["snapshot"]);
    assert_eq!(prefix["window"], "prefix");
    assert_eq!(latest["window"], "latest");
    let one = s
        .workflow_observations
        .metadata
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let two = s
        .workflow_observations
        .metadata
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    assert_eq!(
        request(
            &s,
            &format!("/api/runs/{ID}/observation/frontier?ws_id={WS}")
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    drop((one, two));
    assert_eq!(
        request(
            &s,
            &format!("/api/runs/{ID}/observation/frontier?ws_id={WS}&known_frontier=bad")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
fn member_fixture() -> (tempfile::TempDir, AppState, PathBuf, member::Selector) {
    let (tmp, s, path) = fixture();
    let root = tmp.path().canonicalize().unwrap().join("transcripts");
    fs::create_dir_all(&root).unwrap();
    let transcript = root.join("child.jsonl");
    let mut run: Value = serde_json::from_slice(&fs::read(path.join("run.json")).unwrap()).unwrap();
    run["transcript_dir"] = json!(root);
    fs::write(path.join("run.json"), serde_json::to_vec(&run).unwrap()).unwrap();
    let mut r = result(0);
    r["transcript_path"] = json!(transcript);
    fs::write(path.join("step_results.jsonl"), lines(&[r])).unwrap();
    fs::write(&transcript,lines(&[json!({"type":"run_start","data":{"run_id":"run_agent","workspace_id":WS,"agent":"reviewer","provider":"test","model":"fake","started_at":"2026-11-09T10:00:00Z","mode":"readonly","system_prompt":"never echo this"}})])).unwrap();
    let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
    let token = c.token.clone();
    s.workflow_observations
        .snapshots
        .lock()
        .unwrap()
        .push(Arc::new(c));
    let selector = member::Selector {
        snapshot: token,
        section: "results".into(),
        parent: None,
        step_id: Some("same-step".into()),
        position: 0,
        record_id: "result:0".into(),
    };
    (tmp, s, transcript, selector)
}
#[test]
fn member_proof_binds_captured_selector_and_never_echoes_prompt_or_claims_running() {
    let (_tmp, s, _path, q) = member_fixture();
    let m = member::initial(&s, ID, WS, &q).unwrap();
    assert_eq!(m.authority.child_run_id, "run_agent");
    assert_eq!(m.prologue["workspace_id"], WS);
    assert!(m.prologue.get("system_prompt").is_none());
    assert!(m.prologue.get("status").is_none());
    let mut wrong = q.clone();
    wrong.record_id = "result:1".into();
    assert!(member::initial(&s, ID, WS, &wrong).is_err());
    wrong = q;
    wrong.step_id = Some("different".into());
    assert!(member::initial(&s, ID, WS, &wrong).is_err());
}
#[test]
fn retained_member_authority_rechecks_owner_and_prologue_after_capture_eviction() {
    let (_tmp, s, path, q) = member_fixture();
    let m = member::initial(&s, ID, WS, &q).unwrap();
    s.workflow_observations.snapshots.lock().unwrap().clear();
    assert!(member::reauthorize(&s, &m.authority).is_ok());
    let mut bytes = fs::read(&path).unwrap();
    let at = bytes.windows(9).position(|v| v == b"run_agent").unwrap();
    bytes[at..at + 9].copy_from_slice(b"run_other");
    fs::write(&path, bytes).unwrap();
    assert!(member::reauthorize(&s, &m.authority).is_err());
}
#[test]
fn wrong_workspace_remote_and_symlink_member_proofs_are_refused() {
    let (_tmp, s, path, q) = member_fixture();
    let bytes = fs::read(&path).unwrap();
    fs::write(
        &path,
        String::from_utf8(bytes).unwrap().replace(WS, "ws_other"),
    )
    .unwrap();
    assert!(member::initial(&s, ID, WS, &q).is_err());
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
    assert!(member::initial(&s, ID, WS, &q).is_err());
}

#[test]
fn only_exact_agent_started_events_can_prove_new_live_members() {
    let (_tmp, s, transcript, mut q) = member_fixture();
    let mut event = json!({"type":"agent_started","run_id":ID,"step_id":"same-step","agent":"reviewer","provider":"test","model":"fake","agent_run_id":"run_agent","transcript_path":transcript});
    let path = s.run_store.root.join(ID).join("events.jsonl");
    fs::write(s.run_store.root.join(ID).join("attempts.jsonl"),lines(&[json!({"v":1,"step_id":"same-step","agent_run_id":"run_agent","transcript_path":transcript,"started_at":"2026-11-09T10:00:00Z"})])).unwrap();
    for (kind, expected) in [("local", true), ("remote", false), ("unit", false)] {
        if kind == "remote" {
            event["host_id"] = json!("remote");
        }
        if kind == "unit" {
            event.as_object_mut().unwrap().remove("host_id");
            event["type"] = json!("unit_started");
            event["index"] = json!(0);
            event["unit_key"] = json!("a");
        }
        fs::write(&path, lines(&[event.clone()])).unwrap();
        let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
        q.snapshot = c.token.clone();
        q.section = "events".into();
        q.step_id = None;
        q.record_id = "event:0".into();
        s.workflow_observations.snapshots.lock().unwrap().clear();
        s.workflow_observations
            .snapshots
            .lock()
            .unwrap()
            .push(Arc::new(c));
        assert_eq!(member::initial(&s, ID, WS, &q).is_ok(), expected, "{kind}");
    }
}

#[test]
fn item_proof_needs_exact_captured_local_attribution_and_rejects_conflicts() {
    let (_tmp, s, transcript, mut q) = member_fixture();
    let dir = s.run_store.root.join(ID);
    let mut r = result(1);
    r["transcript_path"] = json!(transcript);
    r["items"][0]["run_id"] = json!("run_agent");
    r["items"][0]["transcript_path"] = json!(transcript);
    fs::write(dir.join("step_results.jsonl"), lines(&[r])).unwrap();
    q.section = "items".into();
    q.parent = Some("result:0".into());
    q.step_id = None;
    q.record_id = "result:0/items:0".into();
    let refresh = |q: &mut member::Selector| {
        let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
        q.snapshot = c.token.clone();
        s.workflow_observations.snapshots.lock().unwrap().clear();
        s.workflow_observations
            .snapshots
            .lock()
            .unwrap()
            .push(Arc::new(c));
    };
    refresh(&mut q);
    assert!(member::initial(&s, ID, WS, &q).is_err()); // local mirror alone is insufficient
    let mut attempt = json!({"v":1,"step_id":"same-step","unit_index":99,"agent_run_id":"run_agent","transcript_path":transcript,"started_at":"2026-11-09T10:00:00Z"});
    fs::write(dir.join("attempts.jsonl"), lines(&[attempt.clone()])).unwrap();
    refresh(&mut q);
    assert!(member::initial(&s, ID, WS, &q).is_err());
    attempt["unit_index"] = json!(0);
    attempt["transcript_path"] = json!("/different/path.jsonl");
    fs::write(dir.join("attempts.jsonl"), lines(&[attempt.clone()])).unwrap();
    refresh(&mut q);
    assert!(member::initial(&s, ID, WS, &q).is_err());
    attempt["transcript_path"] = json!(transcript);
    fs::write(dir.join("attempts.jsonl"), lines(&[attempt])).unwrap();
    refresh(&mut q);
    assert!(member::initial(&s, ID, WS, &q).is_ok());
    fs::write(dir.join("unit_checkpoints.jsonl"),lines(&[json!({"step_id":"same-step","index":0,"item":null,"run_id":"run_agent","transcript_path":transcript,"output":"done","success":true,"finished_at":"2026-11-09T10:00:00Z","host":"remote"})])).unwrap();
    refresh(&mut q);
    assert!(member::initial(&s, ID, WS, &q).is_err());
}
#[test]
fn agent_started_without_captured_attribution_does_not_prove_local_execution() {
    let (_tmp, s, transcript, mut q) = member_fixture();
    fs::write(s.run_store.root.join(ID).join("events.jsonl"),lines(&[json!({"type":"agent_started","run_id":ID,"step_id":"same-step","agent":"reviewer","agent_run_id":"run_agent","transcript_path":transcript})])).unwrap();
    let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
    q.snapshot = c.token.clone();
    q.section = "events".into();
    q.step_id = None;
    q.record_id = "event:0".into();
    s.workflow_observations.snapshots.lock().unwrap().clear();
    s.workflow_observations
        .snapshots
        .lock()
        .unwrap()
        .push(Arc::new(c));
    assert!(member::initial(&s, ID, WS, &q).is_err());
}

#[test]
fn matching_prologue_outside_global_requires_exact_registered_project_root() {
    let (_tmp, s, old_path, mut q) = member_fixture();
    let external = tempfile::tempdir().unwrap();
    let external_root = external.path().canonicalize().unwrap();
    let path = external_root.join("child.jsonl");
    fs::copy(old_path, &path).unwrap();
    let dir = s.run_store.root.join(ID);
    let mut run: Value = serde_json::from_slice(&fs::read(dir.join("run.json")).unwrap()).unwrap();
    run["transcript_dir"] = json!(external_root);
    fs::write(dir.join("run.json"), serde_json::to_vec(&run).unwrap()).unwrap();
    let mut r = result(0);
    r["transcript_path"] = json!(path);
    fs::write(dir.join("step_results.jsonl"), lines(&[r])).unwrap();
    let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
    q.snapshot = c.token.clone();
    s.workflow_observations.snapshots.lock().unwrap().clear();
    s.workflow_observations
        .snapshots
        .lock()
        .unwrap()
        .push(Arc::new(c));
    assert!(member::initial(&s, ID, WS, &q).is_err());
    fs::create_dir_all(s.global_dir.join("workspaces")).unwrap();
    let workspace: rupu_workspace::Workspace = serde_json::from_value(
        json!({"id":WS,"path":external_root,"created_at":"2026-11-09T10:00:00Z"}),
    )
    .unwrap();
    fs::write(
        s.global_dir.join("workspaces").join(format!("{WS}.toml")),
        toml::to_string(&workspace).unwrap(),
    )
    .unwrap();
    assert!(member::initial(&s, ID, WS, &q).is_ok());
}

fn initial_window_url(q: &member::Selector) -> String {
    format!("/api/runs/{ID}/observation/transcript?ws_id={WS}&action=latest&snapshot={}&section=results&step_id=same-step&position=0&record_id=result:0&limit=2",q.snapshot)
}
#[tokio::test]
async fn public_bridge_bounds_first_open_and_keeps_earlier_usable_after_append() {
    let (_tmp, s, path, q) = member_fixture();
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    for n in 0..100 {
        file.write_all(&lines(&[
            json!({"type":"assistant_delta","data":{"content":format!("row-{n}")}}),
        ]))
        .unwrap();
    }
    drop(file);
    let (status, first) = request(&s, &initial_window_url(&q)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["schema"], "transcript.window_follow.v1");
    assert_eq!(first["records"].as_array().unwrap().len(), 2);
    assert!(
        first["window"]["omitted_before_bytes"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    let poll = first["poll_cursor"].as_str().unwrap();
    let earlier = first["earlier_cursor"].as_str().unwrap();
    s.workflow_observations.snapshots.lock().unwrap().clear();
    let (_, unchanged) = request(
        &s,
        &format!(
            "/api/runs/{ID}/observation/transcript?ws_id={WS}&action=poll&cursor={poll}&limit=2"
        ),
    )
    .await;
    assert_eq!(unchanged["state"], "unchanged");
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&lines(&[
        json!({"type":"assistant_delta","data":{"content":"new-row"}}),
    ]))
    .unwrap();
    drop(file);
    let (status, changed) = request(
        &s,
        &format!(
            "/api/runs/{ID}/observation/transcript?ws_id={WS}&action=poll&cursor={poll}&limit=2"
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(changed["state"], "changed");
    assert_eq!(changed["snapshot"], first["snapshot"]);
    let (status,prior)=request(&s,&format!("/api/runs/{ID}/observation/transcript?ws_id={WS}&action=earlier&cursor={earlier}&limit=2")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prior["relationship"], "verified_anchor_fresh_window");
    assert_ne!(prior["records"], first["records"]);
    let (status, latest) = request(
        &s,
        &format!(
            "/api/runs/{ID}/observation/transcript?ws_id={WS}&action=latest&cursor={poll}&limit=2"
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(latest["relationship"], "fresh_latest");
    assert!(latest["prologue"].get("system_prompt").is_none());
}
#[tokio::test]
async fn public_bridge_rejects_raw_paths_mixed_selectors_owner_changes_and_replacement() {
    let (_tmp, s, path, q) = member_fixture();
    assert_eq!(
        request(&s, &format!("{}&path=/etc/passwd", initial_window_url(&q)))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, first) = request(&s, &initial_window_url(&q)).await;
    let token = first["poll_cursor"].as_str().unwrap();
    let uri =
        format!("/api/runs/{ID}/observation/transcript?ws_id={WS}&action=poll&cursor={token}");
    assert_eq!(
        request(&s, &format!("{uri}&snapshot={}", q.snapshot))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_ne!(
        request(&s, &uri.replace(WS, "ws_other")).await.0,
        StatusCode::OK
    );
    let replacement = path.with_extension("new");
    fs::write(&replacement, fs::read(&path).unwrap()).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert_eq!(request(&s, &uri).await.0, StatusCode::CONFLICT);
}
#[tokio::test]
async fn public_bridge_admission_is_bounded_without_queued_file_reads() {
    let (_tmp, s, _path, q) = member_fixture();
    let mut permits = Vec::new();
    for _ in 0..4 {
        permits.push(
            s.workflow_observations
                .follow_jobs
                .clone()
                .acquire_owned()
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        request(&s, &initial_window_url(&q)).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(permits);
}

#[test]
fn cancelled_initial_member_proof_stops_before_capture_or_filesystem_lookup() {
    let (_tmp, s, _path) = fixture();
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let selector = member::Selector {
        snapshot: "missing".into(),
        section: "items".into(),
        parent: None,
        step_id: None,
        position: 0,
        record_id: "missing".into(),
    };
    let error = member::initial_with_cancel(&s, ID, WS, &selector, &cancel)
        .err()
        .unwrap();
    assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error.1, "transcript_window_cancelled");
}

#[test]
fn latest_page_reaches_newest_small_log_without_scanning_pages() {
    let (_tmp, s, path) = fixture();
    let rows: Vec<_> = (0..125)
        .map(|n| json!({"type":"run_paused","run_id":ID,"sequence":n}))
        .collect();
    fs::write(path.join("events.jsonl"), lines(&rows)).unwrap();
    let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
    let page = c.latest_filtered_page("events", None, 50).unwrap();
    assert_eq!(page["selection"], "latest");
    assert_eq!(page["offset"], 75);
    assert_eq!(page["captured_total"], 125);
    assert_eq!(page["records"][0]["value"]["sequence"], 75);
    assert_eq!(page["records"][49]["value"]["sequence"], 124);
    assert_eq!(page["next_cursor"], Value::Null);
    assert_eq!(c.page("events", None, None, 50).unwrap()["offset"], 0);
}
#[test]
fn latest_page_applies_exact_step_filter_and_preserves_unknown_totals() {
    let (_tmp, s, path) = fixture();
    let rows: Vec<_> = (0..140)
        .map(|n| {
            let mut r = result(0);
            r["step_id"] = json!(if n % 2 == 0 { "alpha" } else { "beta" });
            r["output"] = json!(n.to_string());
            r
        })
        .collect();
    let mut bytes = lines(&rows);
    bytes.extend_from_slice(b"malformed\n");
    fs::write(path.join("step_results.jsonl"), bytes).unwrap();
    let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
    let p = c
        .latest_filtered_page("results", Some("alpha"), 10)
        .unwrap();
    assert_eq!(p["offset"], 60);
    assert_eq!(p["captured_total"], 70);
    assert_eq!(p["records"][9]["value"]["output"], "138");
    assert_eq!(p["step_id"], "alpha");
    assert_eq!(p["source_total"], Value::Null);
    assert!(p["unassigned_rows"].as_u64().unwrap() > 0);
    let absent = c
        .latest_filtered_page("results", Some("absent"), 10)
        .unwrap();
    assert_eq!(absent["captured_total"], 0);
    assert_eq!(absent["source_total"], Value::Null);
}
#[test]
fn latest_page_byte_budget_still_includes_final_record() {
    let (_tmp, s, path) = fixture();
    let rows: Vec<_> = (0..100)
        .map(|n| json!({"type":"run_paused","run_id":ID,"sequence":n,"text":"x".repeat(60_000)}))
        .collect();
    fs::write(path.join("events.jsonl"), lines(&rows)).unwrap();
    let c = capture::capture_window(&s, ID, WS, Window::Latest).unwrap();
    let p = c.latest_filtered_page("events", None, 100).unwrap();
    let rows = p["records"].as_array().unwrap();
    assert!(rows.len() < 100 && !rows.is_empty());
    assert_eq!(rows.last().unwrap()["value"]["sequence"], 99);
    assert_eq!(p["offset"].as_u64().unwrap() + rows.len() as u64, 100);
    assert!(serde_json::to_vec(&p).unwrap().len() <= MAX_PAGE);
}
#[tokio::test]
async fn latest_page_http_rejects_prefix_and_mixed_selectors() {
    let (_tmp, s, _path) = fixture();
    let (_, prefix) = request(&s, &format!("/api/runs/{ID}/observation?ws_id={WS}")).await;
    let (_, latest) = request(
        &s,
        &format!("/api/runs/{ID}/observation?ws_id={WS}&window=latest"),
    )
    .await;
    let base = format!(
        "/api/runs/{ID}/observation/page?ws_id={WS}&snapshot={}&latest=true",
        latest["snapshot"].as_str().unwrap()
    );
    for suffix in [
        "&section=steps",
        "&section=events&cursor=bad",
        "&section=events&parent=result:0",
        "&section=events&step_id=alpha",
    ] {
        assert_eq!(
            request(&s, &format!("{base}{suffix}")).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(
            &s,
            &format!(
                "/api/runs/{ID}/observation/page?ws_id={WS}&snapshot={}&latest=true&section=events",
                prefix["snapshot"].as_str().unwrap()
            )
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, empty) = request(&s, &format!("{base}&section=events")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["selection"], "latest");
    assert_eq!(empty["offset"], 0);
    assert_eq!(empty["captured_total"], 0);
}

#[tokio::test]
async fn public_descendant_routes_bind_parent_and_open_only_catalog_members() {
    use rupu_launch::ledger::{LedgerLine, Started};
    let (_tmp, s, _path, q) = member_fixture();
    let parent_dir = s.run_store.root.join("run_agent");
    fs::create_dir_all(&parent_dir).unwrap();
    let child_path = s.global_dir.join("transcripts/descendant.jsonl");
    fs::write(&child_path,lines(&[json!({"type":"run_start","data":{"run_id":"sub_descendant","workspace_id":WS,"agent":"leaf","provider":"test","model":"fake","started_at":"2026-11-09T10:00:00Z","mode":"readonly"}})])).unwrap();
    let mut started = Started::new(
        "ch_descendant",
        rupu_tools::ChildKind::Subagent,
        "leaf",
        "sub_descendant",
        1,
        1,
    );
    started.transcript = Some(child_path);
    let mut bytes = serde_json::to_vec(&LedgerLine::Started(started)).unwrap();
    bytes.push(b'\n');
    fs::write(parent_dir.join("children.jsonl"), bytes).unwrap();
    let (status, parent) = request(&s, &initial_window_url(&q)).await;
    assert_eq!(status, StatusCode::OK);
    let token = parent["poll_cursor"].as_str().unwrap();
    let base = format!(
        "/api/runs/{ID}/observation/transcript/children?ws_id={WS}&parent_cursor={token}&limit=1"
    );
    let (status, catalog) = request(&s, &base).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalog["parent_run_id"], "run_agent");
    assert_eq!(catalog["captured_total"], 1);
    let open=format!("/api/runs/{ID}/observation/transcript/descendant?ws_id={WS}&parent_cursor={token}&snapshot={}&record_id={}&limit=1",catalog["snapshot"].as_str().unwrap(),catalog["records"][0]["record_id"].as_str().unwrap());
    let (status, child) = request(&s, &open).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(child["workflow_run_id"], ID);
    assert_eq!(child["child_run_id"], "sub_descendant");
    assert_eq!(child["prologue"]["run_id"], "sub_descendant");
    assert_eq!(
        request(&s, &format!("{open}&path=/etc/passwd")).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_ne!(
        request(&s, &open.replace(WS, "ws_other")).await.0,
        StatusCode::OK
    );
    let next = child["poll_cursor"].as_str().unwrap();
    let (status, poll) = request(
        &s,
        &format!("/api/runs/{ID}/observation/transcript?ws_id={WS}&cursor={next}&action=poll"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(poll["state"], "unchanged");
    assert_eq!(poll["child_run_id"], "sub_descendant");
}
#[tokio::test]
async fn public_descendant_routes_share_admission_and_reject_unknown_inputs() {
    let (_tmp, s, _path, q) = member_fixture();
    let (_, parent) = request(&s, &initial_window_url(&q)).await;
    let base = format!(
        "/api/runs/{ID}/observation/transcript/children?ws_id={WS}&parent_cursor={}",
        parent["poll_cursor"].as_str().unwrap()
    );
    for suffix in ["&limit=0", "&limit=51", "&path=/etc/passwd", "&host=remote"] {
        assert_ne!(
            request(&s, &format!("{base}{suffix}")).await.0,
            StatusCode::OK
        );
    }
    let permit = s
        .workflow_observations
        .follow_jobs
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    assert_eq!(request(&s, &base).await.0, StatusCode::TOO_MANY_REQUESTS);
    drop(permit);
}
