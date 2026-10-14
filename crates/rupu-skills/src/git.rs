//! Fetch a git source with libgit2: no hooks, no submodules, https only.
//! The default branch is a shallow clone; a ref is a full clone checked
//! out detached at that ref.

use crate::source::{resolve_inside, Fetch, FetchedTree, GitSource, SourceError};
use std::path::Path;

pub struct GitFetcher;

impl Fetch for GitFetcher {
    fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError> {
        if !src.url.starts_with("https://") {
            return Err(SourceError::UnsupportedScheme(src.url.clone()));
        }
        fetch_any(src)
    }
}

/// The fetch itself, without the scheme check (unit tests clone `file://`).
pub(crate) fn fetch_any(src: &GitSource) -> Result<FetchedTree, SourceError> {
    let guard = tempfile::tempdir().map_err(|e| SourceError::Clone(e.to_string()))?;
    let checkout = guard.path().join(repo_dir_name(&src.url));
    let commit = clone_at(&src.url, &checkout, src.git_ref.as_deref()).map_err(clone_error)?;
    let root = match &src.path {
        Some(p) => {
            let r = resolve_inside(&checkout, p)?;
            if !r.is_dir() {
                return Err(SourceError::PathMissing(p.clone()));
            }
            r
        }
        None => checkout,
    };
    Ok(FetchedTree::with_guard(root, Some(commit), guard))
}

fn clone_error(e: git2::Error) -> SourceError {
    let auth = e.code() == git2::ErrorCode::Auth
        || e.message().to_ascii_lowercase().contains("authentication");
    if auth {
        return SourceError::Clone(format!(
            "{}; the repository is private or does not exist. Private repositories are not \
             supported yet: clone it yourself and install from the directory",
            e.message()
        ));
    }
    SourceError::Clone(e.message().to_owned())
}

/// The clone's directory name: the repo's last URL segment, so a repo whose
/// root is a skill keeps a meaningful name in `--list` output.
fn repo_dir_name(url: &str) -> String {
    let last = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("source");
    let name = last.trim_end_matches(".git");
    if name.is_empty() || name.starts_with('.') {
        "source".into()
    } else {
        name.into()
    }
}

