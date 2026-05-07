//! `rupu auth login | logout | status`.

use clap::Subcommand;
use rupu_auth::ProviderId;
use std::io::Read;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Store credentials for a provider.
    Login {
        /// Provider name (anthropic | openai | gemini | copilot | github | gitlab | local).
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
        /// Provider name (omit with --all to clear everything).
        #[arg(long, conflicts_with = "all")]
        provider: Option<String>,
        /// Specific auth mode to remove. If omitted, both api-key and sso
        /// for that provider are removed.
        #[arg(long, value_enum)]
        mode: Option<AuthModeArg>,
        /// Remove every stored credential across all providers and modes.
        #[arg(long, conflicts_with = "provider")]
        all: bool,
        /// Skip the confirmation prompt for --all.
        #[arg(long, requires = "all")]
        yes: bool,
    },
    /// Show configured providers + backend.
    Status,
    /// Inspect or change the credential storage backend (OS keychain
    /// vs chmod-600 JSON file). Use `--use file` if the macOS
    /// keychain is dropping credentials between signed-binary
    /// updates.
    Backend {
        /// `keychain` (default on macOS / Linux with secret-service /
        /// Windows) or `file` (chmod-600 `~/.rupu/auth.json`).
        /// Omit to print the current choice + active source
        /// (env-var, cache, or default probe).
        #[arg(long, value_name = "KIND")]
        r#use: Option<String>,
    },
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
        Action::Logout {
            provider,
            mode,
            all,
            yes,
        } => {
            logout(LogoutOpts {
                provider,
                mode,
                all,
                yes,
            })
            .await
        }
        Action::Status => status().await,
        Action::Backend { r#use } => backend(r#use.as_deref()).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e)
    }
}

