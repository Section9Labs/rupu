//! Sessions carry skills (spec 2026-10-12 §5.1): the set and its prompt
//! section are fixed when the session starts, and the skills the model
//! loaded carry from turn to turn. Turns run through `run_turn` against the
//! `RUPU_MOCK_PROVIDER_SCRIPT` seam.

use super::tests::{test_session_record, with_mock_home};
use super::*;
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::types::ContentBlock;
use serde_json::json;

fn install_skill(global: &Path, name: &str, body: &str) -> PathBuf {
    let dir = global.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: The {name} skill.\n---\n{body}\n"),
    )
    .unwrap();
    dir
}

fn agent(keys: &str) -> rupu_agent::AgentSpec {
    rupu_agent::AgentSpec::parse(&format!(
        "---\nname: issue-reader\nprovider: anthropic\nmodel: claude-sonnet-4-6\n{keys}\n---\n\
         You are a test agent.\n"
    ))
    .unwrap()
}

/// An idle session under `<tmp>/global` with no project layer, its skills
/// frozen from `agent` plus `skills` / `preload` the way `session start`
/// freezes them.
async fn skill_session(
    tmp: &tempfile::TempDir,
    session_id: &str,
    agent: &rupu_agent::AgentSpec,
    skills: &[&str],
    preload: &[&str],
) -> (PathBuf, SessionRecord) {
    let global = tmp.path().join("global");
    let workspace = tmp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("notes.txt"), "invented notes\n").unwrap();
    let mut record = test_session_record();
    record.session_id = session_id.into();
    record.status = SessionStatus::Idle;
    record.workspace_path = workspace;
    record.project_root = None;
    record.repo_ref = None;
    record.issue_ref = None;
    record.target = None;
    record.workspace_strategy = None;
    record.transcripts_dir = global.join("sessions").join(session_id).join("transcripts");
    record.active_run_id = None;
    record.active_transcript_path = None;
    record.active_pid = None;
    record.worker_pid = None;
    record.last_run_id = None;
    record.last_transcript_path = None;
    record.runs = Vec::new();
    record.total_turns = 0;
    let owned = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let (index, prompt) =
        freeze_session_skills(&global, None, agent, owned(skills), owned(preload))
            .await
            .expect("the session's skills resolve");
    record.skill_index = index;
    record.skill_prompt = prompt;
    write_session(&global, SessionScope::Active, &record).unwrap();
    (global, record)
}

/// Runs one turn and returns its transcript.
async fn turn(global: &Path, session_id: &str, run_id: &str, script: &str) -> Vec<TranscriptEvent> {
    with_mock_home(
        global,
        script,
        run_turn(RunTurnArgs {
            session_id: session_id.into(),
            run_id: run_id.into(),
            prompt: format!("prompt for {run_id}"),
            operator_skills: Vec::new(),
        }),
    )
    .await
    .expect("the turn runs");
    let (session, _) = read_session(global, session_id).unwrap();
    assert_eq!(
        session.status,
        SessionStatus::Idle,
        "{:?}",
        session.last_error
    );
    JsonlReader::iter(session.transcripts_dir.join(format!("{run_id}.jsonl")))
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

const ACK: &str = r#"[{ "AssistantText": { "text": "ack", "stop": "end_turn" } }]"#;

/// The model calls `skills.load` for `name`, then answers.
fn load(name: &str) -> String {
    json!([
        { "AssistantToolUse": {
            "text": null, "tool_id": "c1", "tool_name": "skills.load",
            "tool_input": { "name": name }, "stop": "tool_use"
        } },
        { "AssistantText": { "text": "ack", "stop": "end_turn" } }
    ])
    .to_string()
}

fn system_prompt(evs: &[TranscriptEvent]) -> String {
    evs.iter()
        .find_map(|e| match e {
            TranscriptEvent::RunStart { system_prompt, .. } => system_prompt.clone(),
            _ => None,
        })
        .expect("a run_start with a system prompt")
}

/// `(name, reason)` of the turn's `skill_index`.
fn index(evs: &[TranscriptEvent]) -> Vec<(String, String)> {
    evs.iter()
        .find_map(|e| match e {
            TranscriptEvent::SkillIndex { skills } => Some(
                skills
                    .iter()
                    .map(|s| (s.name.clone(), s.reason.clone()))
                    .collect(),
            ),
            _ => None,
        })
        .expect("a skill_index")
}

fn notices(evs: &[TranscriptEvent], kind: &str) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            TranscriptEvent::Notice { kind: k, message } if k == kind => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// Review Focus 1 and 4: every turn's system prompt is the one the session
/// started with, byte for byte, though a preloaded skill was edited and a
/// new skill installed in between; a load judges the edit against the
/// session's baseline.
#[tokio::test]
async fn a_session_freezes_its_skills_at_start() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let global = tmp.path().join("global");
    install_skill(&global, "report-style", "STYLE V1");
    install_skill(&global, "code-review", "REVIEW STEPS");
    let spec = agent("skills: [\"*\"]\npreloadSkills: [report-style]");
    let (global, record) = skill_session(&tmp, "ses_skills_freeze01", &spec, &[], &[]).await;

    let first = turn(&global, &record.session_id, "run_freeze_1", ACK).await;
    install_skill(&global, "report-style", "STYLE V2");
    install_skill(&global, "late", "LATE STEPS");
    let second = turn(
        &global,
        &record.session_id,
        "run_freeze_2",
        &load("report-style"),
    )
    .await;

    let prompt = system_prompt(&first);
    assert_eq!(system_prompt(&second), prompt);
    assert!(prompt.contains("## Skill: report-style"), "{prompt}");
    assert!(prompt.contains("STYLE V1") && !prompt.contains("STYLE V2"));
    assert!(!prompt.contains("late"), "{prompt}");
    let agent_rows = pairs(&[("code-review", "agent"), ("report-style", "agent")]);
    assert_eq!(index(&first), agent_rows);
    assert_eq!(index(&second), agent_rows);
    assert_eq!(
        notices(&second, "skill_changed"),
        vec![
            "skill report-style changed on disk since this run started; the model was given \
             its current version"
                .to_string()
        ]
    );
}

