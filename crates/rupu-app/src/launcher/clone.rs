//! Async wrapper around `rupu_scm::clone_repo_ref`. Constructs a
//! ULID-suffixed tempdir under `~/Library/Caches/rupu.app/clones/`,
//! parses the RepoRef from a user-typed string, calls the connector,
//! and returns the populated path.

use std::path::PathBuf;
use std::sync::Arc;

use rupu_scm::{Platform, Registry, RepoRef};

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("clone failed: {0}")]
    Scm(#[from] rupu_scm::CloneError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn clone_repo_ref(
    registry: Arc<Registry>,
    user_input: &str,
) -> Result<PathBuf, CloneError> {
    let r = parse_repo_ref(user_input)?;
    let root = crate::workspace::storage::clones_dir()
        .map_err(|e| CloneError::Io(std::io::Error::other(e.to_string())))?;
    let id = ulid::Ulid::new().to_string();
    let dir = root.join(id);
    std::fs::create_dir_all(&dir)?;
    rupu_scm::clone_repo_ref(&registry, &r, &dir).await?;
    Ok(dir)
}

/// Parse `<platform>:<owner>/<repo>[@<ref>]` into a `RepoRef`. The
/// `@<ref>` segment is currently silently dropped — D-4 always clones
/// HEAD. Adding ref-aware cloning is a future polish task.
pub fn parse_repo_ref(s: &str) -> Result<RepoRef, CloneError> {
    let (platform_str, rest) = s
        .split_once(':')
        .ok_or_else(|| CloneError::Parse(format!("missing ':' separator in '{s}'")))?;
    let platform = match platform_str.to_lowercase().as_str() {
        "github" | "gh" => Platform::Github,
        "gitlab" | "gl" => Platform::Gitlab,
        other => return Err(CloneError::Parse(format!("unknown platform '{other}'"))),
    };
    let rest = rest.split_once('@').map(|(a, _b)| a).unwrap_or(rest);
    let (owner, repo) = rest
        .split_once('/')
        .ok_or_else(|| CloneError::Parse(format!("missing '/' in '{rest}'")))?;
    Ok(RepoRef {
        platform,
        owner: owner.into(),
        repo: repo.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_owner_repo() {
        let r = parse_repo_ref("github:foo/bar").expect("parse");
        assert!(matches!(r.platform, Platform::Github));
        assert_eq!(r.owner, "foo");
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn parse_drops_ref_suffix() {
        let r = parse_repo_ref("github:foo/bar@main").expect("parse");
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn parse_rejects_unknown_platform() {
        assert!(parse_repo_ref("bitbucket:foo/bar").is_err());
    }

    #[test]
    fn parse_rejects_missing_colon() {
        assert!(parse_repo_ref("foo/bar").is_err());
    }
}
