//! `rupu auth login | logout | status`.

use crate::cmd::ui::{self, LiveViewMode, UiPrefs};
use crate::output::formats::OutputFormat;
use crate::output::palette::{self, BRAND, DIM};
use crate::output::report::{self, CollectionOutput, DetailOutput};
use clap::Subcommand;
use comfy_table::Cell;
use rupu_auth::ProviderId;
use serde::Serialize;
use std::path::Path;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Store credentials for a provider.
    Login {
        /// Account name — the identity. Use a distinct name per identity
        /// (e.g. `anthropic-work`, `anthropic-personal`). A bare vendor
        /// name (`anthropic`) is the single-account default.
        #[arg(long, alias = "provider")]
        account: String,
        /// Vendor this account authenticates against (anthropic | openai |
        /// gemini | copilot | github | gitlab | linear | jira | local).
        /// Required the first time an account name is used; inferred from
        /// config or from the account name afterwards.
        #[arg(long)]
        kind: Option<String>,
        /// Authentication mode.
        #[arg(long, value_enum, default_value = "api-key")]
        mode: AuthModeArg,
        /// API key (only valid with --mode api-key). If omitted, reads from stdin.
        #[arg(long)]
        key: Option<String>,
    },
    /// Remove a stored credential.
    Logout {
        /// Account name (omit with --all to clear everything).
        #[arg(long, alias = "provider", conflicts_with = "all")]
        account: Option<String>,
        /// Specific auth mode to remove. If omitted, both api-key and sso
        /// for that provider are removed.
        #[arg(long, value_enum)]
        mode: Option<AuthModeArg>,
        /// Remove every stored credential across all providers and modes.
        #[arg(long, conflicts_with = "account")]
        all: bool,
        /// Skip the confirmation prompt for --all.
        #[arg(long, requires = "all")]
        yes: bool,
    },
    /// Show configured providers + backend.
    Status,
    /// Show where credentials are stored.
    ///
    /// There is one backend: a chmod-600 JSON file at
    /// `~/.rupu/auth.json`. The OS keychain backend was retired.
    Backend {
        /// `file` — the only supported value, and already in effect.
        /// Accepted so existing scripts keep working; `keychain` is
        /// refused with an explanation. Omit to print where credentials
        /// currently live.
        #[arg(long, value_name = "KIND")]
        r#use: Option<String>,
        /// Human snapshot density (`focused` | `compact` | `full`).
        #[arg(long, value_enum, default_value_t = LiveViewMode::Full)]
        view: LiveViewMode,
        /// Disable colored output.
        #[arg(long)]
        no_color: bool,
        /// Force pager. Default: page when stdout is a tty.
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,
        /// Disable pager.
        #[arg(long, conflicts_with = "pager")]
        no_pager: bool,
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

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::Login {
            account,
            kind,
            mode,
            key,
        } => login(&account, kind.as_deref(), mode, key.as_deref()).await,
        Action::Logout {
            account,
            mode,
            all,
            yes,
        } => {
            logout(LogoutOpts {
                account,
                mode,
                all,
                yes,
            })
            .await
        }
        Action::Status => status(global_format).await,
        Action::Backend {
            r#use,
            view,
            no_color,
            pager,
            no_pager,
        } => {
            let pager_flag = if pager {
                Some(true)
            } else if no_pager {
                Some(false)
            } else {
                None
            };
            backend(r#use.as_deref(), no_color, pager_flag, view, global_format).await
        }
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::Status => ("auth status", report::TABLE_JSON_CSV),
        Action::Backend { .. } => ("auth backend", report::TABLE_JSON),
        Action::Login { .. } => ("auth login", report::TABLE_ONLY),
        Action::Logout { .. } => ("auth logout", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

/// Names of user-defined openai-compatible providers from config.
///
/// Best-effort: an unreadable or absent config yields an empty list
/// rather than an error, because the only caller is a courtesy notice.
fn configured_named_providers() -> Vec<String> {
    let Ok(global) = crate::paths::global_dir() else {
        return Vec::new();
    };
    let global_cfg = global.join("config.toml");
    let Ok(cfg) = rupu_config::layer_files_locked(Some(&global_cfg), None) else {
        return Vec::new();
    };
    cfg.providers
        .keys()
        .filter(|name| {
            rupu_runtime::provider_factory::openai_compatible_params(name, &cfg.providers).is_some()
        })
        .cloned()
        .collect()
}

/// Load layered config and return true if `name` is a declared
/// openai-compatible provider.
fn is_openai_compatible_name(name: &str) -> bool {
    let Ok(global) = crate::paths::global_dir() else {
        return false;
    };
    let global_cfg = global.join("config.toml");
    let cfg = match rupu_config::layer_files_locked(Some(&global_cfg), None) {
        Ok(c) => c,
        Err(_) => return false,
    };
    rupu_runtime::provider_factory::openai_compatible_params(name, &cfg.providers).is_some()
}

fn parse_provider(s: &str) -> anyhow::Result<ProviderId> {
    match s {
        "anthropic" => Ok(ProviderId::Anthropic),
        "openai" => Ok(ProviderId::Openai),
        "gemini" => Ok(ProviderId::Gemini),
        "copilot" => Ok(ProviderId::Copilot),
        "github" => Ok(ProviderId::Github),
        "gitlab" => Ok(ProviderId::Gitlab),
        "linear" => Ok(ProviderId::Linear),
        "jira" => Ok(ProviderId::Jira),
        "local" => Ok(ProviderId::Local),
        _ => Err(anyhow::anyhow!("unknown provider: {s}")),
    }
}

/// Write `[providers.<account>] kind = "<kind>"` into a config file,
/// preserving everything already there.
///
/// Inserts into the parsed table tree directly rather than building a
/// dotted key. A dotted key would have to be quoted to survive an
/// account name containing a `.`, and that quoting contract has four
/// lockstep implementations across this repo — sidestepping it here
/// keeps this off that list.
fn declare_account_in_config(path: &Path, account: &str, kind: &str) -> anyhow::Result<()> {
    use toml::Value;

    let mut root: Value = if path.exists() {
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "refusing to write: {} is not valid TOML ({e}). \
                 Fix or move the file first — writing would discard its contents.",
                path.display()
            )
        })?
    } else {
        Value::Table(Default::default())
    };

    let root_table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config root is not a table"))?;

    let providers = root_table
        .entry("providers".to_string())
        .or_insert_with(|| Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("`providers` is already a value, not a table"))?;

    let entry = providers
        .entry(account.to_string())
        .or_insert_with(|| Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("`providers.{account}` is already a value, not a table"))?;

    entry.insert("kind".to_string(), Value::String(kind.to_string()));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(&root)?)?;
    Ok(())
}