/// Clone `url` into `dest` and return the checked-out commit id.
pub(crate) fn clone_at(
    url: &str,
    dest: &Path,
    git_ref: Option<&str>,
) -> Result<String, git2::Error> {
    let mut fo = git2::FetchOptions::new();
    if git_ref.is_none() && url.starts_with("https://") {
        fo.depth(1);
    }
    let repo = git2::build::RepoBuilder::new()
        .fetch_options(fo)
        .clone(url, dest)?;
    if let Some(r) = git_ref {
        let obj = [
            r.to_owned(),
            format!("origin/{r}"),
            format!("refs/tags/{r}"),
        ]
        .iter()
        .find_map(|spec| repo.revparse_single(spec).ok())
        .ok_or_else(|| git2::Error::from_str(&format!("ref {r:?} not found")))?;
        let commit = obj.peel_to_commit()?;
        repo.checkout_tree(
            commit.as_object(),
            Some(git2::build::CheckoutBuilder::new().force()),
        )?;
        repo.set_head_detached(commit.id())?;
    }
    let head = repo.head()?.peel_to_commit()?;
    Ok(head.id().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_all(repo: &git2::Repository, msg: &str) -> git2::Oid {
        let mut idx = repo.index().unwrap();
        idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.invalid").unwrap();
        let parents: Vec<git2::Commit> = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect();
        let refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs)
            .unwrap()
    }

    fn origin() -> (tempfile::TempDir, git2::Oid, git2::Oid) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let skill = dir.path().join("skills/s");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: s\ndescription: first\n---\n",
        )
        .unwrap();
        let first = commit_all(&repo, "first");
        let obj = repo.find_object(first, None).unwrap();
        repo.tag_lightweight("v1", &obj, false).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: s\ndescription: second\n---\n",
        )
        .unwrap();
        let second = commit_all(&repo, "second");
        (dir, first, second)
    }

    #[test]
    fn clones_head_and_a_tag() {
        let (o, first, second) = origin();
        let url = format!("file://{}", o.path().display());

        let head = fetch_any(&GitSource {
            url: url.clone(),
            git_ref: None,
            path: Some("skills/s".into()),
        })
        .unwrap();
        assert_eq!(head.commit.as_deref(), Some(second.to_string().as_str()));
        assert!(std::fs::read_to_string(head.root.join("SKILL.md"))
            .unwrap()
            .contains("second"));

        let tag = fetch_any(&GitSource {
            url,
            git_ref: Some("v1".into()),
            path: Some("skills/s".into()),
        })
        .unwrap();
        assert_eq!(tag.commit.as_deref(), Some(first.to_string().as_str()));
        assert!(std::fs::read_to_string(tag.root.join("SKILL.md"))
            .unwrap()
            .contains("first"));
    }

    #[test]
    fn missing_path_and_scheme() {
        let (o, _, _) = origin();
        let url = format!("file://{}", o.path().display());
        assert!(matches!(
            fetch_any(&GitSource {
                url: url.clone(),
                git_ref: None,
                path: Some("nope".into())
            }),
            Err(SourceError::PathMissing(_))
        ));
        assert!(matches!(
            GitFetcher.fetch(&GitSource {
                url,
                git_ref: None,
                path: None
            }),
            Err(SourceError::UnsupportedScheme(_))
        ));
    }

    #[test]
    fn repo_dir_names() {
        assert_eq!(
            repo_dir_name("https://github.com/o/report-style"),
            "report-style"
        );
        assert_eq!(repo_dir_name("https://h/team/skills.git"), "skills");
        assert_eq!(repo_dir_name("https://h/"), "h");
    }

    fn url_of(dir: &Path) -> String {
        format!("file://{}", dir.display())
    }

    #[cfg(unix)]
    #[test]
    fn a_path_through_a_symlink_cannot_leave_the_clone() {
        let outside = tempfile::tempdir().unwrap();
        let skill = outside.path().join("b");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: b\ndescription: d\n---\n",
        )
        .unwrap();

        let o = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(o.path()).unwrap();
        std::fs::create_dir_all(o.path().join("a")).unwrap();
        std::os::unix::fs::symlink(outside.path(), o.path().join("skills")).unwrap();
        std::os::unix::fs::symlink(outside.path(), o.path().join("a/link")).unwrap();
        std::fs::write(o.path().join("README.md"), "r\n").unwrap();
        commit_all(&repo, "links");

        for path in ["skills", "skills/b", "a/link/b"] {
            let got = fetch_any(&GitSource {
                url: url_of(o.path()),
                git_ref: None,
                path: Some(path.into()),
            });
            assert!(
                matches!(&got, Err(SourceError::BadPath(p)) if p == path),
                "{path}: {got:?}"
            );
        }
    }

    #[test]
    fn a_tag_checkout_drops_files_added_after_the_tag() {
        let (o, _, _) = origin();
        let repo = git2::Repository::open(o.path()).unwrap();
        std::fs::write(o.path().join("skills/s/later.md"), "added after v1\n").unwrap();
        commit_all(&repo, "third");
        let tag = fetch_any(&GitSource {
            url: url_of(o.path()),
            git_ref: Some("v1".into()),
            path: Some("skills/s".into()),
        })
        .unwrap();
        assert!(tag.root.join("SKILL.md").is_file());
        assert!(!tag.root.join("later.md").exists());
    }

    #[test]
    fn a_ref_names_a_branch_other_than_the_default() {
        let (o, _, second) = origin();
        let repo = git2::Repository::open(o.path()).unwrap();
        let head = repo.find_commit(second).unwrap();
        repo.branch("dev", &head, false).unwrap();
        repo.set_head("refs/heads/dev").unwrap();
        std::fs::write(
            o.path().join("skills/s/SKILL.md"),
            "---\nname: s\ndescription: on dev\n---\n",
        )
        .unwrap();
        let dev = commit_all(&repo, "dev only");
        let default = repo
            .find_branch("master", git2::BranchType::Local)
            .or_else(|_| repo.find_branch("main", git2::BranchType::Local));
        repo.set_head(default.unwrap().get().name().unwrap())
            .unwrap();

        let got = fetch_any(&GitSource {
            url: url_of(o.path()),
            git_ref: Some("dev".into()),
            path: Some("skills/s".into()),
        })
        .unwrap();
        assert_eq!(got.commit.as_deref(), Some(dev.to_string().as_str()));
        assert!(std::fs::read_to_string(got.root.join("SKILL.md"))
            .unwrap()
            .contains("on dev"));
    }

    #[test]
    fn an_auth_failure_says_private_repositories_are_unsupported() {
        let err = clone_error(git2::Error::new(
            git2::ErrorCode::Auth,
            git2::ErrorClass::Http,
            "remote authentication required but no callback set",
        ));
        assert_eq!(
            err.to_string(),
            "fetch failed: remote authentication required but no callback set; the repository \
             is private or does not exist. Private repositories are not supported yet: clone it \
             yourself and install from the directory"
        );
        let other = clone_error(git2::Error::from_str("unexpected EOF"));
        assert_eq!(other.to_string(), "fetch failed: unexpected EOF");
    }

    #[test]
    #[ignore = "network: clones a public GitHub repository"]
    fn https_shallow_clone_of_a_public_repo() {
        let got = GitFetcher
            .fetch(&GitSource {
                url: "https://github.com/octocat/Hello-World".into(),
                git_ref: None,
                path: None,
            })
            .unwrap();
        assert!(git2::Repository::open(&got.root).unwrap().is_shallow());
    }
}
