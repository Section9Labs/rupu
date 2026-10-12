use crate::support::write_skill;
use rupu_skills::{
    parse_grants, resolve_grants, Layer, LayerDirs, ResolveError, SkillCatalog, SkillGrant,
};

fn catalog() -> (tempfile::TempDir, tempfile::TempDir, SkillCatalog) {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(g.path(), "code-review", "code-review", "g");
    write_skill(g.path(), "pdf-processing", "pdf-processing", "g");
    write_skill(g.path(), "bad", "not-bad", "g");
    write_skill(p.path(), "code-review", "code-review", "p");
    write_skill(p.path(), "local-only", "local-only", "p");
    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(g.path().into()),
        project: Some(p.path().into()),
    })
    .unwrap();
    (g, p, cat)
}

fn names(v: &[std::sync::Arc<rupu_skills::Skill>]) -> Vec<(String, Layer)> {
    v.iter().map(|s| (s.name.to_string(), s.layer)).collect()
}

#[test]
fn parse_grammar() {
    assert_eq!("*".parse::<SkillGrant>().unwrap(), SkillGrant::AllGlobal);
    assert_eq!(
        "global:*".parse::<SkillGrant>().unwrap(),
        SkillGrant::AllGlobal
    );
    assert_eq!(
        "project:*".parse::<SkillGrant>().unwrap(),
        SkillGrant::AllProject
    );
    assert_eq!(
        "project:code-review".parse::<SkillGrant>().unwrap(),
        SkillGrant::Qualified(Layer::Project, "code-review".parse().unwrap())
    );
    assert_eq!(
        "pdf-processing".parse::<SkillGrant>().unwrap(),
        SkillGrant::Name("pdf-processing".parse().unwrap())
    );
    assert!("team:x".parse::<SkillGrant>().is_err());
    assert!("Bad".parse::<SkillGrant>().is_err());
}

#[test]
fn star_is_global_only_and_skips_invalid() {
    let (_g, _p, cat) = catalog();
    let got = resolve_grants(&parse_grants(&["*".into()]).unwrap(), &cat).unwrap();
    assert_eq!(
        names(&got),
        vec![
            ("code-review".into(), Layer::Global),
            ("pdf-processing".into(), Layer::Global)
        ]
    );
}

#[test]
fn project_skills_need_an_explicit_grant() {
    let (_g, _p, cat) = catalog();
    let got = resolve_grants(&parse_grants(&["project:*".into()]).unwrap(), &cat).unwrap();
    assert_eq!(
        names(&got),
        vec![
            ("code-review".into(), Layer::Project),
            ("local-only".into(), Layer::Project)
        ]
    );
    let got = resolve_grants(&parse_grants(&["local-only".into()]).unwrap(), &cat).unwrap();
    assert_eq!(names(&got), vec![("local-only".into(), Layer::Project)]);
}

#[test]
fn a_name_in_both_layers_must_be_qualified() {
    let (_g, _p, cat) = catalog();
    let err = resolve_grants(&parse_grants(&["code-review".into()]).unwrap(), &cat).unwrap_err();
    assert_eq!(
        err,
        vec![ResolveError::Ambiguous {
            name: "code-review".into()
        }]
    );
    assert_eq!(
        err[0].to_string(),
        "skill \"code-review\" is installed in both the global and the project layer; grant global:code-review or project:code-review"
    );
    let got = resolve_grants(&parse_grants(&["global:code-review".into()]).unwrap(), &cat).unwrap();
    assert_eq!(names(&got), vec![("code-review".into(), Layer::Global)]);
}

#[test]
fn unknown_and_invalid_names_report_every_error() {
    let (_g, _p, cat) = catalog();
    let err = resolve_grants(
        &parse_grants(&[
            "pdf-procesing".into(),
            "bad".into(),
            "nothing-like-it".into(),
        ])
        .unwrap(),
        &cat,
    )
    .unwrap_err();
    assert_eq!(err.len(), 3);
    assert_eq!(
        err[0].to_string(),
        "unknown skill \"pdf-procesing\" (did you mean pdf-processing?)"
    );
    assert!(
        matches!(&err[1], ResolveError::Invalid { entry, layer: Layer::Global, .. } if entry == "bad")
    );
    assert_eq!(err[2].to_string(), "unknown skill \"nothing-like-it\"");
}

#[test]
fn duplicates_collapse() {
    let (_g, _p, cat) = catalog();
    let got = resolve_grants(
        &parse_grants(&[
            "*".into(),
            "pdf-processing".into(),
            "global:pdf-processing".into(),
        ])
        .unwrap(),
        &cat,
    )
    .unwrap();
    assert_eq!(
        names(&got),
        vec![
            ("code-review".into(), Layer::Global),
            ("pdf-processing".into(), Layer::Global)
        ]
    );
}
