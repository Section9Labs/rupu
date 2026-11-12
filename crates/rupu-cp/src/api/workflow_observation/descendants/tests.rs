use super::*;
use rupu_launch::ledger::{LedgerLine, Started};
use rupu_tools::ChildKind;
use std::{fs, io::Write};
const FLOW: &str = "run_flow";
const WS: &str = "ws_fixture";
fn line(value: &impl Serialize) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    bytes
}
fn prologue(id: &str) -> Vec<u8> {
    line(
        &json!({"type":"run_start","data":{"run_id":id,"workspace_id":WS,"agent":"worker","provider":"test","model":"fake","started_at":"2026-11-09T00:00:00Z","mode":"readonly"}}),
    )
}
struct Fixture {
    _tmp: tempfile::TempDir,
    s: AppState,
    parent: Authority,
    dir: PathBuf,
    token: String,
}
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let s = AppState::new(root.clone(), rupu_config::PricingConfig::default());
    let transcripts = root.join("transcripts");
    fs::create_dir_all(&transcripts).unwrap();
    let path = transcripts.join("run_parent.jsonl");
    let first = prologue("run_parent");
    fs::write(&path, &first).unwrap();
    let run:rupu_orchestrator::runs::RunRecord=serde_json::from_value(json!({"id":FLOW,"workflow_name":"flow","status":"running","inputs":{},"workspace_id":WS,"workspace_path":"/fixture","transcript_dir":transcripts,"started_at":"2026-11-09T00:00:00Z"})).unwrap();
    s.run_store.create(run, "name: flow\nsteps: []\n").unwrap();
    let parent = Authority {
        workflow_run_id: FLOW.into(),
        workspace_id: WS.into(),
        child_run_id: "run_parent".into(),
        path: path.to_string_lossy().into(),
        transcript_root: transcripts.to_string_lossy().into(),
        workflow_started_at: "2026-11-09T00:00:00Z".into(),
        workflow_name: "flow".into(),
        workspace_path: "/fixture".into(),
        step_id: "step".into(),
        member_record_id: "attempt:0".into(),
        prologue_sha256: hash(&first),
        store_kind: "global".into(),
        ancestry: Vec::new(),
    };
    let member = super::super::member::reauthorize(&s, &parent).unwrap();
    let reply = transcript_follow::read(
        &s,
        FLOW,
        WS,
        transcript_follow::Query {
            action: transcript_follow::Action::Latest,
            cursor: None,
            limit: 10,
        },
        Some(member),
        CancellationToken::new(),
    )
    .unwrap();
    let token = serde_json::to_value(reply).unwrap()["poll_cursor"]
        .as_str()
        .unwrap()
        .into();
    let dir = s.run_store.root.join("run_parent");
    fs::create_dir_all(&dir).unwrap();
    Fixture {
        _tmp: tmp,
        s,
        parent,
        dir,
        token,
    }
}
fn start(f: &Fixture, handle: &str, id: &str) -> Started {
    let path =
        f.s.global_dir
            .join("transcripts")
            .join(format!("{id}.jsonl"));
    fs::write(&path, prologue(id)).unwrap();
    let mut v = Started::new(handle, ChildKind::Agent, "worker", id, 1, 1);
    v.transcript = Some(path);
    v
}
fn write_ledger(f: &Fixture, entries: &[Started]) {
    fs::write(
        f.dir.join("children.jsonl"),
        entries
            .iter()
            .flat_map(|s| line(&LedgerLine::Started(s.clone())))
            .collect::<Vec<_>>(),
    )
    .unwrap();
}
fn page(f: &Fixture, cursor: Option<String>, limit: usize) -> ApiResult<Reply> {
    catalog(
        &f.s,
        FLOW,
        WS,
        Query {
            parent_cursor: f.token.clone(),
            cursor,
            limit,
        },
        &CancellationToken::new(),
    )
}
fn selected(f: &Fixture, p: &Reply, index: usize) -> ApiResult<VerifiedMember> {
    select(
        &f.s,
        FLOW,
        WS,
        Selection {
            parent_cursor: f.token.clone(),
            snapshot: p.snapshot.clone(),
            record_id: p.records[index].record_id.clone(),
        },
        &CancellationToken::new(),
    )
}
#[test]
fn live_parent_without_standalone_metadata_has_direct_children_and_unknown_lifecycle() {
    let f = fixture();
    write_ledger(&f, &[start(&f, "h1", "run_one")]);
    let p = page(&f, None, 50).unwrap();
    assert_eq!(p.records.len(), 1);
    assert!(p.records[0].recorded_status.is_none());
    assert!(p.source_complete);
    let child = selected(&f, &p, 0).unwrap();
    assert_eq!(child.authority.leaf_run_id(), "run_one");
    assert_eq!(child.authority.ancestry.len(), 1);
}
#[test]
fn immutable_pages_survive_parent_append_but_do_not_recapture_children() {
    let f = fixture();
    write_ledger(
        &f,
        &[start(&f, "h1", "run_one"), start(&f, "h2", "run_two")],
    );
    let p = page(&f, None, 1).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&f.parent.path)
        .unwrap()
        .write_all(&line(
            &json!({"type":"assistant_delta","data":{"content":"new"}}),
        ))
        .unwrap();
    write_ledger(&f, &[start(&f, "h3", "run_three")]);
    let next = page(&f, p.next_cursor, 1).unwrap();
    assert_eq!(next.records[0].child_run_id, "run_two");
    assert_eq!(next.captured_total, 2);
}
#[test]
fn changed_selected_edge_is_refused_not_refreshed() {
    let f = fixture();
    write_ledger(&f, &[start(&f, "h1", "run_one")]);
    let p = page(&f, None, 50).unwrap();
    write_ledger(&f, &[start(&f, "h1", "run_two")]);
    assert!(selected(&f, &p, 0).is_err());
}
#[test]
fn selected_continuation_survives_catalog_eviction() {
    let f = fixture();
    write_ledger(&f, &[start(&f, "h1", "run_one")]);
    let p = page(&f, None, 50).unwrap();
    let member = selected(&f, &p, 0).unwrap();
    f.s.workflow_observations
        .descendants
        .snapshots
        .lock()
        .unwrap()
        .clear();
    let fresh = super::super::member::reauthorize_with_cancel(
        &f.s,
        &member.authority,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(fresh.authority.leaf_run_id(), "run_one");
    assert!(selected(&f, &p, 0).is_err());
}
#[test]
fn direct_legacy_child_requires_prologue_and_never_fabricates_status() {
    let f = fixture();
    let path = f.dir.join("sub/run_legacy/transcript.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, prologue("run_legacy")).unwrap();
    let p = page(&f, None, 50).unwrap();
    assert_eq!(p.records[0].source, "legacy");
    assert!(p.records[0].recorded_status.is_none());
    assert!(selected(&f, &p, 0).is_ok());
    fs::write(&path, prologue("run_foreign")).unwrap();
    assert!(selected(&f, &p, 0).is_err());
}
#[test]
fn catalog_cursor_mac_and_process_instance_are_required() {
    let a = Cache::default();
    let b = Cache::default();
    let token = sign(
        &a,
        &Cursor {
            snapshot: "s".into(),
            position: 2,
        },
    )
    .unwrap();
    assert_eq!(decode(&a, &token).unwrap().position, 2);
    assert!(decode(&b, &token).is_err());
    assert!(decode(&a, &format!("{token}x")).is_err());
}
#[test]
fn missing_parent_directory_is_unknown_not_complete_empty() {
    let f = fixture();
    fs::remove_dir(&f.dir).unwrap();
    let p = page(&f, None, 50).unwrap();
    assert!(!p.source_complete);
    assert!(p.records.is_empty());
    assert_eq!(p.evidence.ledger_state, "parent_store_absent");
}
#[test]
fn conflicting_active_archive_parent_directories_are_refused() {
    let f = fixture();
    fs::create_dir_all(f.s.run_store.archive_root().join("run_parent")).unwrap();
    assert!(page(&f, None, 50).is_err());
}
#[test]
fn oversized_ledger_is_explicit_and_not_folded_from_prefix() {
    let f = fixture();
    let file = File::create(f.dir.join("children.jsonl")).unwrap();
    file.set_len(MAX_LEDGER as u64 + 1).unwrap();
    let p = page(&f, None, 50).unwrap();
    assert_eq!(p.evidence.ledger_state, "oversized");
    assert!(!p.source_complete);
    assert!(p.records.is_empty());
}
#[test]
fn partial_and_malformed_rows_are_visible_qualifications() {
    let f = fixture();
    let mut bytes = line(&LedgerLine::Started(start(&f, "h1", "run_one")));
    bytes.extend(b"broken\nunfinished");
    fs::write(f.dir.join("children.jsonl"), bytes).unwrap();
    let p = page(&f, None, 50).unwrap();
    assert_eq!(p.records.len(), 1);
    assert_eq!(p.evidence.malformed_rows, 1);
    assert_eq!(p.evidence.ledger_state, "partial_tail");
    assert!(!p.source_complete);
}
#[test]
fn workflow_kind_does_not_issue_agent_transcript_authority() {
    let f = fixture();
    let mut v = start(&f, "h1", "run_one");
    v.kind = ChildKind::Workflow;
    write_ledger(&f, &[v]);
    let p = page(&f, None, 50).unwrap();
    assert!(!p.records[0].transcript_available);
    assert_eq!(
        p.records[0].reason.as_deref(),
        Some("workflow_navigation_required")
    );
    assert!(selected(&f, &p, 0).is_err());
}
#[test]
fn remote_edge_and_foreign_child_prologue_are_refused() {
    let f = fixture();
    let value = start(&f, "h1", "run_one");
    let mut raw = serde_json::to_value(LedgerLine::Started(value.clone())).unwrap();
    raw["host_id"] = json!("remote");
    fs::write(f.dir.join("children.jsonl"), line(&raw)).unwrap();
    let p = page(&f, None, 50).unwrap();
    assert!(selected(&f, &p, 0).is_err());
    write_ledger(&f, std::slice::from_ref(&value));
    let p = page(&f, None, 50).unwrap();
    fs::write(value.transcript.unwrap(), prologue("run_foreign")).unwrap();
    assert!(selected(&f, &p, 0).is_err());
}
#[test]
fn cancelled_catalog_does_not_publish_snapshot() {
    let f = fixture();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(catalog(
        &f.s,
        FLOW,
        WS,
        Query {
            parent_cursor: f.token.clone(),
            cursor: None,
            limit: 1
        },
        &cancel
    )
    .is_err());
    assert!(f
        .s
        .workflow_observations
        .descendants
        .snapshots
        .lock()
        .unwrap()
        .is_empty());
}
#[test]
fn symlinked_parent_and_ledger_are_refused() {
    let f = fixture();
    let other = f.s.global_dir.join("elsewhere");
    fs::create_dir(&other).unwrap();
    fs::remove_dir(&f.dir).unwrap();
    std::os::unix::fs::symlink(&other, &f.dir).unwrap();
    assert!(page(&f, None, 50).is_err());
    fs::remove_file(&f.dir).unwrap();
    fs::create_dir(&f.dir).unwrap();
    let target = other.join("ledger");
    fs::write(&target, []).unwrap();
    std::os::unix::fs::symlink(target, f.dir.join("children.jsonl")).unwrap();
    assert!(page(&f, None, 50).is_err());
}
#[test]
fn recursive_navigation_keeps_root_identity_and_rejects_replaced_ancestor() {
    let mut f = fixture();
    write_ledger(&f, &[start(&f, "h1", "run_one")]);
    let first = page(&f, None, 50).unwrap();
    let child = selected(&f, &first, 0).unwrap();
    let authority = child.authority.clone();
    let reply = transcript_follow::read(
        &f.s,
        FLOW,
        WS,
        transcript_follow::Query {
            action: transcript_follow::Action::Latest,
            cursor: None,
            limit: 10,
        },
        Some(child),
        CancellationToken::new(),
    )
    .unwrap();
    let value = serde_json::to_value(reply).unwrap();
    assert_eq!(value["child_run_id"], "run_one");
    f.token = value["poll_cursor"].as_str().unwrap().into();
    f.dir = f.s.run_store.root.join("run_one");
    fs::create_dir_all(&f.dir).unwrap();
    write_ledger(&f, &[start(&f, "h2", "run_two")]);
    let second = page(&f, None, 50).unwrap();
    assert_eq!(second.depth, 1);
    assert_eq!(second.parent_run_id, "run_one");
    let grandchild = selected(&f, &second, 0).unwrap();
    assert_eq!(grandchild.authority.child_run_id, "run_parent");
    assert_eq!(grandchild.authority.leaf_run_id(), "run_two");
    assert_eq!(grandchild.authority.ancestry.len(), 2);
    let path = PathBuf::from(authority.leaf_path());
    let bytes = fs::read(&path).unwrap();
    fs::rename(&path, path.with_extension("old")).unwrap();
    fs::write(&path, bytes).unwrap();
    assert!(super::super::member::reauthorize_with_cancel(
        &f.s,
        &grandchild.authority,
        &CancellationToken::new()
    )
    .is_err());
}
#[test]
fn catalog_selection_cannot_cross_parent_or_workflow_owner() {
    let f = fixture();
    write_ledger(&f, &[start(&f, "h1", "run_one")]);
    let p = page(&f, None, 50).unwrap();
    let q = Selection {
        parent_cursor: f.token.clone(),
        snapshot: p.snapshot.clone(),
        record_id: p.records[0].record_id.clone(),
    };
    assert!(select(&f.s, "run_other", WS, q, &CancellationToken::new()).is_err());
    let q = Query {
        parent_cursor: f.token.clone(),
        cursor: p.next_cursor.clone(),
        limit: 1,
    };
    assert!(catalog(&f.s, FLOW, "ws_other", q, &CancellationToken::new()).is_err());
}
#[test]
fn inline_subagent_owns_nested_subdirectory_without_global_run_directory() {
    let mut f = fixture();
    let inline = f.dir.join("sub/run_inline/transcript.jsonl");
    fs::create_dir_all(inline.parent().unwrap()).unwrap();
    fs::write(&inline, prologue("run_inline")).unwrap();
    let first = page(&f, None, 50).unwrap();
    let child = selected(&f, &first, 0).unwrap();
    let reply = transcript_follow::read(
        &f.s,
        FLOW,
        WS,
        transcript_follow::Query {
            action: transcript_follow::Action::Latest,
            cursor: None,
            limit: 10,
        },
        Some(child),
        CancellationToken::new(),
    )
    .unwrap();
    f.token = serde_json::to_value(reply).unwrap()["poll_cursor"]
        .as_str()
        .unwrap()
        .into();
    f.dir = inline.parent().unwrap().into();
    let grand = f.dir.join("sub/run_grand/transcript.jsonl");
    fs::create_dir_all(grand.parent().unwrap()).unwrap();
    fs::write(&grand, prologue("run_grand")).unwrap();
    let next = page(&f, None, 50).unwrap();
    assert_eq!(next.parent_run_id, "run_inline");
    assert_eq!(next.records[0].child_run_id, "run_grand");
    let selected = selected(&f, &next, 0).unwrap();
    assert_eq!(selected.authority.leaf_path(), grand.to_str().unwrap());
    fs::create_dir_all(f.s.run_store.root.join("run_inline")).unwrap();
    assert!(page(&f, None, 50).is_err());
}
#[test]
fn directory_scope_reads_exact_registration_and_detects_replacement() {
    let f = fixture();
    let project = f.s.global_dir.join("project");
    fs::create_dir_all(&project).unwrap();
    let dir = f.s.global_dir.join("workspaces");
    fs::create_dir_all(&dir).unwrap();
    let registration: rupu_workspace::Workspace =
        serde_json::from_value(json!({"id":WS,"path":project,"created_at":"2026-11-09T00:00:00Z"}))
            .unwrap();
    fs::write(
        dir.join(format!("{WS}.toml")),
        toml::to_string(&registration).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("unrelated.toml"), "not valid toml").unwrap();
    let scoped = scope(&f.s, &f.parent).unwrap();
    assert!(scoped.verify(&f.s, &f.parent).is_ok());
    fs::rename(&dir, f.s.global_dir.join("old-workspaces")).unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join(format!("{WS}.toml")),
        toml::to_string(&registration).unwrap(),
    )
    .unwrap();
    assert!(scoped.verify(&f.s, &f.parent).is_err());
}
#[test]
fn malformed_identity_rows_do_not_poison_valid_neighbors_and_names_are_byte_bounded() {
    let f = fixture();
    let good = start(&f, "good", "run_good");
    let mut invalid_id = start(&f, "invalid_id", "run_bad");
    invalid_id.run_id = "x".repeat(129);
    let mut invalid_handle = start(&f, "initial", "run_bad_handle");
    invalid_handle.handle = "界".repeat(86);
    let mut long_name = start(&f, "long_name", "run_named");
    long_name.name = format!("{}😀tail", "a".repeat(4095));
    write_ledger(&f, &[invalid_id, good, invalid_handle, long_name]);
    let p = page(&f, None, 50).unwrap();
    assert_eq!(p.records.len(), 2);
    assert_eq!(p.evidence.malformed_rows, 2);
    assert!(!p.source_complete);
    assert_eq!(p.records[0].child_run_id, "run_good");
    assert_eq!(p.records[1].name.len(), 4095);
    assert!(p.records.iter().all(|r| r.child_run_id.len() <= 128
        && r.handle.as_ref().is_none_or(|h| h.len() <= 256)
        && r.name.len() <= 4096));
    assert!(selected(&f, &p, 0).is_ok());
    assert!(selected(&f, &p, 1).is_ok());
}
#[test]
fn concurrent_append_stamp_conflict_is_normal_window_retry_and_other_errors_survive() {
    let f = fixture();
    let path = f.dir.join("children.jsonl");
    fs::write(&path, b"first\n").unwrap();
    let file = open(&path).unwrap();
    let stamp = Stamp::of(&file.metadata().unwrap()).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"second\n")
        .unwrap();
    let error = checked_stamp(&path, &file, &stamp).unwrap_err();
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert_eq!(error.1, "transcript_window_changed_during_read");
    let error = stamp_error(ApiError(StatusCode::FORBIDDEN, "unchanged".into()));
    assert_eq!(error.0, StatusCode::FORBIDDEN);
    assert_eq!(error.1, "unchanged");
}

