//! OAuth flows for Slice B-1 SSO. Two shapes:
//!
//! - `callback`: PKCE browser-redirect flow (Anthropic, OpenAI, Gemini, GitLab).
//! - `device`: device-code polling flow (GitHub Copilot).

pub mod callback;
pub mod device;
pub mod gitlab;
pub mod pkce;
pub mod providers;

pub use providers::{provider_oauth, OAuthFlow, ProviderOAuth};
