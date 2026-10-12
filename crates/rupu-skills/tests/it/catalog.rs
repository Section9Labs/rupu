use crate::support::write_skill;
use rupu_skills::{load_skill, Layer, LayerDirs, SkillCatalog, SkillError};
use std::fs;
use std::path::{Path, PathBuf};

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
    let before = s.content_hash().unwrap();
    fs::write(dir.join("scripts/run.sh"), "#!/bin/sh\necho OK\n").unwrap();
    assert_ne!(s.content_hash().unwrap(), before);
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
        proj.skill().unwrap().frontmatter.description,
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

#[cfg(unix)]
#[test]
fn a_symlinked_project_skills_dir_invalidates_every_project_skill() {
    let t = tempfile::tempdir().unwrap();
    let real = t.path().join("real-skills");
    write_skill(&real, "linked", "linked", "d");
    let link = t.path().join("skills");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let cat = SkillCatalog::discover(&LayerDirs {
        global: None,
        project: Some(link.clone()),
    })
    .unwrap();
    assert_eq!(cat.entries().len(), 1);
    assert!(matches!(
        cat.get(Layer::Project, "linked").unwrap().skill(),
        Err(SkillError::ProjectRootSymlink)
    ));

    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(link),
        project: None,
    })
    .unwrap();
    assert!(cat.get(Layer::Global, "linked").unwrap().is_valid());
}

#[cfg(unix)]
#[test]
fn a_dangling_skill_symlink_is_listed_invalid_and_a_file_symlink_is_skipped() {
    let t = tempfile::tempdir().unwrap();
    let skills = t.path().join("skills");
    write_skill(&skills, "real", "real", "d");
    std::os::unix::fs::symlink(t.path().join("nowhere"), skills.join("gone")).unwrap();
    fs::write(t.path().join("notes.md"), "x").unwrap();
    std::os::unix::fs::symlink(t.path().join("notes.md"), skills.join("notes")).unwrap();

    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(skills),
        project: None,
    })
    .unwrap();
    let got: Vec<(String, bool)> = cat
        .entries()
        .iter()
        .map(|e| (e.dir_name.clone(), e.is_valid()))
        .collect();
    assert_eq!(got, vec![("gone".into(), false), ("real".into(), true)]);
    assert!(matches!(
        cat.get(Layer::Global, "gone").unwrap().skill(),
        Err(SkillError::Io { .. })
    ));
}

#[test]
fn layer_dirs_join_skills_under_each_rupu_dir() {
    let dirs = LayerDirs::for_layers(Path::new("/g/.rupu"), Some(Path::new("/p/.rupu")));
    assert_eq!(dirs.global, Some(PathBuf::from("/g/.rupu/skills")));
    assert_eq!(dirs.project, Some(PathBuf::from("/p/.rupu/skills")));
    assert_eq!(
        dirs.skills_dir(Layer::Global),
        Some(Path::new("/g/.rupu/skills"))
    );
    assert_eq!(
        dirs.skills_dir(Layer::Project),
        Some(Path::new("/p/.rupu/skills"))
    );

    let global_only = LayerDirs::for_layers(Path::new("/g/.rupu"), None);
    assert_eq!(global_only.project, None);
    assert_eq!(global_only.skills_dir(Layer::Project), None);
}

#[test]
fn a_project_layer_that_is_the_global_dir_is_dropped() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "only", "only", "d");
    let dirs = rupu_skills::LayerDirs::for_layers(g.path(), Some(g.path()));
    assert_eq!(dirs.project, None);
    let cat = rupu_skills::SkillCatalog::discover(&dirs).unwrap();
    assert_eq!(cat.named("only").count(), 1);

    let canonical = g.path().canonicalize().unwrap();
    let dirs = LayerDirs::for_layers(g.path(), Some(&canonical));
    assert_eq!(dirs.project, None);
}

#[cfg(unix)]
#[test]
fn a_project_layer_that_is_a_symlinked_global_dir_is_dropped() {
    let t = tempfile::tempdir().unwrap();
    let real = t.path().join("real-rupu");
    write_skill(&real.join("skills"), "only", "only", "d");
    let link = t.path().join("linked-rupu");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    for (global, project) in [(&link, &real), (&real, &link)] {
        let dirs = LayerDirs::for_layers(global, Some(project));
        assert_eq!(dirs.project, None, "{global:?} / {project:?}");
        let cat = SkillCatalog::discover(&dirs).unwrap();
        assert_eq!(cat.named("only").count(), 1);
    }
}

#[test]
fn discover_lists_entries_and_each_loads_on_first_use() {
    let t = tempfile::tempdir().unwrap();
    let skills = t.path().join("skills");
    fs::create_dir_all(skills.join("late")).unwrap();

    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(skills.clone()),
        project: None,
    })
    .unwrap();
    let entry = cat.get(Layer::Global, "late").unwrap();
    assert_eq!(entry.path, skills.join("late"));

    write_skill(&skills, "late", "late", "Written after discovery.");
    let first = entry.skill().unwrap().clone();
    assert_eq!(first.frontmatter.description, "Written after discovery.");

    fs::write(skills.join("late/SKILL.md"), "not a skill").unwrap();
    assert!(std::sync::Arc::ptr_eq(entry.skill().unwrap(), &first));
    assert!(entry.is_valid());
}

#[cfg(unix)]
#[test]
fn skill_dir_is_the_path_as_found() {
    let t = tempfile::tempdir().unwrap();
    let real = write_skill(&t.path().join("elsewhere"), "linked", "linked", "d");
    let skills = t.path().join("skills");
    fs::create_dir_all(&skills).unwrap();
    std::os::unix::fs::symlink(&real, skills.join("linked")).unwrap();
    let s = rupu_skills::load_skill(&skills.join("linked"), rupu_skills::Layer::Global).unwrap();
    assert_eq!(s.dir, skills.join("linked"));
    assert_eq!(s.root, real.canonicalize().unwrap());
}

#[cfg(unix)]
#[test]
fn a_fifo_skill_md_is_refused_without_opening_it() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("piped");
    fs::create_dir_all(&dir).unwrap();
    let status = std::process::Command::new("mkfifo")
        .arg(dir.join("SKILL.md"))
        .status()
        .unwrap();
    assert!(status.success());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(load_skill(&dir, Layer::Global).map(|_| ())));
    let got = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("load_skill opened the FIFO and blocked");
    assert!(
        matches!(got, Err(SkillError::Tree(rupu_skills::TreeError::NotRegular { ref rel })) if rel == "SKILL.md"),
        "{got:?}"
    );
}
