use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tower::ServiceExt;

const SESSION: &str = "ses_capabilities";

struct NoLaunch;
#[async_trait::async_trait]
impl crate::launcher::RunLauncher for NoLaunch {
    async fn launch(
        &self,
        _: crate::launcher::LaunchRequest,
    ) -> Result<String, crate::launcher::LaunchError> {
        panic!("capability reads must never launch work")
    }
}

struct NoStart;
#[async_trait::async_trait]
impl crate::session_starter::SessionStarter for NoStart {
    async fn start(
        &self,
        _: crate::session_starter::SessionStartRequest,
    ) -> Result<String, crate::session_starter::SessionStartError> {
        panic!("capability reads must never start a session")
    }
}

fn fixture() -> (tempfile::TempDir, AppState, String) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&project).unwrap();
    let workspace = rupu_workspace::register(
        &rupu_workspace::WorkspaceStore {
            root: global.join("workspaces"),
        },
        &project,
    )
    .unwrap();
    let dir = global.join("sessions").join(SESSION);
    fs::create_dir_all(&dir).unwrap();
    write_json(
        &dir.join("session.json"),
        &json!({
            "version": 4, "session_id": SESSION, "agent_name": "rupu-session",
            "builtin_harness": true, "provider_name": "fixture-provider", "model": "fixture-model",
            "agent_system_prompt": "private fixture instructions", "agent_tools": ["read_file"],
            "max_turns": 10, "permission_mode": "readonly", "no_stream": true,
            "workspace_id": workspace.id, "workspace_path": project, "project_root": project,
            "transcripts_dir": global.join("transcripts"),
            "created_at": "2026-11-06T00:00:00Z", "updated_at": "2026-11-06T00:00:00Z",
            "status": "idle", "message_history": [], "runs": []
        }),
    );
    // Invalid provider configuration is a negative control: read-only capability
    // inspection has no reason to parse config or resolve a provider.
    fs::write(global.join("config.toml"), "invalid = [").unwrap();
    let state = AppState::new(global, rupu_config::PricingConfig::default());
    (tmp, state, workspace.id)
}

