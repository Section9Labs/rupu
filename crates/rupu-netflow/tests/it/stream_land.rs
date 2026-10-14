//! The run-scoped netflow stream (`stream.rs`) and its landing on the
//! coordinator (`land.rs`), spec 2026-10-14 §N1/§N3.

use rupu_netflow::land::*;
use rupu_netflow::stream::*;
use rupu_netflow::LedgerLine;

fn dropped(n: u64) -> LedgerLine {
    LedgerLine::Dropped {
        count: n,
        ts: chrono::Utc::now(),
    }
}

#[test]
fn begin_is_written_once_and_lines_are_tagged() {
    let tmp = tempfile::tempdir().unwrap();
    let p = stream_path(tmp.path(), "run_A");
    write_stream_begin(&p, "run_A").unwrap();
    write_stream_begin(&p, "run_A").unwrap();
    append_stream_line(&p, "run_A", &dropped(1)).unwrap();
    append_stream_line(&p, "sub_B", &dropped(2)).unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert!(lines[0].contains(r#""kind":"begin""#));
    assert!(lines[1].contains(r#""ledger":"run_A""#) && lines[1].contains(r#""type":"dropped""#));
    assert!(lines[2].contains(r#""ledger":"sub_B""#));
    assert!(stream_has_begin(&p));
}

#[test]
fn a_missing_or_unbegun_stream_has_no_begin() {
    let tmp = tempfile::tempdir().unwrap();
    let p = stream_path(tmp.path(), "run_X");
    assert!(!stream_has_begin(&p));
    append_stream_line(&p, "run_X", &dropped(1)).unwrap();
    assert!(
        !stream_has_begin(&p),
        "a line before begin is not a begun stream"
    );
}

#[test]
fn parallel_writers_interleave_whole_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let p = stream_path(tmp.path(), "run_P");
    write_stream_begin(&p, "run_P").unwrap();
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let p = p.clone();
            std::thread::spawn(move || {
                for _ in 0..50 {
                    append_stream_line(&p, &format!("run_{t}"), &dropped(t)).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let parsed = parse_stream(&std::fs::read(&p).unwrap());
    assert!(parsed.begin_seen);
    assert_eq!(parsed.malformed, 0);
    assert_eq!(parsed.unsafe_ids, 0);
    assert_eq!(parsed.ledgers.len(), 8);
    assert_eq!(parsed.ledgers.values().map(Vec::len).sum::<usize>(), 400);
}

#[test]
fn landing_splits_by_ledger_and_records_complete() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let s = stream_path(tmp.path().join("host").as_path(), "run_U");
    write_stream_begin(&s, "run_U").unwrap();
    append_stream_line(&s, "run_U", &dropped(1)).unwrap();
    append_stream_line(&s, "sub_X", &dropped(2)).unwrap();
    let rec = land_and_record(
        &runs,
        "run_U",
        "host_ssh",
        Some(&std::fs::read(&s).unwrap()),
        true,
        None,
    )
    .unwrap();
    assert_eq!(rec.status, LandingStatus::Complete);
    assert_eq!(rec.reason, None);
    assert_eq!(rec.host, "host_ssh");
    assert_eq!(rec.ledgers, vec!["run_U".to_string(), "sub_X".to_string()]);
    assert!(tmp.path().join("netflow/run_U.jsonl").exists());
    assert!(tmp.path().join("netflow/sub_X.jsonl").exists());
    assert!(
        tmp.path().join("netflow/.gitignore").exists(),
        "the landing directory is self-ignoring"
    );
    let landed: Vec<LedgerLine> = std::fs::read_to_string(tmp.path().join("netflow/run_U.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(landed.len(), 1);
    assert!(matches!(landed[0], LedgerLine::Dropped { count: 1, .. }));
    assert_eq!(read_landing_record(&runs, "run_U").unwrap(), rec);
    let leftovers: Vec<_> = std::fs::read_dir(tmp.path().join("netflow"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn no_stream_and_no_begin_are_unavailable_and_land_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let rec = land_and_record(&runs, "run_N", "h", None, true, None).unwrap();
    assert_eq!(rec.status, LandingStatus::Unavailable);
    assert_eq!(rec.reason.as_deref(), Some(NOT_STREAMED_REASON));
    assert!(rec.ledgers.is_empty());
    assert_eq!(read_landing_record(&runs, "run_N").unwrap(), rec);
    let rec = land_and_record(
        &runs,
        "run_N2",
        "h",
        None,
        true,
        Some("peer refused".into()),
    )
    .unwrap();
    assert_eq!(
        rec.reason.as_deref(),
        Some("peer refused"),
        "a caller's reason wins"
    );
    let body = br#"{"kind":"line","ledger":"run_N","line":{"type":"dropped","count":1,"ts":"2026-10-14T00:00:00Z"}}"#;
    let rec = land_and_record(&runs, "run_M", "h", Some(body), true, None).unwrap();
    assert_eq!(rec.status, LandingStatus::Unavailable);
    assert_eq!(rec.reason.as_deref(), Some(NO_BEGIN_REASON));
    assert!(!tmp.path().join("netflow/run_N.jsonl").exists());
    assert!(
        !tmp.path().join("netflow").exists(),
        "nothing landed, nothing created"
    );
}

#[test]
fn unsafe_ids_and_malformed_lines_are_skipped_and_make_it_partial() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let body = concat!(
        r#"{"kind":"begin","v":1,"run_id":"run_Z"}"#,
        "\n",
        r#"{"kind":"line","ledger":"../escape","line":{"type":"x"}}"#,
        "\n",
        r#"{"kind":"line","ledger":"a/b","line":{"type":"x"}}"#,
        "\n",
        "not json\n",
        r#"{"kind":"line","ledger":"run_Z","line":{"type":"x"}}"#,
        "\n",
    );
    let parsed = parse_stream(body.as_bytes());
    assert_eq!((parsed.malformed, parsed.unsafe_ids), (1, 2));
    let rec = land_and_record(&runs, "run_Z", "h", Some(body.as_bytes()), true, None).unwrap();
    assert_eq!(rec.status, LandingStatus::Partial);
    assert_eq!(rec.ledgers, vec!["run_Z".to_string()]);
    assert!(rec.reason.unwrap().contains("3 malformed"));
    assert!(!tmp.path().join("escape.jsonl").exists());
    assert!(!tmp.path().join("netflow/escape.jsonl").exists());
    assert!(!tmp.path().join("netflow/a").exists());
    assert!(tmp.path().join("netflow/run_Z.jsonl").exists());
    assert!(!is_safe_ledger_id(""));
    assert!(!is_safe_ledger_id(&"a".repeat(129)));
    assert!(is_safe_ledger_id(&"a".repeat(128)));
    assert!(!is_safe_ledger_id("a b"));
    assert!(!is_safe_ledger_id("a.b"));
    assert!(is_safe_ledger_id("run_01ABC-x"));
}

#[test]
fn a_second_begin_a_line_without_ledger_and_an_unknown_kind_are_malformed() {
    let body = concat!(
        r#"{"kind":"begin","v":1,"run_id":"run_W"}"#,
        "\n",
        r#"{"kind":"begin","v":1,"run_id":"run_W"}"#,
        "\n",
        r#"{"kind":"line","line":{"type":"x"}}"#,
        "\n",
        r#"{"kind":"line","ledger":"run_W"}"#,
        "\n",
        r#"{"kind":"mystery"}"#,
        "\n",
        r#"{"no":"kind"}"#,
        "\n",
        "\r\n",
        r#"{"kind":"line","ledger":"run_W","line":{"type":"x"}}"#,
        "\r\n",
    );
    let parsed = parse_stream(body.as_bytes());
    assert!(parsed.begin_seen);
    assert_eq!(parsed.malformed, 5);
    assert_eq!(parsed.unsafe_ids, 0);
    assert_eq!(parsed.ledgers["run_W"], vec![r#"{"type":"x"}"#.to_string()]);
}

#[test]
fn unknown_line_kinds_survive_landing_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let body = concat!(
        r#"{"kind":"begin","v":1,"run_id":"run_F"}"#,
        "\n",
        r#"{"kind":"line","ledger":"run_F","line":{"type":"from_the_future","x":[1,2]}}"#,
        "\n",
    );
    land_and_record(&runs, "run_F", "h", Some(body.as_bytes()), true, None).unwrap();
    let landed = std::fs::read_to_string(tmp.path().join("netflow/run_F.jsonl")).unwrap();
    assert_eq!(landed.trim(), r#"{"type":"from_the_future","x":[1,2]}"#);
}

#[test]
fn complete_is_never_downgraded_and_a_newer_complete_replaces() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let one = concat!(
        r#"{"kind":"begin","v":1,"run_id":"run_R"}"#,
        "\n",
        r#"{"kind":"line","ledger":"run_R","line":{"type":"a"}}"#,
        "\n",
    );
    let first = land_and_record(&runs, "run_R", "h", Some(one.as_bytes()), true, None).unwrap();
    let rec = land_and_record(&runs, "run_R", "h", None, false, Some("later".into())).unwrap();
    assert_eq!(rec, first, "kept the complete record untouched");
    assert_eq!(read_landing_record(&runs, "run_R").unwrap(), first);
    let partial = concat!(
        r#"{"kind":"begin","v":1,"run_id":"run_R"}"#,
        "\n",
        r#"{"kind":"line","ledger":"run_R","line":{"type":"a"}}"#,
        "\n",
        r#"{"kind":"line","ledger":"run_R","line":{"type":"b"}}"#,
        "\n",
        r#"{"kind":"line","ledger":"run_R","line":{"type":"c"}}"#,
        "\n",
    );
    let rec = land_and_record(&runs, "run_R", "h", Some(partial.as_bytes()), false, None).unwrap();
    assert_eq!(
        rec.status,
        LandingStatus::Complete,
        "a partial copy never downgrades"
    );
    let landed = std::fs::read_to_string(tmp.path().join("netflow/run_R.jsonl")).unwrap();
    assert_eq!(
        landed.lines().count(),
        1,
        "a refused downgrade lands nothing: {landed}"
    );
    let two = format!(
        "{one}{}\n",
        r#"{"kind":"line","ledger":"run_R","line":{"type":"b"}}"#
    );
    let rec = land_and_record(&runs, "run_R", "h2", Some(two.as_bytes()), true, None).unwrap();
    assert_eq!(rec.status, LandingStatus::Complete);
    assert_eq!(rec.host, "h2", "a newer complete record replaces");
    let landed = std::fs::read_to_string(tmp.path().join("netflow/run_R.jsonl")).unwrap();
    assert_eq!(
        landed.lines().count(),
        2,
        "replaced, not appended: {landed}"
    );
}

#[test]
fn partial_then_complete_upgrades() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let body = concat!(r#"{"kind":"begin","v":1,"run_id":"run_Q"}"#, "\n");
    let r = land_and_record(
        &runs,
        "run_Q",
        "h",
        Some(body.as_bytes()),
        false,
        Some("snapshot".into()),
    )
    .unwrap();
    assert_eq!(
        (r.status, r.reason.as_deref()),
        (LandingStatus::Partial, Some("snapshot"))
    );
    let r = land_and_record(&runs, "run_Q", "h", Some(body.as_bytes()), true, None).unwrap();
    assert_eq!((r.status, r.reason), (LandingStatus::Complete, None));
    let r = land_and_record(&runs, "run_Q2", "h", None, true, None).unwrap();
    assert_eq!(r.status, LandingStatus::Unavailable);
    let r = land_and_record(
        &runs,
        "run_Q2",
        "h",
        Some(body.as_bytes()),
        false,
        Some("s".into()),
    )
    .unwrap();
    assert_eq!(
        r.status,
        LandingStatus::Partial,
        "unavailable is replaced by partial"
    );
}

#[test]
fn landing_record_round_trips_and_tolerates_missing_optional_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    std::fs::create_dir_all(runs.join("run_J")).unwrap();
    std::fs::write(
        landing_record_path(&runs, "run_J"),
        br#"{"v":1,"host":"h","status":"complete","at":"2026-10-14T00:00:00Z"}"#,
    )
    .unwrap();
    let r = read_landing_record(&runs, "run_J").unwrap();
    assert_eq!(
        (r.status, r.reason, r.ledgers),
        (LandingStatus::Complete, None, vec![])
    );
    std::fs::write(landing_record_path(&runs, "run_J"), b"{").unwrap();
    assert!(read_landing_record(&runs, "run_J").is_none());
    assert!(read_landing_record(&runs, "run_absent").is_none());
    let written = land_and_record(&runs, "run_K", "h", None, false, None).unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(landing_record_path(&runs, "run_K")).unwrap())
            .unwrap();
    assert_eq!(json["status"], "unavailable");
    assert_eq!(json["v"], 1);
    assert_eq!(json["reason"], NOT_STREAMED_REASON);
    assert_eq!(
        serde_json::from_value::<LandingRecord>(json).unwrap(),
        written
    );
}

#[test]
fn roster_appends_and_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let l = RosterLine {
        unit_run_id: "run_u1".into(),
        host: "h".into(),
        step_id: "s".into(),
        index: Some(2),
    };
    append_roster_line(tmp.path(), "run_wf", &l).unwrap();
    append_roster_line(
        tmp.path(),
        "run_wf",
        &RosterLine {
            index: None,
            ..l.clone()
        },
    )
    .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(roster_path(tmp.path(), "run_wf"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"garbage\n"))
        .unwrap();
    let got = read_roster(tmp.path(), "run_wf");
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], l);
    assert_eq!(got[1].index, None);
    let text = std::fs::read_to_string(roster_path(tmp.path(), "run_wf")).unwrap();
    assert!(
        text.lines().nth(1).unwrap().ends_with(r#""step_id":"s"}"#),
        "no index key: {text}"
    );
    assert!(read_roster(tmp.path(), "run_none").is_empty());
    assert!(tmp
        .path()
        .join("run_wf")
        .join("netflow.units.jsonl.lock")
        .exists());
}

#[test]
fn landed_lines_keep_the_hosts_bytes_exactly() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    // keys deliberately out of alphabetical order, odd spacing preserved
    let line = r#"{"type":"dropped","ts":"2026-10-14T00:00:00Z","count":3}"#;
    let body = format!(
        "{}\n{{\"kind\":\"line\",\"ledger\":\"run_K\",\"line\":{line}}}\n",
        r#"{"kind":"begin","v":1,"run_id":"run_K"}"#
    );
    land_and_record(&runs, "run_K", "h", Some(body.as_bytes()), true, None).unwrap();
    let landed = std::fs::read_to_string(tmp.path().join("netflow/run_K.jsonl")).unwrap();
    assert_eq!(landed, format!("{line}\n"));
}
