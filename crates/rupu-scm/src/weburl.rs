//! Pure git-remote-URL parsing and web permalink construction. No IO, no
//! async. Turns a `Workspace.repo_remote` (raw `git remote get-url origin`
//! output) plus a ref (branch or commit) + file + line range into a
//! github/gitlab blob URL. github.com and gitlab.com are always known;
//! self-hosted instances (GitHub Enterprise, self-managed GitLab) are known
//! through [`WebHosts::from_scm`], built from each `[scm.<account>]`'s
//! `base_url`.

use crate::platform::Platform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoWeb {
    pub platform: Platform,
    pub host: String,
    pub owner: String,
    pub repo: String,
}

/// Self-hosted SCM web hosts (host → platform), beyond the always-known
/// github.com / gitlab.com.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebHosts(Vec<(String, Platform)>);

impl WebHosts {
    /// The hosts named by every `[scm.<account>]` with a `base_url`, under
    /// the account's platform (`kind`, else the account name — the same
    /// rule the registry uses). `https://git.acme.internal/api/v3` names
    /// `git.acme.internal`. An account whose kind isn't github/gitlab, or
    /// whose `base_url` has no host, is skipped.
    pub fn from_scm(scm: &rupu_config::ScmSection) -> Self {
        let mut hosts = Vec::new();
        for (name, cfg) in &scm.platforms {
            let kind = cfg
                .kind
                .as_deref()
                .and_then(|k| k.parse::<Platform>().ok())
                .or_else(|| name.parse::<Platform>().ok());
            let (Some(kind), Some(url)) = (kind, cfg.base_url.as_deref()) else {
                continue;
            };
            if let Some(host) = url_host(url) {
                if !hosts.iter().any(|(h, _): &(String, Platform)| h == &host) {
                    hosts.push((host, kind));
                }
            }
        }
        Self(hosts)
    }

    fn platform_for(&self, host: &str) -> Option<Platform> {
        let host = host.to_ascii_lowercase();
        self.0.iter().find(|(h, _)| *h == host).map(|(_, p)| *p)
    }
}