#[tokio::test]
async fn start_flags_add_skills_with_reason_session() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    install_skill(&tmp.path().join("global"), "code-review", "REVIEW STEPS");
    let (global, record) = skill_session(
        &tmp,
        "ses_skills_flags01",
        &agent(""),
        &["code-review"],
        &[],
    )
    .await;
    let evs = turn(&global, &record.session_id, "run_flags_1", ACK).await;
    assert_eq!(index(&evs), pairs(&[("code-review", "session")]));
    assert!(system_prompt(&evs).contains("- code-review: The code-review skill."));
}

/// Review Focus 2 (in-turn compaction): a skill loaded in turn 1 is carried
/// on the session and re-attached when turn 2 compacts.
#[tokio::test]
async fn loaded_skills_persist_across_turns_and_reattach() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let skill_dir = install_skill(&tmp.path().join("global"), "code-review", "REVIEW STEPS");
    let (global, mut record) = skill_session(
        &tmp,
        "ses_skills_loaded01",
        &agent("skills: [code-review]"),
        &[],
        &[],
    )
    .await;
    let dense = "x".repeat(1000);
    let mut history = vec![Message::user(&format!("task: {dense}"))];
    for i in 0..3 {
        history.push(Message::assistant(&format!("assistant {i}: {dense}")));
        history.push(Message::user(&format!("user {i}: {dense}")));
    }
    record.message_history = history;
    write_session(&global, SessionScope::Active, &record).unwrap();

    turn(
        &global,
        &record.session_id,
        "run_loaded_1",
        &load("code-review"),
    )
    .await;
    let (mut after_first, _) = read_session(&global, &record.session_id).unwrap();
    let skill = rupu_skills::load_skill(&skill_dir, rupu_skills::Layer::Global).unwrap();
    assert_eq!(
        after_first.loaded_skills,
        vec![rupu_agent::LoadedSkill {
            name: "code-review".into(),
            content_hash: skill.content_hash().unwrap().hex(),
        }]
    );

    after_first.model_limits = Some(ModelLimits::fixed(1000, 100).with_percent(50));
    write_session(&global, SessionScope::Active, &after_first).unwrap();
    let script = json!([
        { "AssistantBlocksWithUsage": {
            "content": [{ "type": "tool_use", "id": "c2", "name": "read_file",
                          "input": { "path": "notes.txt" } }],
            "stop": "tool_use",
            "usage": { "input_tokens": 900, "output_tokens": 5 }
        } },
        { "AssistantText": { "text": "Summary of prior work.", "stop": "end_turn" } },
        { "AssistantText": { "text": "done", "stop": "end_turn" } }
    ])
    .to_string();
    let second = turn(&global, &record.session_id, "run_loaded_2", &script).await;
    let first_message: Vec<String> = second
        .iter()
        .find_map(|e| match e {
            TranscriptEvent::Compaction { messages, .. } => {
                let messages: Vec<Message> = serde_json::from_value(messages.clone()).unwrap();
                Some(
                    messages[0]
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect(),
                )
            }
            _ => None,
        })
        .expect("turn 2 compacted");
    let reattached: Vec<&String> = first_message
        .iter()
        .filter(|t| t.starts_with(rupu_skills::render::REATTACH_MARKER))
        .collect();
    assert_eq!(reattached.len(), 1, "{first_message:?}");
    assert!(reattached[0].contains("## Skill: code-review"));
    assert!(reattached[0].contains("REVIEW STEPS"));
    let (after_second, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(after_second.loaded_skills, after_first.loaded_skills);
}

#[tokio::test]
async fn a_frozen_skill_removed_mid_session_is_dropped_with_a_notice() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let global = tmp.path().join("global");
    install_skill(&global, "code-review", "REVIEW STEPS");
    let pdf = install_skill(&global, "pdf", "PDF STEPS");
    let (global, record) = skill_session(
        &tmp,
        "ses_skills_removed01",
        &agent("skills: [code-review, pdf]"),
        &[],
        &[],
    )
    .await;
    turn(&global, &record.session_id, "run_removed_1", ACK).await;
    fs::remove_dir_all(&pdf).unwrap();
    let evs = turn(&global, &record.session_id, "run_removed_2", &load("pdf")).await;
    assert_eq!(
        notices(&evs, "skill_missing"),
        vec!["skill pdf (global) is not available to this run: removed".to_string()]
    );
    assert_eq!(index(&evs), pairs(&[("code-review", "agent")]));
    let result = evs
        .iter()
        .find_map(|e| match e {
            TranscriptEvent::ToolResult {
                call_id,
                output,
                error,
                ..
            } if call_id == "c1" => Some(format!("{output} {}", error.clone().unwrap_or_default())),
            _ => None,
        })
        .expect("the load's result");
    assert!(result.contains("no skill named \"pdf\""), "{result}");
    let (after, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(
        after.skill_index, record.skill_index,
        "the frozen set is kept"
    );
}

