use super::*;
use axum::http::StatusCode;
use std::io::Write;

fn event(text: &str) -> String {
    serde_json::json!({"type":"assistant_message","data":{"content":text}}).to_string()
}
fn query(path: &Path, limit: usize) -> PageQuery {
    PageQuery {
        path: path.to_string_lossy().into_owned(),
        run: Some("run_test".into()),
        limit: Some(limit),
        ..Default::default()
    }
}
fn page(path: &Path, query: &PageQuery) -> Page {
    read_page(path, query, "test-scope".into()).unwrap()
}

#[test]
fn numbered_pages_preserve_invalid_lines_and_finite_unterminated_eof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.jsonl");
    std::fs::write(
        &path,
        format!(
            "\n \r\n{}\ninvalid JSON\n{}",
            event("one"),
            event("last without newline")
        ),
    )
    .unwrap();
    let mut q = query(&path, 2);
    let first = page(&path, &q);
    assert_eq!(first.records.len(), 2);
    assert_eq!(first.records[0].position, 1);
    assert_eq!(first.records[0].status, "event");
    assert_eq!(first.records[1].position, 2);
    assert_eq!(first.records[1].status, "malformed");
    assert!(first.records[1].event.is_none());
    assert!(first.has_more);
    q.cursor = first.next_cursor;
    let cursor = decode(q.cursor.as_ref().unwrap()).unwrap();
    assert_eq!(cursor.position, 2);
    assert_eq!(
        cursor.offset,
        (format!("\n \r\n{}\ninvalid JSON\n", event("one"))).len() as u64
    );
    let last = page(&path, &q);
    assert_eq!(last.snapshot, first.snapshot);
    assert_eq!(last.records[0].position, 3);
    assert_eq!(
        last.records[0].event.as_ref().unwrap()["data"]["content"],
        "last without newline"
    );
    assert!(!last.has_more);
    assert!(last.next_cursor.is_none());
    assert_eq!(
        serde_json::to_value(&last).unwrap(),
        serde_json::to_value(page(&path, &q)).unwrap()
    );
}

#[test]
fn oversized_and_invalid_utf8_are_explicit_positions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "{}", "x".repeat(MAX_LINE_BYTES + 1)).unwrap();
    f.write_all(&[0xff, b'\n']).unwrap();
    writeln!(f, "{}", event("after omissions")).unwrap();
    drop(f);
    let result = page(&path, &query(&path, 200));
    assert_eq!(
        result
            .records
            .iter()
            .map(|r| (r.position, r.status))
            .collect::<Vec<_>>(),
        vec![(1, "oversized"), (2, "malformed"), (3, "event")]
    );
    assert!(result.records[0].event.is_none());
    assert!(result.records[1].event.is_none());
    assert!(!result.has_more);
}

#[test]
fn response_budget_includes_envelope_and_cursor_without_losing_next_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.jsonl");
    let text = "a".repeat(240 * 1024);
    std::fs::write(
        &path,
        (0..8).map(|_| event(&text) + "\n").collect::<String>(),
    )
    .unwrap();
    let mut q = query(&path, 200);
    let first = page(&path, &q);
    assert!(serde_json::to_vec(&first).unwrap().len() <= MAX_RESPONSE_BYTES);
    assert_eq!(first.records.len(), 4);
    assert!(first.has_more);
    q.cursor = first.next_cursor;
    let second = page(&path, &q);
    assert_eq!(second.records[0].position, 5);
    assert_eq!(second.records.len(), 4);
    assert!(!second.has_more);
    assert!(serde_json::to_vec(&second).unwrap().len() <= MAX_RESPONSE_BYTES);
}

#[test]
fn recorded_file_over_sixteen_mib_pages_to_eof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.jsonl");
    let line = event(&"x".repeat(100 * 1024)) + "\n";
    let mut f = std::fs::File::create(&path).unwrap();
    for _ in 0..180 {
        f.write_all(line.as_bytes()).unwrap();
    }
    drop(f);
    assert!(std::fs::metadata(&path).unwrap().len() > 16 * 1024 * 1024);
    let mut q = query(&path, 200);
    let mut seen = 0;
    loop {
        let result = page(&path, &q);
        assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_RESPONSE_BYTES);
        for record in &result.records {
            seen += 1;
            assert_eq!(record.position, seen);
            assert_eq!(record.status, "event");
        }
        q.cursor = result.next_cursor;
        if !result.has_more {
            break;
        }
    }
    assert_eq!(seen, 180);
}

