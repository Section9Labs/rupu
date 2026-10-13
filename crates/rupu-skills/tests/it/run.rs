use crate::support::write_skill;
use rupu_skills::run::{
    resolve_run_skills, FrozenSkill, MissingReason, MissingSkill, SkillReason, SkillRequest,
};
use rupu_skills::Layer;
use rupu_skills::{LayerDirs, SkillCatalog};
use std::fs;

fn catalog(g: &std::path::Path, p: Option<&std::path::Path>) -> SkillCatalog {
    SkillCatalog::discover(&LayerDirs::for_layers(g, p)).unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn nothing_requested_is_empty() {
    let g = tempfile::tempdir().unwrap();
    let r = resolve_run_skills(&catalog(g.path(), None), &[]).unwrap();
    assert!(r.is_empty());
    assert_eq!(r.prompt_section(), None);
}

#[test]
fn preload_is_marked_by_membership_and_reason_by_first_request() {
    let g = tempfile::tempdir().unwrap();
    write_skill(
        &g.path().join("skills"),
        "report-style",
        "report-style",
        "House style.",
    );
    write_skill(
        &g.path().join("skills"),
        "code-review",
        "code-review",
        "Review a diff.",
    );
    let cat = catalog(g.path(), None);
    let agent_grants = s(&["code-review"]);
    let step_preload = s(&["report-style"]);
    let r = resolve_run_skills(
        &cat,
        &[
            SkillRequest {
                reason: SkillReason::Agent,
                grants: &agent_grants,
                preload: &[],
            },
            SkillRequest {
                reason: SkillReason::Launch,
                grants: &[],
                preload: &step_preload,
            },
        ],
    )
    .unwrap();
    let rows: Vec<(&str, bool, bool, SkillReason)> = r
        .entries()
        .iter()
        .map(|g| (g.name(), g.preload, g.indexed, g.reason))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("code-review", false, true, SkillReason::Agent),
            ("report-style", true, false, SkillReason::Launch),
        ]
    );
    let section = r.prompt_section().unwrap();
    assert!(section.starts_with("## Skill: report-style\n\nBase directory: "));
    assert!(section.contains("Do the report-style thing."));
    assert!(section.ends_with("- code-review: Review a diff."));
    assert_eq!(r.read_roots().len(), 2);
}

#[test]
fn a_name_bound_from_both_layers_across_requests_is_refused() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "x", "x", "g");
    write_skill(&p.path().join("skills"), "x", "x", "p");
    let cat = catalog(g.path(), Some(p.path()));
    let a = s(&["global:x"]);
    let b = s(&["project:x"]);
    let err = resolve_run_skills(
        &cat,
        &[
            SkillRequest {
                reason: SkillReason::Agent,
                grants: &a,
                preload: &[],
            },
            SkillRequest {
                reason: SkillReason::Agent,
                grants: &[],
                preload: &b,
            },
        ],
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("granted from both the global and the project layer"),
        "{err}"
    );
}

#[test]
fn disable_model_invocation_is_granted_but_not_indexed() {
    let g = tempfile::tempdir().unwrap();
    let dir = write_skill(&g.path().join("skills"), "deploy", "deploy", "Ship it.");
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: deploy\ndescription: Ship it.\ndisable-model-invocation: true\n---\nSteps.\n",
    )
    .unwrap();
    let grants = s(&["deploy"]);
    let r = resolve_run_skills(
        &catalog(g.path(), None),
        &[SkillRequest {
            reason: SkillReason::Agent,
            grants: &grants,
            preload: &[],
        }],
    )
    .unwrap();
    assert!(!r.entries()[0].indexed);
    assert_eq!(r.prompt_section(), None);
}

fn request<'a>(
    reason: SkillReason,
    grants: &'a [String],
    preload: &'a [String],
) -> SkillRequest<'a> {
    SkillRequest {
        reason,
        grants,
        preload,
    }
}

fn rows(r: &rupu_skills::ResolvedSkills) -> Vec<(&str, bool, bool, SkillReason)> {
    r.entries()
        .iter()
        .map(|g| (g.name(), g.preload, g.indexed, g.reason))
        .collect()
}

