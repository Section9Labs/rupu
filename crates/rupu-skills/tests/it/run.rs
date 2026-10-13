use crate::support::write_skill;
use rupu_skills::run::{resolve_run_skills, SkillReason, SkillRequest};
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
