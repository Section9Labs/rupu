use super::*;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn work() -> Work {
    Work {
        cancel: CancellationToken::new(),
        started: Instant::now(),
        bytes: 0,
    }
}
fn event(text: &str) -> Vec<u8> {
    let mut bytes =
        serde_json::to_vec(&json!({"type":"assistant_delta","data":{"content":text}})).unwrap();
    bytes.push(b'\n');
    bytes
}
fn authority(path: &Path) -> Authority {
    Authority {
        ancestry: Vec::new(),
        workflow_run_id: "run_workflow".into(),
        workspace_id: "ws_fixture".into(),
        child_run_id: "run_child".into(),
        path: path.to_str().unwrap().into(),
        transcript_root: path.parent().unwrap().to_str().unwrap().into(),
        workflow_started_at: "2026-11-09T00:00:00Z".into(),
        workflow_name: "flow".into(),
        workspace_path: "/fixture".into(),
        step_id: "step".into(),
        member_record_id: "attempt:0".into(),
        prologue_sha256: "known-prologue".into(),
        store_kind: "global".into(),
    }
}
fn fixture(bytes: &[u8]) -> (tempfile::TempDir, PathBuf, Cache) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().canonicalize().unwrap().join("child.jsonl");
    fs::write(&path, bytes).unwrap();
    (tmp, path, Cache::default())
}
fn member(path: &Path) -> VerifiedMember {
    let file = crate::api::fs_open::open_anchored_regular_file(path).unwrap();
    let stamp = Stamp::of(&file.metadata().unwrap()).unwrap();
    VerifiedMember {
        authority: authority(path),
        file,
        path: path.into(),
        prologue: json!({"run_id":"run_child","workspace_id":"ws_fixture"}),
        stamp,
    }
}
fn query(action: Action, limit: usize) -> Query {
    Query {
        action,
        cursor: None,
        limit,
    }
}
fn run(cache: &Cache, path: &Path, action: Action, prior: Option<Cursor>, limit: usize) -> Reply {
    read_verified(
        cache,
        "run_workflow",
        "ws_fixture",
        query(action, limit),
        prior,
        member(path),
        work(),
    )
    .unwrap()
}
fn as_json(reply: &Reply) -> Value {
    serde_json::to_value(reply).unwrap()
}