async fn login(
    account: &str,
    kind_arg: Option<&str>,
    mode: AuthModeArg,
    key: Option<&str>,
) -> anyhow::Result<()> {
    warn_about_stranded_keychain_credentials();

    let global = crate::paths::global_dir()?;
    let cfg_path = global.join("config.toml");
    let cfg = rupu_config::layer_files_locked(Some(&cfg_path), None).unwrap_or_default();

    // Kind precedence: explicit flag, then config, then the account name
    // itself when it is a vendor (design spec §3.1).
    let declared = rupu_runtime::provider_factory::resolve_kind(account, &cfg.providers);
    let kind = match (kind_arg, declared.as_deref()) {
        (Some(k), Some(d)) if k != d => anyhow::bail!(
            "account '{account}' is already declared with kind \"{d}\"; \
             remove [providers.{account}] from {} to change it",
            cfg_path.display()
        ),
        (Some(k), _) => k.to_string(),
        (None, Some(d)) => d.to_string(),
        (None, None) => anyhow::bail!(
            "'{account}' is not a known vendor and is not declared in config. \
             Pass --kind <vendor> to declare it (anthropic | openai | gemini | \
             copilot | github | gitlab | linear | jira | local)."
        ),
    };

    // Persist the declaration so every later resolver call knows this
    // account exists. Skipped when the account name IS the vendor —
    // that needs no config to resolve.
    if kind != account
        && cfg
            .providers
            .get(account)
            .and_then(|p| p.kind.as_deref())
            .is_none()
    {
        crate::paths::ensure_dir(&global)?;
        declare_account_in_config(&cfg_path, account, &kind)?;
        println!("rupu: declared [providers.{account}] kind = \"{kind}\"");
    }

    if kind == "openai-compatible" {
        let AuthModeArg::ApiKey = mode else {
            anyhow::bail!("openai-compatible providers only support --mode api-key");
        };
        let secret = read_secret(key)?;
        let resolver = rupu_auth::resolver::KeychainResolver::new();
        let sc = rupu_auth::stored::StoredCredential::api_key(secret);
        resolver
            .store_named(account, rupu_providers::AuthMode::ApiKey, &sc)
            .await?;
        println!("rupu: stored {account} api-key credential");
        return Ok(());
    }

    let pid = rupu_auth::ProviderId::from_vendor_str(&kind)
        .ok_or_else(|| anyhow::anyhow!("unknown vendor kind: {kind}"))?;
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    let mode_neutral: rupu_providers::AuthMode = mode.clone().into();

    match mode {
        AuthModeArg::ApiKey => {
            let secret = match key {
                Some(k) => k.to_string(),
                None => read_api_key_from_stdin(account, pid)?,
            };
            if secret.is_empty() {
                anyhow::bail!("empty API key");
            }
            let sc = rupu_auth::stored::StoredCredential::api_key(secret);
            // store_named, not store: the ACCOUNT is the key. For a bare
            // vendor name these produce the identical string.
            resolver.store_named(account, mode_neutral, &sc).await?;
            println!("rupu: stored {account} api-key credential");
        }
        AuthModeArg::Sso => {
            let oauth = rupu_auth::oauth::providers::provider_oauth(pid)
                .ok_or_else(|| anyhow::anyhow!("vendor {kind} has no SSO flow"))?;
            let stored = match oauth.flow {
                rupu_auth::oauth::providers::OAuthFlow::Callback => {
                    rupu_auth::oauth::callback::run(pid).await?
                }
                rupu_auth::oauth::providers::OAuthFlow::Device => {
                    rupu_auth::oauth::device::run(pid).await?
                }
            };
            resolver.store_named(account, mode_neutral, &stored).await?;
            println!("rupu: stored {account} sso credential");
        }
    }
    Ok(())
}