/// Review Focus 5: a record written before sessions carried skills runs
/// its turn as before and is written back without skill keys, still at
/// version 1.
#[tokio::test]
async fn a_record_without_skill_fields_runs_unchanged() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let global = tmp.path().join("global");
    let workspace = tmp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let session_id = "ses_skills_legacy01";
    let path = session_record_path(&global, SessionScope::Active, session_id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let record = json!({
        "version": 1,
        "session_id": session_id,
        "agent_name": "issue-reader",
        "provider_name": "anthropic",
        "model": "claude-sonnet-4-6",
        "agent_system_prompt": "You are a test agent.",
        "agent_tools": ["read_file"],
        "max_turns": 50,
        "permission_mode": "bypass",
        "no_stream": false,
        "workspace_id": "ws_test",
        "workspace_path": workspace,
        "transcripts_dir": global.join("sessions").join(session_id).join("transcripts"),
        "created_at": "2026-10-01T00:00:00Z",
        "updated_at": "2026-10-01T00:00:00Z",
        "status": "idle",
        "runs": [],
    });
    fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let raw = |path: &Path| -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    };
    let skill_keys = ["skill_index", "skill_prompt", "loaded_skills"];

    let evs = turn(&global, session_id, "run_legacy_1", ACK).await;
    assert!(!evs
        .iter()
        .any(|e| matches!(e, TranscriptEvent::SkillIndex { .. })));
    assert!(notices(&evs, "skill_missing").is_empty());
    assert_eq!(
        system_prompt(&evs),
        "You are a test agent.\n\nYour call sign in this run is `jackal` (crew \
         `lilac-quarter`). Sign any comments, issues or PR notes you post with it."
    );
    let after = raw(&path);
    assert!(after
        .as_object()
        .unwrap()
        .keys()
        .all(|k| !skill_keys.contains(&k.as_str())));
    assert_eq!(after["version"], 1);
}

/// A record that carries skills is written as version 2, which a rupu that
/// predates skills would not know to keep; one without stays version 1.
#[tokio::test]
async fn a_record_with_skills_is_written_as_version_2() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    install_skill(&tmp.path().join("global"), "code-review", "REVIEW STEPS");
    let (global, with) = skill_session(
        &tmp,
        "ses_skills_version01",
        &agent("skills: [code-review]"),
        &[],
        &[],
    )
    .await;
    let (_, without) = skill_session(&tmp, "ses_skills_version02", &agent(""), &[], &[]).await;
    let version = |id: &str| -> serde_json::Value {
        let path = session_record_path(&global, SessionScope::Active, id);
        let v: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        v["version"].clone()
    };
    assert_eq!(version(&with.session_id), 2);
    assert_eq!(version(&without.session_id), 1);
}

/// A record a newer rupu wrote is refused, so this one never writes it back
/// without the fields it doesn't know.
#[tokio::test]
async fn a_record_from_a_newer_rupu_is_refused() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (global, record) = skill_session(&tmp, "ses_skills_newer01", &agent(""), &[], &[]).await;
    let path = session_record_path(&global, SessionScope::Active, &record.session_id);
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["version"] = json!(SessionRecord::VERSION + 1);
    fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let newer = "this session was written by a newer rupu; upgrade rupu to continue it";

    let err = with_mock_home(
        &global,
        ACK,
        run_turn(RunTurnArgs {
            session_id: record.session_id.clone(),
            run_id: "run_newer_1".into(),
            prompt: "hi".into(),
            operator_skills: Vec::new(),
        }),
    )
    .await
    .expect_err("the turn is refused");
    assert!(err.to_string().contains(newer), "{err}");
    let err = with_mock_home(
        &global,
        ACK,
        send(SendArgs {
            session_id: record.session_id.clone(),
            prompt: Some("hi".into()),
            skills: Vec::new(),
            detach: true,
            view: None,
        }),
    )
    .await
    .expect_err("the send is refused");
    assert!(err.to_string().contains(newer), "{err}");
    assert_eq!(fs::read(&path).unwrap(), before);
}

