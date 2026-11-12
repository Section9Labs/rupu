use super::*;
use rupu_fleet::work_queue::{WorkActor, WorkRecording, WorkRecordingAnchor, WorkSource};
use std::fs;

fn session(root: &Path, archive: bool, path: &Path) -> owner::Context {
    let collection = if archive {
        "sessions-archive"
    } else {
        "sessions"
    };
    let dir = root.join(collection).join("ses_source");
    fs::create_dir_all(&dir).unwrap();
    let mut transcripts_dir = path.parent().unwrap().to_path_buf();
    if transcripts_dir.file_name().is_some_and(|s| s == "archive") {
        transcripts_dir = transcripts_dir.parent().unwrap().to_path_buf();
    }
    fs::write(dir.join("session.json"),json!({"session_id":"ses_source","status":"idle","workspace_id":"ws_test","transcripts_dir":transcripts_dir,"created_at":"2026-11-12T00:00:00Z",
        "runs":[{"run_id":"run_turn","transcript_path":path}]}).to_string()).unwrap();
    owner::resolve(root, MessageRoute::Sessions, "ses_source", false).unwrap()
}
fn actor(anchor: WorkRecordingAnchor, path: &str, round: Option<u32>) -> WorkActor {
    WorkActor::Agent {
        address: "main".into(),
        run_id: "run_turn".into(),
        execution_id: "execution_exact".into(),
        agent: "reviewer".into(),
        recording: WorkRecording {
            anchor,
            relative_path: path.into(),
            workspace_id: Some("ws_test".into()),
            round,
        },
    }
}
fn prologue(path: &Path, run: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path,format!("{}\n",json!({"type":"run_start","data":{"run_id":run,"workspace_id":"ws_test","agent":"reviewer","provider":"mock","model":"fixture","started_at":"2026-11-12T00:00:00Z","mode":"bypass"}}))).unwrap();
}
fn marker(path: &Path, actor: &WorkActor) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file,"{}",json!({"type":"notice","data":{"kind":"work_queue_execution","message":serde_json::to_string(actor).unwrap()}})).unwrap();
}
#[test]
fn archived_session_uses_exact_membership_path_and_keeps_missing_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let path = root.join("transcripts/archive/run_turn.jsonl");
    prologue(&path, "run_turn");
    let ctx = session(&root, true, &path);
    let actor = actor(
        WorkRecordingAnchor::GlobalTranscripts,
        "run_turn.jsonl",
        None,
    );
    marker(&path, &actor);
    let invocation = WorkSource {
        turn_index: 7,
        call_sequence: 12,
        tool_call_id: "repeated-model-id".into(),
    };
    let value = source::Resolver::new(&ctx).actor(&actor, Some(&invocation));
    assert_eq!(value["availability"], "available", "{value}");
    assert_eq!(value["transcript"]["path"], json!(path));
    assert_eq!(value["transcript"]["invocation"]["call_sequence"], 12);
    fs::remove_file(&path).unwrap();
    let missing = source::Resolver::new(&ctx).actor(&actor, Some(&invocation));
    assert_eq!(missing["availability"], "unavailable");
    assert_eq!(missing["recorded"], value["recorded"]);
    assert!(missing.get("transcript").is_none());
}
#[test]
fn recording_prologue_cannot_fall_back_to_another_run_or_later_record() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let path = root.join("transcripts/run_turn.jsonl");
    prologue(&path, "run_other");
    let ctx = session(&root, false, &path);
    let actor = actor(
        WorkRecordingAnchor::GlobalTranscripts,
        "run_turn.jsonl",
        None,
    );
    marker(&path, &actor);
    assert_eq!(
        source::Resolver::new(&ctx).actor(&actor, None)["availability"],
        "unavailable"
    );
    prologue(&path, "run_turn");
    marker(&path, &actor);
    let correct = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{{\"type\":\"unknown\"}}\n{correct}")).unwrap();
    assert_eq!(
        source::Resolver::new(&ctx).actor(&actor, None)["availability"],
        "unavailable"
    );
    fs::write(&path, correct).unwrap();
    assert_eq!(
        source::Resolver::new(&ctx).actor(&actor, None)["availability"],
        "available"
    );
}
#[test]
fn agentiflow_round_is_exact_even_when_the_run_id_is_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dir = root.join("agentiflows/af_source");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("agentiflow.json"),
        json!({"id":"af_source","status":"completed","workspace_id":"ws_test"}).to_string(),
    )
    .unwrap();
    let path = dir.join("lead/transcript.r2.jsonl");
    prologue(&path, "af_source");
    let ctx = owner::resolve(&root, MessageRoute::Agentiflows, "af_source", false).unwrap();
    let mut who = actor(
        WorkRecordingAnchor::Owner,
        "lead/transcript.r2.jsonl",
        Some(2),
    );
    if let WorkActor::Agent { run_id, .. } = &mut who {
        *run_id = "af_source".into();
    }
    marker(&path, &who);
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "available"
    );
    if let WorkActor::Agent { recording, .. } = &mut who {
        recording.round = Some(1);
    }
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unavailable"
    );
    if let WorkActor::Agent { recording, .. } = &mut who {
        recording.round = Some(2);
        recording.relative_path = "lead/transcript.r1.jsonl".into();
    }
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unavailable"
    );
}
#[test]
fn descendant_requires_recorded_edges_and_an_exact_child_prologue() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let parent_recording = root.join("transcripts/run_turn.jsonl");
    prologue(&parent_recording, "run_turn");
    let ctx = session(&root, false, &parent_recording);
    let path = root.join("runs/run_turn/sub/run_child/transcript.jsonl");
    prologue(&path, "run_child");
    let mut who = actor(
        WorkRecordingAnchor::GlobalRuns,
        "run_turn/sub/run_child/transcript.jsonl",
        None,
    );
    if let WorkActor::Agent { run_id, .. } = &mut who {
        *run_id = "run_child".into();
    }
    marker(&path, &who);
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unsupported"
    );
    let mut started = rupu_launch::ledger::Started::new(
        "child_handle",
        rupu_tools::ChildKind::Subagent,
        "reviewer",
        "run_child",
        std::process::id(),
        1,
    );
    started.transcript = Some(path.clone());
    fs::write(
        root.join("runs/run_turn/children.jsonl"),
        format!(
            "{}\n",
            serde_json::to_string(&rupu_launch::ledger::LedgerLine::Started(started)).unwrap()
        ),
    )
    .unwrap();
    let value = source::Resolver::new(&ctx).actor(&who, None);
    assert_eq!(value["availability"], "available", "{value}");
    prologue(&path, "run_impostor");
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unavailable"
    );
}
#[test]
fn byte_budget_uses_whole_records_and_counts_the_serialized_envelope() {
    let text = "\u{0001}".repeat(8 * 1024);
    let rows: Vec<_> = (0..50)
        .map(|n| json!({"sequence":n,"summary":text}))
        .collect();
    let page=bounded(rows.len(),|n|Ok(json!({"events":&rows[..n],"returned":n,"next_cursor":if n<rows.len() {Some(format!("after:{}",n-1))} else {None},"has_more":n<rows.len()}))).unwrap();
    let n = page["returned"].as_u64().unwrap() as usize;
    assert!(n > 0 && n < 50);
    assert_eq!(page["events"].as_array().unwrap().len(), n);
    assert_eq!(page["events"][n - 1]["summary"], text);
    assert_eq!(page["next_cursor"], format!("after:{}", n - 1));
    assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_PAGE_BYTES);
    let error = bounded(1, |_| Ok(json!({"record":"x".repeat(MAX_PAGE_BYTES)}))).unwrap_err();
    assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
}
#[cfg(unix)]
#[test]
fn source_symlink_never_produces_an_available_target() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let path = root.join("transcripts/run_turn.jsonl");
    let other = root.join("elsewhere.jsonl");
    prologue(&other, "run_turn");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&other, &path).unwrap();
    let ctx = session(&root, false, &path);
    let who = actor(
        WorkRecordingAnchor::GlobalTranscripts,
        "run_turn.jsonl",
        None,
    );
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unavailable"
    );
}

