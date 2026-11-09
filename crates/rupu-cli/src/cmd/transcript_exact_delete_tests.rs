use super::*;
use serde_json::json;
use std::{fs, os::unix::fs::symlink};

#[test]
fn only_missing_process_proves_safe_liveness() {
    assert!(require_absent_process(Err(rustix::io::Errno::SRCH)).is_ok());
    for result in [
        Ok(()),
        Err(rustix::io::Errno::PERM),
        Err(rustix::io::Errno::IO),
        Err(rustix::io::Errno::INVAL),
    ] {
        assert!(matches!(
            require_absent_process(result),
            Err(ExactDeleteError::Unavailable(_))
        ));
    }
}

#[test]
fn relative_metadata_workspace_never_uses_process_cwd() {
    let fixture = Fixture::new(false, false);
    let mut metadata = fixture.metadata(None, Some(i32::MAX as u32));
    metadata.workspace_path = PathBuf::from(".");
    fs::write(
        fixture.metadata_path(),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let context = fixture.context();
    assert_eq!(context.state, "unavailable");
    assert_eq!(
        context.reason.as_deref(),
        Some("recorded workspace ownership is not absolute")
    );
    assert!(Path::new(&fixture.scope.path).exists());
    assert!(fixture.metadata_path().exists());
}

#[test]
fn registered_workspace_alias_is_refused_before_and_after_retarget() {
    let fixture = Fixture::new(false, false);
    let alias = fixture.global.parent().unwrap().join("project-alias");
    symlink(&fixture.project, &alias).unwrap();
    let registry = fixture
        .global
        .join("workspaces")
        .join(format!("{}.toml", fixture.workspace));
    let mut record: rupu_workspace::Workspace =
        toml::from_str(&fs::read_to_string(&registry).unwrap()).unwrap();
    record.path = alias.to_string_lossy().into_owned();
    fs::write(&registry, toml::to_string(&record).unwrap()).unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    fs::remove_file(&alias).unwrap();
    symlink(&fixture.global, &alias).unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    assert!(Path::new(&fixture.scope.path).exists());
    assert!(fixture.metadata_path().exists());
}

#[test]
fn captured_global_alias_retarget_does_not_move_lease_or_authority() {
    let fixture = Fixture::new(false, false);
    let root = fixture.global.parent().unwrap();
    let alias = root.join("global-alias");
    let other = root.join("other-global");
    fs::create_dir(&other).unwrap();
    symlink(&fixture.global, &alias).unwrap();
    let snapshot = capture_with_lease(&alias, &fixture.run, &fixture.scope).unwrap();
    fs::remove_file(&alias).unwrap();
    symlink(&other, &alias).unwrap();
    assert_eq!(snapshot.global.path, fixture.global);
    snapshot.recheck(&fixture.run, &fixture.scope).unwrap();
    assert!(rupu_runtime::retention_lease::RetentionLease::try_acquire(&fixture.global).is_err());
    let other_lease = rupu_runtime::retention_lease::RetentionLease::try_acquire(&other).unwrap();
    drop(other_lease);
    assert!(Path::new(&fixture.scope.path).exists());
    assert!(fixture.metadata_path().exists());
}

#[test]
fn pinned_global_root_replacement_is_refused() {
    let fixture = Fixture::new(false, false);
    let snapshot = capture_with_lease(&fixture.global, &fixture.run, &fixture.scope).unwrap();
    let moved = fixture.global.with_file_name("moved-global");
    fs::rename(&fixture.global, &moved).unwrap();
    fs::create_dir(&fixture.global).unwrap();
    assert!(snapshot.global.recheck().is_err());
    assert!(snapshot.recheck(&fixture.run, &fixture.scope).is_err());
    assert!(Path::new(&fixture.scope.path).exists());
    assert!(fixture.metadata_path().exists());
}

struct Fixture {
    _temporary: tempfile::TempDir,
    global: PathBuf,
    project: PathBuf,
    run: String,
    workspace: String,
    scope: ExactTranscriptScope,
}
impl Fixture {
    fn new(global_source: bool, archived: bool) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let global = root.join("global");
        let project = root.join("project");
        fs::create_dir_all(global.join("workspaces")).unwrap();
        fs::create_dir_all(&project).unwrap();
        let run = format!("run_{}", ulid::Ulid::new());
        let workspace = format!("ws_{}", ulid::Ulid::new());
        let record = rupu_workspace::Workspace {
            id: workspace.clone(),
            path: project.to_string_lossy().into_owned(),
            repo_remote: None,
            initial_branch: None,
            created_at: "2026-11-09T00:00:00Z".into(),
            last_run_at: None,
        };
        fs::write(
            global.join("workspaces").join(format!("{workspace}.toml")),
            toml::to_string(&record).unwrap(),
        )
        .unwrap();
        let mut parent = if global_source {
            global.join("transcripts")
        } else {
            project.join(".rupu/transcripts")
        };
        if archived {
            parent = parent.join("archive");
        }
        fs::create_dir_all(&parent).unwrap();
        let scope = ExactTranscriptScope {
            workspace_id: workspace.clone(),
            path: parent
                .join(format!("{run}.jsonl"))
                .to_string_lossy()
                .into_owned(),
        };
        let fixture = Self {
            _temporary: temporary,
            global,
            project,
            run,
            workspace,
            scope,
        };
        fixture.write_metadata(None, Some(i32::MAX as u32));
        fixture.write_transcript(&fixture.run, &fixture.workspace);
        fixture
    }
    fn metadata_path(&self) -> PathBuf {
        Path::new(&self.scope.path).with_file_name(format!("{}.meta.json", self.run))
    }
    fn metadata(&self, session: Option<&str>, pid: Option<u32>) -> StandaloneRunMetadata {
        StandaloneRunMetadata {
            version: 1,
            run_id: self.run.clone(),
            session_id: session.map(str::to_string),
            archived_at: None,
            workspace_path: self.project.clone(),
            project_root: Some(self.project.clone()),
            repo_ref: None,
            issue_ref: None,
            backend_id: "local_checkout".into(),
            worker_id: None,
            trigger_source: "run_cli".into(),
            target: None,
            workspace_strategy: Some("direct_checkout".into()),
            pid,
        }
    }
    fn write_metadata(&self, session: Option<&str>, pid: Option<u32>) {
        fs::write(
            self.metadata_path(),
            serde_json::to_vec(&self.metadata(session, pid)).unwrap(),
        )
        .unwrap();
    }
    fn write_transcript(&self, run: &str, workspace: &str) {
        let value = json!({"type":"run_start","data":{"run_id":run,"workspace_id":workspace,"agent":"fixture","provider":"fixture","model":"fixture","started_at":"2026-11-09T00:00:00Z","mode":"bypass","schema":2}});
        fs::write(&self.scope.path, format!("{}\n", value)).unwrap();
    }
    fn context(&self) -> ExactTranscriptContext {
        super::super::prepare(&self.global, &self.run, &self.scope).unwrap()
    }
    fn ready_revision(&self) -> String {
        let c = self.context();
        assert_eq!(c.state, "ready", "{:?}", c.reason);
        c.revision.unwrap()
    }
}

