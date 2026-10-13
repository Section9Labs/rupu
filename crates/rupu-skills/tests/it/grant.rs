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
    write_skill(p.path(), "broken-local", "not-broken-local", "p");
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
        matches!(&err[1], ResolveError::Invalid { name, layer: Layer::Global, .. } if name == "bad")
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

fn unknown(entry: &str, suggestion: Option<&str>) -> ResolveError {
    ResolveError::Unknown {
        entry: entry.into(),
        suggestion: suggestion.map(str::to_owned),
    }
}

fn resolve_err(entries: &[&str], cat: &SkillCatalog) -> Vec<ResolveError> {
    let entries: Vec<String> = entries.iter().map(|e| e.to_string()).collect();
    resolve_grants(&parse_grants(&entries).unwrap(), cat).unwrap_err()
}

#[test]
fn project_star_skips_an_invalid_project_skill() {
    let (_g, _p, cat) = catalog();
    assert!(!cat.get(Layer::Project, "broken-local").unwrap().is_valid());
    let got = resolve_grants(&parse_grants(&["project:*".into()]).unwrap(), &cat).unwrap();
    assert_eq!(
        names(&got),
        vec![
            ("code-review".into(), Layer::Project),
            ("local-only".into(), Layer::Project)
        ]
    );
}

#[test]
fn a_qualified_grant_of_an_invalid_skill_is_invalid() {
    let (_g, _p, cat) = catalog();
    let err = resolve_err(&["project:broken-local"], &cat);
    assert_eq!(err.len(), 1);
    assert!(
        matches!(&err[0], ResolveError::Invalid { name, layer: Layer::Project, .. } if name == "broken-local")
    );
}

#[test]
fn one_name_from_both_layers_is_an_error() {
    let (_g, _p, cat) = catalog();
    let both = vec![ResolveError::BothLayers {
        name: "code-review".into(),
    }];
    assert_eq!(resolve_err(&["*", "project:*"], &cat), both);
    assert_eq!(
        resolve_err(&["global:code-review", "project:code-review"], &cat),
        both
    );
    assert_eq!(
        both[0].to_string(),
        "skill \"code-review\" is granted from both the global and the project layer; \
         grant only one of global:code-review or project:code-review"
    );
}

#[test]
fn did_you_mean_follows_the_layer() {
    let (_g, _p, cat) = catalog();
    assert_eq!(
        resolve_err(&["project:pdf-processing"], &cat),
        vec![unknown(
            "project:pdf-processing",
            Some("global:pdf-processing")
        )]
    );
    assert_eq!(
        resolve_err(&["project:pdf-processing"], &cat)[0].to_string(),
        "unknown skill \"project:pdf-processing\" (did you mean global:pdf-processing?)"
    );
    assert_eq!(
        resolve_err(&["project:local-onl"], &cat),
        vec![unknown("project:local-onl", Some("project:local-only"))]
    );
    assert_eq!(
        resolve_err(&["global:local-onl"], &cat),
        vec![unknown("global:local-onl", Some("project:local-only"))]
    );
    assert_eq!(
        resolve_err(&["global:code-reviw"], &cat),
        vec![unknown("global:code-reviw", Some("global:code-review"))]
    );
    assert_eq!(
        resolve_err(&["code-reviw"], &cat),
        vec![unknown("code-reviw", Some("global:code-review"))]
    );
    assert_eq!(
        resolve_err(&["local-onl"], &cat),
        vec![unknown("local-onl", Some("local-only"))]
    );
}

#[test]
fn an_invalid_skill_is_never_suggested() {
    let (_g, _p, cat) = catalog();
    assert_eq!(resolve_err(&["bda"], &cat), vec![unknown("bda", None)]);
    assert_eq!(
        resolve_err(&["broken-locl"], &cat),
        vec![unknown("broken-locl", None)]
    );
}

#[test]
fn a_suggestion_must_share_a_letter_position() {
    let g = tempfile::tempdir().unwrap();
    write_skill(g.path(), "js", "js", "g");
    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(g.path().into()),
        project: None,
    })
    .unwrap();
    assert_eq!(resolve_err(&["go"], &cat), vec![unknown("go", None)]);
    assert_eq!(resolve_err(&["jx"], &cat), vec![unknown("jx", Some("js"))]);
}

#[test]
fn repeated_errors_appear_once_in_first_seen_order() {
    let (_g, _p, cat) = catalog();
    assert_eq!(
        resolve_err(&["nope", "nope"], &cat),
        vec![unknown("nope", None)]
    );
    let err = resolve_err(&["nope", "bad", "nope", "global:bad"], &cat);
    assert_eq!(err.len(), 2, "{err:?}");
    assert_eq!(err[0], unknown("nope", None));
    assert!(
        matches!(&err[1], ResolveError::Invalid { name, layer: Layer::Global, .. } if name == "bad")
    );
}

fn check(entries: &[&str], cat: &SkillCatalog) -> Result<(), String> {
    let entries: Vec<String> = entries.iter().map(|e| e.to_string()).collect();
    rupu_skills::check_grant_names(&entries, cat).map_err(|e| e.to_string())
}

#[test]
fn a_name_check_reads_only_the_listing() {
    let (_g, _p, cat) = catalog();
    assert_eq!(check(&["bad", "project:broken-local", "*"], &cat), Ok(()));
    assert_eq!(
        check(&["pdf-procesing"], &cat),
        Err("unknown skill \"pdf-procesing\" (did you mean pdf-processing?)".into())
    );
    assert_eq!(
        check(&["global:local-only"], &cat),
        Err("unknown skill \"global:local-only\" (did you mean project:local-only?)".into())
    );
    assert_eq!(
        check(&["code-review"], &cat),
        Err(
            "skill \"code-review\" is installed in both the global and the project layer; \
             grant global:code-review or project:code-review"
                .into()
        )
    );
    assert_eq!(
        check(&["Bad Name"], &cat),
        Err("entry \"Bad Name\": skill name may only contain a-z, 0-9 and '-'; found 'B'".into())
    );
}
