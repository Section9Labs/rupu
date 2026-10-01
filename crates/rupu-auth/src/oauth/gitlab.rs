//! GitLab's OAuth application: gitlab.com's default, or a self-managed
//! instance's own.
//!
//! The gitlab.com default is GitLab's own CLI's application (gitlab-org/cli
//! `internal/glinstance/host.go` `DefaultClientID`; redirect URI and scopes
//! from `internal/oauth2/config.go`). Like glab, a self-managed instance
//! needs an application registered on that instance — the same redirect URI
//! and scopes, non-confidential — and its Application ID configured.

/// glab's public gitlab.com application id.
pub const GITLAB_COM_CLIENT_ID: &str =
    "41d48f9422ebd655dd9cf2947d6979681dfaddc6d0c56f7628f6ada59559af1e";

/// The loopback port glab's application is registered with.
pub const REDIRECT_PORT: u16 = 7171;

/// The redirect path glab's application is registered with.
pub const REDIRECT_PATH: &str = "/auth/redirect";

/// glab's registered scope set. A scope outside an application's
/// registration fails the authorize request with `invalid_scope`.
pub const SCOPES: &[&str] = &["openid", "profile", "read_user", "write_repository", "api"];

const GITLAB_COM: &str = "https://gitlab.com";

/// The OAuth application and endpoints for a GitLab account: its
/// `[scm.<account>]` `base_url` (the API root, `<instance>/api/v4`; unset
/// means gitlab.com) and `oauth_client_id`. gitlab.com defaults to glab's
/// application; a self-managed instance has no default and is refused with
/// what to register.
pub fn oauth_client(
    api_base_url: Option<&str>,
    client_id: Option<&str>,
    account: &str,
) -> anyhow::Result<crate::oauth::providers::OAuthClient> {
    let instance = match api_base_url {
        Some(base) => instance_root(base).ok_or_else(|| {
            anyhow::anyhow!(
                "[scm.{account}] base_url = {base:?} is not a URL; set it to the instance's API \
                 root, e.g. \"https://gitlab.example.com/api/v4\""
            )
        })?,
        None => GITLAB_COM.to_string(),
    };
    let client_id = match client_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        None if instance == GITLAB_COM => GITLAB_COM_CLIENT_ID.to_string(),
        None => anyhow::bail!(
            "{instance} needs its own OAuth application for SSO. Have an admin create one on \
             that instance (Admin > Applications, or User settings > Applications) with \
             redirect URI http://localhost:{REDIRECT_PORT}{REDIRECT_PATH}, scopes \"{scopes}\", \
             and Confidential unchecked; then set its Application ID as \
             [scm.{account}] oauth_client_id in the global config.toml and log in again. \
             (Or use --mode api-key with a personal access token.)",
            scopes = SCOPES.join(" ")
        ),
    };
    Ok(crate::oauth::providers::OAuthClient {
        client_id,
        authorize_url: format!("{instance}/oauth/authorize"),
        token_url: format!("{instance}/oauth/token"),
    })
}

/// The instance URL (scheme, host, port and any relative URL root) under
/// an API root `<instance>/api/v4`. `None` when `api_base_url` is not an
/// http(s) URL.
fn instance_root(api_base_url: &str) -> Option<String> {
    let mut url = url::Url::parse(api_base_url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    let s = url.as_str().trim_end_matches('/');
    Some(s.strip_suffix("/api/v4").unwrap_or(s).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitlab_com_defaults_to_glab_s_application() {
        let c = oauth_client(None, None, "gitlab").unwrap();
        assert_eq!(c.client_id, GITLAB_COM_CLIENT_ID);
        assert_eq!(c.authorize_url, "https://gitlab.com/oauth/authorize");
        assert_eq!(c.token_url, "https://gitlab.com/oauth/token");
    }

    #[test]
    fn gitlab_com_s_api_root_is_gitlab_com() {
        let c = oauth_client(Some("https://gitlab.com/api/v4"), None, "gitlab").unwrap();
        assert_eq!(c, oauth_client(None, None, "gitlab").unwrap());
    }

    #[test]
    fn a_configured_client_id_replaces_the_default() {
        let c = oauth_client(None, Some("my-own-app"), "gitlab").unwrap();
        assert_eq!(c.client_id, "my-own-app");
        assert_eq!(c.token_url, "https://gitlab.com/oauth/token");
    }

    #[test]
    fn a_blank_client_id_is_unset() {
        let c = oauth_client(None, Some("  "), "gitlab").unwrap();
        assert_eq!(c.client_id, GITLAB_COM_CLIENT_ID);
    }

    #[test]
    fn a_self_managed_instance_s_endpoints_come_from_its_api_root() {
        let c = oauth_client(
            Some("https://gitlab.example.com/api/v4/"),
            Some("corp-app"),
            "gl-corp",
        )
        .unwrap();
        assert_eq!(c.client_id, "corp-app");
        assert_eq!(
            c.authorize_url,
            "https://gitlab.example.com/oauth/authorize"
        );
        assert_eq!(c.token_url, "https://gitlab.example.com/oauth/token");
    }

    /// GitLab can be served under a relative URL root
    /// (`https://example.com/gitlab`); its API and OAuth endpoints both sit
    /// under it.
    #[test]
    fn an_instance_under_a_relative_url_root_keeps_it() {
        let c = oauth_client(
            Some("https://example.com/gitlab/api/v4"),
            Some("corp-app"),
            "gl-corp",
        )
        .unwrap();
        assert_eq!(
            c.authorize_url,
            "https://example.com/gitlab/oauth/authorize"
        );
        assert_eq!(c.token_url, "https://example.com/gitlab/oauth/token");
    }

    /// glab's application exists only on gitlab.com: another instance has
    /// never heard of it, so there is nothing to fall back to.
    #[test]
    fn a_self_managed_instance_without_a_client_id_says_what_to_register() {
        let err = oauth_client(Some("https://gitlab.example.com/api/v4"), None, "gl-corp")
            .unwrap_err()
            .to_string();
        for needed in [
            "https://gitlab.example.com",
            "[scm.gl-corp]",
            "oauth_client_id",
            "http://localhost:7171/auth/redirect",
            "openid profile read_user write_repository api",
        ] {
            assert!(err.contains(needed), "{needed:?} missing from: {err}");
        }
    }

    #[test]
    fn a_base_url_that_is_not_a_url_is_refused() {
        let err = oauth_client(Some("gitlab.example.com"), Some("x"), "gl-corp")
            .unwrap_err()
            .to_string();
        assert!(err.contains("[scm.gl-corp]"), "{err}");
        assert!(err.contains("base_url"), "{err}");
    }
}