#[test]
fn exact_project_delete_preserves_duplicate_global_files() {
    let fixture = Fixture::new(false, false);
    let global = fixture.global.join("transcripts");
    fs::create_dir_all(&global).unwrap();
    let other = global.join(format!("{}.jsonl", fixture.run));
    let other_meta = global.join(format!("{}.meta.json", fixture.run));
    fs::write(&other, b"unrelated global transcript").unwrap();
    fs::write(&other_meta, b"unrelated metadata").unwrap();
    let context = fixture.context();
    assert_eq!(context.state, "ready");
    assert_eq!(context.effects.len(), 2);
    assert_eq!(context.source_kind.as_deref(), Some("project_active"));
    super::super::delete(
        &fixture.global,
        &fixture.run,
        &fixture.scope,
        context.revision.as_deref().unwrap(),
    )
    .unwrap();
    assert!(!Path::new(&fixture.scope.path).exists());
    assert!(!fixture.metadata_path().exists());
    assert_eq!(fs::read(other).unwrap(), b"unrelated global transcript");
    assert_eq!(fs::read(other_meta).unwrap(), b"unrelated metadata");
}

#[test]
fn exact_global_and_archive_paths_are_explicit_and_not_fallbacks() {
    for (global, archived, kind) in [
        (true, false, "global_active"),
        (true, true, "global_archive"),
        (false, true, "project_archive"),
    ] {
        let fixture = Fixture::new(global, archived);
        let context = fixture.context();
        assert_eq!(context.state, "ready");
        assert_eq!(context.source_kind.as_deref(), Some(kind));
        assert_eq!(context.archived, Some(archived));
        super::super::delete(
            &fixture.global,
            &fixture.run,
            &fixture.scope,
            context.revision.as_deref().unwrap(),
        )
        .unwrap();
        assert!(!Path::new(&fixture.scope.path).exists());
    }
}

