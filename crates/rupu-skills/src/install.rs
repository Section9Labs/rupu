//! Find the skills in a source directory, choose some, and copy them into
//! a layer's `skills/` directory (spec §7.2). Every chosen skill is
//! validated, and every destination checked, before anything is written.
//! Each copy is staged under its real name, validated again in place, and
//! swapped in by rename, so a failed install never damages an installed
//! skill.

use crate::frontmatter::parse_skill_md;
use crate::name::SkillName;
use crate::skill::{load_skill, Layer};
use crate::tree::{self, ContentHash, EntryKind, Stamp, TreeEntry};
use std::path::{Path, PathBuf};

pub const MAX_SEARCH_DEPTH: usize = 6;
const SKIP_DIRS: &[&str] = &[".git", "node_modules"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub rel: String,
    pub path: PathBuf,
    pub description: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Default,
    Named(Vec<String>),
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    Created,
    Replaced,
}

#[derive(Debug)]
pub struct Installed {
    pub name: SkillName,
    pub path: PathBuf,
    pub outcome: InstallOutcome,
    pub stamp: Stamp,
    pub content_hash: ContentHash,
    pub rel: String,
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<Installed>,
    pub skipped: Vec<(String, String)>,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no SKILL.md found in {0}")]
    NoSkill(PathBuf),
    #[error("the source holds several skills ({}); pick with --skill <name> or take them all with --all", .0.join(", "))]
    Several(Vec<String>),
    #[error("no skill named {name:?} in the source (found: {})", found.join(", "))]
    NameNotFound { name: String, found: Vec<String> },
    #[error("two skills named {name:?} in the source ({}); narrow the source with #<path>", paths.join(", "))]
    DuplicateName { name: String, paths: Vec<String> },
    #[error("skill {name:?} is invalid: {error}")]
    Invalid { name: String, error: String },
    #[error("skill {skill:?}: symlink {rel} uses an absolute target; make it relative or replace it with the file")]
    AbsoluteSymlink { skill: String, rel: String },
    #[error("{0} already exists; pass --force to replace it")]
    Exists(PathBuf),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Source(#[from] crate::source::SourceError),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> InstallError + '_ {
    move |source| InstallError::Io {
        path: path.to_owned(),
        source,
    }
}

pub fn find_skills(root: &Path) -> Result<Vec<Found>, InstallError> {
    let mut out = Vec::new();
    search(root, "", 0, &mut out)?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut by_name: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for f in &out {
        by_name.entry(&f.name).or_default().push(&f.rel);
    }
    if let Some((name, paths)) = by_name.into_iter().find(|(_, p)| p.len() > 1) {
        return Err(InstallError::DuplicateName {
            name: name.to_owned(),
            paths: paths.into_iter().map(str::to_owned).collect(),
        });
    }
    Ok(out)
}

fn search(dir: &Path, rel: &str, depth: usize, out: &mut Vec<Found>) -> Result<(), InstallError> {
    let md = dir.join("SKILL.md");
    if md.is_file() {
        out.push(describe(dir, rel, &md));
        return Ok(());
    }
    if depth == MAX_SEARCH_DEPTH {
        return Ok(());
    }
    for item in std::fs::read_dir(dir).map_err(io(dir))? {
        let item = item.map_err(io(dir))?;
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if SKIP_DIRS.contains(&name.as_str()) || !item.file_type().map_err(io(dir))?.is_dir() {
            continue;
        }
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        search(&item.path(), &child_rel, depth + 1, out)?;
    }
    Ok(())
}

fn describe(dir: &Path, rel: &str, md: &Path) -> Found {
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let parsed = std::fs::read_to_string(md)
        .map_err(|e| e.to_string())
        .and_then(|t| parse_skill_md(&t).map_err(|e| e.to_string()));
    match parsed {
        Ok((fm, _)) => Found {
            name: fm.name,
            rel: rel.to_owned(),
            path: dir.to_owned(),
            description: Some(fm.description),
            error: None,
        },
        Err(e) => Found {
            name: dir_name,
            rel: rel.to_owned(),
            path: dir.to_owned(),
            description: None,
            error: Some(e),
        },
    }
}

pub fn choose<'a>(found: &'a [Found], sel: &Selection) -> Result<Vec<&'a Found>, InstallError> {
    let names = || found.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    match sel {
        Selection::All => Ok(found.iter().collect()),
        Selection::Default => match found {
            [] => Err(InstallError::Several(Vec::new())),
            [one] => Ok(vec![one]),
            _ => Err(InstallError::Several(names())),
        },
        Selection::Named(wanted) => wanted
            .iter()
            .map(|w| {
                found
                    .iter()
                    .find(|f| &f.name == w)
                    .ok_or_else(|| InstallError::NameNotFound {
                        name: w.clone(),
                        found: names(),
                    })
            })
            .collect(),
    }
}

