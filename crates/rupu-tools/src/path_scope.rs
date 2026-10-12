//! Workspace path-scope check shared by file-touching tools.

use crate::tool::WorkspaceScope;
use std::path::{Component, Path, PathBuf};

/// Where `candidate` lands once the OS resolves it, or `None` when that
/// can't be known safely. A path that exists is canonicalized. Otherwise its
/// longest prefix that exists (by `lstat`, so a dangling symlink counts and
/// then fails to canonicalize) is canonicalized and the rest appended, which
/// may only name new entries: a `..` there would be resolved against
/// directories a write's `create_dir_all` has yet to create.
fn resolve(candidate: &Path) -> Option<PathBuf> {
    if let Ok(p) = candidate.canonicalize() {
        return Some(p);
    }
    let mut existing = PathBuf::new();
    let mut rest = candidate.components().peekable();
    while let Some(c) = rest.peek() {
        let next = existing.join(c);
        if next.symlink_metadata().is_err() {
            break;
        }
        existing = next;
        rest.next();
    }
    let mut resolved = existing.canonicalize().ok()?;
    for c in rest {
        match c {
            Component::Normal(name) => resolved.push(name),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(resolved)
}

/// True if `candidate` (relative or absolute) resolves into `root`.
pub(crate) fn is_inside(root: &Path, candidate: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    resolve(candidate).is_some_and(|p| p.starts_with(&root))
}

/// Inside the workspace or one of the run's read roots (spec §3.4). The
/// read tools use this; `write_file` and `edit_file` keep `is_inside`.
pub(crate) fn is_readable(ws: &WorkspaceScope, candidate: &Path) -> bool {
    is_inside(&ws.path, candidate) || in_read_root(ws, candidate)
}

/// Inside one of the run's read roots: a skill's files, which are not
/// target files, so reading them is never coverage (spec §3.4).
pub(crate) fn in_read_root(ws: &WorkspaceScope, candidate: &Path) -> bool {
    ws.read_roots.iter().any(|r| is_inside(r, candidate))
}
