use super::*;
use std::io::Write;
const VALID: &str = "---\nname: reviewer\nmodel: opus\n---\nReview code.\n";
fn fixture() -> (tempfile::TempDir, AppState, SourceQuery) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    fs::create_dir(global.join("agents")).unwrap();
    fs::write(
        global.join("agents/reviewer.md"),
        b"---\nname: [broken\n---\nprivate prompt\n",
    )
    .unwrap();
    let q = SourceQuery {
        host: Some("local".into()),
        scope_kind: ScopeKind::Global,
        scope_id: None,
        path: global.join("agents/reviewer.md").to_str().unwrap().into(),
    };
    (
        tmp,
        AppState::new(global, rupu_config::PricingConfig::default()),
        q,
    )
}
fn project(s: &AppState, root: &FsPath, id: &str) -> SourceQuery {
    fs::create_dir_all(root.join(".rupu/agents")).unwrap();
    fs::write(
        root.join(".rupu/agents/reviewer.md"),
        "---\nname: reviewer\n---\nproject\n",
    )
    .unwrap();
    fs::create_dir_all(s.global_dir.join("workspaces")).unwrap();
    let ws = rupu_workspace::Workspace {
        id: id.into(),
        path: root.to_str().unwrap().into(),
        repo_remote: None,
        initial_branch: None,
        created_at: "2026-11-09T00:00:00Z".into(),
        last_run_at: None,
    };
    fs::write(
        s.global_dir.join("workspaces").join(format!("{id}.toml")),
        toml::to_string(&ws).unwrap(),
    )
    .unwrap();
    SourceQuery {
        host: Some("local".into()),
        scope_kind: ScopeKind::Project,
        scope_id: Some(id.into()),
        path: root
            .join(".rupu/agents/reviewer.md")
            .to_str()
            .unwrap()
            .into(),
    }
}
fn body(revision: &str, raw: &str) -> RepairBody {
    RepairBody {
        raw: raw.into(),
        expected_revision: revision.into(),
        request_id: "repair_test".into(),
    }
}
fn revision(s: &AppState, q: &SourceQuery) -> String {
    load(s, q.clone()).unwrap()["revision"]
        .as_str()
        .unwrap()
        .into()
}
fn status<T>(r: ApiResult<T>) -> u16 {
    r.err().expect("must refuse").0.as_u16()
}
#[test]
fn invalid_raw_is_read_without_catalog_parse_or_lock_write() {
    let (_tmp, s, q) = fixture();
    let r = load(&s, q.clone()).unwrap();
    assert!(r["parse_error"].is_string());
    assert!(r["parsed_name"].is_null());
    assert_eq!(r["file_stem"], "reviewer");
    assert_eq!(r["path"], q.path);
    assert_eq!(r["raw"], fs::read_to_string(&q.path).unwrap());
    assert_eq!(r["saved"], false);
    assert!(r["request_id"].is_null());
    assert!(!s.global_dir.join("agents/.agent-definitions.lock").exists());
}
#[test]
fn shadowed_global_and_exact_project_read_and_save_remain_separate() {
    let (tmp, s, global) = fixture();
    let project_q = project(&s, &tmp.path().join("repo"), "ws_owned");
    let old_project = fs::read(&project_q.path).unwrap();
    let old = revision(&s, &global);
    let saved = save(&s, global.clone(), body(&old, VALID), Fault::None).unwrap();
    assert_eq!(saved["scope_kind"], "global");
    assert_eq!(saved["path"], global.path);
    assert_eq!(saved["raw"], VALID);
    assert_eq!(saved["previous_revision"], old);
    assert_eq!(saved["request_id"], "repair_test");
    assert_ne!(saved["revision"], old);
    assert_eq!(fs::read(&project_q.path).unwrap(), old_project);
    let source = load(&s, project_q.clone()).unwrap();
    assert_eq!(source["scope_id"], "ws_owned");
    assert_eq!(source["raw"], fs::read_to_string(project_q.path).unwrap());
}
#[test]
fn name_is_never_used_as_file_identity() {
    let (_tmp, s, q) = fixture();
    fs::write(&q.path, "---\nname: other\n---\nhi\n").unwrap();
    let read = load(&s, q.clone()).unwrap();
    assert_eq!(read["parsed_name"], "other");
    assert_eq!(read["file_stem"], "reviewer");
    assert!(read["authoring_error"].is_string());
    assert_eq!(
        status(save(
            &s,
            q.clone(),
            body(
                read["revision"].as_str().unwrap(),
                "---\nname: other\n---\nhi\n"
            ),
            Fault::None
        )),
        400
    );
    assert!(!s.global_dir.join("agents/other.md").exists());
}
#[test]
fn stale_bytes_and_equal_byte_replaced_file_refuse() {
    for replace in [false, true] {
        let (_tmp, s, q) = fixture();
        let old = revision(&s, &q);
        if replace {
            let p = s.global_dir.join("agents/replacement");
            fs::write(&p, fs::read(&q.path).unwrap()).unwrap();
            fs::rename(p, &q.path).unwrap();
        } else {
            fs::write(&q.path, b"another editor").unwrap();
        }
        let before = fs::read(&q.path).unwrap();
        assert_eq!(status(save(&s, q, body(&old, VALID), Fault::None)), 409);
        assert_eq!(
            fs::read(s.global_dir.join("agents/reviewer.md")).unwrap(),
            before
        );
    }
}
#[test]
fn exact_project_registration_changes_invalidate_revision() {
    let (tmp, s, _) = fixture();
    let q = project(&s, &tmp.path().join("repo"), "ws_owned");
    let old = revision(&s, &q);
    let registry = s.global_dir.join("workspaces/ws_owned.toml");
    fs::OpenOptions::new()
        .append(true)
        .open(registry)
        .unwrap()
        .write_all(b"\n# reassessed registry\n")
        .unwrap();
    assert_eq!(status(save(&s, q, body(&old, VALID), Fault::None)), 409);
}
#[test]
fn foreign_project_id_and_path_never_fall_back_to_global() {
    let (tmp, s, global) = fixture();
    let mut q = project(&s, &tmp.path().join("repo"), "ws_owned");
    q.path = global.path;
    assert_eq!(status(load(&s, q.clone())), 400);
    q.scope_id = Some("ws_missing".into());
    assert_eq!(status(load(&s, q)), 404);
    let mut q = SourceQuery {
        host: None,
        scope_kind: ScopeKind::Global,
        scope_id: Some("ws_owned".into()),
        path: s
            .global_dir
            .join("agents/reviewer.md")
            .display()
            .to_string(),
    };
    assert_eq!(status(load(&s, q.clone())), 400);
    q.scope_kind = ScopeKind::Project;
    q.scope_id = Some("../outside".into());
    assert_eq!(status(load(&s, q)), 400);
}
#[test]
fn malformed_or_foreign_registration_is_not_a_project_authority() {
    for value in ["bad TOML", "id='ws_other'\npath='/tmp'\ncreated_at='now'\n"] {
        let (tmp, s, _) = fixture();
        let q = project(&s, &tmp.path().join("repo"), "ws_owned");
        fs::write(s.global_dir.join("workspaces/ws_owned.toml"), value).unwrap();
        assert_eq!(status(load(&s, q)), 409);
    }
}
#[test]
fn target_and_parent_symlinks_and_hardlinks_are_refused() {
    for kind in ["target", "parent", "hardlink"] {
        let (_tmp, s, q) = fixture();
        let agents = s.global_dir.join("agents");
        let outside = s.global_dir.join("other.md");
        fs::write(&outside, VALID).unwrap();
        match kind {
            "target" => {
                fs::remove_file(&q.path).unwrap();
                std::os::unix::fs::symlink(&outside, &q.path).unwrap();
            }
            "parent" => {
                fs::rename(&agents, s.global_dir.join("moved")).unwrap();
                std::os::unix::fs::symlink(s.global_dir.join("moved"), &agents).unwrap();
            }
            _ => fs::hard_link(&q.path, s.global_dir.join("linked.md")).unwrap(),
        }
        assert_eq!(status(load(&s, q)), 409, "{kind}");
        assert_eq!(fs::read_to_string(outside).unwrap(), VALID);
    }
}
#[test]
fn fifo_target_returns_promptly_without_waiting() {
    let (_tmp, s, q) = fixture();
    fs::remove_file(&q.path).unwrap();
    if !crate::api::fs_open::test_support::mkfifo(FsPath::new(&q.path)) {
        return;
    }
    crate::api::fs_open::test_support::within_deadline(move || {
        assert_eq!(status(load(&s, q)), 409)
    });
}
#[test]
fn exact_source_and_registry_byte_limits_refuse() {
    let (tmp, s, q) = fixture();
    fs::File::create(&q.path)
        .unwrap()
        .set_len(MAX_SOURCE as u64 + 1)
        .unwrap();
    assert_eq!(status(load(&s, q)), 413);
    let q = project(&s, &tmp.path().join("repo"), "ws_owned");
    fs::File::create(s.global_dir.join("workspaces/ws_owned.toml"))
        .unwrap()
        .set_len(MAX_REGISTRY as u64 + 1)
        .unwrap();
    assert_eq!(status(load(&s, q)), 413);
}
#[test]
fn directory_identity_changes_invalidate_the_same_file_bytes() {
    let (_tmp, s, q) = fixture();
    let old = revision(&s, &q);
    let bytes = fs::read(&q.path).unwrap();
    fs::rename(s.global_dir.join("agents"), s.global_dir.join("old_agents")).unwrap();
    fs::create_dir(s.global_dir.join("agents")).unwrap();
    fs::write(&q.path, bytes).unwrap();
    assert_eq!(status(save(&s, q, body(&old, VALID), Fault::None)), 409);
}
#[test]
fn authoring_rules_stay_in_force_and_missing_skills_keep_existing_behavior() {
    let (_tmp, s, q) = fixture();
    let old = revision(&s, &q);
    for raw in [
        "not frontmatter",
        "---\nname: reviewer\nskills: [team:pdf]\n---\nhi\n",
        "---\nname: different\n---\nhi\n",
    ] {
        assert_eq!(
            status(save(&s, q.clone(), body(&old, raw), Fault::None)),
            400
        );
    }
    let accepted = "---\nname: reviewer\nskills: [not-installed-yet]\n---\nhi\n";
    assert!(save(&s, q, body(&old, accepted), Fault::None).is_ok());
}
#[test]
fn unsupported_file_stem_can_be_read_but_not_renamed_through_repair() {
    let (_tmp, s, mut q) = fixture();
    let new = s.global_dir.join("agents/reviewer.v1.md");
    fs::rename(&q.path, &new).unwrap();
    q.path = new.display().to_string();
    let value = load(&s, q.clone()).unwrap();
    assert_eq!(value["save_supported"], false);
    assert_eq!(
        status(save(
            &s,
            q,
            body(value["revision"].as_str().unwrap(), VALID),
            Fault::None
        )),
        400
    );
}
#[test]
fn pre_replace_failure_keeps_source_and_post_replace_failure_is_unknown() {
    for fault in [Fault::BeforeRename, Fault::AfterRename] {
        let (_tmp, s, q) = fixture();
        let old = revision(&s, &q);
        let original = fs::read(&q.path).unwrap();
        let err = save(&s, q.clone(), body(&old, VALID), fault).unwrap_err();
        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        if fault == Fault::BeforeRename {
            assert_eq!(fs::read(&q.path).unwrap(), original);
        } else {
            assert!(err.1.contains("agent_source_save_unknown"));
            assert_eq!(load(&s, q.clone()).unwrap()["raw"], VALID);
            assert_eq!(status(save(&s, q, body(&old, VALID), Fault::None)), 409);
        }
    }
}
#[test]
fn source_refuses_missing_files_instead_of_creating() {
    let (_tmp, s, q) = fixture();
    let old = revision(&s, &q);
    fs::remove_file(&q.path).unwrap();
    assert_eq!(
        status(save(&s, q.clone(), body(&old, VALID), Fault::None)),
        404
    );
    assert!(!FsPath::new(&q.path).exists());
}
#[tokio::test]
async fn legacy_http_writers_share_the_nonreentrant_directory_lock() {
    let (_tmp, s, q) = fixture();
    let old = revision(&s, &q);
    let guard = legacy_lock(&s.global_dir.join("agents")).unwrap();
    assert_eq!(
        status(save(&s, q.clone(), body(&old, VALID), Fault::None)),
        429
    );
    let scope = || ScopeQuery {
        scope_kind: Some(ScopeKind::Global),
        scope_id: None,
    };
    assert_eq!(
        status(
            super::super::write_agent(
                State(s.clone()),
                Path("reviewer".into()),
                Query(scope()),
                Json(AgentWriteBody { raw: VALID.into() })
            )
            .await
        ),
        429
    );
    assert_eq!(
        status(
            super::super::delete_agent(State(s.clone()), Path("reviewer".into()), Query(scope()))
                .await
        ),
        429
    );
    assert_eq!(
        status(
            super::super::create_agent(
                State(s.clone()),
                Json(AgentWriteBody {
                    raw: "---\nname: new-agent\n---\nhi\n".into()
                })
            )
            .await
        ),
        429
    );
    drop(guard);
    assert!(save(&s, q, body(&old, VALID), Fault::None).is_ok());
}
#[tokio::test]
async fn readonly_get_works_while_repair_and_remote_reads_refuse() {
    let (_tmp, s, q) = fixture();
    let r = read_source(State(s.clone()), Query(q.clone()))
        .await
        .unwrap()
        .0;
    assert!(r["parse_error"].is_string());
    assert_eq!(
        status(
            repair_source(
                State(s.clone()),
                Query(q.clone()),
                Json(body(r["revision"].as_str().unwrap(), VALID))
            )
            .await
        ),
        501
    );
    let mut remote = q;
    remote.host = Some("peer".into());
    assert_eq!(status(read_source(State(s), Query(remote)).await), 501);
}
#[test]
fn revision_binding_rejects_another_file_even_with_identical_bytes() {
    let (_tmp, s, q) = fixture();
    let old = revision(&s, &q);
    let second = s.global_dir.join("agents/second.md");
    fs::write(&second, fs::read(&q.path).unwrap()).unwrap();
    let mut other = q;
    other.path = second.display().to_string();
    assert_eq!(
        status(save(
            &s,
            other,
            body(&old, "---\nname: second\n---\nhi\n"),
            Fault::None
        )),
        409
    );
}
