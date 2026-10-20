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

/// Whether an install kept a source record for `update` (spec §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracking {
    Recorded,
    /// A project-layer install from a local directory outside the project:
    /// `update` would refuse its record, so none is written.
    Untracked,
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<Installed>,
    pub skipped: Vec<(String, String)>,
    pub untracked: Vec<SkillName>,
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
    /// A batch failed after `report.installed` had landed (and been
    /// recorded, unless untracked); they stay installed.
    #[error("{error}")]
    Partial {
        report: InstallReport,
        error: Box<InstallError>,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> InstallError + '_ {
    move |source| InstallError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Every skill under `root`. The search never follows a directory symlink,
/// and a `SKILL.md` that resolves outside `root` is reported, not read.
pub fn find_skills(root: &Path) -> Result<Vec<Found>, InstallError> {
    let canon = root.canonicalize().map_err(io(root))?;
    let mut out = Vec::new();
    search(&canon, root, "", 0, &mut out)?;
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

fn search(
    root: &Path,
    dir: &Path,
    rel: &str,
    depth: usize,
    out: &mut Vec<Found>,
) -> Result<(), InstallError> {
    let md = dir.join("SKILL.md");
    if md.is_file() {
        out.push(describe(root, dir, rel, &md));
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
        search(root, &item.path(), &child_rel, depth + 1, out)?;
    }
    Ok(())
}

fn describe(root: &Path, dir: &Path, rel: &str, md: &Path) -> Found {
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let parsed = if md.canonicalize().is_ok_and(|p| p.starts_with(root)) {
        std::fs::read_to_string(md)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_skill_md(&t).map_err(|e| e.to_string()))
    } else {
        Err("SKILL.md is a symlink that leaves the source".into())
    };
    let (name, description, error) = match parsed {
        Ok((fm, _)) => match fm.name.parse::<SkillName>() {
            Ok(_) => (fm.name, Some(fm.description), None),
            Err(e) => {
                let error = format!("name {:?}: {e}", fm.name);
                (fm.name, None, Some(error))
            }
        },
        Err(e) => (dir_name, None, Some(e)),
    };
    Found {
        name,
        rel: rel.to_owned(),
        path: dir.to_owned(),
        description,
        error,
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
        Selection::Named(wanted) => {
            let mut chosen: Vec<&Found> = Vec::new();
            for w in wanted {
                if chosen.iter().any(|f| &f.name == w) {
                    continue;
                }
                chosen.push(found.iter().find(|f| &f.name == w).ok_or_else(|| {
                    InstallError::NameNotFound {
                        name: w.clone(),
                        found: names(),
                    }
                })?);
            }
            Ok(chosen)
        }
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

/// Install the chosen skills, calling `on_installed` right after each one
/// lands, so a failure part way through leaves the earlier ones whole and
/// is returned as `Partial`.
pub fn install_selected(
    found: &[Found],
    sel: &Selection,
    skills_dir: &Path,
    layer: Layer,
    force: bool,
    on_installed: &mut dyn FnMut(&Installed) -> Result<Tracking, InstallError>,
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
        for (_, name) in &ready {
            let dest = skills_dir.join(name.as_str());
            if present(&dest)? {
                return Err(InstallError::Exists(dest));
            }
        }
    }
    for (f, _) in ready {
        let landed = install_one(&f.path, skills_dir, layer, force).and_then(|mut installed| {
            installed.rel = f.rel.clone();
            let tracking = on_installed(&installed);
            if let Ok(Tracking::Untracked) = tracking {
                report.untracked.push(installed.name.clone());
            }
            report.installed.push(installed);
            tracking
        });
        if let Err(error) = landed {
            return Err(if report.installed.is_empty() {
                error
            } else {
                InstallError::Partial {
                    report,
                    error: Box::new(error),
                }
            });
        }
    }
    Ok(report)
}

/// Whether anything is at `path`, a dangling symlink included.
fn present(path: &Path) -> Result<bool, InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io(path)(e)),
    }
}

