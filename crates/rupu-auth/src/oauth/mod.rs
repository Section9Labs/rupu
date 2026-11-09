//! OAuth flows for Slice B-1 SSO. Four shapes:
//!
//! - `callback`: PKCE browser-redirect flow (Anthropic, OpenAI, Gemini, GitLab).
//! - `device`: device-code polling flow (GitHub Copilot, GitLab).
//! - `openai_device`: OpenAI's device auth (Codex CLI's `--device-auth`),
//!   `--mode device` for OpenAI.
//! - `paste`: authorization code shown on a vendor-hosted page and pasted
//!   back (Claude Code's headless login), `--mode device` for Anthropic and
//!   Gemini.
//!
//! `common` is what they share: the token response, the stored credential,
//! the HTTP client, the authorize URL and the code exchange.

pub mod callback;
mod common;
pub mod device;
pub mod gemini;
pub mod gitlab;
pub mod openai_device;
pub mod paste;
pub mod pkce;
pub mod providers;

pub use providers::{provider_oauth, HeadlessFlow, OAuthFlow, ProviderOAuth};
