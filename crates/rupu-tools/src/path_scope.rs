//! Workspace path-scope check shared by file-touching tools.

use crate::tool::WorkspaceScope;
use std::path::Path;

/// True if `candidate` (relative or absolute) resolves into `root`.
/// Canonicalizes both ends; walks up to the nearest existing ancestor
/// so we can validate write paths whose intermediate directories don't
/// exist yet.
pub(crate) fn is_inside(root: &Path, candidate: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let mut cur = candidate.to_path_buf();
    // Walk up until we find a path that exists (or give up at fs root).
    while !cur.exists() {
        match cur.parent() {
            Some(p) if p != cur => cur = p.to_path_buf(),
            _ => return false,
        }
    }
    let Ok(cur) = cur.canonicalize() else {
        return false;
    };
    cur.starts_with(&root)
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
