//! Terminal-output rendering helpers shared by `rupu agent show` and
//! `rupu workflow show`. Owns three concerns:
//!
//! 1. Resolving color / theme / pager preferences from CLI flags,
//!    `NO_COLOR`, and the `[ui]` section of `config.toml`.
//! 2. Syntect-driven highlighting for YAML, Markdown, and the
//!    YAML-frontmatter + Markdown body that an agent file is shaped as.
//! 3. Routing the rendered output through `$PAGER` (default
//!    `less -RFX`) when the resolution says we should page.
//!
//! Failure on any layer falls back to plain output — printing an
//! agent or workflow must never fail because of a syntax-set lookup.

use anyhow::Context;
use clap::{Args as ClapArgs, Subcommand};
use rupu_config::UiConfig;
use serde::Serialize;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

const DEFAULT_THEME: &str = crate::output::theme::DEFAULT_SYNTAX_THEME;

#[derive(Subcommand, Debug, Clone)]
pub enum Action {
    /// List available UI themes.
    Themes,
    /// Manage a specific palette theme.
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ThemeAction {
    /// Show one theme in detail.
    Show { name: String },
    /// Validate a theme file on disk.
    Validate { path: PathBuf },
    /// Import a theme from a local file or URL.
    Import(ImportArgs),
}

#[derive(ClapArgs, Debug, Clone)]
pub struct ImportArgs {
    /// Local path or URL to a theme file.
    pub source: String,
    /// Import format. `auto` tries native rupu theme first, then Base16.
    #[arg(long, default_value = "auto")]
    pub from: crate::output::theme::ThemeImportFormat,
    /// Override the installed theme name.
    #[arg(long)]
    pub name: Option<String>,
    /// Install the theme under the current project's `.rupu/themes/`.
    #[arg(long)]
    pub project: bool,
}

#[derive(Debug, Clone)]
pub struct UiPrefs {
    pub color: ColorMode,
    pub theme: String,
    pub palette_theme: String,
    pub palette: crate::output::palette::UiPaletteTheme,
    pub pager: PagerMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagerMode {
    Auto,
    Always,
    Never,
}

impl UiPrefs {
    /// Resolve preferences from (in priority order): CLI flags, env
    /// vars (`NO_COLOR`, `PAGER`-presence), the `[ui]` section of
    /// the layered config, and built-in defaults.
    pub fn resolve(
        cfg: &UiConfig,
        flag_no_color: bool,
        flag_theme: Option<&str>,
        flag_pager: Option<bool>,
    ) -> Self {
        let color = if flag_no_color || std::env::var_os("NO_COLOR").is_some() {
            ColorMode::Never
        } else {
            parse_color(cfg.color.as_deref()).unwrap_or(ColorMode::Auto)
        };
        let theme = flag_theme
            .map(str::to_string)
            .or_else(|| cfg.syntax.theme.clone())
            .or_else(|| cfg.theme.clone())
            .unwrap_or_else(|| DEFAULT_THEME.to_string());
        let global = crate::paths::global_dir().ok();
        let project_root = std::env::current_dir()
            .ok()
            .and_then(|pwd| crate::paths::project_root_for(&pwd).ok().flatten());
        let palette_requested = cfg.palette.theme.as_deref().or(cfg.theme.as_deref());
        let palette_spec = global
            .as_deref()
            .map(|global| {
                crate::output::theme::resolve_palette_theme(
                    palette_requested,
                    global,
                    project_root.as_deref(),
                )
            })
            .unwrap_or_else(crate::output::theme::default_palette_theme);
        crate::output::palette::set_active_palette(palette_spec.palette.clone());
        let pager = match flag_pager {
            Some(true) => PagerMode::Always,
            Some(false) => PagerMode::Never,
            None => parse_pager(cfg.pager.as_deref()).unwrap_or(PagerMode::Auto),
        };
        UiPrefs {
            color,
            theme,
            palette_theme: palette_spec.name,
            palette: palette_spec.palette,
            pager,
        }
    }

    pub fn use_color(&self) -> bool {
        match self.color {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => std::io::stdout().is_terminal(),
        }
    }

