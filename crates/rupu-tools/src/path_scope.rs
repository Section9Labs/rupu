//! Workspace path-scope check shared by file-touching tools.

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