/// Read a secret from `--key` or stdin, rejecting empty input.
fn read_secret(key: Option<&str>) -> anyhow::Result<String> {
    let secret = match key {
        Some(k) => k.to_string(),
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf.trim().to_string()
        }
    };
    if secret.is_empty() {
        anyhow::bail!("empty API key");
    }
    Ok(secret)
}

/// Read an API key from stdin with proper UI feedback. Two paths:
///
/// - **stdin is a tty**: prompt the user with a one-line message
///   telling them what to paste and how to terminate the input
///   (`Ctrl-D` on Unix, `Ctrl-Z, Enter` on Windows). Without this
///   prompt, `rupu auth login --provider <p>` blocked silently on
///   `read_to_string` waiting for EOF — the symptom users hit was
///   "the command stalls and nothing happens."
///
/// - **stdin is NOT a tty** (pipe / heredoc / CI): silently slurp
///   the buffered input. This preserves the documented
///   `echo $KEY | rupu auth login --provider …` flow.
///
/// Also surfaces the SSO alternative when the provider has one —
/// users typing `rupu auth login --provider github` mostly want the
/// SSO/device-code flow, not a paste-your-PAT prompt.
fn read_api_key_from_stdin(provider: &str, pid: ProviderId) -> anyhow::Result<String> {
    use std::io::{IsTerminal, Read, Write};

    let prefs = crate::output::diag::prefs_for_diag(false);

    if std::io::stdin().is_terminal() {
        let has_sso = rupu_auth::oauth::providers::provider_oauth(pid).is_some();
        let sso_hint = if has_sso {
            format!(" (or rerun with `--mode sso` to authenticate via the {provider} browser flow)")
        } else {
            String::new()
        };
        // Stderr so the prompt doesn't pollute a piped stdout.
        eprintln!("rupu auth login: paste your {provider} API key, then press Ctrl-D to submit{sso_hint}.");
        // Flush in case stderr is line-buffered and the prompt would
        // otherwise lag behind the user's first paste.
        let _ = std::io::stderr().flush();
    } else {
        // Non-tty: silently read whatever was piped in. If the pipe is
        // empty (`< /dev/null`), `read_to_string` returns 0 bytes and
        // we fall through to the empty-secret check in the caller.
        let _ = &prefs;
    }

    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf.trim().to_string())
}