#[test]
fn unrelated_registered_workspace_and_nonexact_path_are_unavailable() {
    let fixture = Fixture::new(false, false);
    let mut scope = fixture.scope.clone();
    scope.path = fixture
        .project
        .join("unrelated.jsonl")
        .to_string_lossy()
        .into_owned();
    let context = super::super::prepare(&fixture.global, &fixture.run, &scope).unwrap();
    assert_eq!(context.state, "unavailable");
    assert!(context.effects.is_empty());
    assert!(context.revision.is_none());
    let registry = fixture
        .global
        .join("workspaces")
        .join(format!("{}.toml", fixture.workspace));
    let mut record: rupu_workspace::Workspace =
        toml::from_str(&fs::read_to_string(&registry).unwrap()).unwrap();
    record.id = "different".into();
    fs::write(registry, toml::to_string(&record).unwrap()).unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    assert!(Path::new(&fixture.scope.path).exists());
}

#[test]
fn session_live_missing_and_invalid_pid_guards_are_never_overridden() {
    let fixture = Fixture::new(false, false);
    for (session, pid) in [
        (Some("ses_fixture"), Some(i32::MAX as u32)),
        (None, Some(std::process::id())),
        (None, None),
        (None, Some(0)),
        (None, Some(u32::MAX)),
    ] {
        fixture.write_metadata(session, pid);
        let context = fixture.context();
        assert_eq!(context.state, "unavailable");
        assert!(context.effects.is_empty());
        assert!(Path::new(&fixture.scope.path).exists());
    }
}

