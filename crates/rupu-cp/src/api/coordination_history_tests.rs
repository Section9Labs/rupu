use super::*;
use rupu_fleet::{Address, Bus, Member, OperatorBoardWrite, OperatorVia, PostKind};
use serde_json::json;
use std::io::Write;

fn seed(global: &Path, route: MessageRoute, id: &str, archived: bool) -> PathBuf {
    let collection = match (route, archived) {
        (MessageRoute::Sessions, false) => "sessions",
        (MessageRoute::Sessions, true) => "sessions-archive",
        (MessageRoute::Runs, false) => "runs",
        (MessageRoute::Runs, true) => "runs-archive",
        (MessageRoute::Agentiflows, _) => "agentiflows",
    };
    let dir = global.join(collection).join(id);
    fs::create_dir_all(&dir).unwrap();
    let (file, record) = match route {
        MessageRoute::Sessions => (
            "session.json",
            json!({"session_id":id,"status":"completed","created_at":"2026-11-11T00:00:00Z"}),
        ),
        MessageRoute::Runs => (
            "run.json",
            json!({"id":id,"workflow_name":"example","status":"completed"}),
        ),
        MessageRoute::Agentiflows => ("agentiflow.json", json!({"id":id,"status":"completed"})),
    };
    fs::write(dir.join(file), record.to_string()).unwrap();
    let space = if route == MessageRoute::Agentiflows {
        dir
    } else {
        dir.join("messages")
    };
    fs::create_dir_all(space.join("board")).unwrap();
    space
}
fn query(source: Source, limit: usize, cursor: Option<String>) -> HistoryQuery {
    HistoryQuery {
        host: None,
        source,
        limit: Some(limit),
        cursor,
    }
}
fn seen(id: usize) -> Value {
    json!({"t":"seen","id":format!("msg_{id}"),"by":"worker","turn":id,"at":"2026-11-11T00:00:00Z"})
}
fn append(path: &Path, value: &Value) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{value}").unwrap();
}
fn value(page: &Page) -> Value {
    serde_json::to_value(page).unwrap()
}

#[test]
fn owner_scoped_sources_preserve_canonical_ids_without_claim_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Sessions, "ses_one", false);
    let worker = Bus::open(&space, "worker");
    worker
        .join(Member {
            agent: "reader".into(),
            run_id: "run_reader".into(),
            ..Default::default()
        })
        .unwrap();
    let operator = Bus::open(&space, "operator");
    let receipt = operator
        .operator_board(
            OperatorBoardWrite::Post {
                kind: PostKind::Question,
                to: Some(Address::parse("worker").unwrap()),
                body: "Review this task".into(),
                expected_run_id: Some("run_reader".into()),
            },
            OperatorVia::Cp,
            None,
            || Ok(()),
        )
        .unwrap();
    operator
        .mark_seen(&[receipt.id().into()], "worker", 4)
        .unwrap();
    let grant = operator
        .operator_claim("work-key", std::time::Duration::from_secs(30), || Ok(()))
        .unwrap();
    let lease = match grant {
        rupu_fleet::OperatorClaimOutcome::Granted { lease } => lease,
        _ => panic!("claim denied"),
    };
    operator
        .operator_release("work-key", &lease.token, || Ok(()))
        .unwrap();
    let messages = read(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Messages, 100, None),
    )
    .unwrap();
    assert_eq!(messages.owner.id, "ses_one");
    assert_eq!(messages.owner.kind, "session");
    assert_eq!(messages.records.len(), 2);
    assert_eq!(value(&messages)["records"][0]["event"]["id"], receipt.id());
    assert_eq!(
        value(&messages)["records"][0]["event"]["post_kind"],
        "question"
    );
    assert_eq!(value(&messages)["records"][1]["event"]["t"], "seen");
    assert!(value(&messages)["records"][1]["event"]
        .get("run_id")
        .is_none());
    let claims = read(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Claims, 100, None),
    )
    .unwrap();
    assert_eq!(claims.records.len(), 2);
    assert_eq!(value(&claims)["records"][0]["event"]["ev"], "claimed");
    assert_eq!(value(&claims)["records"][1]["event"]["ev"], "released");
    assert!(!serde_json::to_string(&claims)
        .unwrap()
        .contains(&lease.token));
    assert!(claims
        .records
        .iter()
        .all(|r| r.event.as_ref().unwrap().get("token").is_none()));
    assert_ne!(messages.snapshot, claims.snapshot);
    assert_eq!(messages.ordering, "source_append_order");
    assert_eq!(messages.consistency, "fenced_source_observation");
}