struct LogoutOpts {
    account: Option<String>,
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
            ProviderId::Linear,
            ProviderId::Jira,
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
    let account = opts
        .account
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--account required (or use --all)"))?;
    if is_openai_compatible_name(account) {
        if parse_provider(account).is_ok() {
            anyhow::bail!(
                "'{account}' is a reserved built-in provider name; \
                 choose a distinct name for an openai-compatible provider in config"
            );
        }
        if !matches!(opts.mode, None | Some(AuthModeArg::ApiKey)) {
            anyhow::bail!(
                "openai-compatible providers only support api-key credentials; \
                 --mode sso is not valid"
            );
        }
        let resolver = rupu_auth::resolver::KeychainResolver::new();
        resolver
            .forget_named(account, rupu_providers::AuthMode::ApiKey)
            .await?;
        println!("rupu: forgot credential(s) for {account}");
        return Ok(());
    }
    let modes = match opts.mode {
        Some(m) => vec![m.into()],
        None => vec![
            rupu_providers::AuthMode::ApiKey,
            rupu_providers::AuthMode::Sso,
        ],
    };
    for m in modes {
        // forget_named, not forget: the ACCOUNT is the key. For a bare
        // vendor name these produce the identical string.
        resolver.forget_named(account, m).await?;
    }
    println!("rupu: forgot credential(s) for {account}");
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct AuthBackendItem {
    requested_backend: Option<String>,
    active_backend: String,
    cache_path: String,
    auth_path: String,
    cache_choice: Option<String>,
    env_override: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AuthBackendReport {
    kind: &'static str,
    version: u8,
    item: AuthBackendItem,
}

struct AuthBackendOutput {
    prefs: UiPrefs,
    report: AuthBackendReport,
}

impl DetailOutput for AuthBackendOutput {
    type JsonReport = AuthBackendReport;

    fn command_name(&self) -> &'static str {
        "auth backend"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let width = crossterm::terminal::size()
            .map(|(value, _)| value.max(40) as usize)
            .unwrap_or(100);
        let body = render_auth_backend_snapshot(
            &self.report.item,
            self.prefs.live_view,
            &self.prefs,
            width,
        );
        ui::paginate(&body, &self.prefs)
    }
}

/// Tell the user once, at login time, if an older rupu left credentials
/// in the macOS keychain. Without this they simply appear logged out
/// with no explanation. Read-only and best-effort: a `security` probe
/// that fails for any reason yields no findings and no output.
fn warn_about_stranded_keychain_credentials() {
    // Named (openai-compatible) providers were stored under the same
    // account shapes as built-ins, so they have to be probed too — the
    // built-in list alone would miss them silently.
    let named = configured_named_providers();
    let stranded = rupu_auth::detect_stranded_keychain_credentials(&named);
    if stranded.is_empty() {
        return;
    }
    eprintln!(
        "note: credentials for {} are still in the macOS keychain from an older rupu.\n      \
         That store was retired; rupu now keeps credentials in a chmod-600 file.\n      \
         Logging in here replaces them. The old entries are left untouched and\n      \
         can be removed with:  security delete-generic-password -s rupu -a <account>",
        stranded.join(", ")
    );
}

async fn backend(
    r#use: Option<&str>,
    no_color: bool,
    pager_flag: Option<bool>,
    view: LiveViewMode,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    // There is only one credential backend: a chmod-600 JSON file. This
    // command survives as a way to report *where* credentials live; the
    // `--use` flag survives only to give an explicit answer to anyone
    // still asking for the retired keychain.
    let global = crate::paths::global_dir()?;
    let auth_path = global.join("auth.json");
    let prefs = auth_ui_prefs(no_color, pager_flag, view)?;

    // The probe cache selected between backends that no longer both
    // exist. Remove it rather than leave a file implying a choice is
    // still being made.
    let stale_cache = global.join("cache/auth-backend.json");
    if stale_cache.exists() {
        if let Err(e) = std::fs::remove_file(&stale_cache) {
            tracing::warn!(
                error = %e,
                path = %stale_cache.display(),
                "could not remove stale auth-backend probe cache"
            );
        }
    }

    if let Some(target) = r#use {
        match target.trim().to_ascii_lowercase().as_str() {
            "file" | "json" | "json-file" | "json_file" => {}
            "keyring" | "keychain" | "os" | "os-keychain" => anyhow::bail!(
                "the OS keychain backend is no longer supported — rupu stores \
                 credentials in a chmod-600 file at `{}`. There is nothing to \
                 select; `--use file` is the only valid value and is already in \
                 effect.",
                auth_path.display()
            ),
            other => {
                anyhow::bail!("unknown backend `{other}` — the only supported value is: file")
            }
        }
        let report = AuthBackendReport {
            kind: "auth_backend",
            version: 1,
            item: AuthBackendItem {
                requested_backend: Some("file".to_string()),
                active_backend: "file".to_string(),
                cache_path: stale_cache.display().to_string(),
                auth_path: auth_path.display().to_string(),
                cache_choice: None,
                env_override: None,
            },
        };
        return report::emit_detail(global_format, &AuthBackendOutput { prefs, report });
    }

