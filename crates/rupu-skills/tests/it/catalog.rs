use crate::support::write_skill;
use rupu_skills::{load_skill, Layer, LayerDirs, OverlaySkill, SkillCatalog, SkillError};
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
        overlay: Vec::new(),
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
        overlay: Vec::new(),
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
        overlay: Vec::new(),
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
        overlay: Vec::new(),
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
        overlay: Vec::new(),
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
        overlay: Vec::new(),
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

/// A SKILL.md over 1 MiB fails closed with its size: the whole body would
/// go into a prompt. Large assets beside it are fine.
#[test]
fn an_oversized_skill_md_is_invalid() {
    let t = tempfile::tempdir().unwrap();
    let d = write_skill(t.path(), "huge", "huge", "d");
    let text = format!(
        "---\nname: huge\ndescription: d\n---\n{}\n",
        "x".repeat(1 << 20)
    );
    fs::write(d.join("SKILL.md"), &text).unwrap();
    fs::write(d.join("scripts/blob.bin"), vec![0u8; 2 << 20]).unwrap();
    let err = load_skill(&d, Layer::Global).unwrap_err();
    assert!(
        matches!(err, SkillError::TooLarge { bytes, limit: 1_048_576 } if bytes == text.len() as u64),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        format!(
            "SKILL.md is {} bytes; the limit is 1048576 bytes",
            text.len()
        )
    );

    let ok = format!(
        "---\nname: huge\ndescription: d\n---\n{}\n",
        "x".repeat(1000)
    );
    fs::write(d.join("SKILL.md"), ok).unwrap();
    assert!(load_skill(&d, Layer::Global).is_ok());
}

/// One layer that can't be read leaves the other's skills, and says why.
#[test]
fn discover_readable_keeps_the_layers_it_can_read() {
    let t = tempfile::tempdir().unwrap();
    let global = t.path().join("global");
    write_skill(&global, "pdf", "pdf", "d");
    let project = t.path().join("project-skills");
    fs::write(&project, "not a directory").unwrap();
    let dirs = LayerDirs {
        global: Some(global),
        project: Some(project.clone()),
        overlay: Vec::new(),
    };

    let (cat, failed) = SkillCatalog::discover_readable(&dirs);
    let names: Vec<(&str, Layer)> = cat
        .entries()
        .iter()
        .map(|e| (e.dir_name.as_str(), e.layer))
        .collect();
    assert_eq!(names, [("pdf", Layer::Global)]);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, Layer::Project);
    assert!(
        failed[0]
            .1
            .to_string()
            .starts_with(&format!("{}: ", project.display())),
        "{}",
        failed[0].1
    );
    assert!(SkillCatalog::discover(&dirs).is_err());
}