/// A turn whose assembly fails (its grant names an unknown tool) is a
/// failed turn that keeps the skills the session had loaded.
#[tokio::test]
async fn a_turn_whose_assembly_fails_keeps_the_loaded_skills() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    install_skill(&tmp.path().join("global"), "code-review", "REVIEW STEPS");
    let (global, record) = skill_session(
        &tmp,
        "ses_skills_assembly01",
        &agent("skills: [code-review]"),
        &[],
        &[],
    )
    .await;
    turn(
        &global,
        &record.session_id,
        "run_assembly_1",
        &load("code-review"),
    )
    .await;
    let (mut loaded, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(loaded.loaded_skills.len(), 1);
    loaded.agent_tools = Some(vec!["no_such_tool".into()]);
    write_session(&global, SessionScope::Active, &loaded).unwrap();

    with_mock_home(
        &global,
        ACK,
        run_turn(RunTurnArgs {
            session_id: record.session_id.clone(),
            run_id: "run_assembly_2".into(),
            prompt: "again".into(),
            operator_skills: Vec::new(),
        }),
    )
    .await
    .expect("a failed turn is recorded, not returned");
    let (after, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(after.status, SessionStatus::Failed);
    let error = after.last_error.clone().unwrap_or_default();
    assert!(error.contains("no_such_tool"), "{error}");
    assert_eq!(after.loaded_skills, loaded.loaded_skills);
}

/// A loaded skill uninstalled for a turn is reported once and stays loaded,
/// so the turn after its reinstall re-attaches it.
#[tokio::test]
async fn a_loaded_skill_missing_for_a_turn_comes_back_on_reinstall() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = install_skill(&tmp.path().join("global"), "code-review", "REVIEW STEPS");
    let (global, record) = skill_session(
        &tmp,
        "ses_skills_reinstall01",
        &agent("skills: [code-review]"),
        &[],
        &[],
    )
    .await;
    turn(
        &global,
        &record.session_id,
        "run_reinstall_1",
        &load("code-review"),
    )
    .await;
    let (loaded, _) = read_session(&global, &record.session_id).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    let gone = turn(&global, &record.session_id, "run_reinstall_2", ACK).await;
    assert_eq!(
        notices(&gone, "skill_missing"),
        vec!["skill code-review (global) is not available to this run: removed".to_string()]
    );
    let (after_gone, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(after_gone.loaded_skills, loaded.loaded_skills);

    install_skill(&global, "code-review", "REVIEW STEPS");
    let back = turn(&global, &record.session_id, "run_reinstall_3", ACK).await;
    assert!(notices(&back, "skill_missing").is_empty());
    assert_eq!(loads(&back), pairs(&[("continuation", "code-review")]));
}

/// A skills directory that can't be read fails the turn the way a failed
/// assembly does: recorded on the session, with a transcript that says why.
#[tokio::test]
async fn an_unreadable_skills_directory_fails_the_turn_with_a_transcript() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = install_skill(&tmp.path().join("global"), "code-review", "REVIEW STEPS");
    let (global, record) = skill_session(
        &tmp,
        "ses_skills_unreadable01",
        &agent("skills: [code-review]"),
        &[],
        &[],
    )
    .await;
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    fs::write(global.join("skills"), "not a directory").unwrap();

    with_mock_home(
        &global,
        ACK,
        run_turn(RunTurnArgs {
            session_id: record.session_id.clone(),
            run_id: "run_unreadable_1".into(),
            prompt: "hi".into(),
            operator_skills: Vec::new(),
        }),
    )
    .await
    .expect("a failed turn is recorded, not returned");
    let (after, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(after.status, SessionStatus::Failed);
    let error = after.last_error.clone().unwrap_or_default();
    assert!(error.starts_with("skills: "), "{error}");
    let transcript = after.transcripts_dir.join("run_unreadable_1.jsonl");
    let closed = JsonlReader::iter(&transcript)
        .expect("the turn has a transcript")
        .filter_map(Result::ok)
        .find_map(|e| match e {
            TranscriptEvent::RunComplete { status, error, .. } => Some((status, error)),
            _ => None,
        })
        .expect("the transcript closes the run");
    assert_eq!(closed.0, RunStatus::Error);
    assert!(
        closed
            .1
            .as_deref()
            .is_some_and(|e| e.starts_with("skills: ")),
        "{closed:?}"
    );
}

/// Runs one turn the operator invoked `invoked` in and returns its
/// transcript.
async fn operator_turn(
    global: &Path,
    session_id: &str,
    run_id: &str,
    prompt: &str,
    invoked: Vec<rupu_skills::FrozenSkill>,
) -> Vec<TranscriptEvent> {
    with_mock_home(
        global,
        ACK,
        run_turn(RunTurnArgs {
            session_id: session_id.into(),
            run_id: run_id.into(),
            prompt: prompt.into(),
            operator_skills: invoked,
        }),
    )
    .await
    .expect("the turn runs");
    let (session, _) = read_session(global, session_id).unwrap();
    JsonlReader::iter(session.transcripts_dir.join(format!("{run_id}.jsonl")))
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

/// What `session send --skill` resolves, as `send` resolves it.
async fn invoke(
    global: &Path,
    session_id: &str,
    entries: &[&str],
) -> anyhow::Result<Vec<rupu_skills::FrozenSkill>> {
    let (session, _) = read_session(global, session_id).unwrap();
    let entries: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
    resolve_send_skills(global, &session, &entries).await
}

fn user_message(evs: &[TranscriptEvent]) -> String {
    evs.iter()
        .find_map(|e| match e {
            TranscriptEvent::UserMessage { content } => Some(content.clone()),
            _ => None,
        })
        .expect("a user message")
}

/// `(via, name)` of every `skill_load`.
fn loads(evs: &[TranscriptEvent]) -> Vec<(String, String)> {
    evs.iter()
        .filter_map(|e| match e {
            TranscriptEvent::SkillLoad { name, via, .. } => Some((via.clone(), name.clone())),
            _ => None,
        })
        .collect()
}

/// The result text of tool call `id`, and whether it failed.
fn tool_result(evs: &[TranscriptEvent], id: &str) -> (String, Option<String>) {
    evs.iter()
        .find_map(|e| match e {
            TranscriptEvent::ToolResult {
                call_id,
                output,
                error,
                ..
            } if call_id == id => Some((output.clone(), error.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a result for {id}"))
}

/// Spec 2026-10-12 §5.2: an invoked skill is injected ahead of the
/// prompt, recorded as an operator load and frozen into the session with
/// reason `operator`, unindexed. The next turn may load it and read its
/// files; no turn's system prompt changes.
#[tokio::test]
async fn send_skill_injects_records_and_grants() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let pdf_dir = install_skill(&tmp.path().join("global"), "pdf", "PDF STEPS");
    fs::write(pdf_dir.join("forms.md"), "FORM NOTES\n").unwrap();
    let (global, record) = skill_session(&tmp, "ses_skills_send01", &agent(""), &[], &[]).await;
    let first = turn(&global, &record.session_id, "run_send_1", ACK).await;

    let invoked = invoke(&global, &record.session_id, &["pdf"]).await.unwrap();
    let second = operator_turn(
        &global,
        &record.session_id,
        "run_send_2",
        "summarise x.pdf",
        invoked,
    )
    .await;
    let pdf = rupu_skills::load_skill(&pdf_dir, rupu_skills::Layer::Global).unwrap();
    let version = pdf.content_hash().unwrap();
    assert_eq!(
        user_message(&second),
        format!(
            "<skill name=\"pdf\" base=\"{}\" version=\"{}\">\nPDF STEPS\n</skill>\n\n\
             summarise x.pdf",
            pdf.root.display(),
            version.short()
        )
    );
    assert_eq!(loads(&second), pairs(&[("operator", "pdf")]));
    let (after, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(
        after.skill_index,
        vec![rupu_skills::FrozenSkill {
            name: "pdf".into(),
            layer: rupu_skills::Layer::Global,
            preload: false,
            indexed: false,
            reason: rupu_skills::SkillReason::Operator,
            stamp: pdf.stamp.hex(),
            content_hash: None,
        }]
    );
    assert_eq!(after.skill_prompt, None);
    assert_eq!(
        after.loaded_skills,
        vec![rupu_agent::LoadedSkill {
            name: "pdf".into(),
            content_hash: version.hex(),
        }]
    );

    let forms = pdf.root.join("forms.md");
    let script = json!([
        { "AssistantToolUse": {
            "text": null, "tool_id": "c1", "tool_name": "skills.load",
            "tool_input": { "name": "pdf" }, "stop": "tool_use"
        } },
        { "AssistantToolUse": {
            "text": null, "tool_id": "c2", "tool_name": "read_file",
            "tool_input": { "path": forms.display().to_string() }, "stop": "tool_use"
        } },
        { "AssistantText": { "text": "ack", "stop": "end_turn" } }
    ])
    .to_string();
    let third = turn(&global, &record.session_id, "run_send_3", &script).await;
    let (loaded, load_error) = tool_result(&third, "c1");
    assert_eq!(load_error, None);
    assert!(loaded.contains("PDF STEPS"), "{loaded}");
    let (read, read_error) = tool_result(&third, "c2");
    assert_eq!(read_error, None);
    assert!(read.contains("FORM NOTES"), "{read}");

    let prompt = system_prompt(&first);
    assert_eq!(system_prompt(&second), prompt);
    assert_eq!(system_prompt(&third), prompt);
    assert!(!prompt.contains("pdf"), "{prompt}");
}

/// Review Focus 3: a typo is refused with a did-you-mean before anything
/// is enqueued, and the session is left as it was.
#[tokio::test]
async fn send_skill_typo_is_refused_before_enqueue() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    install_skill(&tmp.path().join("global"), "pdf", "PDF STEPS");
    let (global, record) = skill_session(&tmp, "ses_skills_typo01", &agent(""), &[], &[]).await;
    let before = fs::read(session_record_path(
        &global,
        SessionScope::Active,
        &record.session_id,
    ))
    .unwrap();
    let err = with_mock_home(
        &global,
        ACK,
        send(SendArgs {
            session_id: record.session_id.clone(),
            prompt: Some("hi".into()),
            skills: vec!["pdff".into()],
            detach: true,
            view: None,
        }),
    )
    .await
    .expect_err("an unknown skill is refused");
    assert!(
        err.to_string()
            .contains("unknown skill \"pdff\" (did you mean pdf?)"),
        "{err}"
    );
    let queue = session_turn_queue_dir(&global, SessionScope::Active, &record.session_id);
    assert!(
        !queue.exists() || fs::read_dir(&queue).unwrap().next().is_none(),
        "nothing was enqueued"
    );
    assert_eq!(
        fs::read(session_record_path(
            &global,
            SessionScope::Active,
            &record.session_id
        ))
        .unwrap(),
        before
    );
}

/// The operator may invoke a project skill by its bare name, and a skill
/// marked `disable-model-invocation`, which exists for exactly this.
#[tokio::test]
async fn send_skill_accepts_project_and_disable_model_invocation_skills() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let manual = tmp.path().join("global/skills/manual");
    fs::create_dir_all(&manual).unwrap();
    fs::write(
        manual.join("SKILL.md"),
        "---\nname: manual\ndescription: d.\ndisable-model-invocation: true\n---\nMANUAL STEPS\n",
    )
    .unwrap();
    let project = tmp.path().join("project");
    install_skill(&project.join(".rupu"), "notes", "NOTE STEPS");
    let (global, mut record) = skill_session(&tmp, "ses_skills_proj01", &agent(""), &[], &[]).await;
    record.project_root = Some(project);
    write_session(&global, SessionScope::Active, &record).unwrap();

    let invoked = invoke(&global, &record.session_id, &["notes", "manual"])
        .await
        .unwrap();
    let layers: Vec<(&str, rupu_skills::Layer)> =
        invoked.iter().map(|f| (f.name.as_str(), f.layer)).collect();
    assert_eq!(
        layers,
        vec![
            ("notes", rupu_skills::Layer::Project),
            ("manual", rupu_skills::Layer::Global)
        ]
    );
    let evs = operator_turn(&global, &record.session_id, "run_proj_1", "", invoked).await;
    let message = user_message(&evs);
    assert!(message.starts_with("<skill name=\"notes\""), "{message}");
    assert!(message.contains("NOTE STEPS"), "{message}");
    assert!(
        message.contains("\n</skill>\n\n<skill name=\"manual\""),
        "{message}"
    );
    assert!(message.ends_with("MANUAL STEPS\n</skill>"), "{message}");
    assert_eq!(
        loads(&evs),
        pairs(&[("operator", "notes"), ("operator", "manual")])
    );
    let next = turn(&global, &record.session_id, "run_proj_2", &load("manual")).await;
    let (loaded, error) = tool_result(&next, "c1");
    assert_eq!(error, None);
    assert!(loaded.contains("MANUAL STEPS"), "{loaded}");
}

/// A wildcard names a set, a bare name in both layers names neither, and a
/// name the session binds from one layer can't be invoked from the other.
#[tokio::test]
async fn send_skill_refuses_wildcards_and_ambiguous_names() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().join("project");
    install_skill(&tmp.path().join("global"), "pdf", "GLOBAL PDF");
    install_skill(&project.join(".rupu"), "pdf", "PROJECT PDF");
    install_skill(&tmp.path().join("global"), "notes", "NOTE STEPS");
    install_skill(&project.join(".rupu"), "notes", "PROJECT NOTES");
    let (global, mut record) =
        skill_session(&tmp, "ses_skills_wild01", &agent(""), &["notes"], &[]).await;
    record.project_root = Some(project);
    write_session(&global, SessionScope::Active, &record).unwrap();

    let refused = |entries: &'static [&'static str]| {
        let global = global.clone();
        let id = record.session_id.clone();
        async move {
            invoke(&global, &id, entries)
                .await
                .expect_err("refused")
                .to_string()
        }
    };
    assert!(refused(&["*"])
        .await
        .contains("skill entry \"*\" is a wildcard; invoke one skill by name"));
    assert!(refused(&["project:*"])
        .await
        .contains("skill entry \"project:*\" is a wildcard"));
    let both = refused(&["pdf"]).await;
    assert!(
        both.contains(
            "skill \"pdf\" is installed in both the global and the project layer; grant \
             global:pdf or project:pdf"
        ),
        "{both}"
    );
    let bound = |name: &str| {
        format!(
            "skill \"{name}\" is granted from both the global and the project layer; grant only \
             one of global:{name} or project:{name}"
        )
    };
    let other_layer = refused(&["project:notes"]).await;
    assert!(
        other_layer.contains("this session already has global:notes; invoke global:notes"),
        "{other_layer}"
    );
    let held_twice = refused(&["global:pdf", "project:pdf"]).await;
    assert!(held_twice.contains(&bound("pdf")), "{held_twice}");

    let ok = invoke(&global, &record.session_id, &["global:pdf", "global:pdf"])
        .await
        .unwrap();
    let names: Vec<(&str, rupu_skills::Layer)> =
        ok.iter().map(|f| (f.name.as_str(), f.layer)).collect();
    assert_eq!(names, vec![("pdf", rupu_skills::Layer::Global)]);
}

/// The error a turn's transcript closed on.
fn closing_error(evs: &[TranscriptEvent]) -> Option<String> {
    evs.iter()
        .find_map(|e| match e {
            TranscriptEvent::RunComplete { status, error, .. } => Some((*status, error.clone())),
            _ => None,
        })
        .filter(|(status, _)| *status == RunStatus::Error)
        .and_then(|(_, error)| error)
}

fn held(record: &SessionRecord) -> Vec<(&str, rupu_skills::Layer)> {
    record
        .skill_index
        .iter()
        .map(|f| (f.name.as_str(), f.layer))
        .collect()
}

/// Two sends queued before either turn runs both pass the send-time check,
/// since the set gains a skill only when its turn runs. The second turn,
/// which names the first one's skill from the other layer, fails with a
/// transcript and leaves the set as the first turn left it, so no later
/// turn inherits a name bound from both layers.
#[tokio::test]
async fn queued_sends_of_one_name_from_both_layers_fail_the_second_turn() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().join("project");
    install_skill(&tmp.path().join("global"), "pdf", "GLOBAL PDF");
    install_skill(&project.join(".rupu"), "pdf", "PROJECT PDF");
    let (global, mut record) = skill_session(&tmp, "ses_skills_race01", &agent(""), &[], &[]).await;
    record.project_root = Some(project);
    write_session(&global, SessionScope::Active, &record).unwrap();
    let id = record.session_id.clone();

    let first = invoke(&global, &id, &["global:pdf"]).await.unwrap();
    let second = invoke(&global, &id, &["project:pdf"]).await.unwrap();
    operator_turn(&global, &id, "run_race_1", "one", first).await;
    let evs = operator_turn(&global, &id, "run_race_2", "two", second).await;

    let conflict = "skills: this session already has global:pdf; invoke global:pdf";
    assert_eq!(closing_error(&evs).as_deref(), Some(conflict));
    let (after, _) = read_session(&global, &id).unwrap();
    assert_eq!(after.status, SessionStatus::Failed);
    assert_eq!(after.last_error.as_deref(), Some(conflict));
    assert_eq!(held(&after), vec![("pdf", rupu_skills::Layer::Global)]);

    let next = turn(&global, &id, "run_race_3", ACK).await;
    assert_eq!(notices(&next, "skill_missing"), Vec::<String>::new());
}

/// A skill removed between its send and its turn is reported by that turn
/// once and is never frozen into the set, so no later turn reports it.
#[tokio::test]
async fn a_skill_removed_since_send_is_reported_once_and_not_frozen() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let pdf = install_skill(&tmp.path().join("global"), "pdf", "PDF STEPS");
    let (global, record) = skill_session(&tmp, "ses_skills_gone01", &agent(""), &[], &[]).await;
    let id = record.session_id.clone();

    let invoked = invoke(&global, &id, &["pdf"]).await.unwrap();
    fs::remove_dir_all(&pdf).unwrap();
    let evs = operator_turn(&global, &id, "run_gone_1", "hi", invoked).await;
    assert_eq!(
        notices(&evs, "skill_missing"),
        vec!["skill pdf (global) is not available to this run: removed".to_string()]
    );
    assert_eq!(user_message(&evs), "hi");
    let (after, _) = read_session(&global, &id).unwrap();
    assert_eq!(after.status, SessionStatus::Idle, "{:?}", after.last_error);
    assert_eq!(held(&after), Vec::new());

    let next = turn(&global, &id, "run_gone_2", ACK).await;
    assert_eq!(notices(&next, "skill_missing"), Vec::<String>::new());
}

