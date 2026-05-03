//! Per-provider OAuth metadata. All values are public client IDs (not
//! secrets); they're embedded in the rupu binary the same way `gh`
//! embeds its client ID.
//!
//! IMPORTANT: client IDs are vendor-controlled and may change. Validate
//! during smoke tests; if a vendor rotates a client ID, ship a patch.

use crate::backend::ProviderId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthFlow {
    /// Browser-callback PKCE flow.
    Callback,
    /// Device-code polling flow.
    Device,
}

#[derive(Debug, Clone)]
pub struct ProviderOAuth {
    pub flow: OAuthFlow,
    pub client_id: &'static str,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub device_url: Option<&'static str>, // device-code only
    pub scopes: &'static [&'static str],
    pub redirect_path: &'static str, // local listener path, e.g. "/callback"
}

pub fn provider_oauth(p: ProviderId) -> Option<ProviderOAuth> {
    match p {
        ProviderId::Anthropic => Some(ProviderOAuth {
            flow: OAuthFlow::Callback,
            // Anthropic's official OAuth client_id; verify before each release.
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            authorize_url: "https://claude.ai/oauth/authorize",
            token_url: "https://console.anthropic.com/v1/oauth/token",
            device_url: None,
            scopes: &["org:create_api_key", "user:profile", "user:inference"],
            redirect_path: "/callback",
        }),
        ProviderId::Openai => Some(ProviderOAuth {
            flow: OAuthFlow::Callback,
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            authorize_url: "https://auth.openai.com/oauth/authorize",
            token_url: "https://auth.openai.com/oauth/token",
            device_url: None,
            scopes: &["openid", "profile", "email", "offline_access"],
            redirect_path: "/callback",
        }),
        ProviderId::Gemini => Some(ProviderOAuth {
            flow: OAuthFlow::Callback,
            client_id: "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            device_url: None,
            scopes: &[
                "https://www.googleapis.com/auth/cloud-platform",
                "openid",
                "email",
            ],
            redirect_path: "/callback",
        }),
        ProviderId::Copilot => Some(ProviderOAuth {
            flow: OAuthFlow::Device,
            client_id: "Iv1.b507a08c87ecfe98", // GitHub Copilot's public client_id
            authorize_url: "",                 // unused for device flow
            token_url: "https://github.com/login/oauth/access_token",
            device_url: Some("https://github.com/login/device/code"),
            scopes: &["read:user"],
            redirect_path: "",
        }),
        ProviderId::Local => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_supported_provider_has_metadata() {
        for p in [
            ProviderId::Anthropic,
            ProviderId::Openai,
            ProviderId::Gemini,
            ProviderId::Copilot,
        ] {
            let cfg = provider_oauth(p).unwrap_or_else(|| panic!("missing oauth config for {p}"));
            assert!(!cfg.client_id.is_empty(), "{p}: empty client_id");
        }
    }

    #[test]
    fn local_has_no_oauth() {
        assert!(provider_oauth(ProviderId::Local).is_none());
    }

    #[test]
    fn copilot_uses_device_flow() {
        let c = provider_oauth(ProviderId::Copilot).unwrap();
        assert_eq!(c.flow, OAuthFlow::Device);
        assert!(c.device_url.is_some());
    }

    #[test]
    fn callback_providers_have_no_device_url() {
        for p in [
            ProviderId::Anthropic,
            ProviderId::Openai,
            ProviderId::Gemini,
        ] {
            let c = provider_oauth(p).unwrap();
            assert_eq!(c.flow, OAuthFlow::Callback);
            assert!(c.device_url.is_none());
        }
    }
}
