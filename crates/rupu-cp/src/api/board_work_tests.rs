use super::*;
use rupu_fleet::work_queue::types::{WorkActor, WorkRecording, WorkRecordingAnchor, WorkSource};
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
    let record:rupu_runtime::sessions::SessionRecord=serde_json::from_value(json!({
        "version":1,"session_id":"ses_source","agent_name":"reviewer","provider_name":"mock","model":"fixture",
        "agent_system_prompt":"Fixture","max_turns":50,"permission_mode":"readonly","no_stream":false,
        "status":"idle","workspace_id":"ws_test","workspace_path":root,"transcripts_dir":transcripts_dir,
        "created_at":"2026-11-12T00:00:00Z","updated_at":"2026-11-12T00:00:00Z",
        "runs":[{"run_id":"run_turn","transcript_path":path,"prompt":"Fixture turn","started_at":"2026-11-12T00:00:00Z"}]})).unwrap();
    fs::write(
        dir.join("session.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
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

#[test]
fn active_workflow_attempts_and_their_recorded_children_have_exact_sources() {
    use rupu_orchestrator::runs::{AttemptRecord, RunRecord, RunStore};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let store = RunStore::new(root.join("runs"));
    let record:RunRecord=serde_json::from_value(json!({"id":"run_workflow","workflow_name":"review","status":"running","inputs":{},
        "workspace_id":"ws_test","workspace_path":root,"transcript_dir":root.join("transcripts"),"started_at":"2026-11-12T00:00:00Z"})).unwrap();
    store.create(record, "name: review\nsteps: []\n").unwrap();
    let directory = root.join("runs/run_workflow");
    let path = directory.join("steps/review/run_step.jsonl");
    prologue(&path, "run_step");
    let mut who = actor(
        WorkRecordingAnchor::Owner,
        "steps/review/run_step.jsonl",
        None,
    );
    if let WorkActor::Agent { run_id, .. } = &mut who {
        *run_id = "run_step".into();
    }
    marker(&path, &who);
    let attempt = AttemptRecord {
        v: 1,
        step_id: "review".into(),
        unit_index: None,
        sub_id: None,
        agent_run_id: "run_step".into(),
        transcript_path: path.clone(),
        host: None,
        continued_from: None,
        started_at: "2026-11-12T00:00:00Z".parse().unwrap(),
    };
    store.append_attempt("run_workflow", &attempt).unwrap();
    // Same exact identity may be recorded twice without manufacturing a new edge.
    store.append_attempt("run_workflow", &attempt).unwrap();
    let ctx = owner::resolve(&root, MessageRoute::Runs, "run_workflow", false).unwrap();
    let value = source::Resolver::new(&ctx).actor(&who, None);
    assert_eq!(value["availability"], "available", "{value}");
    let child_path = root.join("runs/run_step/sub/run_child/transcript.jsonl");
    prologue(&child_path, "run_child");
    let mut child = actor(
        WorkRecordingAnchor::GlobalRuns,
        "run_step/sub/run_child/transcript.jsonl",
        None,
    );
    if let WorkActor::Agent { run_id, .. } = &mut child {
        *run_id = "run_child".into();
    }
    marker(&child_path, &child);
    let mut started = rupu_launch::ledger::Started::new(
        "child",
        rupu_tools::ChildKind::Subagent,
        "reviewer",
        "run_child",
        std::process::id(),
        1,
    );
    started.transcript = Some(child_path);
    fs::write(
        root.join("runs/run_step/children.jsonl"),
        format!(
            "{}\n",
            serde_json::to_string(&rupu_launch::ledger::LedgerLine::Started(started)).unwrap()
        ),
    )
    .unwrap();
    let value = source::Resolver::new(&ctx).actor(&child, None);
    assert_eq!(value["availability"], "available", "{value}");
    // Legacy search must not acquire the opt-in active-attempt membership.
    let old = crate::api::children::pages::capture::search::capture(
        crate::api::children::pages::Owner {
            global: root.clone(),
            runs: root.join("runs"),
        },
        "run_workflow",
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert!(!old.members.iter().any(|m| m.run_id == "run_step"));
    let mut remote = attempt.clone();
    remote.host = Some("remote".into());
    fs::write(
        directory.join("attempts.jsonl"),
        format!("{}\n", serde_json::to_string(&remote).unwrap()),
    )
    .unwrap();
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unsupported"
    );
    let mut conflicting = attempt.clone();
    conflicting.step_id = "another-step".into();
    fs::write(
        directory.join("attempts.jsonl"),
        format!(
            "{}\n{}\n",
            serde_json::to_string(&attempt).unwrap(),
            serde_json::to_string(&conflicting).unwrap()
        ),
    )
    .unwrap();
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unsupported"
    );
    assert_eq!(
        source::Resolver::new(&ctx).actor(&child, None)["availability"],
        "unsupported"
    );
    let mut unrelated = attempt;
    unrelated.agent_run_id = "run_unrelated".into();
    fs::write(
        directory.join("attempts.jsonl"),
        format!("{}\n", serde_json::to_string(&unrelated).unwrap()),
    )
    .unwrap();
    assert_eq!(
        source::Resolver::new(&ctx).actor(&who, None)["availability"],
        "unsupported"
    );
}

#[test]
fn large_serialized_session_record_stays_bounded_and_does_not_leak_prompts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let path = root.join("transcripts/run_turn.jsonl");
    prologue(&path, "run_turn");
    let ctx = session(&root, false, &path);
    let record_path = ctx.directory.join("session.json");
    let mut record: rupu_runtime::sessions::SessionRecord =
        serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    record.runs[0].prompt = "PRIVATE_FIXTURE_PROMPT_".repeat(100_000);
    let bytes = serde_json::to_vec(&record).unwrap();
    assert!(bytes.len() > 1024 * 1024);
    fs::write(&record_path, bytes).unwrap();
    let who = actor(
        WorkRecordingAnchor::GlobalTranscripts,
        "run_turn.jsonl",
        None,
    );
    marker(&path, &who);
    let ctx = owner::resolve(&root, MessageRoute::Sessions, "ses_source", false).unwrap();
    let value = source::Resolver::new(&ctx).actor(&who, None);
    assert_eq!(value["availability"], "available", "{value}");
    assert!(!value.to_string().contains("PRIVATE_FIXTURE_PROMPT_"));
    // The same bounded canonical session proof supports children of this turn.
    let child_path = root.join("runs/run_turn/sub/run_child/transcript.jsonl");
    prologue(&child_path, "run_child");
    let mut child = actor(
        WorkRecordingAnchor::GlobalRuns,
        "run_turn/sub/run_child/transcript.jsonl",
        None,
    );
    if let WorkActor::Agent { run_id, .. } = &mut child {
        *run_id = "run_child".into();
    }
    marker(&child_path, &child);
    let mut start = rupu_launch::ledger::Started::new(
        "child",
        rupu_tools::ChildKind::Subagent,
        "reviewer",
        "run_child",
        std::process::id(),
        1,
    );
    start.transcript = Some(child_path);
    fs::write(
        root.join("runs/run_turn/children.jsonl"),
        format!(
            "{}\n",
            serde_json::to_string(&rupu_launch::ledger::LedgerLine::Started(start)).unwrap()
        ),
    )
    .unwrap();
    let value = source::Resolver::new(&ctx).actor(&child, None);
    assert_eq!(value["availability"], "available", "{value}");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&record_path)
        .unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    let error = owner::resolve(&root, MessageRoute::Sessions, "ses_source", false)
        .err()
        .unwrap();
    assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
}
