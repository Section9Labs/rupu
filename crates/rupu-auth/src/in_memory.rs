//! In-process resolver for tests. Holds `StoredCredential`s in a
//! `RwLock<HashMap>` and lets tests inject a refresh callback.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::RwLock;

use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;

use crate::backend::ProviderId;
use crate::resolver::{CredentialResolver, EXPIRY_REFRESH_BUFFER_SECS};
use crate::stored::StoredCredential;

type RefreshFn =
    dyn Fn(ProviderId, AuthMode, &StoredCredential) -> Result<StoredCredential> + Send + Sync;

#[derive(Default)]
pub struct InMemoryResolver {
    inner: Arc<RwLock<HashMap<(ProviderId, AuthMode), StoredCredential>>>,
    refresh_cb: Arc<RwLock<Option<Box<RefreshFn>>>>,
}

impl InMemoryResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn put(&self, p: ProviderId, mode: AuthMode, sc: StoredCredential) {
        let mut g = self.inner.write().await;
        g.insert((p, mode), sc);
    }

    pub async fn set_refresh_callback<F>(&self, f: F)
    where
        F: Fn(ProviderId, AuthMode, &StoredCredential) -> Result<StoredCredential>
            + Send
            + Sync
            + 'static,
    {
        *self.refresh_cb.write().await = Some(Box::new(f));
    }

    fn parse_provider(name: &str) -> Result<ProviderId> {
        match name {
            "anthropic" => Ok(ProviderId::Anthropic),
            "openai" => Ok(ProviderId::Openai),
            "gemini" => Ok(ProviderId::Gemini),
            "copilot" => Ok(ProviderId::Copilot),
            "local" => Ok(ProviderId::Local),
            "github" => Ok(ProviderId::Github),
            "gitlab" => Ok(ProviderId::Gitlab),
            "linear" => Ok(ProviderId::Linear),
            other => Err(anyhow!("unknown provider: {other}")),
        }
    }
}

#[async_trait]
impl CredentialResolver for InMemoryResolver {
    async fn get(
        &self,
        provider: &str,
        hint: Option<AuthMode>,
    ) -> Result<(AuthMode, AuthCredentials)> {
        let p = Self::parse_provider(provider)?;
        let preferred: Vec<AuthMode> = match hint {
            Some(m) => vec![m],
            None => vec![AuthMode::Sso, AuthMode::ApiKey],
        };

        for mode in preferred {
            let mut sc_opt = {
                let g = self.inner.read().await;
                g.get(&(p, mode)).cloned()
            };
            if let Some(sc) = sc_opt.as_mut() {
                let now = chrono::Utc::now();
                if mode == AuthMode::Sso && sc.is_near_expiry(now, EXPIRY_REFRESH_BUFFER_SECS) {
                    if let Some(cb) = self.refresh_cb.read().await.as_ref() {
                        let new = (cb)(p, mode, sc)?;
                        let mut g = self.inner.write().await;
                        g.insert((p, mode), new.clone());
                        return Ok((mode, new.credentials));
                    } else if sc.is_expired(now) {
                        anyhow::bail!(
                            "{} SSO token expired and no refresh available. \
                             Run: rupu auth login --provider {} --mode sso",
                            provider,
                            provider
                        );
                    }
                }
                return Ok((mode, sc.credentials.clone()));
            }
        }
        Err(anyhow!(
            "no credentials configured for {provider}. \
             Run: rupu auth login --provider {provider} --mode <api-key|sso>"
        ))
    }

    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials> {
        let p = Self::parse_provider(provider)?;
        let sc_opt = { self.inner.read().await.get(&(p, mode)).cloned() };
        let sc = sc_opt.ok_or_else(|| anyhow!("no stored credential for {provider}/{mode:?}"))?;
        let cb_guard = self.refresh_cb.read().await;
        let cb = cb_guard
            .as_ref()
            .ok_or_else(|| anyhow!("refresh callback not set in test resolver"))?;
        let new = (cb)(p, mode, &sc)?;
        drop(cb_guard);
        self.inner.write().await.insert((p, mode), new.clone());
        Ok(new.credentials)
    }
}
