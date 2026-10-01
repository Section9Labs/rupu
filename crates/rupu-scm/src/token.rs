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
use std::time::{Duration, Instant};

use rupu_providers::auth::AuthCredentials;
use rupu_providers::credential_writes::OAuthRefresher;

use crate::error::ScmError;
use crate::platform::Platform;

/// How long a token that is still good keeps being used, unrefreshed, after
/// a failed refresh before the next attempt.
const RETRY_AFTER_FAILED_REFRESH: Duration = Duration::from_secs(30);

pub struct TokenSource {
    platform: Platform,
    /// The configured account name, for the re-login hint.
    account: String,
    /// Held across a refresh, so concurrent requests on one connector wait
    /// for the one refresh instead of each starting their own.
    state: tokio::sync::Mutex<State>,
    refresher: Option<Arc<dyn OAuthRefresher>>,
}

struct State {
    credentials: AuthCredentials,
    /// Set by a failed refresh of a token that was still good: no new
    /// attempt before then.
    retry_after: Option<Instant>,
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
            state: tokio::sync::Mutex::new(State {
                credentials,
                retry_after: None,
            }),
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
    /// refreshed through the store first; two rounds at most, since a
    /// rotation adopted from the store can itself be inside the margin.
    ///
    /// A failed refresh of a token that has not actually expired yet is
    /// not an error: the token is used (and the refresh retried after
    /// [`RETRY_AFTER_FAILED_REFRESH`]) until it does expire.
    ///
    /// Cancel-safe: the store's refresh runs as its own task, so a request
    /// dropped mid-refresh leaves the rotation persisted, and the next call
    /// adopts it from the store.
    pub async fn token(&self) -> Result<String, ScmError> {
        let mut state = self.state.lock().await;
        for _ in 0..2 {
            let AuthCredentials::OAuth {
                access, expires, ..
            } = &state.credentials
            else {
                break;
            };
            let (access, expires) = (access.clone(), *expires);
            // Only a token that still works is spared a retry.
            let backing_off =
                !has_expired(expires) && state.retry_after.is_some_and(|t| Instant::now() < t);
            let Some(refresher) = self
                .refresher
                .as_ref()
                .filter(|_| rupu_providers::auth::is_token_expired(expires) && !backing_off)
            else {
                break;
            };
            match refresher.refresh(state.credentials.clone()).await {
                Ok(fresh) => {
                    state.credentials = fresh;
                    state.retry_after = None;
                }
                Err(e) if !has_expired(expires) => {
                    tracing::warn!(
                        account = %self.account,
                        error = %e,
                        "OAuth token refresh failed; using the current token until it expires"
                    );
                    state.retry_after = Some(Instant::now() + RETRY_AFTER_FAILED_REFRESH);
                    return Ok(access);
                }
                Err(e) => return Err(self.refresh_failed("expired", e)),
            }
        }
        Ok(access_of(&state.credentials))
    }

    /// The server rejected `rejected` (a 401). For an OAuth token, the
    /// replacement to retry with: the one already taken in its place, or a
    /// fresh one through the store's refresher — which adopts a credential
    /// stored since (a `rupu auth login`, another process's refresh) and
    /// otherwise refreshes. `None` when there is nothing to retry with (an
    /// access token, no refresher).
    pub async fn replacement_for(&self, rejected: &str) -> Result<Option<String>, ScmError> {
        let mut state = self.state.lock().await;
        let AuthCredentials::OAuth { access, .. } = &state.credentials else {
            return Ok(None);
        };
        if access != rejected {
            return Ok(Some(access.clone()));
        }
        let Some(refresher) = &self.refresher else {
            return Ok(None);
        };
        state.credentials = refresher
            .refresh(state.credentials.clone())
            .await
            .map_err(|e| self.refresh_failed("was rejected", e))?;
        state.retry_after = None;
        let fresh = access_of(&state.credentials);
        Ok((fresh != rejected).then_some(fresh))
    }

    /// A refresh that failed (a revoked grant, the store's lock held, the
    /// token endpoint down) with no usable token left: an authorization
    /// failure, with the command that gets a new one.
    fn refresh_failed(&self, why: &str, e: rupu_providers::ProviderError) -> ScmError {
        let account = &self.account;
        ScmError::Unauthorized {
            platform: self.platform.as_str().to_string(),
            hint: format!(
                "the '{account}' OAuth token {why} and could not be refreshed ({e}); run: \
                 rupu auth login --account {account} --mode sso"
            ),
        }
    }
}

fn access_of(credentials: &AuthCredentials) -> String {
    match credentials {
        AuthCredentials::OAuth { access, .. } => access.clone(),
        AuthCredentials::ApiKey { key } => key.clone(),
    }
}

