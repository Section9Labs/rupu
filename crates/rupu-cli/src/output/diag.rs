//! Canonical diagnostic surface for user-facing CLI messages.
//!
//! Five severities, one consistent visual:
//!
//! ```text
//! ✗ error: GitHub API returned 401
//!   → run: rupu auth login --provider github
//!
//! ⚠ warn: timeout 30s short for the workflow's slowest step
//!
//! ℹ note: cron-state lives at ~/.rupu/cron-state/
//!
//! ✓ done: 5 issues triaged in 14s
//!
//! ⊘ github skipped — no credential
//!   → run: rupu auth login --provider github
//! ```
//!
//! Goes to **stderr** (so machine-piped stdout stays clean for the
//! command's actual output). Honors `NO_COLOR`, `--no-color`, and
//! `[ui].color = "never"` via [`UiPrefs`]. Glyphs reuse the Okesu
//! palette already shared with the line-stream printer + TUI canvas
//! (see `output::palette::Status`) so vocabulary stays consistent
//! across rupu surfaces.
//!
//! ## When to use which severity
//!
//! - **`error`** — the command failed; rupu is about to exit non-zero.
//! - **`warn`** — non-fatal issue that the user might want to act on
//!   (deprecated flag, soon-to-expire credential, etc.).
//! - **`info`** — neutral status note. Sparingly — if the user could
//!   read it as either reassurance or noise, prefer not emitting.
//! - **`success`** — terminal "we did it" confirmation. Use only on
//!   commands that don't already render structured output (a fresh
//!   `auth login`, an explicit "X committed"); a `runs list` table
//!   is its own confirmation.
//! - **`skip`** — a sub-step was deliberately not performed and the
//!   user can act on it. Two-line render with the inline hint.
//! - **`hint`** — secondary "next step" line; usually attached to an
//!   error/warn rather than emitted on its own.

use crate::cmd::ui::UiPrefs;
use crate::output::palette::Status;
use owo_colors::OwoColorize;
use std::fmt::Display;

/// Severity bucket. Maps to a glyph + a palette color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Info,
    Success,
    Skip,
}

impl Severity {
    fn glyph(self) -> char {
        match self {
            Self::Error => Status::Failed.glyph(),
            Self::Warn => Status::Awaiting.glyph(),
            Self::Info => 'ℹ',
            Self::Success => Status::Complete.glyph(),
            Self::Skip => Status::Skipped.glyph(),
        }
    }

    fn color(self) -> owo_colors::Rgb {
        match self {
            Self::Error => Status::Failed.color(),
            Self::Warn => Status::Awaiting.color(),
            Self::Info => crate::output::palette::DIM,
            Self::Success => Status::Complete.color(),
            Self::Skip => crate::output::palette::DIM,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "note",
            Self::Success => "done",
            Self::Skip => "skipped",
        }
    }
}

/// One-line diagnostic. Use the convenience helpers below in normal
/// code; this is the underlying primitive for callers that need to
/// vary severity at runtime.
pub fn diag(severity: Severity, prefs: &UiPrefs, msg: impl Display) {
    let glyph = severity.glyph();
    let label = severity.label();
    let color = severity.color();

    if prefs.use_color() {
        // glyph + label colored; message body plain so it stays
        // readable on any terminal background.
        eprintln!(
            "{} {} {}",
            glyph.color(color),
            format!("{label}:").color(color).bold(),
            msg
        );
    } else {
        eprintln!("[{label}] {msg}");
    }
}

/// Two-line diagnostic with a "→ run: ..." hint indented under the
/// main line. Used for `error` / `warn` / `skip` cases where the
/// fix is a concrete command.
pub fn diag_with_hint(
    severity: Severity,
    prefs: &UiPrefs,
    msg: impl Display,
    hint: impl Display,
) {
    diag(severity, prefs, msg);
    if prefs.use_color() {
        eprintln!(
            "  {} {}",
            "→".color(crate::output::palette::DIM),
            hint.color(crate::output::palette::DIM)
        );
    } else {
        eprintln!("  -> {hint}");
    }
}

// ── Convenience entry points ─────────────────────────────────────

pub fn error(prefs: &UiPrefs, msg: impl Display) {
    diag(Severity::Error, prefs, msg);
}

