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
use rupu_config::UiConfig;
use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

const DEFAULT_THEME: &str = "base16-ocean.dark";

#[derive(Debug, Clone)]
pub struct UiPrefs {
    pub color: ColorMode,
    pub theme: String,
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
            .or_else(|| cfg.theme.clone())
            .unwrap_or_else(|| DEFAULT_THEME.to_string());
        let pager = match flag_pager {
            Some(true) => PagerMode::Always,
            Some(false) => PagerMode::Never,
            None => parse_pager(cfg.pager.as_deref()).unwrap_or(PagerMode::Auto),
        };
        UiPrefs {
            color,
            theme,
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
            pager: None,
        };
        let prefs = UiPrefs::resolve(&cfg, false, None, None);
        assert_eq!(prefs.color, ColorMode::Never);
        std::env::remove_var("NO_COLOR");
    }
}