#[test]
fn cursor_rejects_tampering_cross_scope_and_every_file_change() {
    for action in ["append", "truncate", "replace", "rewrite", "remove"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        let body = format!("{}\n{}\n", event("a"), event("b"));
        std::fs::write(&path, &body).unwrap();
        let mut q = query(&path, 1);
        q.cursor = page(&path, &q).next_cursor;
        assert_eq!(
            read_page(&path, &q, "foreign-scope".into()).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        let token = q.cursor.clone().unwrap();
        assert!(decode(&(token.clone() + "x")).is_err());
        match action {
            "append" => {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"\n")
                    .unwrap();
            }
            "truncate" => std::fs::write(&path, "").unwrap(),
            "replace" => {
                let new = dir.path().join("other");
                std::fs::write(&new, &body).unwrap();
                std::fs::rename(new, &path).unwrap();
            }
            "rewrite" => std::fs::write(&path, body.replace('a', "c")).unwrap(),
            _ => std::fs::remove_file(&path).unwrap(),
        }
        let error = read_page(&path, &q, "test-scope".into()).unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT, "{action}");
        assert!(error.1.contains("transcript_snapshot_changed"));
    }
}

#[test]
fn huge_single_line_is_scanned_without_retaining_it() {
    let data = vec![b'x'; 8 * 1024 * 1024];
    let mut reader = BufReader::with_capacity(8192, data.as_slice());
    let line = read_line(&mut reader, data.len() as u64).unwrap();
    assert_eq!(line.consumed, data.len() as u64);
    assert!(line.oversized && line.bytes.is_empty());
    assert!(line.bytes.capacity() <= MAX_LINE_BYTES);
}

#[test]
fn unicode_blank_lines_match_existing_numbered_cursor_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.jsonl");
    // Split a multi-byte whitespace scalar at the scanner's buffer boundary.
    let blank = " ".repeat(16 * 1024 - 1) + "\u{2003}\u{a0}\n";
    std::fs::write(&path, format!("{blank}\u{3000}\n{}\n", event("one"))).unwrap();
    let result = page(&path, &query(&path, 1));
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].position, 1);
    assert_eq!(result.records[0].status, "event");
    assert!(!result.has_more);
}

#[test]
fn scan_budget_limits_oversized_and_blank_only_prefixes_without_fake_progress() {
    for byte in [b'x', b' '] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![byte; MAX_SCAN_BYTES as usize + 1])
            .unwrap();
        f.write_all(b"\n").unwrap();
        drop(f);
        let error = read_page(&path, &query(&path, 200), "test-scope".into()).unwrap_err();
        assert_eq!(error.0, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(error.1.starts_with("transcript_page_scan_limit"));
    }
}

#[test]
fn scan_budget_can_return_preceding_complete_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "{}", event("first")).unwrap();
    f.write_all(&vec![b'x'; MAX_SCAN_BYTES as usize + 1])
        .unwrap();
    drop(f);
    let first = page(&path, &query(&path, 200));
    assert_eq!(first.records.len(), 1);
    assert!(first.has_more);
    let cursor = decode(first.next_cursor.as_ref().unwrap()).unwrap();
    assert_eq!(cursor.offset, (event("first").len() + 1) as u64);
}

#[tokio::test]
async fn handler_preserves_authorization_and_cursor_scope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.jsonl");
    std::fs::write(&path, format!("{}\n{}\n", event("a"), event("b"))).unwrap();
    let state = AppState::new(
        dir.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    let mut q = query(&path, 1);
    let Json(first) = get_page(State(state.clone()), Query(q.clone()))
        .await
        .unwrap();
    assert_eq!(first.run.as_deref(), Some("run_test"));
    assert_eq!(first.path, q.path);
    q.cursor = first.next_cursor;
    let mut changed = q.clone();
    changed.run = Some("run_other".into());
    assert_eq!(
        get_page(State(state.clone()), Query(changed))
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut remote = q.clone();
    remote.host = Some("remote".into());
    assert_eq!(
        get_page(State(state.clone()), Query(remote))
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    let outside = tempfile::NamedTempFile::with_suffix(".jsonl").unwrap();
    let forbidden = query(outside.path(), 1);
    assert_eq!(
        get_page(State(state.clone()), Query(forbidden))
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let missing = query(&dir.path().join("pending.jsonl"), 1);
    assert_eq!(
        get_page(State(state.clone()), Query(missing))
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    let Json(last) = get_page(State(state), Query(q)).await.unwrap();
    assert_eq!(last.records[0].position, 2);
}

#[cfg(unix)]
#[tokio::test]
async fn changing_symlink_target_invalidates_same_requested_path() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.jsonl");
    let b = dir.path().join("b.jsonl");
    let alias = dir.path().join("alias.jsonl");
    let body = format!("{}\n{}\n", event("a"), event("b"));
    std::fs::write(&a, &body).unwrap();
    std::fs::write(&b, &body).unwrap();
    std::os::unix::fs::symlink(&a, &alias).unwrap();
    let state = AppState::new(
        dir.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    let mut q = query(&alias, 1);
    q.cursor = get_page(State(state.clone()), Query(q.clone()))
        .await
        .unwrap()
        .0
        .next_cursor;
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&b, &alias).unwrap();
    assert_eq!(
        get_page(State(state), Query(q)).await.unwrap_err().0,
        StatusCode::CONFLICT
    );
}
