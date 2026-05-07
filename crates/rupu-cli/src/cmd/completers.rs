//! Dynamic value completers wired onto positional arguments via
//! `#[arg(add = ArgValueCompleter::new(...))]`. Reached at completion
//! time through `clap_complete::CompleteEnv` (see `lib.rs::run`).
//!
//! These helpers walk the relevant directories cheaply (no frontmatter
//! parse, just basename collection) so tab-completion stays snappy
//! even on large agent / workflow sets.

use clap_complete::CompletionCandidate;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Agent name completer for `rupu agent show / edit` and the like.
/// Lists `.md` basenames from `<global>/agents/` and `.rupu/agents/`
/// (project shadow), de-duplicated.
pub fn agent_names(current: &OsStr) -> Vec<CompletionCandidate> {
    list_basenames("agents", "md", current)
}

/// Workflow name completer for `rupu workflow show / run / edit`
/// and `rupu issues run`. Lists `.yaml` and `.yml` basenames from
/// the same global + project layers.
pub fn workflow_names(current: &OsStr) -> Vec<CompletionCandidate> {
    let mut out = list_basenames("workflows", "yaml", current);
    out.extend(list_basenames("workflows", "yml", current));
    dedupe_in_place(&mut out);
    out
}

/// Walk `<global>/<subdir>` and `<cwd>/.rupu/<subdir>` collecting
/// basenames of files with `ext`. Any IO error along the way returns
/// an empty list (completion must never panic / abort).
fn list_basenames(subdir: &str, ext: &str, current: &OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    let mut names: Vec<String> = Vec::new();

    if let Some(global) = global_dir() {
        push_basenames(&global.join(subdir), ext, &mut names);
    }
    if let Some(project) = project_root() {
        push_basenames(&project.join(".rupu").join(subdir), ext, &mut names);
    }

    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|n| n.starts_with(prefix.as_ref()))
        .map(CompletionCandidate::new)
        .collect()
}

fn push_basenames(dir: &Path, ext: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some(ext) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        out.push(stem.to_string());
    }
}

/// Resolve the user's global rupu directory (`~/.rupu` by default,
/// overridable by `$RUPU_HOME`). Mirrors `paths::global_dir` but
/// returns `None` instead of erroring — completion must degrade
/// gracefully when the dir is missing.
fn global_dir() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("RUPU_HOME") {
        return Some(PathBuf::from(custom));
    }
    dirs::home_dir().map(|h| h.join(".rupu"))
}

/// Walk up from cwd looking for a directory containing `.rupu/`.
/// Cheap stat-walk; bails at filesystem root.
fn project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut cur = cwd.as_path();
    loop {
        if cur.join(".rupu").is_dir() {
            return Some(cur.to_path_buf());
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return None,
        }
    }
}

fn dedupe_in_place(items: &mut Vec<CompletionCandidate>) {
    let mut seen = std::collections::BTreeSet::new();
    items.retain(|c| seen.insert(c.get_value().to_os_string()));
}