/// A turn with no prompt whose invoked skills all can't be given has
/// nothing to send: it fails with a transcript naming why each can't be.
#[tokio::test]
async fn a_turn_with_no_prompt_and_no_skill_to_give_fails_with_a_transcript() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let pdf = install_skill(&tmp.path().join("global"), "pdf", "PDF STEPS");
    let notes = install_skill(&tmp.path().join("global"), "notes", "NOTE STEPS");
    let (global, record) = skill_session(&tmp, "ses_skills_empty01", &agent(""), &[], &[]).await;
    let id = record.session_id.clone();

    let invoked = invoke(&global, &id, &["pdf", "notes"]).await.unwrap();
    fs::remove_dir_all(&pdf).unwrap();
    fs::write(notes.join("SKILL.md"), "no frontmatter\n").unwrap();
    let evs = operator_turn(&global, &id, "run_empty_1", "", invoked).await;

    let error = closing_error(&evs).expect("the transcript closes on the failure");
    assert!(
        error.starts_with(
            "this turn has no prompt, and none of the skills it invoked can be given: \
             pdf (global): removed; notes (global): invalid: "
        ),
        "{error}"
    );
    let (after, _) = read_session(&global, &id).unwrap();
    assert_eq!(after.status, SessionStatus::Failed);
    assert_eq!(after.last_error.as_deref(), Some(error.as_str()));
    assert_eq!(held(&after), Vec::new());
}