#[test]
fn unrelated_valid_same_project_global_recording_is_not_an_owned_execution() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dir = root.join("runs/run_owner");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("run.json"),json!({"id":"run_owner","workflow_name":"agent:reviewer","status":"completed","workspace_id":"ws_test"}).to_string()).unwrap();
    let ctx = owner::resolve(&root, MessageRoute::Runs, "run_owner", false).unwrap();
    let path = root.join("transcripts/run_turn.jsonl");
    prologue(&path, "run_turn");
    let who = actor(
        WorkRecordingAnchor::GlobalTranscripts,
        "run_turn.jsonl",
        None,
    );
    marker(&path, &who);
    let value = source::Resolver::new(&ctx).actor(&who, None);
    assert_eq!(value["availability"], "unsupported", "{value}");
    assert!(value.get("transcript").is_none());
    assert_eq!(value["recorded"]["actor"]["run_id"], "run_turn");
}

#[test]
fn session_owner_recording_retains_its_nested_transcript_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let path = root.join("sessions/ses_source/turns/run_turn.jsonl");
    prologue(&path, "run_turn");
    let ctx = session(&root, false, &path);
    let who = actor(WorkRecordingAnchor::Owner, "turns/run_turn.jsonl", None);
    marker(&path, &who);
    let value = source::Resolver::new(&ctx).actor(&who, None);
    assert_eq!(value["availability"], "available", "{value}");
    assert_eq!(value["transcript"]["path"], json!(path));
    fs::create_dir(root.join("sessions-archive")).unwrap();
    fs::rename(
        root.join("sessions/ses_source"),
        root.join("sessions-archive/ses_source"),
    )
    .unwrap();
    let ctx = owner::resolve(&root, MessageRoute::Sessions, "ses_source", false).unwrap();
    let value = source::Resolver::new(&ctx).actor(&who, None);
    assert_eq!(value["availability"], "available", "{value}");
    assert_eq!(
        value["transcript"]["path"],
        json!(root.join("sessions-archive/ses_source/turns/run_turn.jsonl"))
    );
}

