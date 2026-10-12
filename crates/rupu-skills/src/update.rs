//! `rupu skill update` (spec §7.3): re-fetch installed skills from their
//! recorded source. One fetch per (url, ref); a skill whose installed copy
//! no longer matches its record was edited locally and is left alone
//! unless forced.

use crate::install::{find_skills, install_one, recover_replaced, source_hash};
use crate::name::SkillName;
use crate::record::{
    inside_project, list_records, now_secs, read_record, write_record, SourceRecord,
};
use crate::skill::{load_skill, Layer};
use crate::source::{resolve_inside, Fetch, FetchedTree, GitSource, SourceError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Updated { from: String, to: String },
    UpToDate,
    NoRecord,
    LocallyModified,
    NotInstalled,
    Failed(String),
}

pub fn update_skills(
    skills_dir: &Path,
    layer: Layer,
    names: &[String],
    force: bool,
    fetcher: &dyn Fetch,
) -> Vec<(String, UpdateOutcome)> {
    let names: Vec<String> = if names.is_empty() {
        list_records(skills_dir)
    } else {
        names.to_vec()
    };
    let mut trees: HashMap<(String, Option<String>), Result<FetchedTree, String>> = HashMap::new();
    names
        .into_iter()
        .map(|name| {
            let outcome = match SkillName::from_str(&name) {
                Ok(valid) => update_one(skills_dir, layer, &valid, force, fetcher, &mut trees)
                    .unwrap_or_else(UpdateOutcome::Failed),
                Err(e) => UpdateOutcome::Failed(format!("not a valid skill name: {e}")),
            };
            (name, outcome)
        })
        .collect()
}

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

fn update_one(
    skills_dir: &Path,
    layer: Layer,
    name: &SkillName,
    force: bool,
    fetcher: &dyn Fetch,
    trees: &mut HashMap<(String, Option<String>), Result<FetchedTree, String>>,
) -> Result<UpdateOutcome, String> {
    let name = name.as_str();
    let installed_dir = skills_dir.join(name);
    recover_replaced(skills_dir, name).map_err(|e| e.to_string())?;
    if !installed_dir.join("SKILL.md").is_file() {
        return Ok(UpdateOutcome::NotInstalled);
    }
    let Some(rec) = read_record(skills_dir, name).map_err(|e| e.to_string())? else {
        return Ok(UpdateOutcome::NoRecord);
    };
    let installed = load_skill(&installed_dir, layer).map_err(|e| e.to_string())?;
    let current = installed.content_hash().map_err(|e| e.to_string())?.hex();
    if current != rec.content_hash && !force {
        return Ok(UpdateOutcome::LocallyModified);
    }

    let (source_dir, commit, path) = match (&rec.local, &rec.url) {
        (Some(dir), _) => {
            if layer == Layer::Project
                && !inside_project(skills_dir, dir)
                    .map_err(|e| format!("{}: {e}", dir.display()))?
            {
                return Err(format!(
                    "the source record names {}, which is outside the project; a \
                     project-layer record may only name a directory inside the project",
                    dir.display()
                ));
            }
            (dir.clone(), None, rec.path.clone())
        }
        (None, Some(url)) => {
            let key = (url.clone(), rec.git_ref.clone());
            let tree = trees.entry(key).or_insert_with(|| {
                fetcher
                    .fetch(&GitSource {
                        url: url.clone(),
                        git_ref: rec.git_ref.clone(),
                        path: None,
                    })
                    .map_err(|e| e.to_string())
            });
            let tree = tree.as_ref().map_err(Clone::clone)?;
            let (dir, path) = locate(&tree.root, rec.path.as_deref(), name)?;
            (dir, tree.commit.clone(), path)
        }
        (None, None) => return Err("source record has neither url nor local".into()),
    };

    let (source_name, new_hash) = source_hash(&source_dir).map_err(|e| e.to_string())?;
    if source_name.as_str() != name {
        return Err(format!(
            "the source at {} is now named {source_name}; install it under that name",
            source_dir.display()
        ));
    }
    let new_rec = SourceRecord {
        commit: commit.clone(),
        path,
        content_hash: new_hash.hex(),
        installed_at: now_secs(),
        ..rec.clone()
    };
    if new_hash.hex() == current {
        let moved = (&new_rec.commit, &new_rec.content_hash, &new_rec.path)
            != (&rec.commit, &rec.content_hash, &rec.path);
        if moved {
            write_record(skills_dir, name, &new_rec).map_err(|e| e.to_string())?;
        }
        return Ok(UpdateOutcome::UpToDate);
    }
    install_one(&source_dir, skills_dir, layer, true).map_err(|e| e.to_string())?;
    write_record(skills_dir, name, &new_rec).map_err(|e| e.to_string())?;
    let label = |commit: &Option<String>, hash: &str| short(commit.as_deref().unwrap_or(hash));
    Ok(UpdateOutcome::Updated {
        from: label(&rec.commit, &rec.content_hash),
        to: label(&commit, &new_hash.hex()),
    })
}

/// The skill's directory in a fetched tree and its path there: the recorded
/// path, or a search by name when the repository moved it.
fn locate(
    root: &Path,
    path: Option<&str>,
    name: &str,
) -> Result<(PathBuf, Option<String>), String> {
    if let Some(p) = path {
        match resolve_inside(root, p) {
            Ok(dir) if dir.join("SKILL.md").is_file() => return Ok((dir, Some(p.to_owned()))),
            Ok(_) | Err(SourceError::PathMissing(_)) => {}
            Err(e) => return Err(format!("the source record's path is invalid: {e}")),
        }
    }
    let found = find_skills(root)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|f| f.name == name)
        .ok_or_else(|| format!("{name} is no longer in its source"))?;
    let rel = (!found.rel.is_empty()).then_some(found.rel);
    Ok((found.path, rel))
}