/// A replace renames the installed skill to `.replaced-<name>-<pid>`, then
/// the new copy into place. A crash between the two renames leaves the
/// skill only in the backup: put the newest one back. Once the skill is in
/// place, any backup of it is stale.
pub(crate) fn recover_replaced(skills_dir: &Path, name: &str) -> Result<(), InstallError> {
    let prefix = format!(".replaced-{name}-");
    let Ok(read) = std::fs::read_dir(skills_dir) else {
        return Ok(());
    };
    let mut backups: Vec<(std::time::SystemTime, PathBuf)> = read
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix(&prefix))
                .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| {
            let at = e.metadata().and_then(|m| m.modified());
            (at.unwrap_or(std::time::UNIX_EPOCH), e.path())
        })
        .collect();
    backups.sort();
    let dest = skills_dir.join(name);
    if !present(&dest)? {
        if let Some((_, newest)) = backups.pop() {
            std::fs::rename(&newest, &dest).map_err(io(&newest))?;
        }
    }
    for (_, stale) in backups {
        let _ = std::fs::remove_dir_all(stale);
    }
    Ok(())
}

/// Removes a staging directory on every way out of `install_one`.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
    recover_replaced(skills_dir, name.as_str())?;
    let exists = present(&dest)?;
    if exists && !force {
        return Err(InstallError::Exists(dest));
    }
    std::fs::create_dir_all(skills_dir).map_err(io(skills_dir))?;
    let invalid = |error: String| InstallError::Invalid {
        name: name.to_string(),
        error,
    };
    let pid = std::process::id();
    let staging = Staging(skills_dir.join(format!(".staging-{name}-{pid}")));
    let _ = std::fs::remove_dir_all(&staging.0);
    let staged = staging.0.join(name.as_str());
    copy_tree(&root, &entries, &staged)?;
    let staged_skill = load_skill(&staged, layer).map_err(|e| invalid(e.to_string()))?;
    let content_hash = staged_skill
        .content_hash()
        .map_err(|e| invalid(e.to_string()))?;
    let outcome = if exists {
        let backup = skills_dir.join(format!(".replaced-{name}-{pid}"));
        std::fs::rename(&dest, &backup).map_err(io(&dest))?;
        if let Err(source) = std::fs::rename(&staged, &dest) {
            let _ = std::fs::rename(&backup, &dest);
            return Err(InstallError::Io { path: dest, source });
        }
        let _ = std::fs::remove_dir_all(&backup);
        InstallOutcome::Replaced
    } else {
        std::fs::rename(&staged, &dest).map_err(io(&dest))?;
        InstallOutcome::Created
    };
    drop(staging);
    let installed = load_skill(&dest, layer).map_err(|e| invalid(e.to_string()))?;
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

pub struct InstallSpec<'a> {
    pub source: &'a crate::source::Source,
    pub input: &'a str,
    pub selection: Selection,
    pub skills_dir: &'a Path,
    pub layer: Layer,
    pub force: bool,
}

fn open(
    source: &crate::source::Source,
    fetcher: &dyn crate::source::Fetch,
) -> Result<crate::source::FetchedTree, InstallError> {
    Ok(match source {
        crate::source::Source::Local(p) => crate::source::FetchedTree::local(p.clone()),
        crate::source::Source::Git(g) => fetcher.fetch(g)?,
    })
}

pub fn list_source(
    source: &crate::source::Source,
    fetcher: &dyn crate::source::Fetch,
) -> Result<Vec<Found>, InstallError> {
    let tree = open(source, fetcher)?;
    let found = find_skills(&tree.root)?;
    if found.is_empty() {
        return Err(InstallError::NoSkill(tree.root.clone()));
    }
    Ok(found)
}

/// What installing a source skill into a layer would do: the CP's install
/// dry run. Replacing anything other than `New` needs `force`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewStatus {
    /// Not installed in the layer.
    New,
    /// Installed, and the installed copy is the source's.
    UpToDate,
    /// Installed as recorded; the source has moved on (an update).
    Changed,
    /// Installed, but the copy is not the one its record names (edited,
    /// no record, or it no longer loads): a replace loses that copy.
    LocallyModified,
    /// The source skill can't be installed.
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub found: Found,
    pub status: PreviewStatus,
    /// Why, for `LocallyModified` and `Invalid`.
    pub reason: Option<String>,
}