    pub fn use_pager(&self) -> bool {
        match self.pager {
            PagerMode::Always => true,
            PagerMode::Never => false,
            PagerMode::Auto => std::io::stdout().is_terminal(),
        }
    }
}

#[derive(Serialize)]
struct ThemeListRow {
    name: String,
    kind: String,
    source: String,
    syntax_theme: Option<String>,
    current: bool,
    description: Option<String>,
}

#[derive(Serialize)]
struct ThemeListCsvRow {
    name: String,
    kind: String,
    source: String,
    syntax_theme: String,
    current: bool,
    description: String,
}

#[derive(Serialize)]
struct ThemeListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<ThemeListRow>,
}

struct ThemeListOutput {
    report: ThemeListReport,
    csv_rows: Vec<ThemeListCsvRow>,
}

#[derive(Serialize)]
struct ThemeShowItem {
    name: String,
    kind: String,
    source: String,
    path: Option<String>,
    syntax_theme: Option<String>,
    description: Option<String>,
    palette: Option<crate::output::palette::UiPaletteTheme>,
}

#[derive(Serialize)]
struct ThemeShowReport {
    kind: &'static str,
    version: u8,
    item: ThemeShowItem,
}

struct ThemeShowOutput {
    report: ThemeShowReport,
}

impl crate::output::report::CollectionOutput for ThemeListOutput {
    type JsonReport = ThemeListReport;
    type CsvRow = ThemeListCsvRow;