/// Validate a source skill: frontmatter, name, tree (no escaping symlinks)
/// and no absolute symlinks. Returns its name and the entries to copy.
fn check_source(dir: &Path) -> Result<(SkillName, Vec<TreeEntry>), InstallError> {
    let fallback = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let invalid = |error: String| InstallError::Invalid {
        name: fallback.clone(),
        error,
    };
    let text = std::fs::read_to_string(dir.join("SKILL.md")).map_err(|e| invalid(e.to_string()))?;
    let (fm, _) = parse_skill_md(&text).map_err(|e| invalid(e.to_string()))?;
    let name: SkillName = fm
        .name
        .parse()
        .map_err(|e: crate::NameError| invalid(e.to_string()))?;
    let entries = tree::walk_tree(dir).map_err(|e| InstallError::Invalid {
        name: name.to_string(),
        error: e.to_string(),
    })?;
    for e in &entries {
        if let EntryKind::Symlink { target } = &e.kind {
            if Path::new(target).is_absolute() {
                return Err(InstallError::AbsoluteSymlink {
                    skill: name.to_string(),
                    rel: e.rel.clone(),
                });
            }
        }
    }
    Ok((name, entries))
}

pub fn source_hash(source_dir: &Path) -> Result<(SkillName, ContentHash), InstallError> {
    let (name, entries) = check_source(source_dir)?;
    let root = source_dir.canonicalize().map_err(io(source_dir))?;
    let hash = tree::content_hash(&root, &entries).map_err(|e| InstallError::Invalid {
        name: name.to_string(),
        error: e.to_string(),
    })?;
    Ok((name, hash))
}

pub fn install_selected(
    found: &[Found],
    sel: &Selection,
    skills_dir: &Path,
    layer: Layer,
    force: bool,
) -> Result<InstallReport, InstallError> {
    let mut report = InstallReport::default();
    let mut ready = Vec::new();
    for f in choose(found, sel)? {
        match check_source(&f.path) {
            Ok((name, _)) => ready.push((f, name)),
            Err(e) if *sel == Selection::All => {
                report.skipped.push((f.name.clone(), e.to_string()))
            }
            Err(e) => return Err(e),
        }
    }
    if !force {
        if let Some((_, name)) = ready
            .iter()
            .find(|(_, n)| skills_dir.join(n.as_str()).exists())
        {
            return Err(InstallError::Exists(skills_dir.join(name.as_str())));
        }
    }
    for (f, _) in ready {
        let mut installed = install_one(&f.path, skills_dir, layer, force)?;
        installed.rel = f.rel.clone();
        report.installed.push(installed);
    }
    Ok(report)
}

pub fn install_one(
    source_dir: &Path,
    skills_dir: &Path,
    layer: Layer,
    force: bool,
) -> Result<Installed, InstallError> {
    let (name, entries) = check_source(source_dir)?;
    let root = source_dir.canonicalize().map_err(io(source_dir))?;
    let dest = skills_dir.join(name.as_str());
    let exists = dest.exists();
    if exists && !force {
        return Err(InstallError::Exists(dest));
    }
    std::fs::create_dir_all(skills_dir).map_err(io(skills_dir))?;
    let pid = std::process::id();
    let staging = skills_dir.join(format!(".staging-{name}-{pid}"));
    let _ = std::fs::remove_dir_all(&staging);
    let staged = staging.join(name.as_str());
    let result = copy_tree(&root, &entries, &staged).and_then(|()| {
        load_skill(&staged, layer).map_err(|e| InstallError::Invalid {
            name: name.to_string(),
            error: e.to_string(),
        })
    });
    let staged_skill = match result {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    let content_hash = staged_skill
        .content_hash()
        .map_err(|e| InstallError::Invalid {
            name: name.to_string(),
            error: e.to_string(),
        })?;
    let outcome = if exists {
        let backup = skills_dir.join(format!(".replaced-{name}-{pid}"));
        std::fs::rename(&dest, &backup).map_err(io(&dest))?;
        if let Err(source) = std::fs::rename(&staged, &dest) {
            let _ = std::fs::rename(&backup, &dest);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(InstallError::Io { path: dest, source });
        }
        let _ = std::fs::remove_dir_all(&backup);
        InstallOutcome::Replaced
    } else {
        std::fs::rename(&staged, &dest).map_err(io(&dest))?;
        InstallOutcome::Created
    };
    let _ = std::fs::remove_dir_all(&staging);
    let installed = load_skill(&dest, layer).map_err(|e| InstallError::Invalid {
        name: name.to_string(),
        error: e.to_string(),
    })?;
    Ok(Installed {
        name,
        path: dest,
        outcome,
        stamp: installed.stamp,
        content_hash,
        rel: String::new(),
    })
}

fn copy_tree(root: &Path, entries: &[TreeEntry], to: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(to).map_err(io(to))?;
    for e in entries {
        let dest = to.join(&e.rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        match &e.kind {
            EntryKind::File { .. } => {
                std::fs::copy(root.join(&e.rel), &dest).map_err(io(&dest))?;
            }
            EntryKind::Symlink { target } => symlink(target, &dest)?,
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &str, at: &Path) -> Result<(), InstallError> {
    std::os::unix::fs::symlink(target, at).map_err(io(at))
}

#[cfg(not(unix))]
fn symlink(_target: &str, at: &Path) -> Result<(), InstallError> {
    Err(InstallError::Io {
        path: at.to_owned(),
        source: std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "copying a symlink is supported on Unix only",
        ),
    })
}
