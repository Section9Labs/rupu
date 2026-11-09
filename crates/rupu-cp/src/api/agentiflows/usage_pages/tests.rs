use super::*;
use std::io::Write;
const FLOW: &str = "af_usage_test";
const PROJECT: &str = "ws_project";
fn fixture() -> (tempfile::TempDir, AppState, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let s = AppState::new(global.clone(), rupu_config::PricingConfig::default());
    let root = global.join("agentiflows").join(FLOW);
    fs::create_dir_all(root.join("lead")).unwrap();
    fs::write(root.join("agentiflow.json"), json!({"id":FLOW,"name":"usage","engagement_profiles":[],"trigger":"agentiflow","status":"completed","rounds":1,"goals":[],"started_at":"2026-11-09T00:00:00Z"}).to_string()).unwrap();
    fs::write(root.join("lead/transcript.r0.jsonl"), format!("{}\n", json!({"type":"run_start","data":{"run_id":FLOW,"workspace_id":PROJECT,"agent":"lead","provider":"test","model":"test","started_at":"2026-11-09T00:00:00Z","mode":"readonly"}}))).unwrap();
    (tmp, s, root)
}
fn row(id: &str, at: &str, run: &str, parent: Option<&str>, path: &FsPath) -> Value {
    json!({"v":1,"id":id,"at":at,"kind":"turn","agent_run_id":run,"parent_agent_run_id":parent,"transcript":path,"agent":"agent","provider":"test","model":"test","input_tokens":10,"output_tokens":2,"cached_tokens":3,"cache_write_tokens":1})
}
fn write_rows(root: &FsPath, rows: &[Value]) {
    let bytes: String = rows.iter().map(|r| format!("{r}\n")).collect();
    fs::write(root.join("usage.jsonl"), bytes).unwrap();
}
fn q(snapshot: Option<String>, cursor: Option<String>, limit: usize) -> PageQuery {
    PageQuery {
        host: Some("local".into()),
        project: PROJECT.into(),
        snapshot,
        cursor,
        limit: Some(limit),
    }
}
fn error<T>(r: ApiResult<T>) -> u16 {
    r.err().expect("must refuse").0.as_u16()
}
fn child(root: &FsPath, id: &str, round: u32) {
    let mut c = rupu_launch::ledger::Started::new(
        "ch_recorded",
        rupu_tools::ChildKind::Subagent,
        "worker",
        id,
        1,
        1,
    );
    c.tree = Some(root.into());
    c.round = Some(round);
    rupu_launch::ledger::append(root, &rupu_launch::ledger::LedgerLine::Started(c));
}
#[test]
fn explicit_project_and_canonical_first_record_are_required_even_without_usage() {
    let (_tmp, s, root) = fixture();
    let c = capture(&s, FLOW, PROJECT).unwrap();
    assert_eq!(c.rows.len(), 0);
    assert_eq!(c.envelope["evidence"]["usage_ledger_present"], false);
    assert_eq!(c.envelope["evidence"]["source_complete"], false);
    assert_eq!(error(capture(&s, FLOW, "ws_foreign")), 409);
    fs::write(root.join("lead/transcript.r0.jsonl"), b"{}\n").unwrap();
    assert_eq!(error(capture(&s, FLOW, PROJECT)), 409);
}
#[test]
fn absent_proof_does_not_fall_back_to_paths_or_directory_name() {
    let (_tmp, s, root) = fixture();
    fs::remove_file(root.join("lead/transcript.r0.jsonl")).unwrap();
    assert_eq!(error(capture(&s, FLOW, PROJECT)), 501);
}
#[test]
fn foreign_record_host_and_prologue_aliases_refuse() {
    for field in [
        "record_id",
        "record_host",
        "prologue_host",
        "prologue_run",
        "prologue_workspace",
    ] {
        let (_tmp, s, root) = fixture();
        let path = if field.starts_with("record") {
            root.join("agentiflow.json")
        } else {
            root.join("lead/transcript.r0.jsonl")
        };
        let mut v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match field {
            "record_id" => v["id"] = json!("af_foreign"),
            "record_host" => v["host_id"] = json!("peer"),
            "prologue_host" => v["data"]["host_id"] = json!("peer"),
            "prologue_run" => v["run_id"] = json!("af_foreign"),
            _ => v["workspace_id"] = json!("ws_foreign"),
        }
        fs::write(path, format!("{v}\n")).unwrap();
        assert_eq!(error(capture(&s, FLOW, PROJECT)), 409, "{field}");
    }
}
#[test]
fn attribution_is_explicit_and_unknown_round_is_not_zero() {
    let (_tmp, s, root) = fixture();
    child(&root, "run_unit", 7);
    write_rows(
        &root,
        &[
            row(
                "u0",
                "2026-11-09T00:00:00Z",
                "run_unknown",
                Some(FLOW),
                FsPath::new("/unreadable/u"),
            ),
            row(
                "l1",
                "2026-11-09T00:01:00Z",
                FLOW,
                None,
                &root.join("lead/transcript.r2.jsonl"),
            ),
            row(
                "u1",
                "2026-11-09T00:02:00Z",
                "run_unit",
                Some(FLOW),
                FsPath::new("/unreadable/u"),
            ),
            row(
                "d1",
                "2026-11-09T00:03:00Z",
                "run_deep",
                Some("run_unit"),
                FsPath::new("/unreadable/deep"),
            ),
            row(
                "x1",
                "2026-11-09T00:04:00Z",
                "run_legacy",
                Some(FLOW),
                FsPath::new("/other/transcript.r99.jsonl"),
            ),
        ],
    );
    let c = capture(&s, FLOW, PROJECT).unwrap();
    let sources: Vec<_> = c
        .rows
        .iter()
        .map(|r| r["round_source"].as_str().unwrap())
        .collect();
    assert_eq!(
        sources,
        [
            "unknown",
            "lead_path",
            "unit_catalog",
            "parent_attribution",
            "temporal_fallback"
        ]
    );
    assert!(c.rows[0]["round"].is_null());
    assert_eq!(c.rows[4]["round"], 2);
    assert!(c.rows.iter().all(|r| r["cost_usd"].is_null()));
    assert_eq!(
        c.envelope["evidence"]["navigation"],
        "requires_independent_catalog_and_prologue"
    );
}
#[test]
fn duplicate_ids_are_deduplicated_before_turn_ordinals_and_conflicts_reject() {
    let (_tmp, s, root) = fixture();
    let r = row(
        "call1",
        "2026-11-09T00:02:00Z",
        FLOW,
        None,
        &root.join("lead/transcript.r0.jsonl"),
    );
    let r2 = row(
        "call2",
        "2026-11-09T00:01:00Z",
        FLOW,
        None,
        &root.join("lead/transcript.r0.jsonl"),
    );
    write_rows(&root, &[r.clone(), r.clone(), r2]);
    let c = capture(&s, FLOW, PROJECT).unwrap();
    assert_eq!(c.rows.len(), 2);
    assert_eq!(c.envelope["evidence"]["duplicate_rows"], 1);
    assert_eq!(c.rows[0]["transcript_turn"], 2);
    assert_eq!(c.rows[0]["source_line"], 3);
    let mut conflict = r.clone();
    conflict["input_tokens"] = json!(11);
    write_rows(&root, &[r, conflict]);
    assert_eq!(error(capture(&s, FLOW, PROJECT)), 409);
}
#[test]
fn compaction_has_no_transcript_turn_and_round_conflicts_do_not_pick_a_child() {
    let (_tmp, s, root) = fixture();
    child(&root, "run_unit", 1);
    child(&root, "run_unit", 2);
    let mut r = row(
        "compact",
        "2026-11-09T00:02:00Z",
        "run_unit",
        Some(FLOW),
        FsPath::new("/not/opened"),
    );
    r["kind"] = json!("compaction");
    write_rows(&root, &[r]);
    let c = capture(&s, FLOW, PROJECT).unwrap();
    assert_eq!(c.envelope["evidence"]["ambiguous_round_runs"], 1);
    assert!(c.rows[0]["transcript_turn"].is_null());
    assert_eq!(c.rows[0]["round_source"], "unknown");
}
#[test]
fn malformed_future_and_partial_input_remain_visible() {
    let (_tmp, s, root) = fixture();
    let mut future = row(
        "future",
        "2026-11-09T00:00:00Z",
        FLOW,
        None,
        FsPath::new("/a"),
    );
    future["v"] = json!(999);
    fs::write(
        root.join("usage.jsonl"),
        format!("not json\n{future}\n{{\"partial\":"),
    )
    .unwrap();
    let c = capture(&s, FLOW, PROJECT).unwrap();
    let e = &c.envelope["evidence"];
    assert_eq!(e["malformed_rows"], 1);
    assert_eq!(e["unsupported_rows"], 1);
    assert_eq!(e["incomplete_tail_count"], 1);
    assert_eq!(e["source_complete"], false);
}
#[test]
fn line_and_source_bytes_are_rejected_before_partial_success() {
    let (_tmp, s, root) = fixture();
    fs::write(root.join("usage.jsonl"), vec![b'x'; MAX_LINE + 1]).unwrap();
    assert_eq!(error(capture(&s, FLOW, PROJECT)), 413);
    fs::File::create(root.join("usage.jsonl"))
        .unwrap()
        .set_len(MAX_LEDGER as u64 + 1)
        .unwrap();
    assert_eq!(error(capture(&s, FLOW, PROJECT)), 413);
    let mut e = Evidence::default();
    assert_eq!(error(lines(&vec![b'\n'; MAX_ROWS + 1], &mut e)), 413);
}
#[test]
fn symlink_and_special_sources_are_refused_without_opening_locator_targets() {
    let (_tmp, s, root) = fixture();
    std::os::unix::fs::symlink(root.join("agentiflow.json"), root.join("usage.jsonl")).unwrap();
    assert_eq!(error(capture(&s, FLOW, PROJECT)), 409);
    fs::remove_file(root.join("usage.jsonl")).unwrap();
    fs::create_dir(root.join("usage.jsonl")).unwrap();
    assert_eq!(error(capture(&s, FLOW, PROJECT)), 409);
}
#[test]
fn fifo_source_fails_without_waiting_for_a_writer() {
    let (_tmp, s, root) = fixture();
    if !crate::api::fs_open::test_support::mkfifo(&root.join("usage.jsonl")) {
        return;
    }
    crate::api::fs_open::test_support::within_deadline(move || {
        assert_eq!(error(capture(&s, FLOW, PROJECT)), 409)
    });
}
#[tokio::test]
async fn immutable_pages_preserve_owner_and_do_not_reread_removed_sources() {
    let (_tmp, s, root) = fixture();
    write_rows(
        &root,
        &[
            row(
                "one",
                "2026-11-09T00:00:00Z",
                FLOW,
                None,
                &root.join("lead/transcript.r0.jsonl"),
            ),
            row(
                "two",
                "2026-11-09T00:01:00Z",
                FLOW,
                None,
                &root.join("lead/transcript.r0.jsonl"),
            ),
        ],
    );
    let first = page(State(s.clone()), Path(FLOW.into()), Query(q(None, None, 1)))
        .await
        .unwrap()
        .0;
    let snapshot = first["snapshot"].as_str().unwrap().to_string();
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    fs::remove_dir_all(&root).unwrap();
    let second = page(
        State(s.clone()),
        Path(FLOW.into()),
        Query(q(Some(snapshot.clone()), Some(cursor), 1)),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(second["offset"], 1);
    assert_eq!(second["records"][0]["ledger_id"], "two");
    assert!(second["next_cursor"].is_null());
    assert_eq!(first["owner_instance_id"], second["owner_instance_id"]);
    assert_eq!(first["source_generation"], second["source_generation"]);
    let mut foreign = q(Some(snapshot.clone()), None, 1);
    foreign.project = "ws_foreign".into();
    assert_eq!(
        error(page(State(s.clone()), Path(FLOW.into()), Query(foreign)).await),
        409
    );
    let other = AppState::new(s.global_dir.clone(), rupu_config::PricingConfig::default());
    assert_eq!(
        error(
            page(
                State(other),
                Path(FLOW.into()),
                Query(q(Some(snapshot), None, 1))
            )
            .await
        ),
        409
    );
}
#[tokio::test]
async fn admission_and_query_guards_do_not_silently_fallback() {
    let (_tmp, s, _root) = fixture();
    let permit = s
        .agentiflow_usage_pages
        .gate
        .clone()
        .try_acquire_owned()
        .unwrap();
    assert_eq!(
        error(page(State(s.clone()), Path(FLOW.into()), Query(q(None, None, 1))).await),
        429
    );
    drop(permit);
    let mut remote = q(None, None, 1);
    remote.host = Some("peer".into());
    assert_eq!(
        error(page(State(s.clone()), Path(FLOW.into()), Query(remote)).await),
        501
    );
    for query in [q(None, Some("cursor".into()), 1), q(None, None, 101)] {
        assert_eq!(
            error(page(State(s.clone()), Path(FLOW.into()), Query(query)).await),
            400
        );
    }
}
#[test]
fn wrong_capture_cursor_and_byte_limited_pages_are_explicit() {
    let (_tmp, s, root) = fixture();
    write_rows(
        &root,
        &[
            row("one", "2026-11-09T00:00:00Z", FLOW, None, FsPath::new("/a")),
            row("two", "2026-11-09T00:01:00Z", FLOW, None, FsPath::new("/a")),
        ],
    );
    let mut c = capture(&s, FLOW, PROJECT).unwrap();
    let other = capture(&s, FLOW, PROJECT).unwrap();
    assert_eq!(error(c.page(Some(&other.cursor(1)), 1)), 409);
    c.rows[0]["model"] = json!("x".repeat(600_000));
    c.rows[1]["model"] = json!("x".repeat(600_000));
    let p = c.page(None, 100).unwrap();
    assert_eq!(p["records"].as_array().unwrap().len(), 1);
    assert!(p["next_cursor"].is_string());
}
#[test]
fn large_transcript_body_is_not_read_to_prove_project() {
    let (_tmp, s, root) = fixture();
    let path = root.join("lead/transcript.r0.jsonl");
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(&vec![b'x'; MAX_PROOF * 2])
        .unwrap();
    assert!(capture(&s, FLOW, PROJECT).is_ok());
}
#[tokio::test]
async fn cache_entry_bound_evicts_without_recapturing_on_continuation() {
    let (_tmp, s, _root) = fixture();
    let first = page(State(s.clone()), Path(FLOW.into()), Query(q(None, None, 1)))
        .await
        .unwrap()
        .0;
    for _ in 0..4 {
        page(State(s.clone()), Path(FLOW.into()), Query(q(None, None, 1)))
            .await
            .unwrap();
    }
    assert_eq!(s.agentiflow_usage_pages.entries.lock().unwrap().len(), 4);
    assert_eq!(
        error(
            page(
                State(s.clone()),
                Path(FLOW.into()),
                Query(q(Some(first["snapshot"].as_str().unwrap().into()), None, 1))
            )
            .await
        ),
        409
    );
}

#[test]
fn source_replacement_and_equal_length_rewrite_refuse_but_append_is_excluded() {
    for change in ["replace", "rewrite", "append"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let path = root.join("usage.jsonl");
        fs::write(&path, b"first\n").unwrap();
        let observed = read_checked(&path, 100, false, || match change {
            "replace" => {
                fs::write(root.join("new"), b"first\n").unwrap();
                fs::rename(root.join("new"), &path).unwrap();
            }
            "rewrite" => fs::write(&path, b"other\n").unwrap(),
            _ => {
                fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"later\n")
                    .unwrap();
            }
        });
        if change == "append" {
            assert_eq!(observed.unwrap().unwrap(), b"first\n");
        } else {
            assert_eq!(error(observed), 409, "{change}");
        }
    }
}

#[tokio::test]
async fn expired_snapshot_refuses_and_never_recaptures_implicitly() {
    let (_tmp, s, root) = fixture();
    let first = page(State(s.clone()), Path(FLOW.into()), Query(q(None, None, 1)))
        .await
        .unwrap()
        .0;
    {
        let mut cache = s.agentiflow_usage_pages.entries.lock().unwrap();
        Arc::get_mut(&mut cache[0]).unwrap().born = Instant::now() - TTL;
    }
    fs::remove_dir_all(root).unwrap();
    assert_eq!(
        error(
            page(
                State(s.clone()),
                Path(FLOW.into()),
                Query(q(Some(first["snapshot"].as_str().unwrap().into()), None, 1))
            )
            .await
        ),
        409
    );
    assert!(s.agentiflow_usage_pages.entries.lock().unwrap().is_empty());
}
