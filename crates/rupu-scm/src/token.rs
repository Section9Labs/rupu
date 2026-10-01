//! The token an SCM connector authenticates with, kept current.
//!
//! A connector is built once per [`crate::Registry`], and a registry lives
//! as long as its process: `cp serve`, `mcp serve` and session daemons hold
//! one for hours. An OAuth access token does not: GitLab's expire two hours
//! after issue, and the refresh token rotates on every refresh. So a
//! connector holding an OAuth credential asks this source for the token
//! before every request, and an expiring one is refreshed first — through
//! the credential store's refresher (`rupu-auth`'s `KeychainResolver`),
//! which refreshes under the store's cross-process lock and persists the
//! rotation. A refresh kept only in this process's memory would leave the
//! store holding a dead refresh token for the next one.

use std::sync::Arc;

use rupu_providers::auth::AuthCredentials;
use rupu_providers::credential_writes::OAuthRefresher;

use crate::error::ScmError;
use crate::platform::Platform;

pub struct TokenSource {
    platform: Platform,
    /// The configured account name, for the re-login hint.
    account: String,
    /// Held across a refresh, so concurrent requests on one connector wait
    /// for the one refresh instead of each starting their own.
    current: tokio::sync::Mutex<AuthCredentials>,
    refresher: Option<Arc<dyn OAuthRefresher>>,
}

impl TokenSource {
    /// A token that is used as-is for the connector's whole life: an
    /// access token (PAT), or a test's.
    pub fn fixed(platform: Platform, token: impl Into<String>) -> Self {
        Self::new(
            platform,
            platform.as_str(),
            AuthCredentials::ApiKey { key: token.into() },
            None,
        )
    }

    /// `credentials` as resolved for `account`; `refresher` is the store's
    /// refresher for them (`CredentialResolver::oauth_refresher`), `None`
    /// when the store has none.
    pub fn new(
        platform: Platform,
        account: impl Into<String>,
        credentials: AuthCredentials,
        refresher: Option<Arc<dyn OAuthRefresher>>,
    ) -> Self {
        Self {
            platform,
            account: account.into(),
            current: tokio::sync::Mutex::new(credentials),
            refresher,
        }
    }

    /// The source for `credentials` as `resolver` resolved them for
    /// `account`. An OAuth credential gets the store's refresher, asked for
    /// with `platform` as the vendor kind, so an account name the resolver
    /// was never told about (its `get` hands such a token back unrefreshed)
    /// still refreshes against the right vendor.
    pub fn resolved(
        resolver: &dyn rupu_auth::CredentialResolver,
        platform: Platform,
        account: &str,
        credentials: AuthCredentials,
    ) -> Self {
        let refresher = match &credentials {
            AuthCredentials::OAuth { .. } => resolver.oauth_refresher(account, platform.as_str()),
            AuthCredentials::ApiKey { .. } => None,
        };
        Self::new(platform, account, credentials, refresher)
    }

    /// The token to send now. An OAuth token inside the expiry margin
    /// ([`rupu_providers::auth::is_token_expired`]'s five minutes — a
    /// clone carries the token in its URL and must not outlive it) is
    /// refreshed through the store first. Cancel-safe: the store's refresh
    /// runs as its own task, so a request dropped mid-refresh leaves the
    /// rotation persisted, and the next call adopts it from the store.
    pub async fn token(&self) -> Result<String, ScmError> {
        let mut current = self.current.lock().await;
        let (access, expires) = match &*current {
            AuthCredentials::ApiKey { key } => return Ok(key.clone()),
            AuthCredentials::OAuth {
                access, expires, ..
            } => (access, *expires),
        };
        let Some(refresher) = self
            .refresher
            .as_ref()
            .filter(|_| rupu_providers::auth::is_token_expired(expires))
        else {
            return Ok(access.clone());
        };
        let fresh = refresher
            .refresh(current.clone())
            .await
            .map_err(|e| self.refresh_failed(e))?;
        *current = fresh;
        match &*current {
            AuthCredentials::OAuth { access, .. } => Ok(access.clone()),
            AuthCredentials::ApiKey { key } => Ok(key.clone()),
        }
    }