/// The lower-cased host of a URL (`scheme://[user@]host[:port]/...`), port
/// dropped. `None` when there is no host.
fn url_host(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://").map(|(_, r)| r)?;
    let authority = rest.split('/').next().unwrap_or("");
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    let host = authority.split(':').next().unwrap_or("");
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn platform_for_host(host: &str, extra: &WebHosts) -> Option<Platform> {
    match host {
        "github.com" => Some(Platform::Github),
        "gitlab.com" => Some(Platform::Gitlab),
        other => extra.platform_for(other),
    }
}

/// Parse a raw git remote URL into a `RepoWeb`. Supports scp-style
/// (`git@host:owner/repo.git`), `https://host/owner/repo(.git)`, and
/// `ssh://git@host/owner/repo.git`. Owner may contain `/` (GitLab groups);
/// the last path segment is the repo. Unknown hosts → `None`.
pub fn parse_repo_remote(remote: &str) -> Option<RepoWeb> {
    parse_repo_remote_with(remote, &WebHosts::default())
}

/// [`parse_repo_remote`], also knowing the self-hosted `hosts`.
pub fn parse_repo_remote_with(remote: &str, hosts: &WebHosts) -> Option<RepoWeb> {
    let remote = remote.trim();
    if remote.is_empty() {
        return None;
    }

    // Split into (host, path) across the three URL shapes.
    let (host, path) = if let Some(rest) = remote
        .strip_prefix("https://")
        .or_else(|| remote.strip_prefix("http://"))
        .or_else(|| remote.strip_prefix("ssh://"))
    {
        // scheme URL: [user@]host/owner/.../repo
        let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        // An ssh port (`ssh://git@host:2222/...`) is not the web port.
        let host = if remote.starts_with("ssh://") {
            host.split(':').next().unwrap_or(host)
        } else {
            host
        };
        (host.to_string(), path.to_string())
    } else {
        // scp style: git@host:owner/.../repo
        let rest = remote.strip_prefix("git@")?;
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    };

    let platform = platform_for_host(&host, hosts)?;
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.rsplit_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(RepoWeb {
        platform,
        host,
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

impl RepoWeb {
    /// Repository landing page.
    pub fn home_url(&self) -> String {
        match self.platform {
            Platform::Github => format!("https://{}/{}/{}", self.host, self.owner, self.repo),
            Platform::Gitlab => format!("https://{}/{}/{}", self.host, self.owner, self.repo),
        }
    }

    /// Web blob URL to a file (optionally a line range) at `branch` — a
    /// branch name or a commit sha; the URL is only pinned when it is a
    /// commit. Platform-specific path prefix and line-fragment syntax:
    ///   GitHub: `/blob/<branch>/<path>#L<a>-L<b>`
    ///   GitLab: `/-/blob/<branch>/<path>#L<a>-<b>`
    pub fn blob_url(&self, branch: &str, path: &str, line_range: Option<[u32; 2]>) -> String {
        let base = match self.platform {
            Platform::Github => format!(
                "https://{}/{}/{}/blob/{}/{}",
                self.host, self.owner, self.repo, branch, path
            ),
            Platform::Gitlab => format!(
                "https://{}/{}/{}/-/blob/{}/{}",
                self.host, self.owner, self.repo, branch, path
            ),
        };
        match line_range {
            None => base,
            Some([a, b]) if a == b => format!("{base}#L{a}"),
            Some([a, b]) => match self.platform {
                Platform::Github => format!("{base}#L{a}-L{b}"),
                Platform::Gitlab => format!("{base}#L{a}-{b}"),
            },
        }
    }
}

/// Convenience: parse a remote and build a blob permalink in one call.
/// `branch` defaults to `"main"` when `None`. Returns `None` for unknown hosts.
pub fn repo_permalink(
    remote: &str,
    branch: Option<&str>,
    path: &str,
    line_range: Option<[u32; 2]>,
) -> Option<String> {
    repo_permalink_with(remote, branch, path, line_range, &WebHosts::default())
}

/// [`repo_permalink`], also knowing the self-hosted `hosts`.
pub fn repo_permalink_with(
    remote: &str,
    branch: Option<&str>,
    path: &str,
    line_range: Option<[u32; 2]>,
    hosts: &WebHosts,
) -> Option<String> {
    let rw = parse_repo_remote_with(remote, hosts)?;
    Some(rw.blob_url(branch.unwrap_or("main"), path, line_range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scp_style_github() {
        let r = parse_repo_remote("git@github.com:section9labs/rupu.git").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.host, "github.com");
        assert_eq!(r.owner, "section9labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parses_https_github_without_dot_git() {
        let r = parse_repo_remote("https://github.com/section9labs/rupu").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.owner, "section9labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parses_gitlab_nested_group() {
        let r = parse_repo_remote("https://gitlab.com/group/sub/proj.git").unwrap();
        assert_eq!(r.platform, Platform::Gitlab);
        assert_eq!(r.owner, "group/sub");
        assert_eq!(r.repo, "proj");
    }

    #[test]
    fn parses_ssh_scheme() {
        let r = parse_repo_remote("ssh://git@gitlab.com/group/proj.git").unwrap();
        assert_eq!(r.platform, Platform::Gitlab);
        assert_eq!(r.owner, "group");
        assert_eq!(r.repo, "proj");
    }

    #[test]
    fn unknown_host_is_none() {
        assert!(parse_repo_remote("git@bitbucket.org:x/y.git").is_none());
        assert!(parse_repo_remote("not a url").is_none());
        assert!(parse_repo_remote("").is_none());
    }

    #[test]
    fn github_blob_url_with_range() {
        let r = parse_repo_remote("git@github.com:o/r.git").unwrap();
        assert_eq!(
            r.blob_url("main", "src/a.rs", Some([17, 19])),
            "https://github.com/o/r/blob/main/src/a.rs#L17-L19"
        );
    }

    #[test]
    fn github_blob_url_single_line() {
        let r = parse_repo_remote("git@github.com:o/r.git").unwrap();
        assert_eq!(
            r.blob_url("main", "src/a.rs", Some([17, 17])),
            "https://github.com/o/r/blob/main/src/a.rs#L17"
        );
    }

    #[test]
    fn gitlab_blob_url_uses_dash_blob_and_dash_range() {
        let r = parse_repo_remote("https://gitlab.com/g/s/p.git").unwrap();
        assert_eq!(
            r.blob_url("dev", "a/b.rs", Some([3, 8])),
            "https://gitlab.com/g/s/p/-/blob/dev/a/b.rs#L3-8"
        );
    }

    #[test]
    fn home_urls() {
        assert_eq!(
            parse_repo_remote("git@github.com:o/r.git")
                .unwrap()
                .home_url(),
            "https://github.com/o/r"
        );
        assert_eq!(
            parse_repo_remote("https://gitlab.com/g/p.git")
                .unwrap()
                .home_url(),
            "https://gitlab.com/g/p"
        );
    }

    #[test]
    fn convenience_permalink_defaults_branch_to_main_and_none_on_unknown() {
        assert_eq!(
            repo_permalink("git@github.com:o/r.git", Some("dev"), "x.rs", Some([1, 1])),
            Some("https://github.com/o/r/blob/dev/x.rs#L1".to_string())
        );
        assert_eq!(
            repo_permalink("git@github.com:o/r.git", None, "x.rs", None),
            Some("https://github.com/o/r/blob/main/x.rs".to_string())
        );
        assert_eq!(repo_permalink("bad", Some("main"), "x.rs", None), None);
    }
}
