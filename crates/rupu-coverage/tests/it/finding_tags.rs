//! `ledger::tags::apply`: the one writer of a workspace's finding-tag log.

use chrono::Utc;
use rupu_coverage::ledger::tags::{
    apply, ingest_tag_events, merge_tag_log_copies, TagChange, TagError, TagLogMerge, TagOutcome,
};
use rupu_coverage::{
    append_record, parse_tags, read_tag_events, read_workspace_findings, Attribution,
    CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingScope, Ledger,
    OperatorSurface, RunStream, Severity, Surface, Tag, TagActor, TagEvent, TagLog, TagOp,
};
use std::path::Path;

fn finding(id: &str) -> FindingRecord {
    FindingRecord {
        id: id.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: FindingScope::Repo,
        summary: format!("summary of {id}"),
        severity: Severity::Medium,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: "run_seed".into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: Utc::now(),
        profile: FindingProfile::Summary,
        report: None,
        tags: vec![],
    }
}

/// `fnd_a`, `fnd_b` in target `t1`; `fnd_c` in target `t2`.
fn seed(ws: &Path) {
    let t1 = CoveragePaths::new(ws, "t1");
    let t2 = CoveragePaths::new(ws, "t2");
    append_record(&t1, Ledger::Findings, &finding("fnd_a")).unwrap();
    append_record(&t1, Ledger::Findings, &finding("fnd_b")).unwrap();
    append_record(&t2, Ledger::Findings, &finding("fnd_c")).unwrap();
}

fn tags(raw: &[&str]) -> Vec<Tag> {
    parse_tags(raw).unwrap()
}

fn change(ids: &[&str], add: &[&str], remove: &[&str]) -> TagChange {
    TagChange {
        finding_ids: ids.iter().map(|s| s.to_string()).collect(),
        add: tags(add),
        remove: tags(remove),
    }
}

fn by() -> TagActor {
    TagActor::operator(OperatorSurface::Cli)
}

fn tags_of(ws: &Path, id: &str) -> Vec<String> {
    read_workspace_findings(ws)
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap()
        .tags
        .into_iter()
        .map(String::from)
        .collect()
}

#[test]
fn apply_tags_findings_across_targets_and_reports_before_and_after() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    let out = apply(
        &log,
        &change(&["fnd_c", "fnd_a"], &["class:sqli", "needs-poc"], &[]),
        &by(),
    )
    .unwrap();
    assert_eq!(
        out,
        vec![
            TagOutcome {
                finding_id: "fnd_a".into(),
                before: vec![],
                after: tags(&["class:sqli", "needs-poc"])
            },
            TagOutcome {
                finding_id: "fnd_c".into(),
                before: vec![],
                after: tags(&["class:sqli", "needs-poc"])
            },
        ]
    );
    assert_eq!(tags_of(ws.path(), "fnd_c"), ["class:sqli", "needs-poc"]);
    assert!(tags_of(ws.path(), "fnd_b").is_empty());
    let events = read_tag_events(&log).unwrap();
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|e| matches!(e.by, TagActor::Operator(_))));
}

#[test]
fn a_change_that_changes_nothing_writes_nothing() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    apply(&log, &change(&["fnd_a"], &["x"], &[]), &by()).unwrap();
    let before = std::fs::read(&log.path).unwrap();
    let out = apply(&log, &change(&["fnd_a"], &["x"], &["never-had"]), &by()).unwrap();
    assert!(!out[0].changed());
    assert_eq!(std::fs::read(&log.path).unwrap(), before);
}

#[test]
fn remove_then_readd_is_recorded_and_folds_in_order() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    apply(&log, &change(&["fnd_a"], &["x", "y"], &[]), &by()).unwrap();
    apply(&log, &change(&["fnd_a"], &[], &["x"]), &by()).unwrap();
    assert_eq!(tags_of(ws.path(), "fnd_a"), ["y"]);
    apply(&log, &change(&["fnd_a"], &["x"], &[]), &by()).unwrap();
    assert_eq!(tags_of(ws.path(), "fnd_a"), ["x", "y"]);
}

#[test]
fn an_unknown_id_rejects_the_whole_batch_and_leaves_the_log_untouched() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    apply(&log, &change(&["fnd_a"], &["seed"], &[]), &by()).unwrap();
    let before = std::fs::read(&log.path).unwrap();
    let err = apply(&log, &change(&["fnd_a", "fnd_nope"], &["x"], &[]), &by()).unwrap_err();
    assert!(
        matches!(&err, TagError::UnknownFindings(ids) if ids == &["fnd_nope".to_string()]),
        "{err}"
    );
    assert_eq!(std::fs::read(&log.path).unwrap(), before);
}