#[test]
fn a_preload_from_a_later_request_keeps_the_first_reason() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "x", "x", "X.");
    let cat = catalog(g.path(), None);
    let x = s(&["x"]);
    let r = resolve_run_skills(
        &cat,
        &[
            request(SkillReason::Agent, &x, &[]),
            request(SkillReason::Launch, &[], &x),
        ],
    )
    .unwrap();
    assert_eq!(rows(&r), vec![("x", true, false, SkillReason::Agent)]);

    let r = resolve_run_skills(&cat, &[request(SkillReason::Session, &x, &x)]).unwrap();
    assert_eq!(rows(&r), vec![("x", true, false, SkillReason::Session)]);
}

#[test]
fn star_never_grants_a_project_skill() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "shared", "shared", "g");
    write_skill(&p.path().join("skills"), "local-only", "local-only", "p");
    let star = s(&["*"]);
    let r = resolve_run_skills(
        &catalog(g.path(), Some(p.path())),
        &[request(SkillReason::Agent, &star, &[])],
    )
    .unwrap();
    assert_eq!(r.names(), vec!["shared"]);
}

#[test]
fn read_roots_are_the_canonical_skill_dirs() {
    let g = tempfile::tempdir().unwrap();
    let a = write_skill(&g.path().join("skills"), "a", "a", "A.");
    let b = write_skill(&g.path().join("skills"), "b", "b", "B.");
    let both = s(&["a", "b"]);
    let r = resolve_run_skills(
        &catalog(g.path(), None),
        &[request(SkillReason::Agent, &both, &[])],
    )
    .unwrap();
    assert_eq!(
        r.read_roots(),
        vec![a.canonicalize().unwrap(), b.canonicalize().unwrap()]
    );
}

/// `b` is invalid while the run resolves, then repaired: an entry that was
/// never loaded reads the repaired skill, one that was loaded keeps the
/// error it cached.
fn with_b_broken() -> (tempfile::TempDir, SkillCatalog) {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "a", "a", "A.");
    write_skill(&g.path().join("skills"), "b", "not-b", "B.");
    let cat = catalog(g.path(), None);
    (g, cat)
}

fn repair_b(g: &tempfile::TempDir) {
    write_skill(&g.path().join("skills"), "b", "b", "B.");
}

#[test]
fn a_run_loads_only_the_skills_it_names() {
    let (g, cat) = with_b_broken();
    let grants = s(&["a"]);
    let preload = s(&["global:a"]);
    let r = resolve_run_skills(&cat, &[request(SkillReason::Agent, &grants, &preload)]).unwrap();
    assert_eq!(r.names(), vec!["a"]);
    repair_b(&g);
    let b = cat.get(rupu_skills::Layer::Global, "b").unwrap();
    assert_eq!(b.skill().unwrap().frontmatter.description, "B.");
}

#[test]
fn star_loads_every_entry_of_its_layer() {
    let (g, cat) = with_b_broken();
    let star = s(&["*"]);
    let r = resolve_run_skills(&cat, &[request(SkillReason::Agent, &star, &[])]).unwrap();
    assert_eq!(r.names(), vec!["a"]);
    repair_b(&g);
    assert!(!cat.get(rupu_skills::Layer::Global, "b").unwrap().is_valid());
}

/// Spec §3.2: a `disable-model-invocation` skill is reachable only by
/// preload or an operator's invocation, so the model may load it only then.
#[test]
fn model_loadable_is_indexed_preloaded_or_operator() {
    let g = tempfile::tempdir().unwrap();
    let skills = g.path().join("skills");
    write_skill(&skills, "open", "open", "Indexed.");
    for name in ["hidden", "hidden-pre", "hidden-op"] {
        let dir = write_skill(&skills, name, name, "x");
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: x\ndisable-model-invocation: true\n---\nSteps.\n"
            ),
        )
        .unwrap();
    }
    let agent = s(&["open", "hidden"]);
    let preload = s(&["hidden-pre"]);
    let operator = s(&["hidden-op"]);
    let r = resolve_run_skills(
        &catalog(g.path(), None),
        &[
            request(SkillReason::Agent, &agent, &preload),
            request(SkillReason::Operator, &operator, &[]),
        ],
    )
    .unwrap();
    let rows: Vec<(&str, bool)> = r
        .entries()
        .iter()
        .map(|g| (g.name(), g.model_loadable()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("hidden", false),
            ("hidden-op", true),
            ("hidden-pre", true),
            ("open", true),
        ]
    );
    assert_eq!(r.loadable_names(), vec!["hidden-op", "hidden-pre", "open"]);
}

