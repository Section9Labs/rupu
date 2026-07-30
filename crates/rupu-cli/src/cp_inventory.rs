//! `cp serve` adapter for rupu-cp's [`FleetInventory`] port.
//!
//! Probes every credentialled provider on a TTL and serves the result from an
//! in-memory cache. [`FleetInventory::snapshot`] never performs I/O — the
//! dashboard reads the cache, and only the background refresh task touches the
//! network, so one hung provider can never stall a page render.
//!
//! Providers are built as CONCRETE client types rather than through
//! `ProviderRegistry`'s `Box<dyn LlmProvider>`, mirroring `cmd/models.rs`'s
//! `populate_live`: `async_trait` imposes a `Sync` bound on `&self` methods of
//! a boxed trait object that the concrete types sidestep.

#![deny(clippy::all)]

use chrono::Utc;
use rupu_auth::CredentialResolver;
use rupu_cp::fleet_inventory::{FleetInventory, InventorySnapshot, ProbeState, ProviderProbeRow};
use rupu_providers::{error::ProviderError, provider::LlmProvider};
use std::sync::{Arc, RwLock};

/// How long a probe result stays authoritative. Long enough that a fleet of
/// providers costs a handful of requests an hour; short enough that a revoked
/// key turns the strip red within a coffee break.
pub const PROBE_TTL_SECS: u64 = 300;

/// The providers rupu knows how to authenticate. Same list `rupu models`
/// refreshes — one place to add a provider, not two.
const PROVIDERS: [&str; 4] = ["anthropic", "openai", "gemini", "copilot"];

pub struct CpFleetInventory {
    /// `None` in unit tests, which exercise the cache and `classify` without
    /// touching the keychain.
    resolver: Option<Arc<rupu_auth::resolver::KeychainResolver>>,
    cache: RwLock<InventorySnapshot>,
}

/// Map a probe error to a cache state.
///
/// `NotImplemented` is NOT a failure — it means this provider has no probe, so
/// nothing has been established about it and it must land in `NeverProbed`.
/// Everything auth-shaped is `AuthFailed`; everything transport- or
/// server-shaped is `Unreachable`. That split is what lets the operator tell
/// "my key is wrong" from "the provider is down".
pub fn classify(err: &ProviderError) -> ProbeState {
    match err {
        ProviderError::NotImplemented { .. } => ProbeState::NeverProbed,
        ProviderError::Unauthorized { .. }
        | ProviderError::MissingAuth { .. }
        | ProviderError::TokenRefreshFailed(_)
        | ProviderError::AuthConfig(_) => ProbeState::AuthFailed {
            detail: err.to_string(),
        },
        ProviderError::Api { status, .. } if *status == 401 || *status == 403 => {
            ProbeState::AuthFailed {
                detail: err.to_string(),
            }
        }
        _ => ProbeState::Unreachable {
            detail: err.to_string(),
        },
    }
}

impl CpFleetInventory {
    pub fn new(resolver: Arc<rupu_auth::resolver::KeychainResolver>) -> Self {
        Self {
            resolver: Some(resolver),
            cache: RwLock::new(InventorySnapshot::default()),
        }
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            resolver: None,
            cache: RwLock::new(InventorySnapshot::default()),
        }
    }

    /// Probe every credentialled provider and replace the cache.
    ///
    /// A provider whose credentials cannot be resolved is not "configured" at
    /// all and is omitted entirely — the strip counts what rupu can actually
    /// use. Errors from a provider that IS configured are recorded as states,
    /// never propagated: one dead provider must not blank the other rows.
    pub async fn refresh_providers(&self) {
        let Some(resolver) = &self.resolver else {
            return;
        };

        let mut rows = Vec::new();
        for name in PROVIDERS {
            // No credentials → not configured. Skip rather than record, so
            // `providers_configured` means "rupu can use this" rather than
            // "rupu has heard of this".
            let Ok((_mode, creds)) = resolver.get(name, None).await else {
                continue;
            };
            rows.push(ProviderProbeRow {
                provider: name.to_string(),
                state: probe_one(name, creds).await,
                probed_at: Some(Utc::now()),
            });
        }

        if let Ok(mut c) = self.cache.write() {
            c.providers = rows;
            c.captured_at = Some(Utc::now());
        }
    }
}