#[test]
fn invalid_changes_are_refused_before_anything_is_read() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    assert!(matches!(
        apply(&log, &change(&["fnd_a"], &[], &[]), &by()),
        Err(TagError::EmptyChange)
    ));
    assert!(matches!(
        apply(&log, &change(&[], &["x"], &[]), &by()),
        Err(TagError::NoFindings)
    ));
    assert!(matches!(
        apply(&log, &change(&["fnd_a"], &["x", "y"], &["y"]), &by()),
        Err(TagError::Conflict(t)) if t == tags(&["y"])
    ));
    assert!(!log.path.exists());
}

#[test]
fn the_tag_cap_is_enforced_per_finding() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    let many: Vec<String> = (0..32).map(|i| format!("t{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    apply(&log, &change(&["fnd_a"], &many, &[]), &by()).unwrap();
    let err = apply(&log, &change(&["fnd_a"], &["one-more"], &[]), &by()).unwrap_err();
    assert!(
        matches!(err, TagError::TooManyTags { count: 33, .. }),
        "{err}"
    );
}

#[test]
fn a_workspace_without_coverage_has_no_findings_and_gets_no_files() {
    let ws = tempfile::TempDir::new().unwrap();
    let log = TagLog::for_workspace(ws.path());
    let err = apply(&log, &change(&["fnd_a"], &["x"], &[]), &by()).unwrap_err();
    assert!(matches!(err, TagError::UnknownFindings(_)));
    assert!(!ws.path().join(".rupu").exists());
}

#[test]
fn events_are_mirrored_to_the_run_stream() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let stream = ws.path().join("runs/run_1/coverage.jsonl");
    rupu_coverage::write_stream_begin(&stream, "run_1").unwrap();
    let log = TagLog::for_workspace(ws.path()).with_run_stream(Some(RunStream {
        path: stream.clone(),
        scope_name: "tagger".into(),
    }));
    apply(&log, &change(&["fnd_a"], &["x"], &[]), &by()).unwrap();
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&stream)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let tag_line = lines
        .iter()
        .find(|l| l["ledger"] == "tags")
        .expect("a tags line");
    assert_eq!(tag_line["scope_name"], "tagger");
    assert_eq!(tag_line["record"]["finding_id"], "fnd_a");
    assert_eq!(tag_line["record"]["op"], "add");
}

#[test]
fn concurrent_writers_lose_no_events() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let root = ws.path().to_path_buf();
    let writers: Vec<_> = (0..2)
        .map(|w| {
            let root = root.clone();
            std::thread::spawn(move || {
                let log = TagLog::for_workspace(&root);
                for i in 0..20 {
                    let tag = format!("w{w}-{i}");
                    // Stay under the per-finding cap: writer 0 tags fnd_a, writer 1 fnd_b.
                    let id = if w == 0 { "fnd_a" } else { "fnd_b" };
                    let ch = TagChange {
                        finding_ids: vec![id.to_string()],
                        add: vec![Tag::parse(&tag).unwrap()],
                        remove: if i > 0 {
                            vec![Tag::parse(&format!("w{w}-{}", i - 1)).unwrap()]
                        } else {
                            vec![]
                        },
                    };
                    apply(&log, &ch, &TagActor::operator(OperatorSurface::Cli)).unwrap();
                }
            })
        })
        .collect();
    let appender = {
        let root = root.clone();
        std::thread::spawn(move || {
            let t3 = CoveragePaths::new(&root, "t3");
            for i in 0..20 {
                append_record(&t3, Ledger::Findings, &finding(&format!("fnd_new{i}"))).unwrap();
            }
        })
    };
    for h in writers {
        h.join().unwrap();
    }
    appender.join().unwrap();
    let log = TagLog::for_workspace(&root);
    // 1 add for the first iteration, then 1 remove + 1 add for each of the other 19, per writer.
    assert_eq!(read_tag_events(&log).unwrap().len(), 2 * (1 + 19 * 2));
    let text = std::fs::read_to_string(&log.path).unwrap();
    assert!(text
        .lines()
        .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()));
    assert_eq!(tags_of(&root, "fnd_a"), ["w0-19"]);
    assert_eq!(tags_of(&root, "fnd_b"), ["w1-19"]);
}

/// Deterministic: hold the tag log's sidecar lock ourselves and show a writer
/// waits on it. Unlike the stress test above, this fails if the lock is a no-op.
#[test]
fn a_writer_waits_for_the_tag_log_lock() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    std::fs::create_dir_all(log.lock.parent().unwrap()).unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&log.lock)
        .unwrap();
    held.lock().unwrap();

    let writer = {
        let root = ws.path().to_path_buf();
        std::thread::spawn(move || {
            let log = TagLog::for_workspace(&root);
            apply(&log, &change(&["fnd_a"], &["locked"], &[]), &by()).unwrap()
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        !writer.is_finished(),
        "apply returned while the lock was held"
    );
    assert!(
        !log.path.exists(),
        "apply wrote the tag log while the lock was held"
    );

    drop(held);
    let out = writer.join().unwrap();
    assert!(out[0].changed());
    assert_eq!(read_tag_events(&log).unwrap().len(), 1);
    assert_eq!(tags_of(ws.path(), "fnd_a"), ["locked"]);
}

/// Racing writers asking for the same tag: whoever takes the lock second sees
/// the first's event and finds nothing to do, so exactly one `add` lands.
#[test]
fn racing_writers_adding_the_same_tag_land_exactly_one_event() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let writers: Vec<_> = (0..8)
        .map(|_| {
            let root = ws.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let log = TagLog::for_workspace(&root);
                barrier.wait();
                apply(&log, &change(&["fnd_a"], &["same"], &[]), &by()).unwrap()
            })
        })
        .collect();
    let changed = writers
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|out| out[0].changed())
        .count();
    assert_eq!(changed, 1);
    let events = read_tag_events(&TagLog::for_workspace(ws.path())).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].op, TagOp::Add);
}