#[test]
fn append_between_pages_keeps_original_fence_and_refresh_reads_new_records() {
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Runs, "run_one", false);
    let path = space.join("log.jsonl");
    for id in 0..5 {
        append(&path, &seen(id));
    }
    let first = read(
        tmp.path(),
        MessageRoute::Runs,
        "run_one",
        &query(Source::Messages, 2, None),
    )
    .unwrap();
    append(&path, &seen(5));
    let mut all = first.records.iter().map(|r| r.position).collect::<Vec<_>>();
    let mut cursor = first.next_cursor.clone();
    while let Some(token) = cursor {
        let page = read(
            tmp.path(),
            MessageRoute::Runs,
            "run_one",
            &query(Source::Messages, 2, Some(token)),
        )
        .unwrap();
        assert_eq!(page.snapshot, first.snapshot);
        assert_eq!(page.fence_bytes, first.fence_bytes);
        all.extend(page.records.iter().map(|r| r.position));
        cursor = page.next_cursor;
    }
    assert_eq!(all, [1, 2, 3, 4, 5]);
    let refreshed = read(
        tmp.path(),
        MessageRoute::Runs,
        "run_one",
        &query(Source::Messages, 100, None),
    )
    .unwrap();
    assert_eq!(refreshed.records.len(), 6);
    assert_ne!(refreshed.snapshot, first.snapshot);
}

#[test]
fn append_during_page_read_is_excluded_without_invalidating_returned_records() {
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Sessions, "ses_one", false);
    let path = space.join("log.jsonl");
    append(&path, &seen(1));
    let page = read_checked(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Messages, 10, None),
        || append(&path, &seen(2)),
    )
    .unwrap();
    assert_eq!(page.records.len(), 1);
    assert!(!page.has_more);
    assert!(page.fence_bytes < fs::metadata(path).unwrap().len());
}

#[test]
fn truncation_replacement_and_verified_boundary_rewrites_require_reset() {
    for change in ["truncate", "replace", "head", "tail", "page"] {
        let tmp = tempfile::tempdir().unwrap();
        let space = seed(tmp.path(), MessageRoute::Sessions, "ses_one", false);
        let path = space.join("log.jsonl");
        for id in 0..150 {
            append(&path, &seen(id));
        }
        let first = read(
            tmp.path(),
            MessageRoute::Sessions,
            "ses_one",
            &query(Source::Messages, 1, None),
        )
        .unwrap();
        if change == "page" {
            let result = read_checked(
                tmp.path(),
                MessageRoute::Sessions,
                "ses_one",
                &query(Source::Messages, 1, first.next_cursor.clone()),
                || {
                    let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
                    file.seek(SeekFrom::Start(5000)).unwrap();
                    file.write_all(b"x").unwrap();
                },
            );
            assert_eq!(result.err().unwrap().0, axum::http::StatusCode::CONFLICT);
            continue;
        }
        match change {
            "truncate" => fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(1)
                .unwrap(),
            "replace" => {
                let replacement = space.join("replacement");
                fs::copy(&path, &replacement).unwrap();
                fs::rename(replacement, &path).unwrap();
            }
            _ => {
                let offset = if change == "head" {
                    5
                } else {
                    first.fence_bytes - 5
                };
                let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.seek(SeekFrom::Start(offset)).unwrap();
                file.write_all(b"x").unwrap();
            }
        }
        let error = read(
            tmp.path(),
            MessageRoute::Sessions,
            "ses_one",
            &query(Source::Messages, 1, first.next_cursor),
        )
        .err()
        .unwrap();
        assert_eq!(error.0, axum::http::StatusCode::CONFLICT, "{change}");
    }
}

#[test]
fn opaque_cursors_are_owner_source_and_domain_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let first = seed(tmp.path(), MessageRoute::Sessions, "ses_one", false);
    let second = seed(tmp.path(), MessageRoute::Sessions, "ses_two", false);
    for path in [first.join("log.jsonl"), second.join("log.jsonl")] {
        append(&path, &seen(1));
        append(&path, &seen(2));
    }
    let page = read(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Messages, 1, None),
    )
    .unwrap();
    let token = page.next_cursor.unwrap();
    for (id, source, token) in [
        ("ses_two", Source::Messages, token.clone()),
        ("ses_one", Source::Claims, token.clone()),
        ("ses_one", Source::Messages, token.clone() + "x"),
    ] {
        assert_eq!(
            read(
                tmp.path(),
                MessageRoute::Sessions,
                id,
                &query(source, 1, Some(token))
            )
            .err()
            .unwrap()
            .0,
            axum::http::StatusCode::BAD_REQUEST
        );
    }
    assert!(recorded_cursor::decode::<Cursor>("", &token).is_err());
}

