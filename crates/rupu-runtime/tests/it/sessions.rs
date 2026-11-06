//! Session records' versioned parse and the archived-session prune
//! (`rupu_runtime::sessions`), over a `RUPU_HOME` in a temp dir.

use rupu_runtime::retention::PruneError;
use rupu_runtime::sessions::{
    archived_sessions_dir, parse_session_record, parse_stored_session, prune_report, SessionRecord,
    StoredSession,
};
use std::path::{Path, PathBuf};

/// An archived session record at `version`, last updated `days` ago, with
/// one run whose transcript sits in `<global>/transcripts`.
fn archive_session(global: &Path, id: &str, version: u32, days: i64) -> PathBuf {
    let dir = archived_sessions_dir(global).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let transcripts = global.join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    let transcript = transcripts.join(format!("run_{id}.jsonl"));
    std::fs::write(&transcript, "{}\n").unwrap();
    let at = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    let record = serde_json::json!({
        "version": version,
        "session_id": id,
        "agent_name": "a",
        "provider_name": "anthropic",
        "model": "m",
        "agent_system_prompt": "",
        "max_turns": 1,
        "permission_mode": "readonly",
        "no_stream": true,
        "workspace_id": "ws",
        "workspace_path": global,
        "transcripts_dir": transcripts,
        "created_at": at,
        "updated_at": at,
        "status": "stopped",
        "runs": [{
            "run_id": format!("run_{id}"),
            "prompt": "hi",
            "transcript_path": transcript,
            "started_at": at,
        }],
    });
    std::fs::write(dir.join("session.json"), record.to_string()).unwrap();
    dir
}

#[test]
fn a_record_newer_than_this_rupu_is_shown_never_written_back() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = archive_session(tmp.path(), "ses_new", SessionRecord::VERSION + 1, 1);
    let bytes = std::fs::read(dir.join("session.json")).unwrap();
    let stored = parse_stored_session(&bytes).unwrap();
    assert!(matches!(stored, StoredSession::Newer { .. }));
    assert_eq!(stored.record().session_id, "ses_new");
    assert!(stored
        .newer_warning()
        .unwrap()
        .contains("was written by a newer rupu"));
    assert_eq!(
        parse_session_record(&bytes).unwrap_err().to_string(),
        "this session was written by a newer rupu; upgrade rupu to continue it"
    );
}

#[test]
fn prune_deletes_old_sessions_with_their_transcripts_and_keeps_newer_records() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let old = archive_session(global, "ses_old", 1, 40);
    let recent = archive_session(global, "ses_recent", 1, 2);
    let newer = archive_session(global, "ses_newer", SessionRecord::VERSION + 1, 40);

    let mut warnings = Vec::new();
    let dry = prune_report(global, Some("30d"), true, &mut |w| {
        warnings.push(w.to_string())
    })
    .unwrap();
    let rows = |r: &rupu_runtime::sessions::SessionPruneReport| {
        r.rows
            .iter()
            .map(|row| (row.session_id.clone(), row.action.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(&dry), vec![("ses_old".into(), "would_delete".into())]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("ses_newer"), "{warnings:?}");
    assert!(old.exists(), "a dry run deletes nothing");

    let done = prune_report(global, Some("30d"), false, &mut |_| {}).unwrap();
    assert_eq!(rows(&done), vec![("ses_old".into(), "deleted".into())]);
    assert!(!old.exists());
    assert!(!global.join("transcripts/run_ses_old.jsonl").exists());
    assert!(recent.exists() && newer.exists());
    assert!(global.join("transcripts/run_ses_newer.jsonl").exists());
}

#[test]
fn a_bad_retention_is_the_callers_or_the_configs() {
    let tmp = tempfile::tempdir().unwrap();
    let err = prune_report(tmp.path(), Some("soon"), true, &mut |_| {}).unwrap_err();
    assert!(matches!(err, PruneError::InvalidRetention(_)), "{err}");
    std::fs::write(
        tmp.path().join("config.toml"),
        "[storage]\narchived_session_retention = \"later\"\n",
    )
    .unwrap();
    let err = prune_report(tmp.path(), None, true, &mut |_| {}).unwrap_err();
    assert!(matches!(err, PruneError::Config(_)), "{err}");
}