/// `session send --detach --skill` queues the turn with the skills it
/// resolved, for the worker to run it with.
#[tokio::test]
async fn send_skill_queues_the_resolved_skills_for_the_worker() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let pdf_dir = install_skill(&tmp.path().join("global"), "pdf", "PDF STEPS");
    let (global, mut record) =
        skill_session(&tmp, "ses_skills_queue01", &agent(""), &[], &[]).await;
    let mut worker = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    record.worker_pid = Some(worker.id());
    record.worker_binary_mtime = current_binary_mtime();
    write_session(&global, SessionScope::Active, &record).unwrap();

    let sent = with_mock_home(
        &global,
        ACK,
        send(SendArgs {
            session_id: record.session_id.clone(),
            prompt: None,
            skills: vec!["pdf".into()],
            detach: true,
            view: None,
        }),
    )
    .await;
    let _ = worker.kill();
    let _ = worker.wait();
    sent.expect("the turn is queued");

    let queue = session_turn_queue_dir(&global, SessionScope::Active, &record.session_id);
    let requests: Vec<serde_json::Value> = fs::read_dir(&queue)
        .unwrap()
        .map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap())
        .collect();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let pdf = rupu_skills::load_skill(&pdf_dir, rupu_skills::Layer::Global).unwrap();
    assert_eq!(requests[0]["prompt"], json!(""));
    assert_eq!(
        requests[0]["operator_skills"],
        json!([{
            "name": "pdf",
            "layer": "global",
            "preload": false,
            "indexed": false,
            "reason": "operator",
            "stamp": pdf.stamp.hex(),
        }])
    );
}