#[test]
fn missing_sources_diagnostics_and_partial_tail_are_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Sessions, "ses_one", false);
    let absent = read(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Messages, 100, None),
    )
    .unwrap();
    assert!(!absent.coverage.source_present);
    assert!(absent.records.is_empty());
    let path = space.join("log.jsonl");
    fs::write(
        &path,
        b"\nnot json\n{\"t\":\"future\"}\n{\"t\":\"seen\"}\n\npartial",
    )
    .unwrap();
    let page = read(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Messages, 100, None),
    )
    .unwrap();
    assert_eq!(
        page.records.iter().map(|r| r.position).collect::<Vec<_>>(),
        [2, 3, 4]
    );
    assert_eq!(page.coverage.invalid_records, 2);
    assert_eq!(page.coverage.unsupported_records, 1);
    assert_eq!(page.coverage.incomplete_tail_bytes, 7);
    assert!(!page.has_more);
    // A final valid JSON object without a newline is still incomplete source data.
    fs::write(&path, seen(1).to_string()).unwrap();
    let page = read(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Messages, 100, None),
    )
    .unwrap();
    assert!(page.records.is_empty());
    assert!(page.coverage.incomplete_tail_bytes > 0);
}

#[test]
fn archived_and_completed_owners_are_readable_without_mutation_authority() {
    let tmp = tempfile::tempdir().unwrap();
    for (route, id, archived, kind) in [
        (MessageRoute::Sessions, "ses_old", true, "session"),
        (MessageRoute::Runs, "run_old", true, "workflow"),
        (MessageRoute::Agentiflows, "af_old", false, "agentiflow"),
    ] {
        let space = seed(tmp.path(), route, id, archived);
        append(&space.join("log.jsonl"), &seen(1));
        let page = read(tmp.path(), route, id, &query(Source::Messages, 100, None)).unwrap();
        assert_eq!(page.owner.kind, kind);
        assert_eq!(page.records.len(), 1);
    }
    assert!(read(
        tmp.path(),
        MessageRoute::Runs,
        "ses_old",
        &query(Source::Messages, 100, None)
    )
    .is_err());
    let space = seed(tmp.path(), MessageRoute::Runs, "run_pending", false);
    fs::remove_file(space.parent().unwrap().join("run.json")).unwrap();
    assert_eq!(
        read(
            tmp.path(),
            MessageRoute::Runs,
            "run_pending",
            &query(Source::Claims, 100, None)
        )
        .unwrap()
        .owner
        .kind,
        "standalone"
    );
}

#[test]
fn bounded_response_and_oversized_lines_do_not_leak_payloads() {
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Runs, "run_one", false);
    let path = space.join("log.jsonl");
    let mut bytes = vec![b'x'; 300 * 1024];
    bytes.push(b'\n');
    fs::write(&path, bytes).unwrap();
    append(&path, &seen(1));
    let page = read(
        tmp.path(),
        MessageRoute::Runs,
        "run_one",
        &query(Source::Messages, 100, None),
    )
    .unwrap();
    assert_eq!(page.records[0].status, "oversized");
    assert!(page.records[0].event.is_none());
    assert_eq!(page.records.len(), 2);
    fs::write(&path, vec![b'x'; MAX_SCAN as usize + 1]).unwrap();
    assert_eq!(
        read(
            tmp.path(),
            MessageRoute::Runs,
            "run_one",
            &query(Source::Messages, 100, None)
        )
        .err()
        .unwrap()
        .0,
        axum::http::StatusCode::PAYLOAD_TOO_LARGE
    );
    fs::write(&path, b"").unwrap();
    for id in 0..20 {
        append(
            &path,
            &json!({"t":"msg","id":format!("msg_{id}"),"ts":"t","kind":"post","from":"operator","to":null,"post_kind":"note","body":"x".repeat(200_000),"delivery":{"resolved":[]}}),
        );
    }
    let page = read(
        tmp.path(),
        MessageRoute::Runs,
        "run_one",
        &query(Source::Messages, 100, None),
    )
    .unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_RESPONSE);
    assert!(page.has_more);
    assert!(!page.records.is_empty());
}