#[test]
fn an_entry_reads_its_install_record() {
    let t = tempfile::tempdir().unwrap();
    let skills = t.path().join("skills");
    write_skill(&skills, "pdf", "pdf", "d");
    let dirs = LayerDirs {
        global: Some(skills.clone()),
        project: None,
        overlay: Vec::new(),
    };
    let cat = SkillCatalog::discover(&dirs).unwrap();
    let pdf = cat.get(Layer::Global, "pdf").unwrap();
    assert!(pdf.record().unwrap().is_none());

    fs::create_dir_all(skills.join(".sources")).unwrap();
    fs::write(
        skills.join(".sources/pdf.json"),
        r#"{"version":1,"source":"github:acme/skills#pdf","content_hash":"ab","installed_at":1}"#,
    )
    .unwrap();
    assert_eq!(
        pdf.record().unwrap().unwrap().source,
        "github:acme/skills#pdf"
    );

    let padding = " ".repeat(rupu_skills::record::MAX_RECORD_BYTES as usize);
    fs::write(
        skills.join(".sources/pdf.json"),
        format!(r#"{{"version":1,"source":"s","content_hash":"ab","installed_at":1}}{padding}"#),
    )
    .unwrap();
    assert_eq!(
        pdf.record().unwrap_err().to_string(),
        format!(
            "{}: the record is over {} bytes",
            skills.join(".sources/pdf.json").display(),
            rupu_skills::record::MAX_RECORD_BYTES
        )
    );
}

/// A project `skills/` that is a symlink points outside the repository:
/// its records are not read, whatever is at the other end.
#[cfg(unix)]
#[test]
fn a_symlinked_project_root_reads_no_record() {
    let t = tempfile::tempdir().unwrap();
    let real = t.path().join("real-skills");
    write_skill(&real, "pdf", "pdf", "d");
    fs::create_dir_all(real.join(".sources")).unwrap();
    fs::write(
        real.join(".sources/pdf.json"),
        r#"{"version":1,"source":"s","content_hash":"ab","installed_at":1}"#,
    )
    .unwrap();
    let link = t.path().join("skills");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let cat = SkillCatalog::discover(&LayerDirs {
        global: None,
        project: Some(link),
        overlay: Vec::new(),
    })
    .unwrap();
    assert_eq!(
        cat.get(Layer::Project, "pdf")
            .unwrap()
            .record()
            .unwrap_err()
            .to_string(),
        "the project skills directory may not be a symlink"
    );
}

fn overlay_of(tmp: &Path, name: &str, desc: &str, layer: Layer) -> OverlaySkill {
    let dir = write_skill(&tmp.join(format!("ov-{layer:?}")), name, name, desc);
    OverlaySkill {
        layer,
        name: name.into(),
        dir,
    }
}

#[test]
fn a_shipped_global_skill_replaces_the_installed_one() {
    let home = tempfile::tempdir().unwrap();
    write_skill(&home.path().join("skills"), "pdf", "pdf", "Installed.");
    let dirs = LayerDirs::for_layers(home.path(), None).with_overlay(vec![overlay_of(
        home.path(),
        "pdf",
        "Shipped.",
        Layer::Global,
    )]);
    let cat = SkillCatalog::discover(&dirs).unwrap();
    let s = cat.get(Layer::Global, "pdf").unwrap().skill().unwrap();
    assert_eq!(s.frontmatter.description, "Shipped.");
    assert!(s.shipped);
    assert_eq!(cat.named("pdf").count(), 1);
}

#[test]
fn star_covers_overlay_global_but_not_overlay_project() {
    let home = tempfile::tempdir().unwrap();
    let dirs = LayerDirs::for_layers(home.path(), None).with_overlay(vec![
        overlay_of(home.path(), "a", "A.", Layer::Global),
        overlay_of(home.path(), "b", "B.", Layer::Project),
    ]);
    let cat = SkillCatalog::discover(&dirs).unwrap();
    let grants = rupu_skills::parse_grants(&["*".into()]).unwrap();
    let got = rupu_skills::resolve_grants(&grants, &cat).unwrap();
    let names: Vec<_> = got.iter().map(|s| s.name.as_str().to_string()).collect();
    assert_eq!(names, ["a"]);
}

#[test]
fn a_bare_name_bound_by_an_overlay_project_skill_and_an_installed_global_is_ambiguous() {
    let home = tempfile::tempdir().unwrap();
    write_skill(&home.path().join("skills"), "pdf", "pdf", "Installed.");
    let dirs = LayerDirs::for_layers(home.path(), None).with_overlay(vec![overlay_of(
        home.path(),
        "pdf",
        "P.",
        Layer::Project,
    )]);
    let cat = SkillCatalog::discover(&dirs).unwrap();
    let grants = rupu_skills::parse_grants(&["pdf".into()]).unwrap();
    assert!(rupu_skills::resolve_grants(&grants, &cat).is_err());
}

#[test]
fn installed_skills_load_unshipped() {
    let home = tempfile::tempdir().unwrap();
    write_skill(&home.path().join("skills"), "pdf", "pdf", "Installed.");
    let cat = SkillCatalog::discover(&LayerDirs::for_layers(home.path(), None)).unwrap();
    assert!(
        !cat.get(Layer::Global, "pdf")
            .unwrap()
            .skill()
            .unwrap()
            .shipped
    );
}

#[test]
fn an_overlay_dir_named_for_another_skill_is_an_invalid_entry() {
    let home = tempfile::tempdir().unwrap();
    // Directory `pdf`, frontmatter name `evil`: content shipped under a name
    // it does not carry.
    let dir = write_skill(&home.path().join("ov"), "pdf", "evil", "Spoof.");
    let dirs = LayerDirs::for_layers(home.path(), None).with_overlay(vec![OverlaySkill {
        layer: Layer::Global,
        name: "pdf".into(),
        dir,
    }]);
    let cat = SkillCatalog::discover(&dirs).unwrap();
    let e = cat.get(Layer::Global, "pdf").unwrap();
    assert!(matches!(e.skill(), Err(SkillError::NameMismatch { .. })));
    let grants = rupu_skills::parse_grants(&["pdf".into()]).unwrap();
    assert!(rupu_skills::resolve_grants(&grants, &cat).is_err());
}
