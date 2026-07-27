//! The consumer side of `[scm.<platform>]`.
//!
//! ISSUES.md I-16 / I-17: `clone_protocol` and `timeout_ms` parsed, were
//! documented in `docs/scm.md`, and `clone_protocol` even had a dedicated
//! https/ssh dropdown in CP Settings — yet nothing read either. Clones always
//! went over HTTPS and only GitLab had a timeout, hardcoded at 30s.
//!
//! Every decision the two keys drive is a pure function here so it can be
//! asserted without a network or a real `git clone`.

use std::path::Path;
use std::time::Duration;

use rupu_config::ScmPlatformConfig;

/// SCM HTTP timeout when `[scm.<platform>].timeout_ms` is absent. Matches the
/// value `docs/scm.md` shows in its example config and the GitLab client's
/// previous hardcoded 30s.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// How `clone_to` reaches the remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloneProtocol {
    /// Token-in-URL HTTPS. The default.
    #[default]
    Https,
    /// `git@host:owner/repo.git`, authenticated by the user's SSH agent /
    /// `~/.ssh/config`.
    Ssh,
}

impl CloneProtocol {
    /// Parse `[scm.<platform>].clone_protocol`. Anything other than a
    /// case-insensitive `ssh` is HTTPS — including an unrecognized value,
    /// which is logged rather than failing the clone.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            None | Some("") => Self::Https,
            Some(v) if v.eq_ignore_ascii_case("ssh") => Self::Ssh,
            Some(v) if v.eq_ignore_ascii_case("https") => Self::Https,
            Some(other) => {
                tracing::warn!(
                    clone_protocol = %other,
                    "unrecognized [scm.<platform>].clone_protocol; expected \"https\" or \"ssh\" — using https"
                );
                Self::Https
            }
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Ssh => "ssh",
        }
    }
}

/// Everything a platform connector needs from `[scm.<platform>]`.
#[derive(Debug, Clone)]
pub struct ScmClientOptions {
    pub base_url: Option<String>,
    pub timeout: Duration,
    pub max_concurrency: Option<usize>,
    pub clone_protocol: CloneProtocol,
}

impl Default for ScmClientOptions {
    fn default() -> Self {
        Self {
            base_url: None,
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            max_concurrency: None,
            clone_protocol: CloneProtocol::Https,
        }
    }
}

impl ScmClientOptions {
    /// Resolve from a `[scm.<platform>]` table (absent ⇒ all defaults).
    pub fn from_platform_config(cfg: Option<&ScmPlatformConfig>) -> Self {
        Self {
            base_url: cfg.and_then(|c| c.base_url.clone()),
            timeout: scm_timeout(cfg.and_then(|c| c.timeout_ms)),
            max_concurrency: cfg.and_then(|c| c.max_concurrency),
            clone_protocol: CloneProtocol::parse(
                cfg.and_then(|c| c.clone_protocol.as_deref()),
            ),
        }
    }

    /// An HTTP client builder honoring [`Self::timeout`]. SCM calls are short
    /// request/response round-trips with no streaming, so this is applied as a
    /// total deadline — unlike the provider side, where a total deadline would
    /// truncate a long generation.
    pub fn http_client_builder(&self) -> reqwest::ClientBuilder {
        reqwest::Client::builder().timeout(self.timeout)
    }
}

/// `timeout_ms` → the SCM client deadline. Absent ⇒ [`DEFAULT_TIMEOUT_MS`].
/// `0` is treated as absent: a zero deadline would fail every call instantly.
pub fn scm_timeout(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(timeout_ms.filter(|ms| *ms > 0).unwrap_or(DEFAULT_TIMEOUT_MS))
}

/// The URL `clone_to` hands to git, for `host` (e.g. `github.com`).
///
/// * HTTPS embeds the token in the URL. `userinfo` is the platform's
///   convention: GitHub takes the bare token as the username, GitLab requires
///   the literal `oauth2` username with the token as password.
/// * SSH ignores the token entirely — authentication is the user's SSH agent
///   and `~/.ssh/config`, which is the whole point of choosing it.
pub fn clone_url(
    host: &str,
    owner: &str,
    repo: &str,
    protocol: CloneProtocol,
    userinfo: &str,
) -> String {
    match protocol {
        CloneProtocol::Ssh => format!("git@{host}:{owner}/{repo}.git"),
        CloneProtocol::Https => format!("https://{userinfo}@{host}/{owner}/{repo}.git"),
    }
}

/// The `git` argv for an SSH clone.
///
/// SSH clones shell out to the system `git` rather than going through `git2`.
/// Two reasons, both blocking: rupu's `git2` is built without the `ssh`
/// feature (root `Cargo.toml` enables only `https` + vendored libgit2/openssl),
/// so libgit2 has no ssh transport at all; and even with it, libgit2 does not
/// read `~/.ssh/config`, so host aliases, `IdentityFile`, `ProxyJump`, and
/// agent forwarding — exactly what SSH-only infrastructure depends on — would
/// silently not apply. The HTTPS path stays on `git2`.
pub fn ssh_clone_argv(url: &str, dir: &Path) -> Vec<String> {
    vec![
        "clone".to_string(),
        url.to_string(),
        dir.display().to_string(),
    ]
}

