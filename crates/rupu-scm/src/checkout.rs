//! A local git checkout's repository: the `[remote "origin"]` url in its
//! `.git/config`, read back as a [`RepoRef`], plus the origin url and
//! default branch `git` reports for it. Shared by `rupu repos attach` /
//! `prefer` and the CP's tracked-repo endpoints.

use crate::{Platform, RepoRef};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CheckoutError {
    #[error("not in a git checkout — pass --repo <platform>:<owner>/<repo>")]
    NotACheckout,
    #[error("read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("no [remote \"origin\"] url in {0}")]
    NoOrigin(String),
    #[error("could not parse origin url `{0}` to <platform>:<owner>/<repo>")]
    UnknownRemote(String),
}

/// `<platform>:<owner>/<repo>`: the form the repo registry keys records by.
pub fn canonical_repo_ref(repo: &RepoRef) -> String {
    format!("{}:{}/{}", repo.platform, repo.owner, repo.repo)
}

/// The repository the checkout at (or above) `path` points at: walks up
/// to the nearest `.git/config` and parses its origin url.
pub fn repo_of_checkout(path: &Path) -> Result<RepoRef, CheckoutError> {
    let git_config = find_git_config(path).ok_or(CheckoutError::NotACheckout)?;
    let url = read_origin_url(&git_config)?;
    parse_remote_url(&url).ok_or(CheckoutError::UnknownRemote(url))
}

fn find_git_config(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(d) = cur {
        let candidate = d.join(".git").join("config");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = d.parent();
    }
    None
}

/// The `url` of the `[remote "origin"]` section of a git config file.
pub fn read_origin_url(config_path: &Path) -> Result<String, CheckoutError> {
    let text = std::fs::read_to_string(config_path).map_err(|e| CheckoutError::Read {
        path: config_path.display().to_string(),
        source: e,
    })?;
    let mut in_origin = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            // Match `[remote "origin"]` exactly so a [remote "upstream"]
            // section doesn't trigger.
            in_origin = trimmed == "[remote \"origin\"]";
            continue;
        }
        if !in_origin {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("url = ") {
            return Ok(rest.trim().to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("url=") {
            return Ok(rest.trim().to_string());
        }
    }
    Err(CheckoutError::NoOrigin(config_path.display().to_string()))
}

/// Map common origin URL forms to a `RepoRef`. Handles:
/// - `git@github.com:owner/repo.git`
/// - `https://github.com/owner/repo.git`
/// - `ssh://git@github.com/owner/repo`
/// - GitLab equivalents
/// - `github-<alias>:owner/repo` (SSH alias forms used in user `.ssh/config`)
pub fn parse_remote_url(url: &str) -> Option<RepoRef> {
    let stripped = url.trim().trim_end_matches('/').trim_end_matches(".git");
    // ssh-alias / SSH form: `<host>:owner/repo` (no scheme)
    if !stripped.contains("://") {
        // `git@github.com:owner/repo` or `github-foo:owner/repo`
        if let Some((host_part, path)) = stripped.split_once(':') {
            // Strip a leading `git@` if present.
            let host = host_part.rsplit('@').next().unwrap_or(host_part);
            let platform = host_to_platform(host)?;
            let (owner, repo) = path.split_once('/')?;
            // Some hosts allow nested groups (gitlab). Take the LAST
            // segment as the repo, the rest as owner.
            let (owner, repo) = owner_repo_from_path(&format!("{owner}/{repo}"))?;
            return Some(RepoRef {
                platform,
                owner,
                repo,
            });
        }
        return None;
    }
    // URL form: scheme://[user@]host/path
    let after_scheme = stripped.split_once("://")?.1;
    let (host_and_user, path) = after_scheme.split_once('/')?;
    let host = host_and_user.rsplit('@').next().unwrap_or(host_and_user);
    let platform = host_to_platform(host)?;
    let (owner, repo) = owner_repo_from_path(path)?;
    Some(RepoRef {
        platform,
        owner,
        repo,
    })
}

fn host_to_platform(host: &str) -> Option<Platform> {
    // Match exact + ssh-alias prefix (e.g. `github-personal` from
    // `.ssh/config` Host blocks). Self-hosted GitLab installs need
    // `--repo` since their hostname carries no signal.
    let lower = host.to_ascii_lowercase();
    if lower == "github.com" || lower.starts_with("github-") || lower.starts_with("github.") {
        return Some(Platform::Github);
    }
    if lower == "gitlab.com" || lower.starts_with("gitlab-") || lower.starts_with("gitlab.") {
        return Some(Platform::Gitlab);
    }
    None
}

fn owner_repo_from_path(path: &str) -> Option<(String, String)> {
    let cleaned = path.trim_matches('/').trim_end_matches(".git");
    let mut parts: Vec<&str> = cleaned.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let repo = parts.pop()?.to_string();
    let owner = parts.join("/");
    Some((owner, repo))
}

/// `git remote get-url origin` in `path`; `None` when git reports none.
pub fn origin_url(path: &Path) -> std::io::Result<Option<String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "get-url", "origin"])
        .output()?;
    if !out.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

/// The checkout's default branch: `origin/HEAD`'s target, else the
/// branch checked out now.
pub fn origin_default_branch(path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .output()
        .ok()?;
    if out.status.success() {
        let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
        if let Some(branch) = value.strip_prefix("origin/") {
            if !branch.is_empty() {
                return Some(branch.to_string());
            }
        }
    }

    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_url_https_github() {
        let r = parse_remote_url("https://github.com/Section9Labs/rupu.git").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.owner, "Section9Labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parse_remote_url_ssh_github() {
        let r = parse_remote_url("git@github.com:Section9Labs/rupu.git").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.owner, "Section9Labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parse_remote_url_ssh_alias() {
        // `.ssh/config` Host alias form: github-daneel:section9labs/rupu.git
        let r = parse_remote_url("github-daneel:section9labs/rupu.git").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.owner, "section9labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parse_remote_url_gitlab_nested_groups() {
        // GitLab supports nested groups; owner is everything before the last segment.
        let r = parse_remote_url("https://gitlab.com/foo/bar/baz.git").unwrap();
        assert_eq!(r.platform, Platform::Gitlab);
        assert_eq!(r.owner, "foo/bar");
        assert_eq!(r.repo, "baz");
    }

    #[test]
    fn parse_remote_url_unknown_host_returns_none() {
        assert!(parse_remote_url("https://example.com/owner/repo.git").is_none());
    }

    #[test]
    fn repo_of_checkout_reads_origin_from_a_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(
            tmp.path().join(".git/config"),
            "[remote \"upstream\"]\n\turl = https://github.com/x/y.git\n[remote \"origin\"]\n\turl = git@github.com:acme/widgets.git\n",
        )
        .unwrap();
        let sub = tmp.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let r = repo_of_checkout(&sub).unwrap();
        assert_eq!(canonical_repo_ref(&r), "github:acme/widgets");
    }
}