    // Show current state. `RUPU_AUTH_FILE` can still relocate the store,
    // so surface it — but it selects a path, never a different backend.
    let report = AuthBackendReport {
        kind: "auth_backend",
        version: 1,
        item: AuthBackendItem {
            requested_backend: None,
            active_backend: "file".to_string(),
            cache_path: stale_cache.display().to_string(),
            auth_path: std::env::var("RUPU_AUTH_FILE")
                .unwrap_or_else(|_| auth_path.display().to_string()),
            cache_choice: None,
            env_override: std::env::var("RUPU_AUTH_FILE").ok(),
        },
    };
    report::emit_detail(global_format, &AuthBackendOutput { prefs, report })
}

fn auth_ui_prefs(
    no_color: bool,
    pager_flag: Option<bool>,
    view: LiveViewMode,
) -> anyhow::Result<UiPrefs> {
    let global = crate::paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = crate::paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root
        .as_ref()
        .map(|path| path.join(".rupu/config.toml"));
    // UI prefs only — lock does not apply (I-7)
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref())?;
    Ok(UiPrefs::resolve(
        &cfg.ui,
        no_color,
        None,
        pager_flag,
        Some(view),
    ))
}

fn render_auth_backend_snapshot(
    item: &AuthBackendItem,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
    width: usize,
) -> String {
    let mut rows = vec![
        render_auth_backend_header_line(item, view_mode, width),
        String::new(),
    ];
    rows.extend(render_auth_backend_state_rows(item, width));

    if matches!(view_mode, LiveViewMode::Compact | LiveViewMode::Full) {
        rows.push(String::new());
        rows.extend(render_auth_backend_path_rows(item, width));
    }

    if view_mode == LiveViewMode::Full {
        rows.push(String::new());
        rows.extend(render_auth_backend_command_rows(item, prefs, width));
        if let Some(note_rows) = render_auth_backend_note_rows(item, width) {
            rows.push(String::new());
            rows.extend(note_rows);
        }
    }

    rows.join("\n") + "\n"
}

