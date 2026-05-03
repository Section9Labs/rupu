//! `rupu auth login | logout | status`.

use crate::paths;
use clap::Subcommand;
use rupu_auth::{AuthBackend, ProbeCache, ProviderId};
use std::io::Read;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Store credentials for a provider.
    Login {
        /// Provider name (anthropic | openai | gemini | copilot | local).
        #[arg(long)]
        provider: String,
        /// Authentication mode.
        #[arg(long, value_enum, default_value = "api-key")]
        mode: AuthModeArg,
        /// API key (only valid with --mode api-key). If omitted, reads from stdin.
        #[arg(long)]
        key: Option<String>,
    },
    /// Remove a stored credential.
    Logout {
        #[arg(long)]
        provider: String,
    },
    /// Show configured providers + backend.
    Status,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum AuthModeArg {
    #[clap(name = "api-key")]
    ApiKey,
    Sso,
}

impl From<AuthModeArg> for rupu_providers::AuthMode {
    fn from(a: AuthModeArg) -> Self {
        match a {
            AuthModeArg::ApiKey => Self::ApiKey,
            AuthModeArg::Sso => Self::Sso,
        }
    }
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Login {
            provider,
            mode,
            key,
        } => login(&provider, mode, key.as_deref()).await,
        Action::Logout { provider } => logout(&provider).await,
        Action::Status => status().await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu auth: {e}");
            ExitCode::from(1)
        }
    }
}

fn parse_provider(s: &str) -> anyhow::Result<ProviderId> {
    match s {
        "anthropic" => Ok(ProviderId::Anthropic),
        "openai" => Ok(ProviderId::Openai),
        "gemini" => Ok(ProviderId::Gemini),
        "copilot" => Ok(ProviderId::Copilot),
        "local" => Ok(ProviderId::Local),
        _ => Err(anyhow::anyhow!("unknown provider: {s}")),
    }
}

async fn login(provider: &str, mode: AuthModeArg, key: Option<&str>) -> anyhow::Result<()> {
    let pid = parse_provider(provider)?;
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    let mode_neutral: rupu_providers::AuthMode = mode.clone().into();
    match mode {
        AuthModeArg::ApiKey => {
            let secret = match key {
                Some(k) => k.to_string(),
                None => {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf.trim().to_string()
                }
            };
            if secret.is_empty() {
                anyhow::bail!("empty API key");
            }
            let sc = rupu_auth::stored::StoredCredential::api_key(secret);
            resolver.store(pid, mode_neutral, &sc).await?;
            println!("rupu: stored {provider} api-key credential");
        }
        AuthModeArg::Sso => {
            let oauth = rupu_auth::oauth::providers::provider_oauth(pid)
                .ok_or_else(|| anyhow::anyhow!("provider {provider} has no SSO flow"))?;
            let stored = match oauth.flow {
                rupu_auth::oauth::providers::OAuthFlow::Callback => {
                    rupu_auth::oauth::callback::run(pid).await?
                }
                rupu_auth::oauth::providers::OAuthFlow::Device => {
                    rupu_auth::oauth::device::run(pid).await?
                }
            };
            resolver.store(pid, mode_neutral, &stored).await?;
            println!("rupu: stored {provider} sso credential");
        }
    }
    Ok(())
}

async fn logout(provider: &str) -> anyhow::Result<()> {
    let pid = parse_provider(provider)?;
    let backend = backend_for_global()?;
    backend.forget(pid)?;
    println!("rupu: forgot credential for {provider}");
    Ok(())
}

async fn status() -> anyhow::Result<()> {
    let backend = backend_for_global()?;
    println!("backend: {}", backend.name());
    for p in [
        ProviderId::Anthropic,
        ProviderId::Openai,
        ProviderId::Gemini,
        ProviderId::Copilot,
        ProviderId::Local,
    ] {
        let configured = backend.retrieve(p).is_ok();
        println!(
            "{:<10} {}",
            p.as_str(),
            if configured { "configured" } else { "-" }
        );
    }
    Ok(())
}

fn backend_for_global() -> anyhow::Result<Box<dyn AuthBackend>> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let cache = ProbeCache::new(global.join("cache/auth-backend.json"));
    let auth_json = global.join("auth.json");
    Ok(rupu_auth::select_backend(&cache, auth_json))
}

#[cfg(test)]
mod parse_provider_tests {
    use super::*;

    #[test]
    fn recognizes_all_four_providers() {
        assert_eq!(parse_provider("anthropic").unwrap(), ProviderId::Anthropic);
        assert_eq!(parse_provider("openai").unwrap(), ProviderId::Openai);
        assert_eq!(parse_provider("gemini").unwrap(), ProviderId::Gemini);
        assert_eq!(parse_provider("copilot").unwrap(), ProviderId::Copilot);
    }

    #[test]
    fn rejects_unknown() {
        assert!(parse_provider("typo").is_err());
    }
}