#[test]
fn workflow_root_back_edge_is_rejected_even_with_matching_child_prologue() {
    let f = fixture();
    let started = start(&f, "ch_back", FLOW);
    write_ledger(&f, &[started]);
    let catalog = page(&f, None, 10).unwrap();
    let error = selected(&f, &catalog, 0).err().unwrap();
    assert_eq!(error.1, "descendant_ancestry_mismatch");
}
#[test]
fn cancellation_while_waiting_for_cache_lock_prevents_publication() {
    let f = fixture();
    let (parent, identity) =
        transcript_follow::authorize_parent(&f.s, FLOW, WS, &f.token, &CancellationToken::new())
            .unwrap();
    let snapshot = Arc::new(capture(&f.s, &parent, identity, &CancellationToken::new()).unwrap());
    let cache = &f.s.workflow_observations.descendants;
    let held = cache.snapshots.lock().unwrap();
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let s = f.s.clone();
    let (ready, waiting) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let work = Work::new(&worker_cancel);
        ready.send(()).unwrap();
        publish(&s.workflow_observations.descendants, snapshot, &work)
    });
    waiting.recv().unwrap();
    cancel.cancel();
    drop(held);
    assert!(worker.join().unwrap().is_err());
    assert!(cache.snapshots.lock().unwrap().is_empty());
}
