//! Account-key layout for the credential store.
//!
//! These strings are the **on-disk key format** of `auth.json`: each
//! provider × mode gets its own entry, e.g. `anthropic/api-key` and
//! `anthropic/sso`. Changing them orphans every existing user's
//! credentials, so treat this module as a compatibility contract.
//!
//! Originally (Slice B-1 spec §9b) these were keychain account names,
//! which is why the shape looks the way it does — the keychain backend
//! is retired but the key format outlived it, because the file backend
//! was built to use the same strings. The Slice A layout used the bare
//! provider name; [`legacy_account_for`] keeps that readable so
//! credentials written before the mode suffix are not lost.

use rupu_providers::AuthMode;

use crate::backend::ProviderId;

/// Current key: `<provider>/<mode>`.
pub fn account_for(provider: ProviderId, mode: AuthMode) -> String {
    format!("{}/{}", provider.as_str(), mode.as_str())
}

/// Legacy single-mode key from Slice A; read-side only. Any value found
/// under it is treated as an API key.
pub fn legacy_account_for(provider: ProviderId) -> String {
    provider.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_for_separates_modes() {
        let api = account_for(ProviderId::Anthropic, AuthMode::ApiKey);
        let sso = account_for(ProviderId::Anthropic, AuthMode::Sso);
        assert_ne!(api, sso);
        assert_eq!(api, "anthropic/api-key");
        assert_eq!(sso, "anthropic/sso");
    }

    #[test]
    fn legacy_account_keeps_old_shape() {
        assert_eq!(legacy_account_for(ProviderId::Openai), "openai");
    }

    /// The exact strings are a compatibility contract with every
    /// `auth.json` already on disk. If this test needs updating, you are
    /// about to strand people's credentials.
    #[test]
    fn key_format_is_stable_across_providers() {
        assert_eq!(
            account_for(ProviderId::Github, AuthMode::ApiKey),
            "github/api-key"
        );
        assert_eq!(account_for(ProviderId::Gemini, AuthMode::Sso), "gemini/sso");
    }
}