/// A crash can leave the log ending mid-line with no newline; the next event
/// must start on its own line instead of gluing onto the torn one.
#[test]
fn an_event_appended_after_a_torn_tail_line_is_not_swallowed() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    std::fs::create_dir_all(log.path.parent().unwrap()).unwrap();
    std::fs::write(
        &log.path,
        br#"{"id":"tge_torn","finding_id":"fnd_a","op":"ad"#,
    )
    .unwrap();

    apply(&log, &change(&["fnd_a"], &["after-crash"], &[]), &by()).unwrap();

    let events = read_tag_events(&log).unwrap();
    assert_eq!(
        events.len(),
        1,
        "the torn line is skipped, the new event kept"
    );
    assert_eq!(events[0].tag.as_str(), "after-crash");
    assert_eq!(tags_of(ws.path(), "fnd_a"), ["after-crash"]);
}

#[test]
fn a_finding_already_over_the_cap_can_still_shrink_but_never_grow() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    // Two remote units, each within the cap, push fnd_a to 34 on the coordinator.
    let events: Vec<TagEvent> = (0..34)
        .map(|i| TagEvent {
            id: format!("tge_seed{i}"),
            finding_id: "fnd_a".into(),
            op: TagOp::Add,
            tag: Tag::parse(&format!("t{i}")).unwrap(),
            by: by(),
            at: Utc::now(),
        })
        .collect();
    assert_eq!(ingest_tag_events(&log, events).unwrap(), (34, 0));
    assert_eq!(tags_of(ws.path(), "fnd_a").len(), 34);

    // An add that grows it past the cap is refused.
    let err = apply(&log, &change(&["fnd_a"], &["one-more"], &[]), &by()).unwrap_err();
    assert!(
        matches!(err, TagError::TooManyTags { count: 35, .. }),
        "{err}"
    );

    // A change that keeps the count (remove 1, add 1) is allowed.
    let out = apply(&log, &change(&["fnd_a"], &["swapped"], &["t0"]), &by()).unwrap();
    assert_eq!(out[0].after.len(), 34);
    assert_eq!(tags_of(ws.path(), "fnd_a").len(), 34);

    // A remove-only change lowers it.
    let out = apply(&log, &change(&["fnd_a"], &[], &["t1", "t2"]), &by()).unwrap();
    assert_eq!(out[0].before.len(), 34);
    assert_eq!(out[0].after.len(), 32);
    assert_eq!(tags_of(ws.path(), "fnd_a").len(), 32);
}

#[test]
fn merging_a_units_copy_of_the_log_keeps_events_written_meanwhile() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    // Both copies start from the log as it was at dispatch.
    apply(&log, &change(&["fnd_a"], &["shared"], &[]), &by()).unwrap();
    let at_dispatch = std::fs::read(&log.path).unwrap();

    // The unit tags on its copy (another workspace) ...
    let unit_ws = tempfile::TempDir::new().unwrap();
    seed(unit_ws.path());
    let unit_log = TagLog::for_workspace(unit_ws.path());
    std::fs::write(&unit_log.path, &at_dispatch).unwrap();
    apply(&unit_log, &change(&["fnd_b"], &["from-unit"], &[]), &by()).unwrap();
    let mut unit_copy = std::fs::read(&unit_log.path).unwrap();
    unit_copy.extend_from_slice(b"{not an event}\n\n");

    // ... while an operator tags on the coordinator.
    apply(&log, &change(&["fnd_a"], &["from-operator"], &[]), &by()).unwrap();

    let merged = merge_tag_log_copies(&log, &[&unit_copy]).unwrap();
    assert_eq!(
        merged,
        TagLogMerge {
            appended: 1,
            duplicates: 1,
            unreadable: 1
        }
    );
    assert_eq!(tags_of(ws.path(), "fnd_a"), ["from-operator", "shared"]);
    assert_eq!(tags_of(ws.path(), "fnd_b"), ["from-unit"]);
    // Merging the same copy again changes nothing.
    assert_eq!(
        merge_tag_log_copies(&log, &[&unit_copy, &unit_copy]).unwrap(),
        TagLogMerge {
            appended: 0,
            duplicates: 4,
            unreadable: 2
        },
        "merging copies again, twice in one pass, adds nothing"
    );
}