fn session_dir(state: &AppState) -> PathBuf {
    state.global_dir.join("sessions").join(SESSION)
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn edit_record(state: &AppState, change: impl FnOnce(&mut Value)) {
    let path = session_dir(state).join("session.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut record);
    write_json(&path, &record);
}

fn install(root: &Path, name: &str) {
    let dir = root.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Fixture skill.\n---\nFixture instructions.\n"),
    )
    .unwrap();
}

/// Include directory names as well as bytes, so accidental lock files and
/// empty state directories are observable read-only violations.
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn collect(root: &Path, here: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(here).unwrap() {
            let path = entry.unwrap().path();
            let key = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                out.insert(key, None);
                collect(root, &path, out);
            } else {
                out.insert(key, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    collect(root, root, &mut out);
    out
}

async fn get(state: AppState, path: &str) -> (StatusCode, Value) {
    let response = routes()
        .with_state(state)
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn observed(state: &AppState, workspace: &str) -> Value {
    edit_record(state, |record| {
        record["last_run_id"] = json!("run_first");
        record["runs"] = json!([{
            "run_id": "run_first", "prompt": "fixture turn",
            "transcript_path": "unused-fixture-transcript.jsonl",
            "started_at": "2026-11-06T00:00:00Z"
        }]);
    });
    json!({
        "schema_version": 1, "session_id": SESSION, "workspace_id": workspace,
        "settings_revision": 0, "orchestration_revision": 0,
        "effective": {
            "schema_version": 1, "run_id": "run_first", "agent": "rupu-session",
            "provider": "fixture-provider", "model": "fixture-model",
            "observed_at": "2026-11-06T00:00:00Z", "applies": "assembled_turn",
            "permission_mode": "readonly", "approval_channel_available": false,
            "tools": [], "skills": [], "connectors": [], "limitations": ["Fixture observation only."]
        }
    })
}

#[tokio::test]
async fn planning_is_readonly_explicit_and_unassembled_on_a_readonly_deployment() {
    let (tmp, state, workspace) = fixture();
    install(&state.global_dir, "coverage-assets");
    install(&state.global_dir, "unrelated");
    install(&tmp.path().join("project/.rupu"), "finding-report");
    let before = tree(tmp.path());
    for state in [
        state.clone(),
        state
            .with_launcher(Some(Arc::new(NoLaunch)))
            .with_session_starter(Some(Arc::new(NoStart))),
    ] {
        let (status, response) = get(
            state,
            &format!("/api/projects/{workspace}/session-capabilities?host=local"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["schema_version"], 1);
        assert_eq!(response["workspace_id"], workspace);
        assert_eq!(response["preset"]["id"], "rupu-session-v1");
        assert_eq!(response["preset"]["enabled"], true);
        assert_eq!(
            response["skills"]["defaults"],
            json!(["global:coverage-assets"])
        );
        assert_eq!(response["skills"]["selected"].as_array().unwrap().len(), 1);
        assert_eq!(
            response["declared_tools"],
            json!(rupu_agent::builtin::session_agent().tools)
        );
        assert_eq!(response["permission_mode"], "readonly");
        assert_eq!(response["applies"], "new_session");
        assert!(response["effective"].is_null());
        assert!(response.get("provider").is_none());
        assert!(response.get("services").is_none());
    }
    assert_eq!(tree(tmp.path()), before);
}

#[tokio::test]
async fn planning_opt_out_and_missing_skills_do_not_initialize_state() {
    let (tmp, state, workspace) = fixture();
    let before = tree(tmp.path());
    let path = format!("/api/projects/{workspace}/session-capabilities");
    let (status, response) = get(state.clone(), &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["skills"]["selected"], json!([]));
    assert_eq!(
        response["skills"]["diagnostics"].as_array().unwrap().len(),
        4
    );
    let (status, response) = get(state, &format!("{path}?skill_preset=none")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["preset"]["enabled"], false);
    assert_eq!(response["skills"]["diagnostics"], json!([]));
    assert_eq!(tree(tmp.path()), before);
}

#[tokio::test]
async fn zero_turn_legacy_session_has_no_effective_evidence_or_implied_preset() {
    let (tmp, state, workspace) = fixture();
    install(&state.global_dir, "coverage-assets");
    let before = tree(tmp.path());
    let (status, response) = get(state, &format!("/api/sessions/{SESSION}/capabilities")).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["workspace_id"], workspace);
    assert!(response["preset"].is_null());
    assert_eq!(response["preset_diagnostics"], json!([]));
    assert_eq!(response["skills"]["entries"], json!([]));
    assert!(response["effective"].is_null());
    assert!(response["effective_settings_revision"].is_null());
    assert!(response["effective_orchestration_revision"].is_null());
    assert_eq!(response["effective_current"], false);
    assert!(response["effective_notice"]
        .as_str()
        .unwrap()
        .contains("unobserved"));
    assert!(response.get("message_history").is_none());
    assert!(response.get("agent_system_prompt").is_none());
    assert_eq!(tree(tmp.path()), before);
}

#[tokio::test]
async fn frozen_selection_never_expands_and_reports_changed_and_removed_members() {
    let (tmp, state, _workspace) = fixture();
    install(&state.global_dir, "coverage-assets");
    let plan = session_skill_preset::plan(
        &LayerDirs::for_layers(&state.global_dir, None),
        true,
        &[],
        &[],
    )
    .unwrap();
    edit_record(&state, |record| {
        record["skill_index"] = json!(plan.selected);
        record["skill_preset"] = json!(plan.preset);
        record["skill_preset_diagnostics"] = json!(plan.diagnostics);
    });
    install(&state.global_dir, "finding-report");
    let skill = state.global_dir.join("skills/coverage-assets/SKILL.md");
    let mut text = fs::read_to_string(&skill).unwrap();
    text.push_str("Changed instructions.\n");
    fs::write(&skill, text).unwrap();
    let before = tree(tmp.path());
    let path = format!("/api/sessions/{SESSION}/capabilities");
    let (status, response) = get(state.clone(), &path).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["skills"]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        response["skills"]["entries"][0]["entry"],
        "global:coverage-assets"
    );
    assert_eq!(response["skills"]["entries"][0]["changed"], true);
    assert_eq!(response["skills"]["ready"], true);
    assert_eq!(response["preset_diagnostics"].as_array().unwrap().len(), 3);
    assert!(
        response["preset_diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| {
                diagnostic["entry"] == "global:finding-report"
                    && diagnostic["code"] == "not_installed"
            }),
        "creation-time diagnostics remain frozen after later installation"
    );
    assert_eq!(tree(tmp.path()), before);
    fs::remove_dir_all(skill.parent().unwrap()).unwrap();
    let (status, response) = get(state, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["skills"]["ready"], false);
    assert_eq!(response["skills"]["entries"][0]["missing"], "removed");
    assert_eq!(response["skills"]["entries"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn archived_session_reads_its_own_empty_tool_override_and_revisions() {
    let (tmp, state, _workspace) = fixture();
    write_json(
        &session_dir(&state).join("settings.json"),
        &json!({"revision":7,"tools":[]}),
    );
    write_json(
        &session_dir(&state).join("orchestration.json"),
        &json!({"revision":3,"enabled":true}),
    );
    let archive = state.global_dir.join("sessions-archive");
    fs::create_dir_all(&archive).unwrap();
    fs::rename(session_dir(&state), archive.join(SESSION)).unwrap();
    let before = tree(tmp.path());
    let (status, response) = get(state, &format!("/api/sessions/{SESSION}/capabilities")).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["archived"], true);
    assert_eq!(response["declared_tools"], json!([]));
    assert_eq!(response["settings_revision"], 7);
    assert_eq!(response["orchestration_revision"], 3);
    assert!(response["effective"].is_null());
    assert_eq!(tree(tmp.path()), before);
}

#[tokio::test]
async fn current_snapshot_requires_exact_turn_and_both_revisions() {
    let (tmp, state, workspace) = fixture();
    let snapshot = observed(&state, &workspace);
    write_json(&session_dir(&state).join("capabilities.json"), &snapshot);
    let before = tree(tmp.path());
    let path = format!("/api/sessions/{SESSION}/capabilities");
    let (status, response) = get(state.clone(), &path).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["effective"], snapshot["effective"]);
    assert_eq!(response["effective_current"], true);
    assert_eq!(response["effective_settings_revision"], 0);
    assert_eq!(response["effective_orchestration_revision"], 0);
    assert_eq!(tree(tmp.path()), before);
    for (file, value) in [
        ("settings.json", json!({"revision": 1, "tools": []})),
        (
            "orchestration.json",
            json!({"revision": 1, "enabled": false}),
        ),
    ] {
        write_json(&session_dir(&state).join(file), &value);
        let (status, response) = get(state.clone(), &path).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["effective_current"], false);
        assert_eq!(response["effective"], snapshot["effective"]);
        assert!(response["effective_notice"]
            .as_str()
            .unwrap()
            .contains("historical"));
        fs::remove_file(session_dir(&state).join(file)).unwrap();
    }
    edit_record(&state, |record| {
        record["active_run_id"] = json!("run_second")
    });
    let (_, response) = get(state, &path).await;
    assert_eq!(
        response["effective_current"], false,
        "active turn takes precedence over last turn"
    );
}

#[tokio::test]
async fn foreign_corrupt_partial_and_future_snapshots_are_refused_without_writes() {
    let (tmp, state, workspace) = fixture();
    let baseline = observed(&state, &workspace);
    let path = format!("/api/sessions/{SESSION}/capabilities");
    for (pointer, value) in [
        ("/session_id", json!("ses_foreign")),
        ("/workspace_id", json!("ws_foreign")),
        ("/schema_version", json!(2)),
        ("/effective/schema_version", json!(2)),
        ("/effective/run_id", json!("run_foreign")),
        ("/effective/applies", json!("next_turn")),
        ("/settings_revision", json!(null)),
    ] {
        let mut snapshot = baseline.clone();
        *snapshot.pointer_mut(pointer).unwrap() = value;
        write_json(&session_dir(&state).join("capabilities.json"), &snapshot);
        let before = tree(tmp.path());
        let (status, response) = get(state.clone(), &path).await;
        assert_eq!(status, StatusCode::CONFLICT, "{pointer}: {response}");
        assert!(response.get("effective").is_none());
        assert_eq!(tree(tmp.path()), before);
    }
    for snapshot in [json!({}), json!({"effective": baseline["effective"]}), {
        let mut snapshot = baseline.clone();
        snapshot["unknown"] = json!(true);
        snapshot
    }] {
        write_json(&session_dir(&state).join("capabilities.json"), &snapshot);
        assert_eq!(get(state.clone(), &path).await.0, StatusCode::CONFLICT);
    }
    fs::write(session_dir(&state).join("capabilities.json"), "{").unwrap();
    assert_eq!(get(state, &path).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn query_host_identity_version_and_missing_guards_are_readonly() {
    let (tmp, state, workspace) = fixture();
    let before = tree(tmp.path());
    let project = format!("/api/projects/{workspace}/session-capabilities");
    let session = format!("/api/sessions/{SESSION}/capabilities");
    for route in [&project, &session] {
        for suffix in ["?host=remote", "?host=", "?host=unknown"] {
            assert_eq!(
                get(state.clone(), &format!("{route}{suffix}")).await.0,
                StatusCode::NOT_IMPLEMENTED
            );
        }
        assert_eq!(
            get(state.clone(), &format!("{route}?unknown=true")).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    for preset in ["future-preset", "*", "global:*"] {
        assert_eq!(
            get(state.clone(), &format!("{project}?skill_preset={preset}"))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        get(state.clone(), &format!("{session}?skill_preset=none"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(state.clone(), "/api/sessions/ses_missing/capabilities")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(
            state.clone(),
            "/api/projects/ws_missing/session-capabilities"
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(tree(tmp.path()), before);
    for (field, value) in [
        ("session_id", json!("ses_other")),
        ("version", json!(SessionRecord::VERSION + 1)),
    ] {
        edit_record(&state, |record| record[field] = value);
        let before = tree(tmp.path());
        assert_eq!(get(state.clone(), &session).await.0, StatusCode::CONFLICT);
        assert_eq!(tree(tmp.path()), before);
        edit_record(&state, |record| record["session_id"] = json!(SESSION));
    }
}

#[tokio::test]
async fn duplicate_active_and_archived_identity_is_ambiguous() {
    let (tmp, state, _workspace) = fixture();
    let archive = state.global_dir.join("sessions-archive").join(SESSION);
    fs::create_dir_all(&archive).unwrap();
    fs::copy(
        session_dir(&state).join("session.json"),
        archive.join("session.json"),
    )
    .unwrap();
    let before = tree(tmp.path());
    assert_eq!(
        get(state, &format!("/api/sessions/{SESSION}/capabilities"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(tree(tmp.path()), before);
}