/// Past `expires_ms` (absolute ms since the epoch; `0` = no expiry) with no
/// margin: the token no longer works at all.
fn has_expired(expires_ms: u64) -> bool {
    expires_ms != 0 && chrono::Utc::now().timestamp_millis() as u64 >= expires_ms
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
    /// each refresh was asked for, and answers from `answers` in order (the
    /// last one repeating).
    struct FakeRefresher {
        seen: Mutex<Vec<AuthCredentials>>,
        answers: Mutex<Vec<Result<AuthCredentials, String>>>,
        delay: Duration,
    }

    impl FakeRefresher {
        fn answering(fresh: AuthCredentials) -> Arc<Self> {
            Self::with_answers(vec![Ok(fresh)])
        }

        fn failing(error: &str) -> Arc<Self> {
            Self::with_answers(vec![Err(error.to_string())])
        }

        fn with_answers(answers: Vec<Result<AuthCredentials, String>>) -> Arc<Self> {
            Arc::new(Self {
                seen: Mutex::new(Vec::new()),
                answers: Mutex::new(answers),
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
            let answer = {
                let mut answers = self.answers.lock().unwrap();
                if answers.len() > 1 {
                    answers.remove(0)
                } else {
                    answers[0].clone()
                }
            };
            answer.map_err(rupu_providers::ProviderError::TokenRefreshFailed)
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
            answers: Mutex::new(vec![Ok(oauth("a2", "r2", now_ms() + 7_200_000))]),
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
        let refresher = FakeRefresher::failing("refresh failed for 'gl-work': HTTP 400");
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

    /// The refresh starts five minutes early. A token that is still good
    /// when its refresh fails (the token endpoint down for a moment, the
    /// store's lock held by a stalled process) keeps being used instead of
    /// failing a request that would have worked.
    #[tokio::test]
    async fn a_failed_refresh_of_a_token_that_still_works_falls_back_to_it() {
        let refresher = FakeRefresher::failing("refresh request: connection refused");
        let src = source(oauth("a1", "r1", now_ms() + 180_000), &refresher);

        assert_eq!(src.token().await.unwrap(), "a1");
        assert_eq!(refresher.calls(), 1);
    }

    /// A refresh that keeps failing (an outage) is not retried on every
    /// request: each attempt can wait out the store's lock or the token
    /// endpoint's timeout, and requests on the connector queue behind it.
    #[tokio::test]
    async fn a_failed_refresh_is_not_retried_on_the_very_next_request() {
        let refresher = FakeRefresher::failing("refresh request: connection refused");
        let src = source(oauth("a1", "r1", now_ms() + 180_000), &refresher);

        assert_eq!(src.token().await.unwrap(), "a1");
        assert_eq!(src.token().await.unwrap(), "a1");
        assert_eq!(refresher.calls(), 1);
    }

    /// The backoff only spares a token that still works: once it has
    /// expired, the next request tries the refresh again.
    #[tokio::test]
    async fn an_expired_token_is_refreshed_again_despite_an_earlier_failure() {
        let refresher = FakeRefresher::failing("refresh request: connection refused");
        let src = source(oauth("a1", "r1", now_ms() + 50), &refresher);

        assert_eq!(src.token().await.unwrap(), "a1");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(
            src.token().await,
            Err(ScmError::Unauthorized { .. })
        ));
        assert_eq!(refresher.calls(), 2);
    }

    /// An adopted rotation can itself be inside the margin (another process
    /// refreshed long ago); it is refreshed once more rather than handed to
    /// a clone that may outlive it.
    #[tokio::test]
    async fn an_adopted_token_still_inside_the_margin_is_refreshed_once_more() {
        let refresher = FakeRefresher::with_answers(vec![
            Ok(oauth("a2", "r2", now_ms() + 180_000)),
            Ok(oauth("a3", "r3", now_ms() + 7_200_000)),
        ]);
        let src = source(oauth("a1", "r1", now_ms() - 1), &refresher);

        assert_eq!(src.token().await.unwrap(), "a3");
        assert_eq!(refresher.calls(), 2);
    }

    /// The server rejected a token the store has since replaced (a
    /// `rupu auth login`, another process's refresh): the replacement comes
    /// from the store, via the refresher, which adopts it.
    #[tokio::test]
    async fn a_rejected_token_is_replaced_from_the_store() {
        let refresher = FakeRefresher::answering(oauth("a2", "r2", now_ms() + 7_200_000));
        let src = source(oauth("a1", "r1", now_ms() + 3_600_000), &refresher);

        assert_eq!(
            src.replacement_for("a1").await.unwrap().as_deref(),
            Some("a2")
        );
        assert_eq!(src.token().await.unwrap(), "a2", "and kept");
        assert_eq!(refresher.calls(), 1);
    }

    /// Two requests rejected with the same token: the second finds it
    /// already replaced and takes the replacement without another refresh.
    #[tokio::test]
    async fn a_token_already_replaced_is_not_replaced_again() {
        let refresher = FakeRefresher::answering(oauth("a2", "r2", now_ms() + 7_200_000));
        let src = source(oauth("a1", "r1", now_ms() + 3_600_000), &refresher);

        src.replacement_for("a1").await.unwrap();
        assert_eq!(
            src.replacement_for("a1").await.unwrap().as_deref(),
            Some("a2")
        );
        assert_eq!(refresher.calls(), 1);
    }

    #[tokio::test]
    async fn a_rejected_access_token_has_no_replacement() {
        let refresher = FakeRefresher::answering(oauth("a2", "r2", now_ms() + 7_200_000));
        let src = source(
            AuthCredentials::ApiKey {
                key: "glpat-x".into(),
            },
            &refresher,
        );

        assert_eq!(src.replacement_for("glpat-x").await.unwrap(), None);
        assert_eq!(refresher.calls(), 0);
    }

    /// The server already refused the token, so a failed refresh has
    /// nothing to fall back to.
    #[tokio::test]
    async fn a_rejected_token_whose_refresh_fails_is_unauthorized() {
        let refresher = FakeRefresher::failing("refresh failed for 'gl-work': HTTP 400");
        let src = source(oauth("a1", "r1", now_ms() + 3_600_000), &refresher);

        assert!(matches!(
            src.replacement_for("a1").await,
            Err(ScmError::Unauthorized { .. })
        ));
    }
}