/// Every skill in `source`, with what installing it into `skills_dir`
/// would do. Reads the source and the layer; writes nothing.
pub fn preview_install(
    source: &crate::source::Source,
    fetcher: &dyn crate::source::Fetch,
    skills_dir: &Path,
    layer: Layer,
) -> Result<Vec<Preview>, InstallError> {
    Ok(list_source(source, fetcher)?
        .into_iter()
        .map(|found| {
            let (status, reason) = preview_one(&found, skills_dir, layer);
            Preview {
                found,
                status,
                reason,
            }
        })
        .collect())
}

fn preview_one(f: &Found, skills_dir: &Path, layer: Layer) -> (PreviewStatus, Option<String>) {
    use PreviewStatus as P;
    if let Some(e) = &f.error {
        return (P::Invalid, Some(format!("{:?}: {e}", f.name)));
    }
    let source = match source_hash(&f.path) {
        Ok((_, hash)) => hash.hex(),
        Err(e) => return (P::Invalid, Some(e.to_string())),
    };
    let dest = skills_dir.join(&f.name);
    match present(&dest) {
        Ok(false) => return (P::New, None),
        Ok(true) => {}
        Err(e) => return (P::LocallyModified, Some(e.to_string())),
    }
    let installed = load_skill(&dest, layer)
        .map_err(|e| e.to_string())
        .and_then(|s| s.content_hash().map_err(|e| e.to_string()));
    let installed = match installed {
        Ok(hash) => hash.hex(),
        Err(e) => {
            return (
                P::LocallyModified,
                Some(format!("the installed copy doesn't load: {e}")),
            )
        }
    };
    if installed == source {
        return (P::UpToDate, None);
    }
    match crate::record::read_record(skills_dir, &f.name) {
        Ok(None) => (
            P::LocallyModified,
            Some("installed without a source record".into()),
        ),
        Err(e) => (P::LocallyModified, Some(e.to_string())),
        Ok(Some(rec)) if rec.content_hash != installed => (
            P::LocallyModified,
            Some("edited since it was installed".into()),
        ),
        Ok(Some(_)) => (P::Changed, None),
    }
}

pub fn install_from(
    spec: &InstallSpec<'_>,
    fetcher: &dyn crate::source::Fetch,
) -> Result<InstallReport, InstallError> {
    use crate::record::{inside_project, now_secs, write_record, SourceRecord, RECORD_VERSION};
    use crate::source::Source;
    let tree = open(spec.source, fetcher)?;
    let found = find_skills(&tree.root)?;
    if found.is_empty() {
        return Err(InstallError::NoSkill(tree.root.clone()));
    }
    let mut record = |inst: &Installed| {
        let (url, git_ref, path, local) = match spec.source {
            Source::Git(g) => {
                let joined = match (&g.path, inst.rel.as_str()) {
                    (Some(p), "") => Some(p.clone()),
                    (Some(p), r) => Some(format!("{p}/{r}")),
                    (None, "") => None,
                    (None, r) => Some(r.to_owned()),
                };
                (Some(g.url.clone()), g.git_ref.clone(), joined, None)
            }
            Source::Local(root) => {
                let dir = if inst.rel.is_empty() {
                    root.clone()
                } else {
                    root.join(&inst.rel)
                };
                if spec.layer == Layer::Project
                    && !inside_project(spec.skills_dir, &dir).map_err(io(&dir))?
                {
                    return Ok(Tracking::Untracked);
                }
                (None, None, None, Some(dir))
            }
        };
        let rec = SourceRecord {
            version: RECORD_VERSION,
            source: spec.input.to_owned(),
            url,
            git_ref,
            path,
            commit: tree.commit.clone(),
            local,
            content_hash: inst.content_hash.hex(),
            installed_at: now_secs(),
        };
        write_record(spec.skills_dir, inst.name.as_str(), &rec).map_err(|source| {
            InstallError::Io {
                path: crate::record::record_path(spec.skills_dir, inst.name.as_str()),
                source,
            }
        })?;
        Ok(Tracking::Recorded)
    };
    install_selected(
        &found,
        &spec.selection,
        spec.skills_dir,
        spec.layer,
        spec.force,
        &mut record,
    )
}
