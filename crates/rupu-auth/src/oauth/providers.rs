//! Per-provider OAuth metadata. All values are public client IDs (not
//! secrets); they're embedded in the rupu binary the same way `gh`
//! embeds its client ID.
//!
//! IMPORTANT: client IDs are vendor-controlled and may change. Validate
//! during smoke tests; if a vendor rotates a client ID, ship a patch.
//!
//! ## Honest acknowledgements
//!
//! We currently impersonate two existing first-party CLI clients:
//!
//! - Anthropic: Claude Code's `9d1c250a-...` (consent screen reads
//!   "Claude Code wants access ..."). Scope set matches Claude Code so
//!   the consent UI is internally consistent.
//! - OpenAI: Codex CLI's `app_EMoamEEZ73f0CkXaXp7hrann`. Required port
//!   range (1455 / 1457) and `/auth/callback` path are pinned because
//!   they're allowlisted on OpenAI's Hydra registration for that
//!   client.
//!
//! Long-term we should register our own OAuth clients for rupu (see
//! `TODO.md`). Until then, mirroring the upstream CLI's request shape
//! is necessary for the flow to succeed.

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
    /// Host name to advertise in the redirect URI (`localhost` vs
    /// `127.0.0.1`). Some IdPs (notably OpenAI Hydra) only allowlist
    /// the literal "localhost" form.
    pub redirect_host: &'static str,
    /// Required redirect-listener ports. `None` = let the OS pick a
    /// free port (PKCE-loopback default). `Some(&[a, b, ...])` = try
    /// each in order; bind fails if none succeed (used for IdPs that
    /// allowlist specific ports).
    pub fixed_ports: Option<&'static [u16]>,
    /// Extra fixed query parameters appended to the authorize URL.
    /// Provider-specific signaling (e.g., Codex CLI flags).
    pub extra_authorize_params: &'static [(&'static str, &'static str)],
}

pub fn provider_oauth(p: ProviderId) -> Option<ProviderOAuth> {
    match p {
        ProviderId::Anthropic => Some(ProviderOAuth {
            flow: OAuthFlow::Callback,
            // Claude Code's official OAuth client_id; matches what we see
            // when impersonating its flow. See module-level note.
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            authorize_url: "https://claude.ai/oauth/authorize",
            token_url: "https://console.anthropic.com/v1/oauth/token",
            device_url: None,
            // Full Claude Code scope set so the consent screen we
            // surface matches what users already trust from Claude Code,
            // and so we have headroom for future MCP/session features
            // without forcing a re-login round.
            scopes: &[
                "user:inference",
                "user:profile",
                "user:sessions:claude_code",
                "user:mcp_servers",
            ],
            redirect_path: "/callback",
            redirect_host: "127.0.0.1",
            fixed_ports: None,
            extra_authorize_params: &[],
        }),
        ProviderId::Openai => Some(ProviderOAuth {
            flow: OAuthFlow::Callback,
            // Codex CLI's public client_id. Mirrored from
            // openai/codex codex-rs/login/src/auth/manager.rs.
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            authorize_url: "https://auth.openai.com/oauth/authorize",
            // NOTE: this is auth.openai.com, NOT api.openai.com. The
            // earlier rupu config pointed at console.anthropic.com/...
            // which was simply wrong.
            token_url: "https://auth.openai.com/oauth/token",
            device_url: None,
            scopes: &[
                "openid",
                "profile",
                "email",
                "offline_access",
                "api.connectors.read",
                "api.connectors.invoke",
            ],
            redirect_path: "/auth/callback",
            // Codex CLI uses "localhost" — the literal string is in
            // OpenAI's Hydra allowlist for this client.
            redirect_host: "localhost",
            // Hydra-registered ports. Try 1455 first, fall back to 1457
            // if it's already bound.
            fixed_ports: Some(&[1455, 1457]),
            extra_authorize_params: &[
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
                // Originator tag mirrored from Codex CLI; OpenAI's
                // analytics expect this. Mirroring the upstream value
                // for now since we're impersonating their client.
                ("originator", "codex_cli_rs"),
            ],
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
            redirect_host: "127.0.0.1",
            fixed_ports: None,
            extra_authorize_params: &[],
        }),
        ProviderId::Copilot => Some(ProviderOAuth {
            flow: OAuthFlow::Device,
            client_id: "Iv1.b507a08c87ecfe98", // GitHub Copilot's public client_id
            authorize_url: "",                 // unused for device flow
            token_url: "https://github.com/login/oauth/access_token",
            device_url: Some("https://github.com/login/device/code"),
            scopes: &["read:user"],
            redirect_path: "",
            redirect_host: "",
            fixed_ports: None,
            extra_authorize_params: &[],
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

    #[test]
    fn anthropic_carries_full_claude_code_scope_set() {
        let c = provider_oauth(ProviderId::Anthropic).unwrap();
        assert!(c.scopes.contains(&"user:inference"));
        assert!(c.scopes.contains(&"user:profile"));
        assert!(c.scopes.contains(&"user:sessions:claude_code"));
        assert!(c.scopes.contains(&"user:mcp_servers"));
        // The legacy "org:create_api_key" Console-flow scope must NOT
        // be present — it was the cause of the "Invalid request format"
        // rejection on claude.ai.
        assert!(!c.scopes.contains(&"org:create_api_key"));
    }

    #[test]
    fn openai_uses_codex_cli_request_shape() {
        let c = provider_oauth(ProviderId::Openai).unwrap();
        assert_eq!(c.token_url, "https://auth.openai.com/oauth/token");
        assert_eq!(c.redirect_host, "localhost");
        assert_eq!(c.fixed_ports, Some(&[1455u16, 1457u16][..]));
        assert_eq!(c.redirect_path, "/auth/callback");
        assert!(c.scopes.contains(&"api.connectors.invoke"));
        let extras: std::collections::HashMap<_, _> =
            c.extra_authorize_params.iter().copied().collect();
        assert_eq!(extras.get("id_token_add_organizations"), Some(&"true"));
        assert_eq!(extras.get("codex_cli_simplified_flow"), Some(&"true"));
        assert_eq!(extras.get("originator"), Some(&"codex_cli_rs"));
    }
}
