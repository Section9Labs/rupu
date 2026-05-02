//! CredentialSource trait — the unified interface for credential management.
//!
//! All providers access credentials through this trait. The concrete
//! implementation (`CredentialStore`) backs it with `cortex/auth.json`.

use std::time::{Duration, Instant};

use crate::auth::AuthCredentials;
use crate::error::ProviderError;
use crate::provider_id::ProviderId;

/// Status of a provider's credentials in the store.
#[derive(Debug, Clone)]
pub enum ProviderAuthStatus {
    /// Not configured — no credentials in the store.
    NotConfigured,
    /// Configured and available for use.
    Available { expires_ms: Option<u64> },
    /// Temporarily invalidated — credentials exist but marked unusable.
    /// Cleared implicitly when update() succeeds or reload() detects new credentials.
    Invalidated {
        reason: String,
        since: Instant,
        retry_after: Option<Duration>,
    },
}

impl ProviderAuthStatus {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// Unified credential management interface.
///
/// Providers call `get()` to retrieve credentials and `update()` after
/// refreshing tokens. The store handles persistence, file locking, and
/// lifecycle tracking.
///
/// `get()` returns credentials regardless of invalidation state — providers
/// need the refresh token to recover even when invalidated. Use `available()`
/// or `status()` to check usability before making API calls.
pub trait CredentialSource: Send + Sync {
    /// Get current credentials for a provider. Returns None if not configured.
    /// Returns credentials REGARDLESS of invalidation state.
    fn get(&self, provider: ProviderId) -> Option<AuthCredentials>;

    /// Update credentials after a successful token refresh.
    /// Persists to disk atomically with file locking.
    /// Implicitly clears any invalidation state for this provider.
    fn update(&self, provider: ProviderId, creds: AuthCredentials) -> Result<(), ProviderError>;

    /// Mark a provider as temporarily unavailable.
    /// Persists invalidation state to auth_status.json.
    fn invalidate(
        &self,
        provider: ProviderId,
        reason: &str,
        retry_after: Option<Duration>,
    ) -> Result<(), ProviderError>;

    /// List all providers with valid, non-invalidated credentials.
    /// Automatically recovers providers whose retry_after window has passed.
    fn available(&self) -> Vec<ProviderId>;

    /// Get the status of a specific provider.
    fn status(&self, provider: ProviderId) -> ProviderAuthStatus;

    /// Re-read credentials from disk. Called by fsnotify or when the
    /// router exhausts all providers as a last resort.
    /// Clears invalidation for any provider whose credentials changed.
    fn reload(&self) -> Result<(), ProviderError>;
}
