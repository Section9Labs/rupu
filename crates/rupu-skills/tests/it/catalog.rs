use crate::support::write_skill;
use rupu_skills::{load_skill, Layer, LayerDirs, SkillCatalog, SkillError};
use std::fs;

#[test]
fn load_skill_reads_everything() {
    let t = tempfile::tempdir().unwrap();
    let dir = write_skill(t.path(), "pdf-processing", "pdf-processing", "PDF things.");
    let s = load_skill(&dir, Layer::Global).unwrap();
    assert_eq!(s.name.as_str(), "pdf-processing");
    assert_eq!(s.layer, Layer::Global);
    assert_eq!(s.root, dir.canonicalize().unwrap());
    assert_eq!(s.frontmatter.description, "PDF things.");
    assert_eq!(s.body, "Do the pdf-processing thing.");
    assert_eq!(s.files(), vec!["SKILL.md", "scripts/run.sh"]);
    assert_eq!(s.content_hash().unwrap(), s.content_hash().unwrap());
}

#[test]
fn load_skill_errors() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join("empty")).unwrap();
    assert!(matches!(
        load_skill(&t.path().join("empty"), Layer::Global),
        Err(SkillError::MissingSkillMd(_))
    ));

    let d = write_skill(t.path(), "dir-name", "other-name", "d");
    assert!(matches!(
        load_skill(&d, Layer::Global),
        Err(SkillError::NameMismatch { name, dir }) if name == "other-name" && dir == "dir-name"
    ));

    let d = write_skill(t.path(), "Bad_Name", "Bad_Name", "d");
    assert!(matches!(
        load_skill(&d, Layer::Global),
        Err(SkillError::Name { .. })
    ));
}

#[cfg(unix)]
#[test]
fn symlinked_skill_dir_is_global_only() {
    let t = tempfile::tempdir().unwrap();
    let real = write_skill(&t.path().join("elsewhere"), "linked", "linked", "d");
    let skills = t.path().join("skills");
    fs::create_dir_all(&skills).unwrap();
    std::os::unix::fs::symlink(&real, skills.join("linked")).unwrap();
    assert!(load_skill(&skills.join("linked"), Layer::Global).is_ok());
    assert!(matches!(
        load_skill(&skills.join("linked"), Layer::Project),
        Err(SkillError::ProjectSymlink)
    ));
}

#[test]
fn discover_keeps_both_layers_and_invalid_entries() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(g.path(), "code-review", "code-review", "global review");
    write_skill(g.path(), "pdf-processing", "pdf-processing", "pdf");
    write_skill(p.path(), "code-review", "code-review", "project review");
    write_skill(p.path(), "broken", "not-broken", "d");
    fs::create_dir_all(p.path().join(".hidden")).unwrap();
    fs::write(p.path().join("README.md"), "not a skill").unwrap();

    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(g.path().to_owned()),
        project: Some(p.path().to_owned()),
    })
    .unwrap();
    let got: Vec<(String, Layer, bool)> = cat
        .entries()
        .iter()
        .map(|e| (e.dir_name.clone(), e.layer, e.is_valid()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("broken".into(), Layer::Project, false),
            ("code-review".into(), Layer::Global, true),
            ("code-review".into(), Layer::Project, true),
            ("pdf-processing".into(), Layer::Global, true),
        ]
    );
    assert_eq!(cat.named("code-review").count(), 2);
    let proj = cat.get(Layer::Project, "code-review").unwrap();
    assert_eq!(
        proj.skill.as_ref().unwrap().frontmatter.description,
        "project review"
    );
}

#[test]
fn missing_layer_dirs_are_empty() {
    let t = tempfile::tempdir().unwrap();
    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(t.path().join("absent")),
        project: None,
    })
    .unwrap();
    assert!(cat.entries().is_empty());
}
