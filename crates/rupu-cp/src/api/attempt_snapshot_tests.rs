use super::*;
use std::io::Write;

fn fixture(count: usize) -> (tempfile::TempDir, Owner, PathBuf, Cache) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("runs");
    fs::create_dir_all(root.join("run_root")).unwrap();
    let mut file =
        std::io::BufWriter::new(FileForTest::create(root.join("run_root/attempts.jsonl")).unwrap());
    for i in 0..count {
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({
                "v":1, "step_id":if i % 2 == 0 { "review" } else { "check" },
                "unit_index":i % 7, "sub_id":if i % 3 == 0 { Some("panel") } else { None },
                "agent_run_id":format!("run_{i}"), "transcript_path":format!("/absent/{i}.jsonl"),
                "started_at":"2026-11-06T10:00:00Z"
            }),
        )
        .unwrap();
        file.write_all(b"\n").unwrap();
    }
    file.flush().unwrap();
    let owner = Owner {
        global_dir: tmp.path().into(),
        run_store: root.clone(),
    };
    (tmp, owner, root, Cache::default())
}

use std::fs::File as FileForTest;

#[test]
fn attempt_snapshot_pages_large_filtered_population_once_and_ignores_appends() {
    let (_tmp, owner, root, mut cache) = fixture(12000);
    let filters = Filters {
        step_id: Some("review".into()),
        unit_index: Some(0),
        sub_id: Some("panel".into()),
    };
    let first = cache
        .page(&owner, &root, "run_root", filters.clone(), 17, None)
        .unwrap();
    let expected: Vec<_> = (0..12000)
        .filter(|i| i % 2 == 0 && i % 7 == 0 && i % 3 == 0)
        .map(|i| format!("run_{i}"))
        .collect();
    assert_eq!(first.recorded_matching_total, Some(expected.len()));
    let capture = cache.snapshots[0].bytes.as_ptr();
    let path = root.join("run_root/attempts.jsonl");
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"not part of this snapshot\n")
        .unwrap();
    let mut observed: Vec<_> = first.rows.into_iter().map(|r| r.agent_run_id).collect();
    let mut cursor = first.next_cursor;
    while let Some(token) = cursor {
        let page = cache
            .page(&owner, &root, "run_root", filters.clone(), 17, Some(&token))
            .unwrap();
        assert_eq!(page.snapshot, first.snapshot);
        assert_eq!(page.evidence.malformed_rows, 0);
        assert_eq!(page.recorded_matching_total, Some(expected.len()));
        observed.extend(page.rows.into_iter().map(|r| r.agent_run_id));
        cursor = page.next_cursor;
    }
    assert_eq!(observed, expected);
    assert_eq!(cache.snapshots.len(), 1);
    assert_eq!(
        cache.snapshots[0].bytes.as_ptr(),
        capture,
        "pages reuse one captured buffer"
    );
    let refreshed = cache
        .page(&owner, &root, "run_root", filters, 17, None)
        .unwrap();
    assert_ne!(refreshed.snapshot, first.snapshot);
    assert_eq!(refreshed.evidence.malformed_rows, 1);
}

#[test]
fn attempt_snapshot_rejects_truncation_replacement_and_in_place_rewrite() {
    for change in ["truncate", "replace", "rewrite"] {
        let (_tmp, owner, root, mut cache) = fixture(4);
        let page = cache
            .page(&owner, &root, "run_root", Filters::default(), 1, None)
            .unwrap();
        let path = root.join("run_root/attempts.jsonl");
        match change {
            "truncate" => FileForTest::create(&path).unwrap().set_len(1).unwrap(),
            "replace" => {
                fs::write(path.with_extension("replacement"), fs::read(&path).unwrap()).unwrap();
                fs::rename(path.with_extension("replacement"), &path).unwrap();
            }
            _ => {
                let file = OpenOptions::new().write(true).open(&path).unwrap();
                file.set_modified(SystemTime::now() + Duration::from_secs(2))
                    .unwrap();
            }
        }
        let error = cache
            .page(
                &owner,
                &root,
                "run_root",
                Filters::default(),
                1,
                page.next_cursor.as_deref(),
            )
            .err()
            .unwrap();
        assert_eq!(error.0, axum::http::StatusCode::CONFLICT, "{change}");
    }
}