#[test]
fn missing_malformed_unknown_version_and_wrong_workspace_metadata_refuse() {
    let fixture = Fixture::new(false, false);
    fs::remove_file(fixture.metadata_path()).unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    fs::write(fixture.metadata_path(), b"{").unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    for variant in ["version", "run", "workspace", "backend"] {
        let mut metadata = fixture.metadata(None, Some(i32::MAX as u32));
        match variant {
            "version" => metadata.version = 99,
            "run" => metadata.run_id = format!("run_{}", ulid::Ulid::new()),
            "workspace" => metadata.workspace_path = fixture.global.clone(),
            _ => metadata.backend_id = "remote".into(),
        };
        fs::write(
            fixture.metadata_path(),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        assert_eq!(fixture.context().state, "unavailable");
    }
}

#[test]
fn run_start_requires_exact_registered_workspace_full_id_and_known_schema() {
    let fixture = Fixture::new(false, false);
    fixture.write_transcript(&fixture.run, "another-workspace");
    assert_eq!(fixture.context().state, "unavailable");
    fixture.write_transcript("run_wrong", &fixture.workspace);
    assert_eq!(fixture.context().state, "unavailable");
    fs::write(
        &fixture.scope.path,
        b"{\"type\":\"turn_start\",\"data\":{\"turn_idx\":0}}\n",
    )
    .unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    fixture.write_transcript(&fixture.run, &fixture.workspace);
    let content = fs::read_to_string(&fixture.scope.path)
        .unwrap()
        .replace("\"schema\":2", "\"schema\":99");
    fs::write(&fixture.scope.path, content).unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    assert!(super::super::prepare(&fixture.global, "run_short", &fixture.scope).is_err());
}

#[test]
fn changed_transcript_metadata_registration_and_added_cleanup_refuse_before_unlink() {
    for change in ["transcript", "metadata", "registration", "cleanup"] {
        let fixture = Fixture::new(false, false);
        let revision = fixture.ready_revision();
        match change {
            "transcript" => {
                use std::io::Write;
                let mut f = fs::OpenOptions::new()
                    .append(true)
                    .open(&fixture.scope.path)
                    .unwrap();
                writeln!(f, "{{\"type\":\"turn_start\",\"data\":{{\"turn_idx\":0}}}}").unwrap();
            }
            "metadata" => {
                let mut metadata = fixture.metadata(None, Some(i32::MAX as u32));
                metadata.target = Some("changed".into());
                fs::write(
                    fixture.metadata_path(),
                    serde_json::to_vec(&metadata).unwrap(),
                )
                .unwrap();
            }
            "registration" => {
                use std::io::Write;
                let mut f = fs::OpenOptions::new()
                    .append(true)
                    .open(
                        fixture
                            .global
                            .join("workspaces")
                            .join(format!("{}.toml", fixture.workspace)),
                    )
                    .unwrap();
                writeln!(f, "\n# changed registration").unwrap();
            }
            _ => {
                let directory = fixture.global.join("runs").join(&fixture.run);
                fs::create_dir_all(&directory).unwrap();
                fs::write(
                    directory.join("coverage.jsonl"),
                    b"{\"ledger\":\"begin\",\"v\":1}",
                )
                .unwrap();
            }
        }
        assert!(
            super::super::delete(&fixture.global, &fixture.run, &fixture.scope, &revision).is_err()
        );
        assert!(Path::new(&fixture.scope.path).exists());
        assert!(fixture.metadata_path().exists());
    }
}

#[test]
fn replaced_same_length_transcript_inode_is_stale() {
    let fixture = Fixture::new(false, false);
    let revision = fixture.ready_revision();
    let bytes = fs::read(&fixture.scope.path).unwrap();
    let replacement = fixture.project.join("replacement");
    fs::write(&replacement, bytes).unwrap();
    fs::rename(&replacement, &fixture.scope.path).unwrap();
    assert!(matches!(
        super::super::delete(&fixture.global, &fixture.run, &fixture.scope, &revision),
        Err(ExactDeleteError::Conflict(_))
    ));
    assert!(fixture.metadata_path().exists());
}

#[test]
fn nofollow_regular_and_single_link_guards_cover_all_selected_files() {
    for target in ["transcript", "metadata", "registry"] {
        let fixture = Fixture::new(false, false);
        let path = match target {
            "transcript" => PathBuf::from(&fixture.scope.path),
            "metadata" => fixture.metadata_path(),
            _ => fixture
                .global
                .join("workspaces")
                .join(format!("{}.toml", fixture.workspace)),
        };
        let outside = fixture.project.join("outside");
        fs::rename(&path, &outside).unwrap();
        symlink(&outside, &path).unwrap();
        assert_eq!(fixture.context().state, "unavailable");
        fs::remove_file(&path).unwrap();
        fs::rename(&outside, &path).unwrap();
        fs::hard_link(&path, &outside).unwrap();
        assert_eq!(fixture.context().state, "unavailable");
        assert!(path.exists());
    }
}

#[test]
fn symlinked_transcript_parent_is_unavailable() {
    let fixture = Fixture::new(false, false);
    let parent = Path::new(&fixture.scope.path).parent().unwrap();
    let moved = fixture.project.join("moved-transcripts");
    fs::rename(parent, &moved).unwrap();
    symlink(&moved, parent).unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    assert!(moved.join(format!("{}.jsonl", fixture.run)).exists());
}

#[test]
fn global_stream_only_directories_are_not_deleted_by_project_selection() {
    for collection in ["runs", "runs-archive"] {
        let fixture = Fixture::new(false, false);
        let directory = fixture.global.join(collection).join(&fixture.run);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("coverage.jsonl"),
            format!(
                "{{\"ledger\":\"begin\",\"v\":1,\"run_id\":\"{}\"}}\n",
                fixture.run
            ),
        )
        .unwrap();
        let context = fixture.context();
        assert_eq!(context.state, "unavailable");
        assert!(context.reason.unwrap().contains("ownership is unproven"));
        assert!(context.effects.is_empty());
        assert!(directory.join("coverage.jsonl").exists());
    }
}

#[test]
fn unsupported_stream_subdirectories_and_entry_overflow_refuse_before_mutation() {
    for many in [false, true] {
        let fixture = Fixture::new(false, false);
        let directory = fixture.global.join("runs").join(&fixture.run);
        fs::create_dir_all(&directory).unwrap();
        if many {
            for n in 0..=MAX_ENTRIES {
                fs::write(directory.join(format!("unknown-{n}")), b"preserve").unwrap();
            }
        } else {
            fs::create_dir(directory.join("nested")).unwrap();
        }
        let context = fixture.context();
        assert_eq!(context.state, "unavailable");
        assert!(Path::new(&fixture.scope.path).exists());
        assert!(fixture.metadata_path().exists());
    }
}

#[test]
fn managed_run_directory_is_preserved_and_part_of_reviewed_disposition() {
    let fixture = Fixture::new(false, false);
    let directory = fixture.global.join("runs").join(&fixture.run);
    fs::create_dir_all(directory.join("other-artifacts")).unwrap();
    fs::write(directory.join("run.json"), b"managed elsewhere").unwrap();
    fs::write(directory.join("other-artifacts/preserve"), b"preserve").unwrap();
    let revision = fixture.ready_revision();
    super::super::delete(&fixture.global, &fixture.run, &fixture.scope, &revision).unwrap();
    assert_eq!(
        fs::read(directory.join("run.json")).unwrap(),
        b"managed elsewhere"
    );
    assert_eq!(
        fs::read(directory.join("other-artifacts/preserve")).unwrap(),
        b"preserve"
    );
}