/// A preloaded skill's body goes into the system prompt at resolve time, so
/// its content hash is taken then too; an indexed skill is hashed when the
/// model loads it.
#[test]
fn a_preloaded_skill_carries_its_content_hash() {
    let g = tempfile::tempdir().unwrap();
    let pre = write_skill(&g.path().join("skills"), "pre", "pre", "P.");
    write_skill(&g.path().join("skills"), "idx", "idx", "I.");
    let (grants, preload) = (s(&["idx"]), s(&["pre"]));
    let r = resolve_run_skills(
        &catalog(g.path(), None),
        &[request(SkillReason::Agent, &grants, &preload)],
    )
    .unwrap();
    let want = rupu_skills::load_skill(&pre, rupu_skills::Layer::Global)
        .unwrap()
        .content_hash()
        .unwrap();
    assert_eq!(r.get("pre").unwrap().content_hash, Some(want));
    assert_eq!(r.get("idx").unwrap().content_hash, None);
}

/// A preloaded skill whose files can't be read fails the run's skills: its
/// body would enter the prompt with no version to record.
#[test]
fn a_preloaded_skill_that_cannot_be_hashed_fails_resolution() {
    let g = tempfile::tempdir().unwrap();
    let pre = write_skill(&g.path().join("skills"), "pre", "pre", "P.");
    let cat = catalog(g.path(), None);
    cat.get(rupu_skills::Layer::Global, "pre")
        .unwrap()
        .skill()
        .unwrap();
    fs::remove_file(pre.join("scripts/run.sh")).unwrap();
    let preload = s(&["pre"]);
    let err = resolve_run_skills(&cat, &[request(SkillReason::Agent, &[], &preload)]).unwrap_err();
    assert!(
        matches!(&err, rupu_skills::RunSkillsError::Hash { name, .. } if name == "pre"),
        "{err:?}"
    );
    assert!(
        err.to_string()
            .starts_with("skill \"pre\" (global) can't be read: "),
        "{err}"
    );
    let kinds: Vec<_> = err.io_errors().map(|e| e.kind()).collect();
    assert_eq!(kinds, vec![std::io::ErrorKind::NotFound]);
}

#[test]
fn freeze_then_resolve_frozen_keeps_flags_and_baselines() {
    let g = tempfile::tempdir().unwrap();
    write_skill(
        &g.path().join("skills"),
        "report-style",
        "report-style",
        "House style.",
    );
    write_skill(
        &g.path().join("skills"),
        "code-review",
        "code-review",
        "Review a diff.",
    );
    let cat = catalog(g.path(), None);
    let grants = s(&["*"]);
    let preload = s(&["report-style"]);
    let r = resolve_run_skills(
        &cat,
        &[SkillRequest {
            reason: SkillReason::Agent,
            grants: &grants,
            preload: &preload,
        }],
    )
    .unwrap();
    let frozen = r.freeze();
    // A skill installed after the freeze is not offered.
    write_skill(&g.path().join("skills"), "late", "late", "Installed later.");
    // The preloaded skill is edited after the freeze.
    fs::write(
        g.path().join("skills/report-style/SKILL.md"),
        "---\nname: report-style\ndescription: House style.\n---\nEdited.\n",
    )
    .unwrap();
    let (again, missing) = rupu_skills::run::resolve_frozen(&catalog(g.path(), None), &frozen);
    assert!(missing.is_empty());
    assert_eq!(again.names(), vec!["code-review", "report-style"]);
    let rs = again.get("report-style").unwrap();
    assert!(rs.preload && !rs.indexed);
    assert_eq!(
        rs.baseline_stamp.hex(),
        frozen
            .iter()
            .find(|f| f.name == "report-style")
            .unwrap()
            .stamp
    );
    assert_ne!(
        rs.baseline_stamp, rs.skill.stamp,
        "the edit moved the live stamp, not the baseline"
    );
    assert_eq!(rs.content_hash, r.get("report-style").unwrap().content_hash);
    let cr = again.get("code-review").unwrap();
    assert_eq!(
        (cr.preload, cr.indexed, cr.reason, cr.content_hash),
        (false, true, SkillReason::Agent, None)
    );
}