#[test]
fn attempt_snapshot_ttl_and_eviction_are_explicit_conflicts() {
    let (_tmp, owner, root, mut cache) = fixture(2);
    let first = cache
        .page(&owner, &root, "run_root", Filters::default(), 1, None)
        .unwrap();
    cache.snapshots[0].created = Instant::now() - TTL;
    assert_eq!(
        cache
            .page(
                &owner,
                &root,
                "run_root",
                Filters::default(),
                1,
                first.next_cursor.as_deref()
            )
            .err()
            .unwrap()
            .0,
        axum::http::StatusCode::CONFLICT
    );
    for _ in 0..MAX_SNAPSHOTS + 1 {
        cache
            .page(&owner, &root, "run_root", Filters::default(), 1, None)
            .unwrap();
    }
    assert_eq!(cache.snapshots.len(), MAX_SNAPSHOTS);
    assert!(cache.snapshots.iter().map(Snapshot::weight).sum::<usize>() <= MAX_CACHE_BYTES);
    assert_eq!(
        cache
            .page(
                &owner,
                &root,
                "run_root",
                Filters::default(),
                1,
                first.next_cursor.as_deref()
            )
            .err()
            .unwrap()
            .0,
        axum::http::StatusCode::CONFLICT
    );
}

#[test]
fn attempt_snapshot_preserves_duplicate_run_ids_and_physical_line_locations() {
    let (_tmp, owner, root, mut cache) = fixture(1);
    let path = root.join("run_root/attempts.jsonl");
    let line = fs::read(&path).unwrap();
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"\n").unwrap();
    file.write_all(&line).unwrap();
    let page = cache
        .page(&owner, &root, "run_root", Filters::default(), 200, None)
        .unwrap();
    assert_eq!(page.recorded_matching_total, Some(2));
    assert_eq!(page.rows[0].agent_run_id, page.rows[1].agent_run_id);
    assert_eq!((page.rows[0].ledger_line, page.rows[1].ledger_line), (1, 3));
}

#[test]
fn attempt_snapshot_refuses_oversize_without_truncating_and_reports_bad_rows() {
    let (_tmp, owner, root, mut cache) = fixture(0);
    let path = root.join("run_root/attempts.jsonl");
    FileForTest::create(&path)
        .unwrap()
        .set_len((MAX_LEDGER_BYTES + 1) as u64)
        .unwrap();
    assert_eq!(
        cache
            .page(&owner, &root, "run_root", Filters::default(), 1, None)
            .err()
            .unwrap()
            .0,
        axum::http::StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(cache.snapshots.is_empty());
    let mut file = FileForTest::create(&path).unwrap();
    file.write_all(&vec![b'x'; MAX_ROW_BYTES + 1]).unwrap();
    file.write_all(b"\n\n{bad}\n").unwrap();
    file.write_all(br#"{"v":2,"step_id":"s","agent_run_id":"run_x","transcript_path":"/x","started_at":"2026-11-06T10:00:00Z"}"#).unwrap();
    file.write_all(b"\n{\"v\":3}\nunfinished").unwrap();
    let page = cache
        .page(&owner, &root, "run_root", Filters::default(), 1, None)
        .unwrap();
    assert_eq!(page.evidence.oversized_rows, 1);
    assert_eq!(page.evidence.malformed_rows, 1);
    assert_eq!(page.evidence.unsupported_rows, 2);
    assert_eq!(page.evidence.incomplete_tail_bytes, 10);
    assert_eq!(page.evidence.parsing, "partial");
    assert_eq!(page.recorded_matching_total, Some(0));
}

#[cfg(unix)]
#[test]
fn attempt_snapshot_later_pages_need_no_ledger_content_read() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, owner, root, mut cache) = fixture(4);
    let first = cache
        .page(&owner, &root, "run_root", Filters::default(), 1, None)
        .unwrap();
    let path = root.join("run_root/attempts.jsonl");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
    let result = cache.page(
        &owner,
        &root,
        "run_root",
        Filters::default(),
        1,
        first.next_cursor.as_deref(),
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(result.unwrap().rows[0].agent_run_id, "run_1");
}
