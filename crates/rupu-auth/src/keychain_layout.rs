//! Construct the (service, account) tuple used as the keychain key.
//!
//! Slice B-1 spec §9b: each provider × mode is its own entry, e.g.
//! `rupu/anthropic/api-key` and `rupu/anthropic/sso`. The legacy Slice
//! A layout used `rupu` as the service and the provider name as the
//! account. We keep that for backwards compat at read time but write
//! the new shape going forward.

use rupu_providers::AuthMode;

use crate::backend::ProviderId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainKey {
    pub service: String,
    pub account: String,
}

pub fn key_for(provider: ProviderId, mode: AuthMode) -> KeychainKey {
    KeychainKey {
        service: "rupu".into(),
        account: format!("{}/{}", provider.as_str(), mode.as_str()),
    }
}

/// Legacy single-mode key from Slice A; only used for read-side
/// compatibility (treat any value found here as API-key for migration).
pub fn legacy_key_for(provider: ProviderId) -> KeychainKey {
    KeychainKey {
        service: "rupu".into(),
        account: provider.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_for_separates_modes() {
        let api = key_for(ProviderId::Anthropic, AuthMode::ApiKey);
        let sso = key_for(ProviderId::Anthropic, AuthMode::Sso);
        assert_ne!(api.account, sso.account);
        assert_eq!(api.account, "anthropic/api-key");
        assert_eq!(sso.account, "anthropic/sso");
    }

    #[test]
    fn legacy_key_keeps_old_shape() {
        let k = legacy_key_for(ProviderId::Openai);
        assert_eq!(k.account, "openai");
    }
}