fn render_auth_backend_header_line(
    item: &AuthBackendItem,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "▶", BRAND);
    buf.push(' ');
    let _ = palette::write_bold_colored(&mut buf, "auth backend", BRAND);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_bold_colored(&mut buf, effective_backend_kind(item), BRAND);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, view_mode.as_str(), DIM);
    truncate_auth_backend_ansi_line(&buf, width)
}

fn render_auth_backend_state_rows(item: &AuthBackendItem, width: usize) -> Vec<String> {
    let mut rows = vec![render_auth_backend_section_header(
        "state",
        "resolved backend",
        width,
    )];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["KEY", "VALUE"]);
    if let Some(requested) = item.requested_backend.as_deref() {
        table.add_row(vec![Cell::new("requested"), Cell::new(requested)]);
    }
    table.add_row(vec![
        Cell::new("active"),
        Cell::new(effective_backend_kind(item)),
    ]);
    table.add_row(vec![
        Cell::new("source"),
        Cell::new(backend_resolution_source(item)),
    ]);
    table.add_row(vec![
        Cell::new("cache"),
        Cell::new(item.cache_choice.as_deref().unwrap_or("none")),
    ]);
    table.add_row(vec![
        Cell::new("override"),
        Cell::new(item.env_override.as_deref().unwrap_or("none")),
    ]);
    table.add_row(vec![
        Cell::new("detail"),
        Cell::new(crate::cmd::transcript::truncate_single_line(
            &item.active_backend,
            72,
        )),
    ]);
    rows.extend(
        table
            .to_string()
            .lines()
            .map(|line| truncate_auth_backend_ansi_line(line, width)),
    );
    rows
}

fn render_auth_backend_path_rows(item: &AuthBackendItem, width: usize) -> Vec<String> {
    let mut rows = vec![render_auth_backend_section_header(
        "paths",
        "storage files",
        width,
    )];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["NAME", "PATH"]);
    table.add_row(vec![Cell::new("cache"), Cell::new(&item.cache_path)]);
    table.add_row(vec![Cell::new("auth"), Cell::new(&item.auth_path)]);
    rows.extend(
        table
            .to_string()
            .lines()
            .map(|line| truncate_auth_backend_ansi_line(line, width)),
    );
    rows
}

fn render_auth_backend_command_rows(
    item: &AuthBackendItem,
    _prefs: &UiPrefs,
    width: usize,
) -> Vec<String> {
    let mut rows = vec![render_auth_backend_section_header(
        "commands",
        "switch or override",
        width,
    )];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["MODE", "COMMAND", "EFFECT"]);
    // There is one backend, so nothing here offers a choice — these are
    // the levers that still exist: relocate the store, or populate it.
    table.add_row(vec![
        Cell::new("relocate"),
        Cell::new("export RUPU_AUTH_FILE=/path/to/auth.json"),
        Cell::new("store credentials at an explicit path"),
    ]);
    table.add_row(vec![
        Cell::new("relocate"),
        Cell::new("export RUPU_HOME=/path/to/rupu-dir"),
        Cell::new("move the whole rupu directory, auth.json included"),
    ]);
    if item.requested_backend.as_deref() == Some("file") {
        table.add_row(vec![
            Cell::new("next"),
            Cell::new("rupu auth login --provider <name>"),
            Cell::new("populate the local auth file with credentials"),
        ]);
    }
    rows.extend(
        table
            .to_string()
            .lines()
            .map(|line| truncate_auth_backend_ansi_line(line, width)),
    );
    rows
}

