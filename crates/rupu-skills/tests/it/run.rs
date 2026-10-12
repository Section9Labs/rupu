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
                reason: SkillReason::Step,
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
            ("report-style", true, false, SkillReason::Step),
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
            request(SkillReason::Step, &[], &x),
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