#[cfg(unix)]
#[test]
fn source_and_parent_symlinks_and_fifos_are_refused() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Sessions, "ses_one", false);
    let target = tmp.path().join("outside");
    fs::write(&target, format!("{}\n", seen(1))).unwrap();
    let path = space.join("log.jsonl");
    symlink(&target, &path).unwrap();
    assert!(read(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Messages, 100, None)
    )
    .is_err());
    fs::remove_file(&path).unwrap();
    if crate::api::fs_open::test_support::mkfifo(&path) {
        let global = tmp.path().to_path_buf();
        assert!(
            crate::api::fs_open::test_support::within_deadline(move || read(
                &global,
                MessageRoute::Sessions,
                "ses_one",
                &query(Source::Messages, 100, None)
            ))
            .is_err()
        );
        fs::remove_file(&path).unwrap();
    }
    fs::remove_dir(space.join("board")).unwrap();
    fs::remove_dir(&space).unwrap();
    symlink(tmp.path(), &space).unwrap();
    assert!(read(
        tmp.path(),
        MessageRoute::Sessions,
        "ses_one",
        &query(Source::Messages, 100, None)
    )
    .is_err());
}

#[tokio::test]
async fn http_route_capability_and_remote_query_guards() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), MessageRoute::Sessions, "ses_one", false);
    let state = AppState::new(tmp.path().into(), Default::default());
    let router = crate::server::router(state, None);
    use tower::ServiceExt;
    for (query, status) in [
        ("source=messages", 200),
        ("source=claims&host=remote", 501),
        ("source=unknown", 400),
        ("source=messages&limit=0", 400),
        ("source=messages&extra=true", 400),
    ] {
        let reply = router
            .clone()
            .oneshot(
                http::Request::builder()
                    .uri(format!(
                        "/api/sessions/ses_one/coordination/history?{query}"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply.status().as_u16(), status);
    }
    let reply = router
        .oneshot(
            http::Request::builder()
                .uri("/api/host/info")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(reply.into_body(), MAX_RESPONSE)
        .await
        .unwrap();
    let info: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(info["features"]
        .as_array()
        .unwrap()
        .contains(&json!(CAPABILITY)));
}

#[test]
fn observation_does_not_claim_to_verify_unread_interior_ranges() {
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Runs, "run_one", false);
    let path = space.join("log.jsonl");
    for id in 0..2000 {
        append(&path, &seen(id));
    }
    let first = read(
        tmp.path(),
        MessageRoute::Runs,
        "run_one",
        &query(Source::Messages, 1, None),
    )
    .unwrap();
    // This is outside the head, tail, continuation boundary and next page's
    // bounded lookahead. The API promises range checks, not whole-file proof.
    let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(80_000)).unwrap();
    file.write_all(b"x").unwrap();
    let next = read(
        tmp.path(),
        MessageRoute::Runs,
        "run_one",
        &query(Source::Messages, 1, first.next_cursor),
    )
    .unwrap();
    assert_eq!(next.consistency, "fenced_source_observation");
    assert_eq!(next.snapshot, first.snapshot);
}

#[test]
fn blank_scan_window_returns_an_advancing_empty_page() {
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Runs, "run_one", false);
    let path = space.join("log.jsonl");
    let mut blank = vec![b' '; 1023];
    blank.push(b'\n');
    fs::write(&path, blank.repeat(MAX_SCAN as usize / 1024 + 1)).unwrap();
    append(&path, &seen(1));
    let first = read(
        tmp.path(),
        MessageRoute::Runs,
        "run_one",
        &query(Source::Messages, 100, None),
    )
    .unwrap();
    assert!(first.records.is_empty());
    assert!(first.has_more);
    let next = read(
        tmp.path(),
        MessageRoute::Runs,
        "run_one",
        &query(Source::Messages, 100, first.next_cursor),
    )
    .unwrap();
    assert_eq!(next.records.len(), 1);
    assert_eq!(next.records[0].position, MAX_SCAN / 1024 + 2);
    assert!(!next.has_more);
}