fn queued_request(extra: serde_json::Value) -> SessionTurnRequest {
    let mut request = json!({
        "version": 1,
        "request_id": "01KTEST0000000000000000001",
        "run_id": "run_queued_1",
        "prompt": "hi",
        "transcript_path": "/sessions/ses_q/transcripts/run_queued_1.jsonl",
        "enqueued_at": "2026-10-01T00:00:00Z",
    });
    request
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(request).unwrap()
}

/// A request queued by a rupu that predates `--skill` still reads, as a
/// turn that invoked nothing.
#[test]
fn a_queued_request_without_operator_skills_reads_as_invoking_none() {
    let request = queued_request(json!({}));
    assert_eq!(request.operator_skills, Vec::new());
    let args = request.turn_args("ses_q");
    assert_eq!(args.operator_skills, Vec::new());
}

/// The worker runs a queued request's turn with the skills it carries.
#[test]
fn the_worker_runs_a_queued_turn_with_its_invoked_skills() {
    let pdf = rupu_skills::FrozenSkill {
        name: "pdf".into(),
        layer: rupu_skills::Layer::Project,
        preload: false,
        indexed: false,
        reason: rupu_skills::SkillReason::Operator,
        stamp: "ab".repeat(32),
        content_hash: None,
    };
    let request = queued_request(json!({ "operator_skills": [pdf] }));
    let args = request.turn_args("ses_q");
    assert_eq!(
        (
            args.session_id.as_str(),
            args.run_id.as_str(),
            args.prompt.as_str()
        ),
        ("ses_q", "run_queued_1", "hi")
    );
    assert_eq!(args.operator_skills, vec![pdf]);
}

const SUMMARY: &str = r#"[{ "AssistantText": { "text": "Summary of prior work.", "stop": "end_turn", "input_tokens": 500, "output_tokens": 10 } }]"#;

fn dense_history(len: usize) -> Vec<Message> {
    let dense = "x".repeat(1000);
    let mut history = vec![Message::user(&format!("task: {dense}"))];
    for i in 0..len {
        history.push(Message::assistant(&format!("assistant {i}: {dense}")));
        history.push(Message::user(&format!("user {i}: {dense}")));
    }
    history
}