    fn command_name(&self) -> &'static str {
        "ui themes"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "name",
            "kind",
            "source",
            "syntax_theme",
            "current",
            "description",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "NAME",
            "KIND",
            "SOURCE",
            "SYNTAX",
            "CURRENT",
            "DESCRIPTION",
        ]);
        for row in &self.report.rows {
            table.add_row(vec![
                comfy_table::Cell::new(&row.name),
                comfy_table::Cell::new(&row.kind),
                comfy_table::Cell::new(&row.source),
                comfy_table::Cell::new(row.syntax_theme.as_deref().unwrap_or("—")),
                comfy_table::Cell::new(if row.current { "yes" } else { "" }),
                comfy_table::Cell::new(row.description.as_deref().unwrap_or("—")),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

impl crate::output::report::DetailOutput for ThemeShowOutput {
    type JsonReport = ThemeShowReport;

    fn command_name(&self) -> &'static str {
        "ui theme show"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let item = &self.report.item;
        println!("name: {}", item.name);
        println!("kind: {}", item.kind);
        println!("source: {}", item.source);
        if let Some(path) = item.path.as_deref() {
            println!("path: {path}");
        }
        if let Some(theme) = item.syntax_theme.as_deref() {
            println!("syntax theme: {theme}");
        }
        if let Some(description) = item.description.as_deref() {
            println!("description: {description}");
        }
        if let Some(palette) = item.palette.as_ref() {
            println!("palette:");
            for (name, value) in [
                ("running", palette.running),
                ("complete", palette.complete),
                ("failed", palette.failed),
                ("awaiting", palette.awaiting),
                ("skipped", palette.skipped),
                ("soft_failed", palette.soft_failed),
                ("retrying", palette.retrying),
                ("dim", palette.dim),
                ("brand", palette.brand),
                ("brand_subtle", palette.brand_subtle),
                ("tool_arrow", palette.tool_arrow),
                ("separator", palette.separator),
                ("sev_critical", palette.sev_critical),
                ("sev_high", palette.sev_high),
                ("sev_medium", palette.sev_medium),
                ("sev_low", palette.sev_low),
                ("sev_info", palette.sev_info),
            ] {
                println!("  {name}: {}", format_hex(value));
            }
            if !palette.label_palette.is_empty() {
                println!(
                    "  label_palette: {}",
                    palette
                        .label_palette
                        .iter()
                        .copied()
                        .map(format_hex)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        Ok(())
    }
}

pub async fn handle(
    action: Action,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> std::process::ExitCode {
    let result = match action {
        Action::Themes => themes(global_format).await,
        Action::Theme { action } => match action {
            ThemeAction::Show { name } => show(&name, global_format).await,
            ThemeAction::Validate { path } => validate(&path, global_format).await,
            ThemeAction::Import(args) => import(args, global_format).await,
        },
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[error] {error:#}");
            std::process::ExitCode::from(1)
        }
    }
}

pub fn ensure_output_format(
    action: &Action,
    format: crate::output::formats::OutputFormat,
) -> anyhow::Result<()> {
    match action {
        Action::Themes => crate::output::formats::ensure_supported(
            "ui themes",
            format,
            crate::output::report::TABLE_JSON_CSV,
        ),
        Action::Theme { action } => match action {
            ThemeAction::Show { .. } | ThemeAction::Validate { .. } | ThemeAction::Import(_) => {
                crate::output::formats::ensure_supported(
                    "ui theme",
                    format,
                    crate::output::report::TABLE_JSON,
                )
            }
        },
    }
}

fn parse_color(raw: Option<&str>) -> Option<ColorMode> {
    match raw?.to_ascii_lowercase().as_str() {
        "auto" => Some(ColorMode::Auto),
        "always" => Some(ColorMode::Always),
        "never" => Some(ColorMode::Never),
        _ => None,
    }
}

fn parse_pager(raw: Option<&str>) -> Option<PagerMode> {
    match raw?.to_ascii_lowercase().as_str() {
        "auto" => Some(PagerMode::Auto),
        "always" => Some(PagerMode::Always),
        "never" => Some(PagerMode::Never),
        _ => None,
    }
}

/// Highlight a YAML buffer.
pub fn highlight_yaml(text: &str, prefs: &UiPrefs) -> String {
    if !prefs.use_color() {
        return text.to_string();
    }
    highlight_with_extension(text, "yaml", prefs).unwrap_or_else(|| text.to_string())
}

/// Highlight a Markdown buffer (covers fenced code blocks, headings,
/// lists, emphasis). Used by the line-stream printer to colorize
/// assistant-text chunks so agent output is visually delineated from
/// rupu's chrome lines. Falls back to plain text on syntect lookup
/// failure or when color is disabled.
pub fn highlight_markdown(text: &str, prefs: &UiPrefs) -> String {
    if !prefs.use_color() {
        return text.to_string();
    }
    highlight_with_extension(text, "md", prefs).unwrap_or_else(|| text.to_string())
}

/// Highlight an agent file: split on the trailing `---` of the YAML
/// frontmatter, color the frontmatter as YAML and the body as Markdown.
/// If the split fails (no frontmatter or shape unexpected), fall back
/// to whole-buffer Markdown highlighting.
pub fn highlight_agent_file(text: &str, prefs: &UiPrefs) -> String {
    if !prefs.use_color() {
        return text.to_string();
    }

    if let Some((fm, body)) = split_frontmatter(text) {
        let yaml = highlight_with_extension(fm, "yaml", prefs).unwrap_or_else(|| fm.to_string());
        let md = highlight_with_extension(body, "md", prefs).unwrap_or_else(|| body.to_string());
        return format!("{yaml}{md}");
    }

    highlight_with_extension(text, "md", prefs).unwrap_or_else(|| text.to_string())
}

/// Returns `Some((frontmatter_with_fences, body))` if the input
/// begins with `---\n...\n---\n`, else `None`.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    let close = rest.find("\n---\n")?;
    let split_at = "---\n".len() + close + "\n---\n".len();
    Some(text.split_at(split_at))
}

fn highlight_with_extension(text: &str, ext: &str, prefs: &UiPrefs) -> Option<String> {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme: &Theme = ts
        .themes
        .get(&prefs.theme)
        .or_else(|| ts.themes.get(DEFAULT_THEME))?;
    let syntax = ss.find_syntax_by_extension(ext)?;
    let mut hl = HighlightLines::new(syntax, theme);
    let mut out = String::with_capacity(text.len() + 32);
    for line in syntect::util::LinesWithEndings::from(text) {
        let ranges: Vec<(Style, &str)> = hl.highlight_line(line, &ss).ok()?;
        out.push_str(&as_24_bit_terminal_escaped(&ranges, false));
    }
    // Reset terminal styling at end so subsequent output isn't tinted.
    out.push_str("\x1b[0m");
    Some(out)
}

/// Print `body` either directly to stdout or through a pager
/// subprocess depending on `prefs.use_pager()`. Pager command is
/// `$PAGER` if set, else `less -RFX`. On any pager-spawn failure,
/// falls back to direct print.
pub fn paginate(body: &str, prefs: &UiPrefs) -> anyhow::Result<()> {
    if !prefs.use_pager() {
        let mut out = std::io::stdout().lock();
        out.write_all(body.as_bytes())?;
        return Ok(());
    }

    let (cmd, args) = pager_command();
    let spawn_result = Command::new(&cmd).args(&args).stdin(Stdio::piped()).spawn();
    let mut child = match spawn_result {
        Ok(c) => c,
        Err(_) => {
            // Pager not available; print straight to stdout.
            let mut out = std::io::stdout().lock();
            out.write_all(body.as_bytes())?;
            return Ok(());
        }
    };
    {
        let stdin = child
            .stdin
            .as_mut()
            .context("pager stdin pipe was not opened")?;
        // Pager-write errors are usually "pager exited early" (q in
        // less). Treat as a normal end-of-output, not an error.
        let _ = stdin.write_all(body.as_bytes());
    }
    let _ = child.wait();
    Ok(())
}

fn pager_command() -> (String, Vec<String>) {
    if let Ok(env_pager) = std::env::var("PAGER") {
        let trimmed = env_pager.trim();
        if !trimmed.is_empty() {
            let mut parts = trimmed.split_whitespace();
            let bin = parts.next().unwrap().to_string();
            let args: Vec<String> = parts.map(str::to_string).collect();
            return (bin, args);
        }
    }
    // -R: pass ANSI escapes through; -F: quit if output fits one
    // screen; -X: don't clear the screen on exit.
    ("less".to_string(), vec!["-RFX".to_string()])
}

async fn themes(global_format: Option<crate::output::formats::OutputFormat>) -> anyhow::Result<()> {
    let global = crate::paths::global_dir()?;
    let project_root = std::env::current_dir()
        .ok()
        .and_then(|pwd| crate::paths::project_root_for(&pwd).ok().flatten());
    let cfg = rupu_config::layer_files(
        Some(&global.join("config.toml")),
        project_root
            .as_deref()
            .map(|root| root.join(".rupu/config.toml"))
            .as_deref(),
    )?;
    let current_palette = crate::output::theme::resolve_palette_theme(
        cfg.ui.palette.theme.as_deref().or(cfg.ui.theme.as_deref()),
        &global,
        project_root.as_deref(),
    )
    .name;
    let current_syntax = cfg
        .ui
        .syntax
        .theme
        .clone()
        .or(cfg.ui.theme.clone())
        .unwrap_or_else(|| DEFAULT_THEME.to_string());

    let mut rows = crate::output::theme::list_palette_themes(&global, project_root.as_deref())?
        .into_iter()
        .map(|theme| ThemeListRow {
            current: theme.name == current_palette,
            name: theme.name,
            kind: "palette".into(),
            source: theme.source,
            syntax_theme: theme.syntax_theme,
            description: theme.description,
        })
        .collect::<Vec<_>>();

    rows.extend(
        crate::output::theme::builtin_syntax_themes()
            .into_iter()
            .map(|theme| ThemeListRow {
                current: theme == current_syntax,
                name: theme.to_string(),
                kind: "syntax".into(),
                source: "builtin syntect".into(),
                syntax_theme: None,
                description: Some("Built-in syntect syntax theme".into()),
            }),
    );
    rows.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    let csv_rows = rows
        .iter()
        .map(|row| ThemeListCsvRow {
            name: row.name.clone(),
            kind: row.kind.clone(),
            source: row.source.clone(),
            syntax_theme: row.syntax_theme.clone().unwrap_or_default(),
            current: row.current,
            description: row.description.clone().unwrap_or_default(),
        })
        .collect();
    let output = ThemeListOutput {
        report: ThemeListReport {
            kind: "ui_theme_list",
            version: 1,
            rows,
        },
        csv_rows,
    };
    crate::output::report::emit_collection(global_format, &output)
}

async fn show(
    name: &str,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = crate::paths::global_dir()?;
    let project_root = std::env::current_dir()
        .ok()
        .and_then(|pwd| crate::paths::project_root_for(&pwd).ok().flatten());
    let item = if let Some(theme) =
        crate::output::theme::list_palette_themes(&global, project_root.as_deref())?
            .into_iter()
            .find(|theme| theme.name.eq_ignore_ascii_case(name))
    {
        ThemeShowItem {
            name: theme.name,
            kind: "palette".into(),
            source: theme.source,
            path: theme.path,
            syntax_theme: theme.syntax_theme,
            description: theme.description,
            palette: Some(theme.palette),
        }
    } else if crate::output::theme::builtin_syntax_themes()
        .iter()
        .any(|theme| theme.eq_ignore_ascii_case(name))
    {
        ThemeShowItem {
            name: name.to_string(),
            kind: "syntax".into(),
            source: "builtin syntect".into(),
            path: None,
            syntax_theme: Some(name.to_string()),
            description: Some("Built-in syntect syntax theme".into()),
            palette: None,
        }
    } else {
        anyhow::bail!("unknown theme `{name}`");
    };
    let output = ThemeShowOutput {
        report: ThemeShowReport {
            kind: "ui_theme_show",
            version: 1,
            item,
        },
    };
    crate::output::report::emit_detail(global_format, &output)
}

async fn validate(
    path: &PathBuf,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = crate::paths::global_dir()?;
    let project_root = std::env::current_dir()
        .ok()
        .and_then(|pwd| crate::paths::project_root_for(&pwd).ok().flatten());
    let theme = crate::output::theme::validate_theme_file(path, &global, project_root.as_deref())?;
    let output = ThemeShowOutput {
        report: ThemeShowReport {
            kind: "ui_theme_show",
            version: 1,
            item: ThemeShowItem {
                name: theme.name,
                kind: "palette".into(),
                source: theme.source,
                path: theme.path,
                syntax_theme: theme.syntax_theme,
                description: theme.description,
                palette: Some(theme.palette),
            },
        },
    };
    crate::output::report::emit_detail(global_format, &output)
}

async fn import(
    args: ImportArgs,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = crate::paths::global_dir()?;
    let project_root = std::env::current_dir()
        .ok()
        .and_then(|pwd| crate::paths::project_root_for(&pwd).ok().flatten());
    let theme = crate::output::theme::import_theme(
        &args.source,
        args.from,
        args.name.as_deref(),
        &global,
        project_root.as_deref(),
        args.project,
    )
    .await?;
    let output = ThemeShowOutput {
        report: ThemeShowReport {
            kind: "ui_theme_show",
            version: 1,
            item: ThemeShowItem {
                name: theme.name,
                kind: "palette".into(),
                source: theme.source,
                path: theme.path,
                syntax_theme: theme.syntax_theme,
                description: theme.description,
                palette: Some(theme.palette),
            },
        },
    };
    crate::output::report::emit_detail(global_format, &output)
}

fn format_hex(color: crate::output::palette::RgbColor) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_well_formed() {
        let s = "---\nname: foo\n---\nbody text\n";
        let (fm, body) = split_frontmatter(s).unwrap();
        assert_eq!(fm, "---\nname: foo\n---\n");
        assert_eq!(body, "body text\n");
    }

    #[test]
    fn split_frontmatter_missing_fence_returns_none() {
        assert!(split_frontmatter("name: foo\nbody").is_none());
    }

    #[test]
    fn ui_prefs_no_color_env_overrides_config() {
        std::env::set_var("NO_COLOR", "1");
        let cfg = UiConfig {
            color: Some("always".into()),
            theme: None,
            syntax: Default::default(),
            palette: Default::default(),
            pager: None,
        };
        let prefs = UiPrefs::resolve(&cfg, false, None, None);
        assert_eq!(prefs.color, ColorMode::Never);
        std::env::remove_var("NO_COLOR");
    }
}