fn render_auth_backend_note_rows(item: &AuthBackendItem, width: usize) -> Option<Vec<String>> {
    let effective = effective_backend_kind(item);
    let detail = if effective == "file" {
        "JSON-file credentials are written with chmod 600 on every update."
    } else {
        "Keychain mode relies on the platform credential store and avoids a local auth.json file."
    };
    Some(vec![
        render_auth_backend_section_header("notes", "backend behavior", width),
        render_auth_backend_kv_row("note", detail, width),
    ])
}

fn render_auth_backend_section_header(label: &str, detail: &str, width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, BRAND);
    if !detail.is_empty() {
        let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
        let _ = palette::write_colored(&mut buf, detail, DIM);
    }
    truncate_auth_backend_ansi_line(&buf, width)
}

fn render_auth_backend_kv_row(label: &str, value: &str, width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, &format!("{label:<10}"), BRAND);
    let _ = palette::write_colored(
        &mut buf,
        &crate::cmd::transcript::truncate_single_line(value, width.saturating_sub(11)),
        DIM,
    );
    truncate_auth_backend_ansi_line(&buf, width)
}

fn effective_backend_kind(item: &AuthBackendItem) -> &'static str {
    let candidate = item
        .env_override
        .as_deref()
        .or(item.requested_backend.as_deref())
        .or(item.cache_choice.as_deref())
        .unwrap_or(&item.active_backend);
    if candidate.to_ascii_lowercase().contains("key") {
        "keychain"
    } else {
        "file"
    }
}

fn backend_resolution_source(item: &AuthBackendItem) -> &'static str {
    if item.env_override.is_some() {
        "env override"
    } else if item.cache_choice.is_some() {
        "probe cache"
    } else {
        "default"
    }
}

fn truncate_auth_backend_ansi_line(value: &str, width: usize) -> String {
    if crate::output::printer::visible_len(value) <= width {
        value.to_string()
    } else {
        crate::output::printer::wrap_with_ansi(value, width)
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize)]
struct AuthStatusRow {
    provider: String,
    api_key: bool,
    sso: String,
}

#[derive(Debug, Clone, Serialize)]
struct AuthStatusCsvRow {
    provider: String,
    api_key: String,
    sso: String,
}

#[derive(Debug, Clone, Serialize)]
struct AuthStatusReport {
    kind: &'static str,
    version: u8,
    rows: Vec<AuthStatusRow>,
}

struct AuthStatusOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: AuthStatusReport,
    csv_rows: Vec<AuthStatusCsvRow>,
}

impl CollectionOutput for AuthStatusOutput {
    type JsonReport = AuthStatusReport;
    type CsvRow = AuthStatusCsvRow;