#[test]
fn near_limit_admission_reserves_final_cursor_and_diagnostics() {
    let tmp = tempfile::tempdir().unwrap();
    let space = seed(tmp.path(), MessageRoute::Runs, "run_one", false);
    let path = space.join("log.jsonl");
    // Four individually bounded rows approach the response limit; malformed
    // rows grow final coverage and ensure the continuation boundary is nonzero.
    for body_len in [260_000, 261_000, 261_500, 261_800] {
        fs::write(&path, b"invalid\n".repeat(12)).unwrap();
        for id in 0..6 {
            append(
                &path,
                &json!({"t":"msg","id":format!("msg_{id}"),
                "ts":"t","kind":"post","from":"operator","to":null,
                "post_kind":"note","body":"x".repeat(body_len),
                "delivery":{"resolved":[]}}),
            );
        }
        let first = read(
            tmp.path(),
            MessageRoute::Runs,
            "run_one",
            &query(Source::Messages, 100, None),
        )
        .unwrap();
        assert_eq!(first.coverage.invalid_records, 12);
        assert!(first.has_more);
        assert!(serde_json::to_vec(&first).unwrap().len() <= MAX_RESPONSE);
        let last_position = first.records.last().unwrap().position;
        let next = read(
            tmp.path(),
            MessageRoute::Runs,
            "run_one",
            &query(Source::Messages, 100, first.next_cursor),
        )
        .unwrap();
        assert_eq!(next.records.first().unwrap().position, last_position + 1);
    }
}

/// Opt-in read-only acceptance against explicitly selected local history. The
/// private manifest and exported DTOs never belong in the repository. Default
/// CI has no dependency on any real owner, path, content, or running service.
#[test]
fn optional_real_owner_history_exports_production_dtos() {
    let Some(path) = std::env::var_os("RUPU_COORDINATION_HISTORY_MANIFEST") else {
        return;
    };
    #[derive(Deserialize)]
    struct Manifest {
        selections: Vec<Selection>,
    }
    #[derive(Deserialize)]
    struct Selection {
        global_dir: PathBuf,
        route: String,
        id: String,
        source: Source,
        source_sha256: String,
        expected_message_ids: Vec<String>,
        expected_claim_count: usize,
        export_file: PathBuf,
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .unwrap()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() <= 1024 * 1024, "private manifest byte bound");
    let manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
    assert!((1..=8).contains(&manifest.selections.len()));
    for selected in manifest.selections {
        let route = MessageRoute::ALL
            .into_iter()
            .find(|r| r.segment() == selected.route)
            .expect("known owner route");
        let context = context(&selected.global_dir, route, &selected.id, selected.source).unwrap();
        let mut source = open_anchored_regular_file(&context.path).unwrap();
        let source_bytes = source.metadata().unwrap().len();
        assert!(source_bytes <= MAX_SCAN, "acceptance source byte bound");
        assert!(
            range_hash(&mut source, 0, source_bytes).unwrap() == selected.source_sha256,
            "private manifest source changed before observation"
        );
        let page = read(
            &selected.global_dir,
            route,
            &selected.id,
            &query(selected.source, 200, None),
        )
        .unwrap();
        assert!(page.owner.id == selected.id, "exact owner identity");
        assert_eq!(page.source, selected.source);
        assert_eq!(page.fence_bytes, source_bytes);
        assert!(
            !page.has_more,
            "private acceptance selection must fit one bounded page"
        );
        assert_eq!(page.coverage.invalid_records, 0);
        assert_eq!(page.coverage.unsupported_records, 0);
        for expected in selected.expected_message_ids {
            assert!(
                page.records.iter().any(|r| r.event.as_ref().is_some_and(
                    |e| e["t"] == "msg" && e["id"].as_str() == Some(expected.as_str())
                )),
                "expected canonical message identity missing"
            );
        }
        if selected.source == Source::Claims {
            assert_eq!(page.records.len(), selected.expected_claim_count);
            for record in &page.records {
                let event = record.event.as_ref().unwrap().as_object().unwrap();
                assert!(
                    event
                        .keys()
                        .all(|key| ["ev", "key", "owner", "at", "lease_expires_at"]
                            .contains(&key.as_str())),
                    "claim export contains only whitelisted fields"
                );
            }
        }
        assert!(
            range_hash(&mut source, 0, source_bytes).unwrap() == selected.source_sha256,
            "private manifest source changed during observation"
        );
        let exported = serde_json::to_vec_pretty(&page).unwrap();
        assert!(exported.len() <= MAX_RESPONSE);
        fs::write(selected.export_file, exported).unwrap();
    }
}