/// A session over `code-review` and `pdf` whose conversation loaded both,
/// with a history long enough to compact under a 1,000-token window.
async fn compactable_session(
    tmp: &tempfile::TempDir,
    session_id: &str,
) -> (PathBuf, SessionRecord, PathBuf) {
    let global = tmp.path().join("global");
    let review = install_skill(&global, "code-review", "REVIEW STEPS");
    let pdf = install_skill(&global, "pdf", "PDF STEPS");
    let (global, mut record) = skill_session(
        tmp,
        session_id,
        &agent("skills: [code-review, pdf]"),
        &[],
        &[],
    )
    .await;
    let loaded = |dir: &Path| {
        let skill = rupu_skills::load_skill(dir, rupu_skills::Layer::Global).unwrap();
        rupu_agent::LoadedSkill {
            name: skill.name.as_str().to_owned(),
            content_hash: skill.content_hash().unwrap().hex(),
        }
    };
    record.loaded_skills = vec![loaded(&pdf), loaded(&review)];
    record.message_history = dense_history(5);
    record.context_window_tokens = Some(1000);
    fs::create_dir_all(&record.transcripts_dir).unwrap();
    write_session(&global, SessionScope::Active, &record).unwrap();
    (global, record, pdf)
}

fn history_len(global: &Path, session_id: &str) -> usize {
    read_session(global, session_id)
        .unwrap()
        .0
        .message_history
        .len()
}

/// The re-attach blocks in the session's first message.
fn reattach_blocks(global: &Path, session_id: &str) -> Vec<String> {
    let (session, _) = read_session(global, session_id).unwrap();
    session.message_history[0]
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text }
                if text.starts_with(rupu_skills::render::REATTACH_MARKER) =>
            {
                Some(text.clone())
            }
            _ => None,
        })
        .collect()
}

fn compact_request(record: &SessionRecord, run_id: &str) -> SessionTurnRequest {
    SessionTurnRequest {
        version: SessionTurnRequest::VERSION,
        request_id: format!("req_{run_id}"),
        run_id: run_id.into(),
        prompt: "[compact]".into(),
        transcript_path: record.transcripts_dir.join(format!("{run_id}.jsonl")),
        enqueued_at: Utc::now(),
        compact: true,
        operator_skills: Vec::new(),
    }
}

/// Appends dense turns so the next compaction has something to summarise.
fn grow_history(global: &Path, session_id: &str) {
    let (mut session, scope) = read_session(global, session_id).unwrap();
    session
        .message_history
        .extend(dense_history(5).into_iter().skip(1));
    write_session(global, scope, &session).unwrap();
}

/// Review Focus 2 (`rupu session compact`): the loaded skills are put back
/// in the compacted first message, once, however often it compacts.
#[tokio::test]
async fn session_compact_reattaches_loaded_skills_once() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (global, record, _) = compactable_session(&tmp, "ses_skills_compact01").await;
    for _ in 0..2 {
        let before = history_len(&global, &record.session_id);
        with_mock_home(&global, SUMMARY, compact(&record.session_id, None))
            .await
            .expect("compact completes");
        assert!(
            history_len(&global, &record.session_id) < before,
            "it compacted"
        );
        let blocks = reattach_blocks(&global, &record.session_id);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert!(
            blocks[0].contains("## Skill: code-review\n"),
            "{}",
            blocks[0]
        );
        assert!(blocks[0].contains("REVIEW STEPS"), "{}", blocks[0]);
        assert!(blocks[0].contains("PDF STEPS"), "{}", blocks[0]);
        grow_history(&global, &record.session_id);
    }
    let (after, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(after.loaded_skills, record.loaded_skills);
}

/// Review Focus 2 (the worker's compact pseudo-turn): the same re-attach,
/// recorded in the pseudo-turn's transcript.
#[tokio::test]
async fn compact_pseudo_turn_reattaches_loaded_skills_once() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (global, record, _) = compactable_session(&tmp, "ses_skills_compact02").await;
    for run in ["run_compact_a", "run_compact_b"] {
        let request = compact_request(&record, run);
        let before = history_len(&global, &record.session_id);
        with_mock_home(
            &global,
            SUMMARY,
            run_compact_request(&global, SessionScope::Active, &record.session_id, &request),
        )
        .await
        .expect("the pseudo-turn completes");
        assert!(
            history_len(&global, &record.session_id) < before,
            "it compacted"
        );
        let blocks = reattach_blocks(&global, &record.session_id);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert!(blocks[0].contains("REVIEW STEPS"), "{}", blocks[0]);
        let evs: Vec<TranscriptEvent> = JsonlReader::iter(&request.transcript_path)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            loads(&evs),
            pairs(&[("reattach", "pdf"), ("reattach", "code-review")])
        );
        grow_history(&global, &record.session_id);
    }
    let (after, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(after.loaded_skills, record.loaded_skills);
}

/// A loaded skill uninstalled before compaction is named as no longer
/// installed, reported, and kept in the loaded set for a reinstall.
#[tokio::test]
async fn compaction_names_a_loaded_skill_that_was_uninstalled() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (global, record, pdf) = compactable_session(&tmp, "ses_skills_compact03").await;
    fs::remove_dir_all(&pdf).unwrap();

    with_mock_home(&global, SUMMARY, compact(&record.session_id, None))
        .await
        .expect("compact completes");
    let blocks = reattach_blocks(&global, &record.session_id);
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert!(
        blocks[0].contains("No longer installed: pdf"),
        "{}",
        blocks[0]
    );
    assert!(!blocks[0].contains("PDF STEPS"), "{}", blocks[0]);

    grow_history(&global, &record.session_id);
    let request = compact_request(&record, "run_compact_gone");
    with_mock_home(
        &global,
        SUMMARY,
        run_compact_request(&global, SessionScope::Active, &record.session_id, &request),
    )
    .await
    .expect("the pseudo-turn completes");
    let evs: Vec<TranscriptEvent> = JsonlReader::iter(&request.transcript_path)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(
        notices(&evs, "skill_missing"),
        vec!["skill pdf is no longer installed and was not re-attached".to_string()]
    );
    let (after, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(after.loaded_skills, record.loaded_skills);
}