#[test]
fn a_frozen_skill_that_was_removed_is_reported_missing() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "gone", "gone", "d");
    let grants = s(&["gone"]);
    let r = resolve_run_skills(
        &catalog(g.path(), None),
        &[SkillRequest {
            reason: SkillReason::Agent,
            grants: &grants,
            preload: &[],
        }],
    )
    .unwrap();
    let frozen = r.freeze();
    fs::remove_dir_all(g.path().join("skills/gone")).unwrap();
    let (again, missing) = rupu_skills::run::resolve_frozen(&catalog(g.path(), None), &frozen);
    assert!(again.is_empty());
    assert_eq!(
        missing,
        vec![MissingSkill {
            name: "gone".into(),
            layer: Layer::Global,
            reason: MissingReason::Removed,
        }]
    );
    assert_eq!(missing[0].reason.to_string(), "removed");
}

#[test]
fn a_frozen_entry_with_a_corrupt_stamp_is_reported_missing() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    let grants = s(&["pdf"]);
    let cat = catalog(g.path(), None);
    let mut frozen = resolve_run_skills(
        &cat,
        &[SkillRequest {
            reason: SkillReason::Agent,
            grants: &grants,
            preload: &[],
        }],
    )
    .unwrap()
    .freeze();
    frozen[0].stamp = "not-hex".into();
    let (again, missing) = rupu_skills::run::resolve_frozen(&cat, &frozen);
    assert!(again.is_empty());
    assert_eq!(
        missing,
        vec![MissingSkill {
            name: "pdf".into(),
            layer: Layer::Global,
            reason: MissingReason::BadStamp,
        }]
    );
    assert_eq!(missing[0].reason.to_string(), "corrupt record: bad stamp");
}

/// A frozen preload of `pdf`, resolved against `cat`.
fn frozen_preload(cat: &SkillCatalog) -> Vec<FrozenSkill> {
    let preload = s(&["pdf"]);
    resolve_run_skills(
        cat,
        &[SkillRequest {
            reason: SkillReason::Agent,
            grants: &[],
            preload: &preload,
        }],
    )
    .unwrap()
    .freeze()
}

#[test]
fn a_frozen_entry_with_a_corrupt_content_hash_is_reported_missing() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    let cat = catalog(g.path(), None);
    let mut frozen = frozen_preload(&cat);
    frozen[0].content_hash = Some("zz".into());
    let (again, missing) = rupu_skills::run::resolve_frozen(&cat, &frozen);
    assert!(again.is_empty());
    assert_eq!(
        missing,
        vec![MissingSkill {
            name: "pdf".into(),
            layer: Layer::Global,
            reason: MissingReason::BadHash,
        }]
    );
    assert_eq!(missing[0].reason.to_string(), "corrupt record: bad hash");
}

#[test]
fn a_frozen_entry_whose_preload_and_hash_disagree_is_reported_missing() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    let cat = catalog(g.path(), None);
    let preloaded = frozen_preload(&cat);
    let mut no_hash = preloaded.clone();
    no_hash[0].content_hash = None;
    let mut not_preloaded = preloaded;
    not_preloaded[0].preload = false;
    for frozen in [no_hash, not_preloaded] {
        let (again, missing) = rupu_skills::run::resolve_frozen(&cat, &frozen);
        assert!(again.is_empty());
        assert_eq!(
            missing,
            vec![MissingSkill {
                name: "pdf".into(),
                layer: Layer::Global,
                reason: MissingReason::PreloadHashMismatch,
            }]
        );
        assert_eq!(
            missing[0].reason.to_string(),
            "corrupt record: preload and content hash disagree"
        );
    }
}

