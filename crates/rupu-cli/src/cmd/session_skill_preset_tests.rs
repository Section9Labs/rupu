//! CLI fixture checks for curated selections frozen before any worker starts.
//! These temporary on-disk skills are unit fixtures, not live provider acceptance.

use super::*;
use clap::Parser;
use rupu_runtime::session_skill_preset::{PRESET_ENTRIES, PRESET_ID, PRESET_VERSION};
use rupu_skills::Layer;

fn args(extra: &[&str]) -> StartArgs {
    let mut argv = vec![
        "rupu",
        "session",
        "start",
        "rupu-session",
        "--provider",
        "openai",
        "--model",
        "fixture-model",
        "--prompt",
        "Inspect this fixture project.",
    ];
    argv.extend_from_slice(extra);
    match crate::Cli::try_parse_from(argv).unwrap().command {
        crate::Cmd::Session {
            action: Action::Start(args),
        } => args,
        _ => panic!("expected session start"),
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let global = temp.path().join("global");
    let project = temp.path().join("project");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&project).unwrap();
    (temp, global, project)
}

fn install(rupu_dir: &Path, name: &str) {
    let dir = rupu_dir.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: Fixture guidance for {name}.\n---\n\
             PRIVATE_FIXTURE_BODY_{name}\n"
        ),
    )
    .unwrap();
}

fn install_preset(global: &Path) {
    for entry in PRESET_ENTRIES {
        install(global, entry.strip_prefix("global:").unwrap());
    }
}

fn selections(record: &SessionRecord) -> Vec<String> {
    record
        .skill_index
        .iter()
        .map(|skill| format!("{}:{}", skill.layer, skill.name))
        .collect()
}

fn assert_unstarted(global: &Path, record: &SessionRecord) {
    assert!(record.runs.is_empty());
    assert!(record.message_history.is_empty());
    assert!(record.worker_pid.is_none());
    assert_eq!(record.total_turns, 0);
    assert!(!global.join("runs").exists());
}

#[tokio::test]
async fn builtin_start_freezes_actual_installed_defaults_without_preloading_or_all_grants() {
    let (_temp, global, project) = fixture();
    install_preset(&global);
    install(&global, "unrelated");
    let (record, _) = prepare_start(&args(&[]), global.clone(), project)
        .await
        .unwrap();
    assert_eq!(
        selections(&record),
        PRESET_ENTRIES
            .iter()
            .map(|entry| (*entry).to_string())
            .collect::<Vec<_>>()
    );
    let preset = record.skill_preset.as_ref().unwrap();
    assert!(preset.enabled);
    assert_eq!(preset.id, PRESET_ID);
    assert_eq!(preset.version, PRESET_VERSION);
    assert!(record.skill_index.iter().all(|skill| {
        skill.layer == Layer::Global
            && skill.indexed
            && !skill.preload
            && skill.content_hash.is_none()
            && rupu_skills::Stamp::from_hex(&skill.stamp).is_some()
    }));
    let prompt = record.skill_prompt.as_deref().unwrap();
    assert!(prompt.contains("rupu-session-operations"));
    assert!(!prompt.contains("unrelated"));
    assert!(!prompt.contains("PRIVATE_FIXTURE_BODY_"));
    assert_unstarted(&global, &record);
    assert!(!session_dir(&global, SessionScope::Active, &record.session_id).exists());
}

#[tokio::test]
async fn builtin_start_skips_absent_optional_preset_members() {
    let (_temp, global, project) = fixture();
    install(&global, "rupu-session-operations");
    let (record, _) = prepare_start(&args(&[]), global.clone(), project)
        .await
        .unwrap();
    assert_eq!(selections(&record), vec!["global:rupu-session-operations"]);
    assert!(record.skill_preset.as_ref().unwrap().enabled);
    assert_eq!(record.skill_preset_diagnostics.len(), 3);
    assert!(record
        .skill_preset_diagnostics
        .iter()
        .all(|entry| entry.code
            == rupu_runtime::session_skill_preset::SkillDiagnosticCode::NotInstalled));
    assert_unstarted(&global, &record);
}

#[tokio::test]
async fn builtin_preset_opt_out_preserves_explicit_skill_requests() {
    let (_temp, global, project) = fixture();
    install_preset(&global);
    install(&global, "explicit-only");
    let (empty, _) = prepare_start(
        &args(&["--skill-preset", "none"]),
        global.clone(),
        project.clone(),
    )
    .await
    .unwrap();
    assert!(!empty.skill_preset.as_ref().unwrap().enabled);
    assert!(empty.skill_index.is_empty());
    assert!(empty.skill_prompt.is_none());
    let (explicit, _) = prepare_start(
        &args(&["--skill-preset", "none", "--skill", "global:explicit-only"]),
        global.clone(),
        project,
    )
    .await
    .unwrap();
    assert!(!explicit.skill_preset.as_ref().unwrap().enabled);
    assert_eq!(selections(&explicit), vec!["global:explicit-only"]);
    assert_unstarted(&global, &empty);
    assert_unstarted(&global, &explicit);
}

#[tokio::test]
async fn explicit_project_skill_replaces_same_name_default_without_hiding_ambiguity() {
    let (_temp, global, project) = fixture();
    install(&global, "rupu-session-operations");
    install(&project.join(".rupu"), "rupu-session-operations");
    let (default, _) = prepare_start(&args(&[]), global.clone(), project.clone())
        .await
        .unwrap();
    assert_eq!(selections(&default), vec!["global:rupu-session-operations"]);
    let (explicit, _) = prepare_start(
        &args(&["--skill", "project:rupu-session-operations"]),
        global.clone(),
        project.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        selections(&explicit),
        vec!["project:rupu-session-operations"]
    );
    let ambiguous = prepare_start(
        &args(&["--skill", "rupu-session-operations"]),
        global.clone(),
        project,
    )
    .await
    .unwrap_err();
    assert!(ambiguous.to_string().contains("installed in both"));
    assert_unstarted(&global, &explicit);
}

#[tokio::test]
async fn persisted_session_does_not_expand_when_more_preset_skills_are_installed() {
    let (_temp, global, project) = fixture();
    install(&global, "rupu-session-operations");
    let (record, _) = prepare_start(&args(&[]), global.clone(), project)
        .await
        .unwrap();
    write_session(&global, SessionScope::Active, &record).unwrap();
    install_preset(&global);
    install(&global, "unrelated");
    let (stored, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(stored.skill_index, record.skill_index);
    assert_eq!(stored.skill_prompt, record.skill_prompt);
    assert_eq!(stored.skill_preset, record.skill_preset);
    let (resolved, missing) = rupu_runtime::assembly::resolve_frozen_skills(
        &skill_dirs(&global, stored.project_root.as_deref()),
        &stored.skill_index,
    )
    .await
    .unwrap();
    assert!(missing.is_empty());
    assert_eq!(resolved.unwrap().freeze(), record.skill_index);
    assert_eq!(selections(&stored), vec!["global:rupu-session-operations"]);
    assert_unstarted(&global, &stored);
}
