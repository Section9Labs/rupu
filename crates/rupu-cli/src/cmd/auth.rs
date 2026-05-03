//! `rupu auth login | logout | status`.

use crate::paths;
use clap::Subcommand;
use rupu_auth::{AuthBackend, ProbeCache, ProviderId};
use std::io::Read;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Store an API key for a provider.
    Login {
        /// Provider name (anthropic | openai | gemini | copilot | local).
        #[arg(long)]
        provider: String,
        /// API key. If omitted, reads from stdin.
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

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Login { provider, key } => login(&provider, key.as_deref()).await,
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

async fn login(provider: &str, key: Option<&str>) -> anyhow::Result<()> {
    let pid = parse_provider(provider)?;
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
    let backend = backend_for_global()?;
    backend.store(pid, &secret)?;
    println!(
        "rupu: stored credential for {provider} via {}",
        backend.name()
    );
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
