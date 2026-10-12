//! What `rupu skill install` accepts (spec §7.1): a local directory, a
//! GitHub `owner/repo`, a GitHub tree or blob URL, or any https git URL
//! with an optional `#<path>`.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    Git(GitSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    pub url: String,
    pub git_ref: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("no such directory: {0}")]
    NotFound(String),
    #[error("only https:// git URLs are fetched ({0}); for a local checkout pass its directory")]
    UnsupportedScheme(String),
    #[error("#{0}: the path must be relative and must not contain '..'")]
    BadPath(String),
    #[error("the URL names ref {url_ref:?} but --ref says {flag_ref:?}")]
    RefConflict { url_ref: String, flag_ref: String },
    #[error("{0:?} is not a directory, owner/repo, or https git URL")]
    Unrecognized(String),
    #[error("fetch failed: {0}")]
    Clone(String),
    #[error("#{0} does not exist in the repository")]
    PathMissing(String),
}

pub fn parse_source(
    input: &str,
    git_ref: Option<&str>,
    home: Option<&Path>,
) -> Result<Source, SourceError> {
    let expanded = match (input.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => PathBuf::from(input),
    };
    if expanded.is_dir() {
        let canon = expanded
            .canonicalize()
            .map_err(|_| SourceError::NotFound(input.to_owned()))?;
        return Ok(Source::Local(canon));
    }
    if input.starts_with('.') || input.starts_with('/') || input.starts_with('~') {
        return Err(SourceError::NotFound(input.to_owned()));
    }
    if input.starts_with("git@") {
        return Err(SourceError::UnsupportedScheme(input.to_owned()));
    }
    let (base, hash_path) = match input.split_once('#') {
        Some((b, p)) => (b, Some(check_path(p)?)),
        None => (input, None),
    };
    let flag_ref = git_ref.map(str::to_owned);
    if let Some((scheme, _)) = base.split_once("://") {
        if scheme != "https" {
            return Err(SourceError::UnsupportedScheme(input.to_owned()));
        }
        if let Some(tree) = github_tree(base)? {
            let (url, url_ref, path) = tree;
            if let Some(f) = &flag_ref {
                if *f != url_ref {
                    return Err(SourceError::RefConflict {
                        url_ref,
                        flag_ref: f.clone(),
                    });
                }
            }
            return Ok(Source::Git(GitSource {
                url,
                git_ref: Some(url_ref),
                path: hash_path.or(path),
            }));
        }
        return Ok(Source::Git(GitSource {
            url: base.trim_end_matches('/').to_owned(),
            git_ref: flag_ref,
            path: hash_path,
        }));
    }
    let parts: Vec<&str> = base.split('/').collect();
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if parts.len() == 2 && parts.iter().all(|p| ok(p)) {
        return Ok(Source::Git(GitSource {
            url: format!("https://github.com/{}/{}", parts[0], parts[1]),
            git_ref: flag_ref,
            path: hash_path,
        }));
    }
    Err(SourceError::Unrecognized(input.to_owned()))
}

fn check_path(p: &str) -> Result<String, SourceError> {
    let trimmed = p.trim_end_matches('/');
    let path = Path::new(trimmed);
    if trimmed.is_empty()
        || p.starts_with('/')
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(SourceError::BadPath(p.to_owned()));
    }
    Ok(trimmed.to_owned())
}

/// `https://github.com/o/r/tree/<ref>/<path>` or `.../blob/<ref>/<path>/SKILL.md`
/// → (repo URL, ref, path). The ref is one segment; a ref with `/` needs
/// `--ref` and `#<path>`.
fn github_tree(url: &str) -> Result<Option<(String, String, Option<String>)>, SourceError> {
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return Ok(None);
    };
    let segs: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if segs.len() < 4 || !matches!(segs[2], "tree" | "blob") {
        return Ok(None);
    }
    let repo = format!("https://github.com/{}/{}", segs[0], segs[1]);
    let mut path_segs: Vec<&str> = segs[4..].to_vec();
    if segs[2] == "blob" && path_segs.last() == Some(&"SKILL.md") {
        path_segs.pop();
    }
    let path = if path_segs.is_empty() {
        None
    } else {
        Some(check_path(&path_segs.join("/"))?)
    };
    Ok(Some((repo, segs[3].to_owned(), path)))
}

/// A fetched source tree: `root` is the repo root, or `<path>` inside it.
#[derive(Debug)]
pub struct FetchedTree {
    pub root: PathBuf,
    pub commit: Option<String>,
    _guard: Option<tempfile::TempDir>,
}

impl FetchedTree {
    pub fn local(root: PathBuf) -> Self {
        Self {
            root,
            commit: None,
            _guard: None,
        }
    }

    /// A tree already on disk, with the commit it stands for (if any).
    pub fn local_with_commit(root: PathBuf, commit: Option<String>) -> Self {
        Self {
            root,
            commit,
            _guard: None,
        }
    }

    pub fn with_guard(root: PathBuf, commit: Option<String>, guard: tempfile::TempDir) -> Self {
        Self {
            root,
            commit,
            _guard: Some(guard),
        }
    }
}

/// How a git source is fetched. `git::GitFetcher` is the production one;
/// tests pass their own.
pub trait Fetch {
    fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError>;
}