#[test]
fn registered_project_session_recording_uses_exact_workspace_and_membership() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let project = root.join("project");
    let path = project.join(".rupu/transcripts/run_turn.jsonl");
    prologue(&path, "run_turn");
    fs::create_dir(root.join("workspaces")).unwrap();
    let workspace = rupu_workspace::Workspace {
        id: "ws_test".into(),
        path: project.to_string_lossy().into_owned(),
        repo_remote: None,
        initial_branch: None,
        created_at: "2026-11-12T00:00:00Z".into(),
        last_run_at: None,
    };
    fs::write(
        root.join("workspaces/ws_test.toml"),
        toml::to_string(&workspace).unwrap(),
    )
    .unwrap();
    let ctx = session(&root, false, &path);
    let who = actor(
        WorkRecordingAnchor::ProjectTranscripts,
        "run_turn.jsonl",
        None,
    );
    marker(&path, &who);
    let value = source::Resolver::new(&ctx).actor(&who, None);
    assert_eq!(value["availability"], "available", "{value}");
    fs::remove_file(root.join("workspaces/ws_test.toml")).unwrap();
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unsupported"
    );
}

#[test]
fn missing_or_replaced_execution_marker_never_resolves_as_the_original_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let path = root.join("transcripts/run_turn.jsonl");
    prologue(&path, "run_turn");
    let ctx = session(&root, false, &path);
    let who = actor(
        WorkRecordingAnchor::GlobalTranscripts,
        "run_turn.jsonl",
        None,
    );
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unavailable"
    );
    marker(&path, &who);
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "available"
    );
    let mut replacement = who.clone();
    if let WorkActor::Agent { execution_id, .. } = &mut replacement {
        *execution_id = "replacement_execution".into();
    }
    prologue(&path, "run_turn");
    marker(&path, &replacement);
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unavailable"
    );
    // Even a matching later marker cannot repair the first execution marker.
    marker(&path, &who);
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unavailable"
    );
}
