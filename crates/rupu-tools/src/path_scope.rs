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
    root.canonicalize()
        .is_ok_and(|root| within(&root, candidate))
}

/// [`is_inside`] for a `root` that is already canonical.
pub(crate) fn within(root: &Path, candidate: &Path) -> bool {
    resolve(candidate).is_some_and(|p| p.starts_with(root))
}

/// A run's workspace and read roots (spec §3.4), canonicalized once for a
/// tool call that checks many paths.
pub(crate) struct Roots {
    workspace: Option<PathBuf>,
    read: Vec<PathBuf>,
}

impl Roots {
    pub(crate) fn of(ws: &WorkspaceScope) -> Self {
        Self {
            workspace: ws.path.canonicalize().ok(),
            read: ws
                .read_roots
                .iter()
                .filter_map(|r| r.canonicalize().ok())
                .collect(),
        }
    }

    /// Where `candidate` resolves, when that is inside the workspace or one
    /// of the read roots: the path a read tool opens, so a link swapped
    /// after this check can't redirect the read. `write_file` and
    /// `edit_file` keep `is_inside`.
    pub(crate) fn readable(&self, candidate: &Path) -> Option<PathBuf> {
        resolve(candidate).filter(|p| self.has_workspace(p) || self.has_read_root(p))
    }

    /// Inside the workspace and not under a read root: a target file, whose
    /// reads are coverage. A skill's files never are, even a project skill's
    /// inside the workspace.
    pub(crate) fn is_target(&self, candidate: &Path) -> bool {
        resolve(candidate).is_some_and(|p| self.has_workspace(&p) && !self.has_read_root(&p))
    }

    fn has_workspace(&self, resolved: &Path) -> bool {
        self.workspace
            .as_deref()
            .is_some_and(|w| resolved.starts_with(w))
    }

    fn has_read_root(&self, resolved: &Path) -> bool {
        self.read.iter().any(|r| resolved.starts_with(r))
    }
}

/// `shown`, a path a tool printed under the `resolved` path it opened, with
/// that prefix put back as `given`: what the tool reports is spelled the way
/// it was asked for.
pub(crate) fn as_given(shown: &str, resolved: &Path, given: &Path) -> String {
    let prefix = resolved.to_string_lossy();
    match shown.strip_prefix(&*prefix) {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', ':']) => {
            format!("{}{rest}", given.display())
        }
        _ => shown.to_owned(),
    }
}