    /// A refresh that failed (a revoked grant, the store's lock held, the
    /// token endpoint down) leaves no usable token: an authorization
    /// failure, with the command that gets a new one.
    fn refresh_failed(&self, e: rupu_providers::ProviderError) -> ScmError {
        let account = &self.account;
        ScmError::Unauthorized {
            platform: self.platform.as_str().to_string(),
            hint: format!(
                "the '{account}' OAuth token expired and could not be refreshed ({e}); run: \
                 rupu auth login --account {account} --mode sso"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    fn now_ms() -> u64 {
        chrono::Utc::now().timestamp_millis() as u64
    }

    fn oauth(access: &str, refresh: &str, expires: u64) -> AuthCredentials {
        AuthCredentials::OAuth {
            access: access.into(),
            refresh: refresh.into(),
            expires,
            extra: Default::default(),
        }
    }

    /// Stands in for the store's refresher: records the stale credential
    /// each refresh was asked for, and answers with `fresh` (or an error).
    struct FakeRefresher {
        seen: Mutex<Vec<AuthCredentials>>,
        fresh: Result<AuthCredentials, String>,
        delay: Duration,
    }

    impl FakeRefresher {
        fn answering(fresh: AuthCredentials) -> Arc<Self> {
            Arc::new(Self {
                seen: Mutex::new(Vec::new()),
                fresh: Ok(fresh),
                delay: Duration::ZERO,
            })
        }

        fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl OAuthRefresher for FakeRefresher {
        async fn refresh(
            &self,
            stale: AuthCredentials,
        ) -> Result<AuthCredentials, rupu_providers::ProviderError> {
            self.seen.lock().unwrap().push(stale);
            tokio::time::sleep(self.delay).await;
            self.fresh
                .clone()
                .map_err(rupu_providers::ProviderError::TokenRefreshFailed)
        }
    }

    fn source(creds: AuthCredentials, refresher: &Arc<FakeRefresher>) -> TokenSource {
        TokenSource::new(
            Platform::Gitlab,
            "gl-work",
            creds,
            Some(refresher.clone() as Arc<dyn OAuthRefresher>),
        )
    }

    #[tokio::test]
    async fn a_token_far_from_expiry_is_used_as_is() {
        let refresher = FakeRefresher::answering(oauth("a2", "r2", now_ms() + 7_200_000));
        let src = source(oauth("a1", "r1", now_ms() + 3_600_000), &refresher);

        assert_eq!(src.token().await.unwrap(), "a1");
        assert_eq!(refresher.calls(), 0);
    }

    /// Within the expiry margin the token is refreshed before it is handed
    /// out, through the store's refresher (given the credential the
    /// connector holds, so the store can tell whether someone else already
    /// rotated it), and the fresh token is kept for the next request.
    #[tokio::test]
    async fn an_expiring_token_is_refreshed_through_the_store_before_use() {
        let refresher = FakeRefresher::answering(oauth("a2", "r2", now_ms() + 7_200_000));
        let src = source(oauth("a1", "r1", now_ms() + 60_000), &refresher);

        assert_eq!(src.token().await.unwrap(), "a2");
        assert_eq!(src.token().await.unwrap(), "a2");
        assert_eq!(refresher.calls(), 1, "the fresh token is reused");
        let handed = refresher.seen.lock().unwrap()[0].clone();
        match handed {
            AuthCredentials::OAuth {
                access, refresh, ..
            } => assert_eq!((access.as_str(), refresh.as_str()), ("a1", "r1")),
            other => panic!("refresher was handed {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_already_expired_token_is_refreshed_before_use() {
        let refresher = FakeRefresher::answering(oauth("a2", "r2", now_ms() + 7_200_000));
        let src = source(oauth("a1", "r1", now_ms() - 60_000), &refresher);

        assert_eq!(src.token().await.unwrap(), "a2");
        assert_eq!(refresher.calls(), 1);
    }

    #[tokio::test]
    async fn an_access_token_is_never_refreshed() {
        let refresher = FakeRefresher::answering(oauth("a2", "r2", now_ms() + 7_200_000));
        let src = source(
            AuthCredentials::ApiKey {
                key: "glpat-x".into(),
            },
            &refresher,
        );

        assert_eq!(src.token().await.unwrap(), "glpat-x");
        assert_eq!(refresher.calls(), 0);
    }

    /// `expires: 0` is the store's "no expiry" (a GitHub OAuth-App token).
    #[tokio::test]
    async fn an_oauth_token_without_an_expiry_is_never_refreshed() {
        let refresher = FakeRefresher::answering(oauth("a2", "r2", now_ms() + 7_200_000));
        let src = source(oauth("a1", "r1", 0), &refresher);

        assert_eq!(src.token().await.unwrap(), "a1");
        assert_eq!(refresher.calls(), 0);
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_refresh() {
        let refresher = Arc::new(FakeRefresher {
            seen: Mutex::new(Vec::new()),
            fresh: Ok(oauth("a2", "r2", now_ms() + 7_200_000)),
            delay: Duration::from_millis(50),
        });
        let src = Arc::new(source(oauth("a1", "r1", now_ms() - 1), &refresher));

        // Spawned all at once (collected before any is awaited), so the
        // eight requests really are in flight together.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let src = src.clone();
                tokio::spawn(async move { src.token().await.unwrap() })
            })
            .collect();
        let mut tokens = Vec::new();
        for h in handles {
            tokens.push(h.await.unwrap());
        }

        assert!(tokens.iter().all(|t| t == "a2"), "{tokens:?}");
        assert_eq!(refresher.calls(), 1);
    }

    /// A refresh that fails (revoked grant, store locked, endpoint down) is
    /// an authorization failure the user can act on, naming the account.
    #[tokio::test]
    async fn a_failed_refresh_is_unauthorized_and_names_the_login_command() {
        let refresher = Arc::new(FakeRefresher {
            seen: Mutex::new(Vec::new()),
            fresh: Err("refresh failed for 'gl-work': HTTP 400".into()),
            delay: Duration::ZERO,
        });
        let src = source(oauth("a1", "r1", now_ms() - 1), &refresher);

        match src.token().await {
            Err(ScmError::Unauthorized { platform, hint }) => {
                assert_eq!(platform, "gitlab");
                assert!(hint.contains("HTTP 400"), "{hint}");
                assert!(
                    hint.contains("rupu auth login --account gl-work --mode sso"),
                    "{hint}"
                );
            }
            other => panic!("expected Unauthorized, got {other:?}"),
        }
    }

    /// Without a refresher there is nothing to refresh with: the token goes
    /// out as it is and the server's answer decides.
    #[tokio::test]
    async fn without_a_refresher_an_expiring_token_is_used_as_is() {
        let src = TokenSource::new(
            Platform::Gitlab,
            "gl-work",
            oauth("a1", "r1", now_ms() - 1),
            None,
        );

        assert_eq!(src.token().await.unwrap(), "a1");
    }
}
