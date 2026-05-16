//! `rupu auth login | logout | status`.

use crate::cmd::ui::{self, LiveViewMode, UiPrefs};
use crate::output::formats::OutputFormat;
use crate::output::palette::{self, BRAND, DIM};
use crate::output::report::{self, CollectionOutput, DetailOutput};
use clap::Subcommand;
use comfy_table::Cell;
use rupu_auth::ProviderId;
use serde::Serialize;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Store credentials for a provider.
    Login {
        /// Provider name (anthropic | openai | gemini | copilot | github | gitlab | linear | jira | local).
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

async fn login(provider: &str, mode: AuthModeArg, key: Option<&str>) -> anyhow::Result<()> {
    let pid = parse_provider(provider)?;
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    let mode_neutral: rupu_providers::AuthMode = mode.clone().into();
    match mode {
        AuthModeArg::ApiKey => {
            let secret = match key {
                Some(k) => k.to_string(),
                None => read_api_key_from_stdin(provider, pid)?,
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

async fn backend(
    r#use: Option<&str>,
    no_color: bool,
    pager_flag: Option<bool>,
    view: LiveViewMode,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    // Persist the user's choice via a tiny shell-rc-friendly env-export
    // hint rather than writing to the cache directly: the env var
    // lives at the session boundary, and any in-process change here
    // wouldn't outlive `rupu auth backend` itself. The cache file is
    // still updated below for cases where probe behavior matters.
    let global = crate::paths::global_dir()?;
    let cache_path = global.join("cache/auth-backend.json");
    let cache = rupu_auth::ProbeCache::new(cache_path.clone());
    let auth_path = global.join("auth.json");
    let prefs = auth_ui_prefs(no_color, pager_flag, view)?;

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
        if matches!(global_format, Some(OutputFormat::Json)) {
            let report = AuthBackendReport {
                kind: "auth_backend",
                version: 1,
                item: AuthBackendItem {
                    requested_backend: Some(env_value.to_string()),
                    active_backend: format!("cached: {env_value}"),
                    cache_path: cache_path.display().to_string(),
                    auth_path: auth_path.display().to_string(),
                    cache_choice: Some(env_value.to_string()),
                    env_override: None,
                },
            };
            return report::emit_detail(global_format, &AuthBackendOutput { prefs, report });
        }
        let report = AuthBackendReport {
            kind: "auth_backend",
            version: 1,
            item: AuthBackendItem {
                requested_backend: Some(env_value.to_string()),
                active_backend: format!("cached: {env_value}"),
                cache_path: cache_path.display().to_string(),
                auth_path: auth_path.display().to_string(),
                cache_choice: Some(env_value.to_string()),
                env_override: None,
            },
        };
        return report::emit_detail(global_format, &AuthBackendOutput { prefs, report });
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
    let report = AuthBackendReport {
        kind: "auth_backend",
        version: 1,
        item: AuthBackendItem {
            requested_backend: None,
            active_backend: active,
            cache_path: cache_path.display().to_string(),
            auth_path: auth_path.display().to_string(),
            cache_choice: cached.map(|choice| match choice {
                rupu_auth::BackendChoice::JsonFile => "file".to_string(),
                rupu_auth::BackendChoice::Keyring => "keychain".to_string(),
            }),
            env_override,
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
    let project_cfg = project_root.as_ref().map(|path| path.join(".rupu/config.toml"));
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
    let mut rows = vec![render_auth_backend_header_line(item, view_mode, width), String::new()];
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
    table.add_row(vec![
        Cell::new("persist"),
        Cell::new("rupu auth backend --use file"),
        Cell::new("store credentials in ~/.rupu/auth.json"),
    ]);
    table.add_row(vec![
        Cell::new("persist"),
        Cell::new("rupu auth backend --use keychain"),
        Cell::new("store credentials in the OS keychain"),
    ]);
    table.add_row(vec![
        Cell::new("shell"),
        Cell::new("export RUPU_AUTH_BACKEND=file"),
        Cell::new("override the backend for the current shell"),
    ]);
    table.add_row(vec![
        Cell::new("shell"),
        Cell::new("export RUPU_AUTH_BACKEND=keychain"),
        Cell::new("override the backend for the current shell"),
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