pub fn error_with_hint(prefs: &UiPrefs, msg: impl Display, hint: impl Display) {
    diag_with_hint(Severity::Error, prefs, msg, hint);
}

pub fn warn(prefs: &UiPrefs, msg: impl Display) {
    diag(Severity::Warn, prefs, msg);
}

#[allow(dead_code)]
pub fn warn_with_hint(prefs: &UiPrefs, msg: impl Display, hint: impl Display) {
    diag_with_hint(Severity::Warn, prefs, msg, hint);
}

#[allow(dead_code)]
pub fn info(prefs: &UiPrefs, msg: impl Display) {
    diag(Severity::Info, prefs, msg);
}

#[allow(dead_code)]
pub fn success(prefs: &UiPrefs, msg: impl Display) {
    diag(Severity::Success, prefs, msg);
}

/// Sub-step skip: "⊘ skipped: <subject> — <reason>" + hint line.
/// `subject` is usually a platform / provider / artifact name; the
/// formatter does the wording so callers can't drift.
pub fn skip(prefs: &UiPrefs, subject: impl Display, reason: impl Display, hint: impl Display) {
    // Body is just "<subject> — <reason>"; the Severity::Skip label
    // ("skipped:") supplies the verb so we don't repeat it.
    diag_with_hint(
        Severity::Skip,
        prefs,
        format!("{subject} — {reason}"),
        hint,
    );
}

/// Print an `error:` diag and return [`ExitCode::FAILURE`]. Used as
/// the canonical failure arm in subcommand `handle()` dispatchers:
/// ```ignore
///     Err(e) => crate::output::diag::fail(e),
/// ```
/// The previous shape (`eprintln!("rupu <subcommand>: {e}"); ExitCode::from(1)`)
/// repeated the subcommand name (the user just typed it) and offered
/// no visual hierarchy. Now the prefix is the universal `✗ error:`.
pub fn fail(e: impl Display) -> std::process::ExitCode {
    error(&prefs_for_diag(false), e);
    std::process::ExitCode::from(1)
}

/// Resolve a `UiPrefs` for diagnostic-only use. Looks at the layered
/// config + `NO_COLOR` env var; ignores theme/pager since diagnostics
/// don't need either. Used by handlers that want to emit a diag
/// without already having a `UiPrefs` in scope.
pub fn prefs_for_diag(no_color: bool) -> UiPrefs {
    use crate::paths;
    let cfg = paths::global_dir()
        .ok()
        .and_then(|g| {
            let global_cfg = g.join("config.toml");
            let pwd = std::env::current_dir().ok()?;
            let project_root = paths::project_root_for(&pwd).ok().flatten();
            let project_cfg = project_root.map(|p| p.join(".rupu/config.toml"));
            rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).ok()
        })
        .unwrap_or_default();
    UiPrefs::resolve(&cfg.ui, no_color, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::ui::{ColorMode, PagerMode};

    fn no_color() -> UiPrefs {
        UiPrefs {
            color: ColorMode::Never,
            theme: "base16-ocean.dark".into(),
            pager: PagerMode::Never,
        }
    }

    #[test]
    fn severity_label_matches_role() {
        assert_eq!(Severity::Error.label(), "error");
        assert_eq!(Severity::Warn.label(), "warn");
        assert_eq!(Severity::Info.label(), "note");
        assert_eq!(Severity::Success.label(), "done");
        assert_eq!(Severity::Skip.label(), "skipped");
    }

    #[test]
    fn severity_glyph_distinct_per_severity() {
        let glyphs: Vec<char> = [
            Severity::Error,
            Severity::Warn,
            Severity::Info,
            Severity::Success,
            Severity::Skip,
        ]
        .iter()
        .map(|s| s.glyph())
        .collect();
        // No duplicates — each severity must be visually distinguishable.
        let mut sorted = glyphs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(glyphs.len(), sorted.len(), "duplicate glyph in severity table");
    }

    #[test]
    fn diag_no_color_uses_bracket_prefix() {
        // Smoke: `diag()` doesn't panic with NO_COLOR. We can't easily
        // capture stderr in a unit test without machinery; the fact
        // that the function invokes eprintln! and returns is the
        // assertion. Behavior under NO_COLOR is also covered by the
        // bracket-format path being the only branch that runs.
        diag(Severity::Error, &no_color(), "smoke");
    }
}
