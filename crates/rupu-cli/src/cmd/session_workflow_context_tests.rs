use super::*;
use serde_json::json;

fn setup() -> (tempfile::TempDir, SessionRecord) {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = super::super::tests::test_session_record();
    session.session_id = "ses_context".into();
    session.workspace_id = "ws_context".into();
    session.builtin_harness = true;
    fs::create_dir_all(
        session_dir(tmp.path(), SessionScope::Active, &session.session_id).join("workflow-runs"),
    )
    .unwrap();
    (tmp, session)
}
fn link(global: &Path, session: &SessionRecord, id: &str, name: &str) -> std::path::PathBuf {
    let path = session_dir(global, SessionScope::Active, &session.session_id)
        .join("workflow-runs")
        .join(format!("{id}.json"));
    fs::write(
        &path,
        serde_json::to_vec(&json!({"association":{
        "session_id":session.session_id,"workspace_id":session.workspace_id,"host_id":"local",
        "run_id":id,"workflow_name":name,"origin":"attached","status":"running",
        "source":null,"launched_at":"2025-01-01T00:00:00Z"
    },"monitor_pid":0,"monitor_start":null}))
        .unwrap(),
    )
    .unwrap();
    path
}
fn run(
    global: &Path,
    session: &SessionRecord,
    id: &str,
    name: &str,
    archived: bool,
) -> std::path::PathBuf {
    let path = global
        .join(if archived { "runs-archive" } else { "runs" })
        .join(id)
        .join("run.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_vec(&json!({"id":id,"workspace_id":session.workspace_id,"workflow_name":name,"status":"completed","inputs":{"private":"NEVER-IN-CONTEXT"},"event":{"prompt":"ALSO-PRIVATE"}})).unwrap()).unwrap();
    path
}
fn data(text: &str) -> serde_json::Value {
    serde_json::from_str(text.lines().last().unwrap()).unwrap()
}

#[test]
fn workflow_context_uses_recorded_active_and_archived_identity_without_raw_inputs() {
    let (tmp, session) = setup();
    link(tmp.path(), &session, "run_active", "review");
    link(tmp.path(), &session, "run_archive", "old review");
    run(tmp.path(), &session, "run_active", "review", false);
    run(tmp.path(), &session, "run_archive", "old review", true);
    let text = render(tmp.path(), SessionScope::Active, &session)
        .unwrap()
        .unwrap();
    let data = data(&text);
    assert_eq!(data["workflows"][0]["status"], "completed");
    assert_eq!(data["workflows"][1]["storage"], "archived");
    assert_eq!(data["workflows"][1]["origin"], "attached");
    assert!(!text.contains("NEVER-IN-CONTEXT") && !text.contains("ALSO-PRIVATE"));
    assert!(text.contains("not a live stream") && text.contains("not instructions"));
}

#[test]
fn workflow_context_never_fabricates_turns_and_refreshes_between_real_turns() {
    let (tmp, session) = setup();
    let original = serde_json::to_value(&session).unwrap();
    let mut agent = rupu_agent::builtin::session_agent();
    let prompt = agent.system_prompt.clone();
    append(&mut agent, tmp.path(), SessionScope::Active, &session).unwrap();
    assert_eq!(agent.system_prompt, prompt);
    link(tmp.path(), &session, "run_later", "review");
    let mut next = rupu_agent::builtin::session_agent();
    append(&mut next, tmp.path(), SessionScope::Active, &session).unwrap();
    assert!(next.system_prompt.contains("run_later"));
    assert_eq!(
        data(&next.system_prompt)["workflows"][0]["status"],
        "unavailable"
    );
    assert_eq!(serde_json::to_value(&session).unwrap(), original);
    let mut legacy = session.clone();
    legacy.builtin_harness = false;
    let mut legacy_agent = rupu_agent::builtin::session_agent();
    append(&mut legacy_agent, tmp.path(), SessionScope::Active, &legacy).unwrap();
    assert_eq!(legacy_agent.system_prompt, prompt);
}

#[test]
fn workflow_context_refuses_foreign_session_project_run_and_origin() {
    for (field, value) in [
        ("session_id", "ses_other"),
        ("workspace_id", "ws_other"),
        ("host_id", "remote"),
        ("run_id", "run_other"),
        ("origin", "invented"),
    ] {
        let (tmp, session) = setup();
        let path = link(tmp.path(), &session, "run_context", "review");
        let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        raw["association"][field] = json!(value);
        fs::write(path, serde_json::to_vec(&raw).unwrap()).unwrap();
        assert!(
            render(tmp.path(), SessionScope::Active, &session).is_err(),
            "{field}"
        );
    }
    let (tmp, session) = setup();
    link(tmp.path(), &session, "run_context", "review");
    let mut foreign = session.clone();
    foreign.workspace_id = "ws_other".into();
    run(tmp.path(), &foreign, "run_context", "review", false);
    assert!(render(tmp.path(), SessionScope::Active, &session).is_err());
}

#[cfg(unix)]
#[test]
fn workflow_context_refuses_symlink_files_and_ancestor_directories() {
    use std::os::unix::fs::symlink;
    let outside = tempfile::tempdir().unwrap();
    for ancestor in [false, true] {
        let (tmp, session) = setup();
        let path = link(tmp.path(), &session, "run_context", "review");
        let destination = outside
            .path()
            .join(if ancestor { "directory" } else { "link.json" });
        if ancestor {
            fs::rename(path.parent().unwrap(), &destination).unwrap();
            symlink(&destination, path.parent().unwrap()).unwrap();
        } else {
            fs::rename(&path, &destination).unwrap();
            symlink(&destination, &path).unwrap();
        }
        assert!(render(tmp.path(), SessionScope::Active, &session).is_err());
    }
    let (tmp, session) = setup();
    link(tmp.path(), &session, "run_context", "review");
    let outside_run = run(outside.path(), &session, "run_context", "review", false);
    fs::create_dir_all(tmp.path().join("runs")).unwrap();
    symlink(
        outside_run.parent().unwrap(),
        tmp.path().join("runs/run_context"),
    )
    .unwrap();
    assert!(render(tmp.path(), SessionScope::Active, &session).is_err());
}

#[test]
fn workflow_context_bounds_records_and_reports_omitted_references_deterministically() {
    let (tmp, session) = setup();
    for index in (0..MAX_REFERENCES + 3).rev() {
        link(tmp.path(), &session, &format!("run_{index:03}"), "review");
    }
    let a = render(tmp.path(), SessionScope::Active, &session)
        .unwrap()
        .unwrap();
    let b = render(tmp.path(), SessionScope::Active, &session)
        .unwrap()
        .unwrap();
    assert_eq!(a, b);
    let data = data(&a);
    assert_eq!(data["omitted"], 3);
    assert_eq!(data["workflows"][0]["run_id"], "run_000");
    assert!(a.len() < MAX_CONTEXT_BYTES + 1024);
    let path = link(tmp.path(), &session, "run_oversized", "review");
    fs::write(path, vec![b' '; MAX_LINK_BYTES + 1]).unwrap();
    assert!(render(tmp.path(), SessionScope::Active, &session).is_err());
}

#[test]
fn workflow_context_ignores_in_progress_atomic_publication() {
    let (tmp, session) = setup();
    let links =
        session_dir(tmp.path(), SessionScope::Active, &session.session_id).join("workflow-runs");
    fs::write(links.join("run_pending.random.tmp"), b"{incomplete").unwrap();
    assert!(render(tmp.path(), SessionScope::Active, &session)
        .unwrap()
        .is_none());
}