/// Probe a single provider by its concrete client type.
///
/// Only Anthropic implements a real `probe` today; the rest inherit the trait
/// default and therefore report `NeverProbed` — honestly "not checked", not a
/// green light. A client that cannot even be constructed is an auth/config
/// problem, which `classify` already names.
async fn probe_one(name: &str, creds: rupu_providers::auth::AuthCredentials) -> ProbeState {
    let result = match name {
        "anthropic" => {
            // Route OAuth through `from_auth` so the probe sends
            // `Authorization: Bearer` + the OAuth beta; stuffing an OAuth
            // token into the api-key constructor yields a 401 that would be
            // misreported as a bad credential (see `cmd/models.rs`).
            let auth_method = creds.into_anthropic_auth_method();
            rupu_providers::AnthropicClient::from_auth(auth_method)
                .probe()
                .await
        }
        "openai" => match rupu_providers::OpenAiCodexClient::new(creds, None) {
            Ok(c) => c.probe().await,
            Err(e) => Err(e),
        },
        "gemini" => match rupu_providers::GoogleGeminiClient::new(
            creds,
            rupu_providers::google_gemini::GeminiVariant::GeminiCli,
            None,
        ) {
            Ok(c) => c.probe().await,
            Err(e) => Err(e),
        },
        "copilot" => match rupu_providers::GithubCopilotClient::new(creds, None) {
            Ok(c) => c.probe().await,
            Err(e) => Err(e),
        },
        // Unreachable given `PROVIDERS`, but a new entry added there without a
        // arm here must report "not checked" rather than "healthy".
        _ => Err(ProviderError::NotImplemented {
            provider: name.to_string(),
        }),
    };

    match result {
        Ok(()) => ProbeState::Ok,
        // Rate limiting is NOT a health failure: a 429 proves the credential
        // works. Reporting it red would light the strip up during normal
        // heavy use.
        Err(ProviderError::RateLimited { .. }) => ProbeState::Ok,
        Err(e) => classify(&e),
    }
}

impl FleetInventory for CpFleetInventory {
    fn snapshot(&self) -> InventorySnapshot {
        self.cache.read().map(|c| c.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_shaped_errors_classify_as_auth_failed() {
        let cases = [
            ProviderError::Unauthorized {
                provider: "anthropic".into(),
                auth_mode: rupu_providers::auth_mode::AuthMode::ApiKey,
                hint: "check your key".into(),
            },
            ProviderError::MissingAuth {
                provider: "anthropic".into(),
                env_hint: "ANTHROPIC_API_KEY".into(),
            },
            ProviderError::TokenRefreshFailed("expired".into()),
            ProviderError::AuthConfig("bad auth.json".into()),
            ProviderError::Api {
                status: 401,
                message: "nope".into(),
            },
            ProviderError::Api {
                status: 403,
                message: "nope".into(),
            },
        ];
        for err in cases {
            assert!(
                matches!(classify(&err), ProbeState::AuthFailed { .. }),
                "{err:?} must classify as AuthFailed"
            );
        }
    }

    #[test]
    fn transport_and_server_errors_classify_as_unreachable() {
        let cases = [
            ProviderError::Http("connection refused".into()),
            ProviderError::Api {
                status: 503,
                message: "down".into(),
            },
        ];
        for err in cases {
            assert!(
                matches!(classify(&err), ProbeState::Unreachable { .. }),
                "{err:?} must classify as Unreachable"
            );
        }
    }

    /// A provider with no `probe()` impl must report NeverProbed — the whole
    /// point of the NotImplemented default. Classifying it as Unreachable
    /// would put a red count on a provider that may be perfectly fine.
    #[test]
    fn not_implemented_classifies_as_never_probed() {
        let err = ProviderError::NotImplemented {
            provider: "gemini".into(),
        };
        assert!(matches!(classify(&err), ProbeState::NeverProbed));
    }

    /// Before the first refresh the cache is empty, so the strip reports
    /// nothing about providers rather than "0 configured".
    #[test]
    fn snapshot_before_any_refresh_is_empty() {
        let inv = CpFleetInventory::new_for_test();
        let snap = inv.snapshot();
        assert!(snap.providers.is_empty());
        assert!(snap.captured_at.is_none());
    }

    /// Without a resolver (the unit-test shape) a refresh is a no-op rather
    /// than a panic or a fabricated empty-but-stamped cache.
    #[tokio::test]
    async fn refresh_without_a_resolver_leaves_the_cache_untouched() {
        let inv = CpFleetInventory::new_for_test();
        inv.refresh_providers().await;
        assert!(inv.snapshot().captured_at.is_none());
    }
}
