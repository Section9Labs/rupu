//! OAuth flows for Slice B-1 SSO. Two shapes:
//!
//! - `callback`: PKCE browser-redirect flow (Anthropic, OpenAI, Gemini, GitLab).
//! - `device`: device-code polling flow (GitHub Copilot).
//!
//! `common` is what they share: the token response, the stored credential,
//! the HTTP client, the authorize URL and the code exchange.

pub mod callback;
mod common;
pub mod device;
pub mod gemini;
pub mod gitlab;
pub mod pkce;
pub mod providers;

pub use providers::{provider_oauth, HeadlessFlow, OAuthFlow, ProviderOAuth};