#[test]
fn latest_is_bounded_near_eof_and_selects_last_records_without_prefix_scan() {
    let mut bytes = vec![b'x'; 4 * MAX_WINDOW as usize];
    bytes.push(b'\n');
    let start = bytes.len();
    bytes.extend(event("one"));
    bytes.extend(event("two"));
    bytes.extend(event("three"));
    let (_tmp, path, cache) = fixture(&bytes);
    let reply = run(&cache, &path, Action::Latest, None, 2);
    let v = as_json(&reply);
    assert_eq!(v["records"].as_array().unwrap().len(), 2);
    assert_eq!(v["records"][0]["event"]["data"]["content"], "two");
    assert_eq!(v["records"][1]["event"]["data"]["content"], "three");
    assert!(
        v["window"]["display_start_byte"]
            .as_str()
            .unwrap()
            .parse::<usize>()
            .unwrap()
            > start
    );
    let cursor = decode(&cache, &reply.poll_cursor).unwrap();
    assert_eq!(cursor.anchor.end - cursor.anchor.start, MAX_WINDOW);
    assert!(cursor.anchor.start > 0);
    assert_eq!(v["window"]["complete_source"], false);
}
#[test]
fn unchanged_poll_keeps_same_window_and_parses_no_body() {
    let (_tmp, path, cache) = fixture(&event("hello"));
    let first = run(&cache, &path, Action::Latest, None, 100);
    let old = decode(&cache, &first.poll_cursor).unwrap();
    let poll = run(&cache, &path, Action::Poll, Some(old), 100);
    let p = as_json(&poll);
    assert_eq!(p["state"], "unchanged");
    assert_eq!(p["relationship"], "same_snapshot");
    assert_eq!(poll.snapshot, first.snapshot);
    assert!(poll.records.is_empty());
    assert!(poll.window.is_none());
}
#[test]
fn ordinary_append_is_normal_changed_then_bounded_fresh_latest() {
    let (_tmp, path, cache) = fixture(&event("old"));
    let first = run(&cache, &path, Action::Latest, None, 100);
    let old = decode(&cache, &first.poll_cursor).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&event("new"))
        .unwrap();
    let poll = run(&cache, &path, Action::Poll, Some(old.clone()), 100);
    assert_eq!(poll.state, "changed");
    assert_eq!(poll.relationship, "retained_snapshot");
    assert_eq!(poll.snapshot, first.snapshot);
    assert!(poll.records.is_empty());
    let latest = run(&cache, &path, Action::Latest, Some(old), 100);
    assert_eq!(latest.relationship, "fresh_latest");
    assert_ne!(latest.snapshot, first.snapshot);
    assert_eq!(latest.records.len(), 2);
    assert_eq!(latest.records[0].identity, first.records[0].identity);
    assert_eq!(
        latest.records[0].content_sha256,
        first.records[0].content_sha256
    );
}
#[test]
fn earlier_remains_usable_after_append_with_only_bounded_anchor_proof() {
    let bytes = [event("one"), event("two"), event("three"), event("four")].concat();
    let (_tmp, path, cache) = fixture(&bytes);
    let recent = run(&cache, &path, Action::Latest, None, 2);
    let prior = decode(&cache, recent.earlier_cursor.as_ref().unwrap()).unwrap();
    let target = prior.anchor.before.unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&event("five"))
        .unwrap();
    let earlier = run(&cache, &path, Action::Earlier, Some(prior), 2);
    let v = as_json(&earlier);
    assert_eq!(earlier.relationship, "verified_anchor_fresh_window");
    assert_eq!(earlier.previous_snapshot, Some(recent.snapshot));
    assert_eq!(v["window"]["requested_before_byte"], target.to_string());
    assert_eq!(v["records"][0]["event"]["data"]["content"], "one");
    assert_eq!(v["records"][1]["event"]["data"]["content"], "two");
    assert!(earlier.earlier_cursor.is_none());
}
#[test]
fn earlier_refuses_changed_observed_anchor_even_with_same_file_identity() {
    let bytes = [event("one"), event("two"), event("three")].concat();
    let (_tmp, path, cache) = fixture(&bytes);
    let recent = run(&cache, &path, Action::Latest, None, 1);
    let prior = decode(&cache, recent.earlier_cursor.as_ref().unwrap()).unwrap();
    let mut replacement = bytes;
    let location = replacement.windows(3).position(|s| s == b"one").unwrap();
    replacement[location..location + 3].copy_from_slice(b"ONE");
    fs::write(&path, replacement).unwrap();
    let error = read_verified(
        &cache,
        "run_workflow",
        "ws_fixture",
        query(Action::Earlier, 100),
        Some(prior),
        member(&path),
        work(),
    )
    .err()
    .unwrap();
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert_eq!(error.1, "transcript_window_anchor_changed");
}
#[test]
fn earlier_does_not_claim_unobserved_prefix_unchanged() {
    let mut bytes = event("prologue-placeholder-for-private-unit-helper");
    bytes.extend(event("old-prefix"));
    bytes.extend(vec![b' '; MAX_WINDOW as usize]);
    bytes.push(b'\n');
    bytes.extend(event("near"));
    bytes.extend(event("last"));
    let (_tmp, path, cache) = fixture(&bytes);
    let recent = run(&cache, &path, Action::Latest, None, 1);
    let prior = decode(&cache, recent.earlier_cursor.as_ref().unwrap()).unwrap();
    assert!(prior.anchor.start > 0);
    // A rewrite outside the checked anchor is permitted only as a new finite
    // observation, never as proof that old global history remained unchanged.
    let at = bytes.windows(10).position(|v| v == b"old-prefix").unwrap();
    bytes[at..at + 3].copy_from_slice(b"OLD");
    bytes.extend(event("appended")); // guarantees a different full stamp
    fs::write(&path, bytes).unwrap();
    let earlier = run(&cache, &path, Action::Earlier, Some(prior), 100);
    assert_eq!(earlier.relationship, "verified_anchor_fresh_window");
}
#[test]
fn file_replacement_and_observed_truncation_fail_closed() {
    let bytes = [event("one"), event("two")].concat();
    let (_tmp, path, cache) = fixture(&bytes);
    let first = run(&cache, &path, Action::Latest, None, 1);
    let old = decode(&cache, &first.poll_cursor).unwrap();
    fs::write(&path, event("x")).unwrap();
    let error = read_verified(
        &cache,
        "run_workflow",
        "ws_fixture",
        query(Action::Poll, 1),
        Some(old.clone()),
        member(&path),
        work(),
    )
    .err()
    .unwrap();
    assert_eq!(error.1, "transcript_window_file_truncated");
    let replacement = path.with_extension("new");
    fs::write(&replacement, &bytes).unwrap();
    fs::rename(replacement, &path).unwrap();
    let error = read_verified(
        &cache,
        "run_workflow",
        "ws_fixture",
        query(Action::Poll, 1),
        Some(old),
        member(&path),
        work(),
    )
    .err()
    .unwrap();
    assert_eq!(error.1, "transcript_window_file_replaced");
}
#[test]
fn exact_mac_scope_refuses_tampering_other_instance_and_owner() {
    let (_tmp, path, cache) = fixture(&event("hello"));
    let first = run(&cache, &path, Action::Latest, None, 1);
    assert!(decode(&Cache::default(), &first.poll_cursor).is_err());
    let mut token = first.poll_cursor.into_bytes();
    token[4] = if token[4] == b'A' { b'B' } else { b'A' };
    assert!(decode(&cache, std::str::from_utf8(&token).unwrap()).is_err());
    assert!(validate_owner(&authority(&path), "run_other", "ws_fixture").is_err());
    assert!(validate_owner(&authority(&path), "run_workflow", "ws_other").is_err());
}
#[test]
fn partial_leading_and_unterminated_tail_are_not_fabricated_events() {
    let raw = [
        b"cut leading\n".to_vec(),
        event("complete"),
        b"{\"type\":\"assistant_delta\"".to_vec(),
    ]
    .concat();
    let auth = authority(Path::new("/fixture/a.jsonl"));
    let parsed = window(&raw, 10, 10 + raw.len() as u64, 100, "file", &auth, &work()).unwrap();
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].byte_offset, (10 + 12).to_string());
    assert_eq!(
        parsed.partial_tail,
        b"{\"type\":\"assistant_delta\"".len() as u64
    );
    assert!(parsed.before.is_some());
    let raw = b"unterminated suffix";
    let parsed = window(raw, 42, 42 + raw.len() as u64, 100, "file", &auth, &work()).unwrap();
    assert_eq!(parsed.partial_tail, raw.len() as u64);
    assert!(parsed.records.is_empty());
    assert!(parsed.before.is_none());
}
#[test]
fn known_start_boundary_and_newline_end_are_exact_byte_positions() {
    let raw = [b"\n".to_vec(), event("one"), event("two")].concat();
    let auth = authority(Path::new("/fixture/a.jsonl"));
    let parsed = window(&raw, 99, 99 + raw.len() as u64, 100, "file", &auth, &work()).unwrap();
    assert_eq!(parsed.records[0].byte_offset, "100");
    assert_eq!(
        parsed.records.last().unwrap().byte_end,
        (99 + raw.len()).to_string()
    );
    assert_eq!(parsed.partial_tail, 0);
}
#[test]
fn malformed_oversized_and_foreign_run_start_rows_remain_explicit() {
    let mut oversized = vec![b'x'; 300 * 1024];
    oversized.push(b'\n');
    let foreign = serde_json::json!({"type":"run_start","data":{"run_id":"run_other","workspace_id":"ws_fixture","agent":"a","provider":"p","model":"m","started_at":"2026-11-09T00:00:00Z","mode":"readonly"}});
    let mut foreign = serde_json::to_vec(&foreign).unwrap();
    foreign.push(b'\n');
    let raw = [b"not-json\n".to_vec(), oversized, foreign].concat();
    let auth = authority(Path::new("/fixture/a.jsonl"));
    let parsed = window(&raw, 0, raw.len() as u64, 100, "file", &auth, &work()).unwrap();
    assert_eq!(parsed.records[0].status, "malformed");
    assert_eq!(parsed.records[1].status, "oversized");
    assert_eq!(parsed.records[2].status, "scope_mismatch");
    assert!(parsed.records.iter().all(|r| r.event.is_none()));
}
#[test]
fn cancellation_is_checked_before_body_io_and_bounded_intervals() {
    let (_tmp, path, _cache) = fixture(&event("hello"));
    let mut file = File::open(&path).unwrap();
    let mut w = work();
    w.cancel.cancel();
    assert_eq!(
        w.interval(&mut file, 0, 1).err().unwrap().0,
        StatusCode::REQUEST_TIMEOUT
    );
    assert_eq!(w.bytes, 0);
    let mut w = work();
    assert_eq!(
        w.interval(&mut file, 0, MAX_WINDOW + 1).err().unwrap().0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(w.bytes, 0);
}
#[test]
fn cursor_boundary_and_anchor_limits_are_validated_before_io() {
    let (_tmp, path, cache) = fixture(&event("hello"));
    let first = run(&cache, &path, Action::Latest, None, 1);
    let mut c = decode(&cache, &first.poll_cursor).unwrap();
    c.anchor.end = c.anchor.start + MAX_WINDOW + 1;
    assert!(decode(&cache, &encode(&cache, &c).unwrap()).is_err());
    let mut c = decode(&cache, &first.poll_cursor).unwrap();
    c.anchor.before = Some(c.anchor.end + 1);
    assert!(decode(&cache, &encode(&cache, &c).unwrap()).is_err());
}
#[test]
fn mutation_after_prologue_stamp_is_refused_before_publishing() {
    let (_tmp, path, cache) = fixture(&event("old"));
    let m = member(&path);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&event("new"))
        .unwrap();
    let e = read_verified(
        &cache,
        "run_workflow",
        "ws_fixture",
        query(Action::Latest, 100),
        None,
        m,
        work(),
    )
    .err()
    .unwrap();
    assert_eq!(e.1, "transcript_window_changed_during_read");
}

#[test]
fn cancellation_during_reply_serialization_cannot_publish_success() {
    struct CancelOnSerialize(CancellationToken);
    impl Serialize for CancelOnSerialize {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.0.cancel();
            serializer.serialize_str("serialized payload")
        }
    }
    let w = work();
    let value = CancelOnSerialize(w.cancel.clone());
    assert_eq!(
        serialized_size_checked(&value, &w).err().unwrap().1,
        "transcript_window_cancelled"
    );
    let w = Work {
        cancel: CancellationToken::new(),
        started: Instant::now() - std::time::Duration::from_secs(6),
        bytes: 0,
    };
    assert_eq!(
        serialized_size_checked(&"bounded", &w).err().unwrap().1,
        "transcript_window_work_limit"
    );
}