    fn command_name(&self) -> &'static str {
        "auth status"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["provider", "api_key", "sso"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["PROVIDER", "API-KEY", "SSO"]);
        for row in &self.report.rows {
            let api_cell = if row.api_key {
                comfy_table::Cell::new("✓").fg(crate::output::tables::status_color(
                    "completed",
                    &self.prefs,
                )
                .unwrap_or(comfy_table::Color::Reset))
            } else {
                comfy_table::Cell::new("—").fg(comfy_table::Color::DarkGrey)
            };
            let sso_cell = sso_status_cell(&row.sso, &self.prefs);
            table.add_row(vec![
                comfy_table::Cell::new(&row.provider),
                api_cell,
                sso_cell,
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

async fn status(global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    let prefs = crate::output::diag::prefs_for_diag(false);
    let mut rows = Vec::new();

    for (label, pid) in [
        ("anthropic", ProviderId::Anthropic),
        ("openai", ProviderId::Openai),
        ("gemini", ProviderId::Gemini),
        ("copilot", ProviderId::Copilot),
        ("github", ProviderId::Github),
        ("gitlab", ProviderId::Gitlab),
        ("linear", ProviderId::Linear),
        ("jira", ProviderId::Jira),
    ] {
        let api_present = resolver.peek(pid, rupu_providers::AuthMode::ApiKey).await;
        rows.push(AuthStatusRow {
            provider: label.to_string(),
            api_key: api_present,
            sso: resolver.peek_sso(pid).await.unwrap_or_default(),
        });
    }
    if let Ok(global) = crate::paths::global_dir() {
        let global_cfg = global.join("config.toml");
        if let Ok(cfg) = rupu_config::layer_files_locked(Some(&global_cfg), None) {
            for (name, p) in &cfg.providers {
                if p.kind.as_deref() != Some("openai-compatible") {
                    continue;
                }
                if parse_provider(name).is_ok() {
                    continue;
                }
                let present = resolver
                    .peek_named(name, rupu_providers::AuthMode::ApiKey)
                    .await
                    || std::env::var(format!("RUPU_{}_API_KEY", name.to_ascii_uppercase()))
                        .map(|v| !v.is_empty())
                        .unwrap_or(false);
                rows.push(AuthStatusRow {
                    provider: name.clone(),
                    api_key: present,
                    sso: String::new(),
                });
            }
        }
    }
    let csv_rows = rows
        .iter()
        .map(|row| AuthStatusCsvRow {
            provider: row.provider.clone(),
            api_key: if row.api_key {
                "yes".into()
            } else {
                "no".into()
            },
            sso: row.sso.clone(),
        })
        .collect();
    let output = AuthStatusOutput {
        prefs,
        report: AuthStatusReport {
            kind: "auth_status",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

fn sso_status_cell(value: &str, prefs: &crate::cmd::ui::UiPrefs) -> comfy_table::Cell {
    if value.is_empty() {
        return comfy_table::Cell::new("—").fg(comfy_table::Color::DarkGrey);
    }
    let lower = value.to_ascii_lowercase();
    let color = if lower.contains("expired") {
        comfy_table::Color::Red
    } else if lower.contains("expires in") && is_soon(value) {
        comfy_table::Color::Yellow
    } else {
        crate::output::tables::status_color("completed", prefs).unwrap_or(comfy_table::Color::Reset)
    };
    let glyph = if lower.contains("expired") {
        "✗"
    } else {
        "✓"
    };
    comfy_table::Cell::new(format!("{glyph} {value}")).fg(color)
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
mod tests {
    use super::*;

    #[test]
    fn declare_account_writes_kind_without_dotted_key_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_provider = \"anthropic\"\n").unwrap();

        declare_account_in_config(&path, "anthropic-work", "anthropic").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            v["providers"]["anthropic-work"]["kind"].as_str(),
            Some("anthropic")
        );
        // Pre-existing keys survive.
        assert_eq!(v["default_provider"].as_str(), Some("anthropic"));
    }

    /// An account name containing a dot must land as ONE table key, not
    /// two nested tables. This is why we insert into the table tree
    /// directly instead of building a dotted key string.
    #[test]
    fn declare_account_treats_a_dotted_name_as_one_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        declare_account_in_config(&path, "gpt-4.1-box", "openai").unwrap();
        let v: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            v["providers"]["gpt-4.1-box"]["kind"].as_str(),
            Some("openai")
        );
    }

    #[test]
    fn declare_account_refuses_to_clobber_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not = = toml").unwrap();
        assert!(declare_account_in_config(&path, "x", "openai").is_err());
    }
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
        assert_eq!(parse_provider("linear").unwrap(), ProviderId::Linear);
        assert_eq!(parse_provider("jira").unwrap(), ProviderId::Jira);
        assert_eq!(parse_provider("local").unwrap(), ProviderId::Local);
    }

    #[test]
    fn rejects_unknown() {
        assert!(parse_provider("typo").is_err());
    }
}