fn parse_provider(s: &str) -> anyhow::Result<ProviderId> {
    match s {
        "anthropic" => Ok(ProviderId::Anthropic),
        "openai" => Ok(ProviderId::Openai),
        "gemini" => Ok(ProviderId::Gemini),
        "copilot" => Ok(ProviderId::Copilot),
        "github" => Ok(ProviderId::Github),
        "gitlab" => Ok(ProviderId::Gitlab),
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

struct LogoutOpts {
    provider: Option<String>,
    mode: Option<AuthModeArg>,
    all: bool,
    yes: bool,
}

async fn logout(opts: LogoutOpts) -> anyhow::Result<()> {
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    if opts.all {
        if !opts.yes {
            // Refuse to prompt when stdin isn't a tty (CI, pipes, scripts)
            // because `read_line` would otherwise block forever or read EOF
            // and silently abort. Match the same posture `rupu run` takes for
            // its `ask` permission mode.
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() {
                anyhow::bail!(
                    "rupu auth logout --all in non-tty refuses to prompt — \
                     pass --yes to confirm, or run from an interactive terminal"
                );
            }
            print!("Remove all stored credentials? [y/N]: ");
            std::io::Write::flush(&mut std::io::stdout())?;
            let mut buf = String::new();
            std::io::stdin().read_line(&mut buf)?;
            if !matches!(buf.trim(), "y" | "yes" | "Y") {
                println!("aborted.");
                return Ok(());
            }
        }
        for p in [
            ProviderId::Anthropic,
            ProviderId::Openai,
            ProviderId::Gemini,
            ProviderId::Copilot,
            ProviderId::Github,
            ProviderId::Gitlab,
            ProviderId::Local,
        ] {
            for m in [
                rupu_providers::AuthMode::ApiKey,
                rupu_providers::AuthMode::Sso,
            ] {
                let _ = resolver.forget(p, m).await;
            }
        }
        println!("rupu: cleared all credentials");
        return Ok(());
    }
    let provider = opts
        .provider
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--provider required (or use --all)"))?;
    let pid = parse_provider(provider)?;
    let modes = match opts.mode {
        Some(m) => vec![m.into()],
        None => vec![
            rupu_providers::AuthMode::ApiKey,
            rupu_providers::AuthMode::Sso,
        ],
    };
    for m in modes {
        resolver.forget(pid, m).await?;
    }
    println!("rupu: forgot credential(s) for {provider}");
    Ok(())
}

async fn backend(r#use: Option<&str>) -> anyhow::Result<()> {
    // Persist the user's choice via a tiny shell-rc-friendly env-export
    // hint rather than writing to the cache directly: the env var
    // lives at the session boundary, and any in-process change here
    // wouldn't outlive `rupu auth backend` itself. The cache file is
    // still updated below for cases where probe behavior matters.
    let global = crate::paths::global_dir()?;
    let cache_path = global.join("cache/auth-backend.json");
    let cache = rupu_auth::ProbeCache::new(cache_path.clone());

    if let Some(target) = r#use {
        let target_norm = target.trim().to_ascii_lowercase();
        let choice = match target_norm.as_str() {
            "file" | "json" | "json-file" | "json_file" => rupu_auth::BackendChoice::JsonFile,
            "keyring" | "keychain" | "os" | "os-keychain" => rupu_auth::BackendChoice::Keyring,
            other => anyhow::bail!("unknown backend `{other}` — expected one of: file | keychain"),
        };
        // Update the cache so future invocations without the env var
        // pick the same backend.
        if let Err(e) = cache.write(choice) {
            tracing::warn!(error = %e, "failed to write probe cache");
        }
        let env_value = match choice {
            rupu_auth::BackendChoice::JsonFile => "file",
            rupu_auth::BackendChoice::Keyring => "keychain",
        };
        println!(
            "rupu: persisted backend choice = {env_value} (cache: {})",
            cache_path.display()
        );
        println!();
        println!("To override per-shell session (e.g. while debugging):");
        println!("  export RUPU_AUTH_BACKEND={env_value}");
        if matches!(choice, rupu_auth::BackendChoice::JsonFile) {
            let auth_path = global.join("auth.json");
            println!();
            println!("Credentials will be stored at:");
            println!("  {}", auth_path.display());
            println!("  (chmod 600 enforced on every write)");
            println!();
            println!("Re-run `rupu auth login --provider <name>` to populate the file.");
        }
        return Ok(());
    }

    // Show current state.
    let env_override = std::env::var(rupu_auth::ENV_BACKEND_OVERRIDE).ok();
    let cached = cache.read();
    let active = match (env_override.as_deref(), cached) {
        (Some(v), _) => format!("env-var override: {v}"),
        (None, Some(rupu_auth::BackendChoice::Keyring)) => "cached: keychain".into(),
        (None, Some(rupu_auth::BackendChoice::JsonFile)) => "cached: file".into(),
        (None, None) => "default: file (chmod-600 ~/.rupu/auth.json)".into(),
    };
    println!("Active backend : {active}");
    println!("Cache file     : {}", cache_path.display());
    println!();
    println!("To switch persistently:");
    println!("  rupu auth backend --use file       # store in ~/.rupu/auth.json (chmod 600)");
    println!("  rupu auth backend --use keychain   # store in OS keychain");
    println!();
    println!("To override per-shell session:");
    println!("  export RUPU_AUTH_BACKEND=file      # or `keychain`");
    Ok(())
}

async fn status() -> anyhow::Result<()> {
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    let prefs = crate::output::diag::prefs_for_diag(false);

    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["PROVIDER", "API-KEY", "SSO"]);

    for (label, pid) in [
        ("anthropic", ProviderId::Anthropic),
        ("openai", ProviderId::Openai),
        ("gemini", ProviderId::Gemini),
        ("copilot", ProviderId::Copilot),
        ("github", ProviderId::Github),
        ("gitlab", ProviderId::Gitlab),
    ] {
        let api_present = resolver.peek(pid, rupu_providers::AuthMode::ApiKey).await;
        let api_cell = if api_present {
            comfy_table::Cell::new("✓").fg(
                crate::output::tables::status_color("completed", &prefs)
                    .unwrap_or(comfy_table::Color::Reset),
            )
        } else {
            comfy_table::Cell::new("—").fg(comfy_table::Color::DarkGrey)
        };

        let sso_cell = match resolver.peek_sso(pid).await {
            Some(expiry_repr) => {
                let lower = expiry_repr.to_ascii_lowercase();
                let color = if lower.contains("expired") {
                    comfy_table::Color::Red
                } else if lower.contains("expires in") && is_soon(&expiry_repr) {
                    comfy_table::Color::Yellow
                } else {
                    crate::output::tables::status_color("completed", &prefs)
                        .unwrap_or(comfy_table::Color::Reset)
                };
                let glyph = if lower.contains("expired") { "✗" } else { "✓" };
                comfy_table::Cell::new(format!("{glyph} {expiry_repr}")).fg(color)
            }
            None => comfy_table::Cell::new("—").fg(comfy_table::Color::DarkGrey),
        };

        table.add_row(vec![comfy_table::Cell::new(label), api_cell, sso_cell]);
    }
    println!("{table}");
    Ok(())
}

/// Heuristic: SSO expiry strings like `expires in 8d` / `expires in 47h`
/// count as "soon" when the duration is under 7 days. Keeps the
/// renderer free of full date parsing — the source `expiry_repr` is
/// already a human-friendly relative form built by the resolver.
fn is_soon(repr: &str) -> bool {
    let trimmed = repr.trim_start_matches("expires in ").trim();
    if let Some(num) = trimmed.strip_suffix('d') {
        return num.parse::<u32>().map(|d| d < 7).unwrap_or(false);
    }
    if trimmed.ends_with('h') || trimmed.ends_with('m') || trimmed.ends_with('s') {
        return true;
    }
    false
}

#[cfg(test)]
mod parse_provider_tests {
    use super::*;

    #[test]
    fn recognizes_all_providers() {
        assert_eq!(parse_provider("anthropic").unwrap(), ProviderId::Anthropic);
        assert_eq!(parse_provider("openai").unwrap(), ProviderId::Openai);
        assert_eq!(parse_provider("gemini").unwrap(), ProviderId::Gemini);
        assert_eq!(parse_provider("copilot").unwrap(), ProviderId::Copilot);
        assert_eq!(parse_provider("github").unwrap(), ProviderId::Github);
        assert_eq!(parse_provider("gitlab").unwrap(), ProviderId::Gitlab);
        assert_eq!(parse_provider("local").unwrap(), ProviderId::Local);
    }

    #[test]
    fn rejects_unknown() {
        assert!(parse_provider("typo").is_err());
    }
}