#[test]
fn bounded_metadata_and_prologue_do_not_enable_delete_on_partial_identity() {
    let fixture = Fixture::new(false, false);
    fs::write(fixture.metadata_path(), vec![b' '; META_BYTES + 1]).unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    fixture.write_metadata(None, Some(i32::MAX as u32));
    fs::write(&fixture.scope.path, vec![b' '; PROLOGUE_BYTES + 1]).unwrap();
    assert_eq!(fixture.context().state, "unavailable");
    fixture.write_transcript(&fixture.run, &fixture.workspace);
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(&fixture.scope.path)
        .unwrap();
    f.write_all(&vec![b' '; PROLOGUE_BYTES * 2]).unwrap();
    drop(f);
    let context = fixture.context();
    assert_eq!(context.state, "ready");
    assert!(context.effects[0].bytes.parse::<u64>().unwrap() > PROLOGUE_BYTES as u64);
    assert!(serde_json::to_vec(&context).unwrap().len() < 65536);
}

#[test]
fn advisory_lease_contention_refuses_context_without_source_changes() {
    let fixture = Fixture::new(false, false);
    let held = rupu_runtime::retention_lease::RetentionLease::try_acquire(&fixture.global).unwrap();
    let context = fixture.context();
    assert_eq!(context.state, "unavailable");
    assert!(Path::new(&fixture.scope.path).exists());
    drop(held);
    assert_eq!(fixture.context().state, "ready");
}

#[test]
fn pinned_preflight_detects_replacement_before_first_unlink() {
    let fixture = Fixture::new(false, false);
    let snapshot = capture_with_lease(&fixture.global, &fixture.run, &fixture.scope).unwrap();
    let replacement = fixture.project.join("replacement-meta");
    fs::write(
        &replacement,
        serde_json::to_vec(&fixture.metadata(Some("ses_other"), Some(i32::MAX as u32))).unwrap(),
    )
    .unwrap();
    fs::rename(replacement, fixture.metadata_path()).unwrap();
    assert!(snapshot.recheck(&fixture.run, &fixture.scope).is_err());
    assert!(Path::new(&fixture.scope.path).exists());
    assert!(fixture.metadata_path().exists());
}

#[test]
fn normalized_path_aliases_do_not_count_as_exact_source_spelling() {
    let fixture = Fixture::new(false, false);
    for replacement in ["/.rupu//transcripts/", "/.rupu/./transcripts/"] {
        let mut scope = fixture.scope.clone();
        scope.path = scope.path.replace("/.rupu/transcripts/", replacement);
        let result = super::super::prepare(&fixture.global, &fixture.run, &scope);
        match result {
            Ok(context) => assert_eq!(context.state, "unavailable"),
            Err(ExactDeleteError::Conflict(_)) => (),
            Err(other) => panic!("unexpected alias result: {other}"),
        }
        assert!(Path::new(&fixture.scope.path).exists());
    }
}

#[test]
fn additive_remote_hosts_and_contradictory_flat_identity_refuse_deletion() {
    let fixture = Fixture::new(false, false);
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture.scope.path).unwrap()).unwrap();
    for (nested, field, value) in [
        (false, "host", json!("remote")),
        (false, "host_id", json!("remote")),
        (true, "host", json!("remote")),
        (true, "host_id", json!("remote")),
        (false, "host", json!(42)),
        (true, "host_id", json!({"name":"local"})),
        (false, "run_id", json!("run_other")),
        (false, "workspace_id", json!("ws_other")),
    ] {
        let mut value_record = original.clone();
        if nested {
            value_record["data"][field] = value;
        } else {
            value_record[field] = value;
        }
        fs::write(&fixture.scope.path, format!("{value_record}\n")).unwrap();
        let context = fixture.context();
        assert_eq!(context.state, "unavailable");
        assert!(context.revision.is_none());
        assert!(context.effects.is_empty());
        assert!(Path::new(&fixture.scope.path).exists());
        assert!(fixture.metadata_path().exists());
    }
    let mut matching = original;
    matching["host"] = json!("local");
    matching["host_id"] = json!("local");
    matching["data"]["host"] = json!("local");
    matching["data"]["host_id"] = json!("local");
    matching["run_id"] = json!(fixture.run);
    matching["workspace_id"] = json!(fixture.workspace);
    fs::write(&fixture.scope.path, format!("{matching}\n")).unwrap();
    assert_eq!(fixture.context().state, "ready");
}