#[test]
fn a_frozen_skill_that_became_invalid_is_reported_missing_with_its_error() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    let cat = catalog(g.path(), None);
    let grants = s(&["pdf"]);
    let frozen = resolve_run_skills(
        &cat,
        &[SkillRequest {
            reason: SkillReason::Agent,
            grants: &grants,
            preload: &[],
        }],
    )
    .unwrap()
    .freeze();
    fs::write(
        g.path().join("skills/pdf/SKILL.md"),
        "---\nname: other\ndescription: d\n---\nx\n",
    )
    .unwrap();
    let (again, missing) = rupu_skills::run::resolve_frozen(&catalog(g.path(), None), &frozen);
    assert!(again.is_empty());
    assert_eq!(missing.len(), 1);
    assert_eq!(
        (missing[0].name.as_str(), missing[0].layer),
        ("pdf", Layer::Global)
    );
    assert_eq!(
        missing[0].reason.to_string(),
        "invalid: name \"other\" does not match its directory \"pdf\""
    );
}

#[test]
fn operator_entries_resolve_any_installed_skill_but_not_wildcards() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    write_skill(&p.path().join("skills"), "notes", "notes", "d");
    write_skill(&g.path().join("skills"), "both", "both", "d");
    write_skill(&p.path().join("skills"), "both", "both", "d");
    let cat = catalog(g.path(), Some(p.path()));
    let f = rupu_skills::run::resolve_operator(&cat, &[], "notes").unwrap();
    assert_eq!(
        (f.layer, f.reason, f.preload, f.indexed),
        (
            rupu_skills::Layer::Project,
            SkillReason::Operator,
            false,
            false
        )
    );
    let notes = cat
        .get(rupu_skills::Layer::Project, "notes")
        .unwrap()
        .skill()
        .unwrap();
    assert_eq!(f.stamp, notes.stamp.hex());
    assert_eq!(f.content_hash, None);
    assert!(rupu_skills::run::resolve_operator(&cat, &[], "global:pdf").is_ok());
    for wild in ["*", "global:*", "project:*"] {
        let err = rupu_skills::run::resolve_operator(&cat, &[], wild)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            format!("skill entry {wild:?} is a wildcard; invoke one skill by name")
        );
    }
    let amb = rupu_skills::run::resolve_operator(&cat, &[], "both")
        .unwrap_err()
        .to_string();
    assert!(
        amb.contains("both the global and the project layer"),
        "{amb}"
    );
    let typo = rupu_skills::run::resolve_operator(&cat, &[], "pdff")
        .unwrap_err()
        .to_string();
    assert!(typo.contains("did you mean"), "{typo}");
}

#[test]
fn with_frozen_appends_operator_entries_and_skips_present_names() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    write_skill(&g.path().join("skills"), "deploy", "deploy", "d");
    let cat = catalog(g.path(), None);
    let grants = s(&["pdf"]);
    let r = resolve_run_skills(
        &cat,
        &[SkillRequest {
            reason: SkillReason::Agent,
            grants: &grants,
            preload: &[],
        }],
    )
    .unwrap();
    let ops = vec![
        rupu_skills::run::resolve_operator(&cat, &[], "pdf").unwrap(),
        rupu_skills::run::resolve_operator(&cat, &[], "deploy").unwrap(),
    ];
    fs::remove_dir_all(g.path().join("skills/deploy")).unwrap();
    let (merged, missing) = r.clone().with_frozen(&catalog(g.path(), None), &ops);
    assert_eq!(
        missing,
        vec![MissingSkill {
            name: "deploy".into(),
            layer: Layer::Global,
            reason: MissingReason::Removed,
        }]
    );
    assert_eq!(merged.names(), vec!["pdf"]);
    assert_eq!(merged.get("pdf").unwrap().reason, SkillReason::Agent);

    write_skill(&g.path().join("skills"), "deploy", "deploy", "d");
    let ops = vec![rupu_skills::run::resolve_operator(&cat, &[], "deploy").unwrap()];
    let (merged, missing) = r.with_frozen(&catalog(g.path(), None), &ops);
    assert!(missing.is_empty());
    assert_eq!(merged.names(), vec!["pdf", "deploy"]);
    let d = merged.get("deploy").unwrap();
    assert_eq!(
        (d.reason, d.preload, d.indexed, d.model_loadable()),
        (SkillReason::Operator, false, false, true)
    );
}

