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
    #[error("#{0}: the path must be relative, must not contain '..', and must stay inside the repository")]
    BadPath(String),
    #[error("the URL names ref {url_ref:?} but --ref says {flag_ref:?}{hint}")]
    RefConflict {
        url_ref: String,
        flag_ref: String,
        hint: String,
    },
    #[error("the URL names path {url_path:?} and #{hash_path:?}; give one")]
    PathConflict { url_path: String, hash_path: String },
    #[error(
        "the URL embeds credentials; never put a token in a source URL (it would be saved in \
         the source record). Private repositories are not supported yet: clone it yourself \
         and install from the directory"
    )]
    Credentials,
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
    parse_git_source(input, git_ref).map(Source::Git)
}

/// The git half of [`parse_source`]: `owner/repo`, an https URL (a GitHub
/// tree URL too) and an optional `#<path>`. It never consults the
/// filesystem, so a caller whose input must not depend on the working
/// directory (a built-in profile's pin) uses it directly.
pub(crate) fn parse_git_source(
    input: &str,
    git_ref: Option<&str>,
) -> Result<GitSource, SourceError> {
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
    if let Some((scheme, rest)) = base.split_once("://") {
        if !scheme.eq_ignore_ascii_case("https") {
            return Err(SourceError::UnsupportedScheme(input.to_owned()));
        }
        if rest
            .split('/')
            .next()
            .is_some_and(|host| host.contains('@'))
        {
            return Err(SourceError::Credentials);
        }
        let base = format!("https://{rest}");
        if let Some(tree) = github_tree(&base)? {
            if let Some(f) = &flag_ref {
                if *f != tree.git_ref {
                    return Err(ref_conflict(&tree, f));
                }
            }
            let path = match (hash_path, tree.path) {
                (Some(h), Some(u)) => {
                    return Err(SourceError::PathConflict {
                        url_path: u,
                        hash_path: h,
                    })
                }
                (h, u) => h.or(u),
            };
            return Ok(GitSource {
                url: tree.repo,
                git_ref: Some(tree.git_ref),
                path,
            });
        }
        return Ok(GitSource {
            url: base.trim_end_matches('/').to_owned(),
            git_ref: flag_ref,
            path: hash_path,
        });
    }
    let parts: Vec<&str> = base.split('/').collect();
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if parts.len() == 2 && parts.iter().all(|p| ok(p)) {
        return Ok(GitSource {
            url: format!("https://github.com/{}/{}", parts[0], parts[1]),
            git_ref: flag_ref,
            path: hash_path,
        });
    }
    Err(SourceError::Unrecognized(input.to_owned()))
}

/// A path inside a repository: relative, no `..`, no root or Windows
/// prefix. Applied to `#<path>`, tree-URL paths and recorded paths.
pub(crate) fn check_path(p: &str) -> Result<String, SourceError> {
    let trimmed = p.trim_end_matches('/');
    let path = Path::new(trimmed);
    if trimmed.is_empty()
        || p.starts_with('/')
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(SourceError::BadPath(p.to_owned()));
    }
    Ok(trimmed.to_owned())
}

/// `root/<rel>` after `check_path`, resolved through symlinks and required
/// to stay inside `root`: a repository can hold `skills -> /anywhere`.
pub(crate) fn resolve_inside(root: &Path, rel: &str) -> Result<PathBuf, SourceError> {
    let rel = check_path(rel)?;
    let root = root
        .canonicalize()
        .map_err(|_| SourceError::PathMissing(rel.clone()))?;
    let resolved = root
        .join(&rel)
        .canonicalize()
        .map_err(|_| SourceError::PathMissing(rel.clone()))?;
    if !resolved.starts_with(&root) {
        return Err(SourceError::BadPath(rel));
    }
    Ok(resolved)
}

struct TreeUrl {
    repo: String,
    git_ref: String,
    path: Option<String>,
    /// The segments after `tree/` or `blob/`: the ref and the path, joined.
    rest: Vec<String>,
}

/// `https://github.com/o/r/tree/<ref>/<path>` or `.../blob/<ref>/<path>/SKILL.md`.
/// The ref is one segment; a ref with `/` needs `--ref` and `#<path>`.
fn github_tree(url: &str) -> Result<Option<TreeUrl>, SourceError> {
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return Ok(None);
    };
    let segs: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if segs.len() < 4 || !matches!(segs[2], "tree" | "blob") {
        return Ok(None);
    }
    let mut tail: Vec<&str> = segs[3..].to_vec();
    if segs[2] == "blob" && tail.len() > 1 && tail.last() == Some(&"SKILL.md") {
        tail.pop();
    }
    let path = if tail.len() == 1 {
        None
    } else {
        Some(check_path(&tail[1..].join("/"))?)
    };
    Ok(Some(TreeUrl {
        repo: format!("https://github.com/{}/{}", segs[0], segs[1]),
        git_ref: tail[0].to_owned(),
        path,
        rest: tail.iter().map(|s| (*s).to_owned()).collect(),
    }))
}

/// When `--ref` has a `/` and the tree URL starts with it, the operator
/// copied a URL whose ref we read as its first segment: say how to name it.
fn ref_conflict(tree: &TreeUrl, flag_ref: &str) -> SourceError {
    let flag_segs: Vec<&str> = flag_ref.split('/').collect();
    let hint = if flag_segs.len() > 1 {
        let copied = tree.rest.len() >= flag_segs.len()
            && tree.rest.iter().zip(&flag_segs).all(|(a, b)| a == b);
        let at = match tree.rest.get(flag_segs.len()..) {
            Some(path) if copied && !path.is_empty() => format!("#{}", path.join("/")),
            _ if copied => String::new(),
            _ => "#<path>".to_owned(),
        };
        format!(
            "; a ref containing `/` can't be named in a tree URL: use {}{at} --ref {flag_ref}",
            tree.repo
        )
    } else {
        String::new()
    };
    SourceError::RefConflict {
        url_ref: tree.git_ref.clone(),
        flag_ref: flag_ref.to_owned(),
        hint,
    }
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