/// Execute a clone of `url` into `dir`.
///
/// HTTPS goes through `git2` (the token is already embedded in `url`). SSH
/// shells out to the system `git` — see [`ssh_clone_argv`] for why that is not
/// optional.
pub async fn run_clone(
    url: String,
    dir: std::path::PathBuf,
    protocol: CloneProtocol,
) -> Result<(), crate::error::ScmError> {
    use crate::error::ScmError;
    tokio::task::spawn_blocking(move || -> Result<(), ScmError> {
        match protocol {
            CloneProtocol::Https => {
                git2::Repository::clone(&url, &dir)
                    .map_err(|e| ScmError::Network(anyhow::anyhow!("git clone failed: {e}")))?;
                Ok(())
            }
            CloneProtocol::Ssh => {
                let output = std::process::Command::new("git")
                    .args(ssh_clone_argv(&url, &dir))
                    .output()
                    .map_err(|e| {
                        ScmError::Network(anyhow::anyhow!(
                            "ssh clone: could not run `git` (clone_protocol = \"ssh\" shells out \
                             to the system git so your ~/.ssh config applies): {e}"
                        ))
                    })?;
                if output.status.success() {
                    return Ok(());
                }
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                Err(ScmError::Network(anyhow::anyhow!(
                    "git clone over ssh failed ({}): {stderr}",
                    output.status
                )))
            }
        }
    })
    .await
    .map_err(|e| ScmError::Transient(anyhow::anyhow!("clone task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_protocol_parses_ssh_case_insensitively() {
        assert_eq!(CloneProtocol::parse(Some("ssh")), CloneProtocol::Ssh);
        assert_eq!(CloneProtocol::parse(Some("SSH")), CloneProtocol::Ssh);
        assert_eq!(CloneProtocol::parse(Some(" ssh ")), CloneProtocol::Ssh);
        assert_eq!(CloneProtocol::parse(Some("https")), CloneProtocol::Https);
        assert_eq!(CloneProtocol::parse(None), CloneProtocol::Https);
        // Unrecognized values fall back to https rather than failing a clone.
        assert_eq!(CloneProtocol::parse(Some("git")), CloneProtocol::Https);
    }

    /// I-16 at the decision point: `clone_protocol = "ssh"` produces an ssh
    /// URL and drops the token; the default produces the token-bearing https
    /// URL both connectors used to hardcode.
    #[test]
    fn clone_url_honors_ssh_protocol() {
        assert_eq!(
            clone_url("github.com", "o", "r", CloneProtocol::Ssh, "ghp_secret"),
            "git@github.com:o/r.git"
        );
        assert_eq!(
            clone_url("github.com", "o", "r", CloneProtocol::Https, "ghp_secret"),
            "https://ghp_secret@github.com/o/r.git"
        );
        assert_eq!(
            clone_url("gitlab.com", "grp/sub", "r", CloneProtocol::Ssh, "oauth2:glpat"),
            "git@gitlab.com:grp/sub/r.git"
        );
        assert_eq!(
            clone_url(
                "gitlab.com",
                "grp/sub",
                "r",
                CloneProtocol::Https,
                "oauth2:glpat"
            ),
            "https://oauth2:glpat@gitlab.com/grp/sub/r.git"
        );
    }

    #[test]
    fn ssh_clone_url_never_leaks_the_token() {
        let url = clone_url("github.com", "o", "r", CloneProtocol::Ssh, "ghp_secret");
        assert!(!url.contains("ghp_secret"), "token leaked into ssh url: {url}");
    }

    #[test]
    fn ssh_clone_argv_is_a_plain_git_clone() {
        assert_eq!(
            ssh_clone_argv("git@github.com:o/r.git", Path::new("/tmp/dest")),
            vec!["clone", "git@github.com:o/r.git", "/tmp/dest"]
        );
    }

    #[test]
    fn scm_timeout_defaults_to_30s() {
        assert_eq!(scm_timeout(None), Duration::from_millis(30_000));
        assert_eq!(scm_timeout(Some(1_500)), Duration::from_millis(1_500));
        assert_eq!(scm_timeout(Some(0)), Duration::from_millis(30_000));
    }

    #[test]
    fn options_read_every_configured_key() {
        let cfg = ScmPlatformConfig {
            base_url: Some("https://ghe.example.com/api/v3".into()),
            timeout_ms: Some(7_000),
            max_concurrency: Some(3),
            clone_protocol: Some("ssh".into()),
        };
        let o = ScmClientOptions::from_platform_config(Some(&cfg));
        assert_eq!(o.base_url.as_deref(), Some("https://ghe.example.com/api/v3"));
        assert_eq!(o.timeout, Duration::from_millis(7_000));
        assert_eq!(o.max_concurrency, Some(3));
        assert_eq!(o.clone_protocol, CloneProtocol::Ssh);

        let d = ScmClientOptions::from_platform_config(None);
        assert_eq!(d.timeout, Duration::from_millis(30_000));
        assert_eq!(d.clone_protocol, CloneProtocol::Https);
        assert!(d.base_url.is_none());
        assert!(d.max_concurrency.is_none());
    }
}