#[test]
fn frozen_skills_round_trip_through_json() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    let grants = s(&["pdf"]);
    let frozen = resolve_run_skills(
        &catalog(g.path(), None),
        &[SkillRequest {
            reason: SkillReason::Launch,
            grants: &grants,
            preload: &[],
        }],
    )
    .unwrap()
    .freeze();
    let json = serde_json::to_string(&frozen).unwrap();
    assert!(
        json.contains("\"reason\":\"launch\"") && json.contains("\"layer\":\"global\""),
        "{json}"
    );
    assert!(!json.contains("content_hash"), "{json}");
    assert_eq!(
        serde_json::from_str::<Vec<rupu_skills::run::FrozenSkill>>(&json).unwrap(),
        frozen
    );
}

/// A session holding `global:x`, over a catalog with `x` in both layers.
fn session_holding_global_x() -> (tempfile::TempDir, tempfile::TempDir, Vec<FrozenSkill>) {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "x", "x", "d");
    write_skill(&p.path().join("skills"), "x", "x", "d");
    let grants = s(&["global:x"]);
    let held = resolve_run_skills(
        &catalog(g.path(), Some(p.path())),
        &[SkillRequest {
            reason: SkillReason::Agent,
            grants: &grants,
            preload: &[],
        }],
    )
    .unwrap()
    .freeze();
    (g, p, held)
}

#[test]
fn an_operator_entry_binding_the_other_layer_of_a_held_name_is_refused() {
    let (g, p, held) = session_holding_global_x();
    let cat = catalog(g.path(), Some(p.path()));
    let err = rupu_skills::run::resolve_operator(&cat, &held, "project:x").unwrap_err();
    assert_eq!(
        err.to_string(),
        "this session already has global:x; invoke global:x"
    );
    // The held skill itself may be invoked.
    let same = rupu_skills::run::resolve_operator(&cat, &held, "global:x").unwrap();
    assert_eq!((same.name.as_str(), same.layer), ("x", Layer::Global));
    // A bare name that now binds only the project's `x` is refused too, and
    // the refusal says which `x` the session holds.
    fs::remove_dir_all(g.path().join("skills/x")).unwrap();
    let cat = catalog(g.path(), Some(p.path()));
    let err = rupu_skills::run::resolve_operator(&cat, &held, "x").unwrap_err();
    assert_eq!(
        err.to_string(),
        "this session already has global:x, which is no longer installed; \
         it can't also take project:x"
    );
}

#[test]
fn with_frozen_reports_a_same_name_entry_from_the_other_layer_as_a_conflict() {
    let (g, p, held) = session_holding_global_x();
    let cat = catalog(g.path(), Some(p.path()));
    let project_x = rupu_skills::run::resolve_operator(&cat, &[], "project:x").unwrap();
    let global_x = rupu_skills::run::resolve_operator(&cat, &[], "global:x").unwrap();
    let (set, missing) = rupu_skills::run::resolve_frozen(&cat, &held);
    assert!(missing.is_empty());
    let (merged, missing) = set.with_frozen(&cat, &[global_x, project_x]);
    assert_eq!(
        missing,
        vec![MissingSkill {
            name: "x".into(),
            layer: Layer::Project,
            reason: MissingReason::Conflict {
                held: Layer::Global
            },
        }]
    );
    assert_eq!(
        missing[0].reason.to_string(),
        "the set already holds the global skill of that name"
    );
    assert_eq!(merged.names(), vec!["x"]);
    let x = merged.get("x").unwrap();
    assert_eq!(
        (x.skill.layer, x.reason),
        (Layer::Global, SkillReason::Agent)
    );
    let project_root = fs::canonicalize(p.path().join("skills/x")).unwrap();
    assert!(!merged.read_roots().contains(&project_root));
}
