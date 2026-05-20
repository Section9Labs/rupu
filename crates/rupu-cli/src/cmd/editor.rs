//! Shared editor-spawn helper used by `agent edit/create`,
//! `workflow edit/create`, and `autoflow create`.
//!
//! Resolution order (highest priority first):
//!   1. explicit `--editor <bin>` flag (passed through by the caller)
//!   2. `[ui].editor` from layered rupu config (project shadows global)
//!   3. `$VISUAL`
//!   4. `$EDITOR`
//!   5. fallback: `vi` on Unix, `notepad` on Windows
//!
//! Config is placed above env vars because `$VISUAL`/`$EDITOR` are often
//! left at distro defaults (`vi`) — a value the user wrote into their
//! rupu config is an explicit preference and should win.
//!
//! The resolved value may include flags (`code --wait`, `vim -p`); we
//! split on whitespace and pass the file path as the final positional.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Command;

/// Resolve the editor command and spawn it on `path`, blocking until
/// the process exits. Inherits stdio so terminal editors (vi, nvim,
/// emacs -nw) work correctly.
pub fn open_for_edit(explicit: Option<&str>, path: &Path) -> Result<()> {
    let editor_str = resolve_editor(explicit)?;
    let mut parts = editor_str.split_whitespace();
    let bin = parts
        .next()
        .ok_or_else(|| anyhow!("editor command was empty"))?;
    let extra_args: Vec<&str> = parts.collect();

    let status = Command::new(bin)
        .args(&extra_args)
        .arg(path)
        .status()
        .with_context(|| format!("spawn editor `{bin}`"))?;

    if !status.success() {
        return Err(anyhow!(
            "editor `{}` exited with non-zero status: {}",
            editor_str,
            status
        ));
    }
    Ok(())
}

fn resolve_editor(explicit: Option<&str>) -> Result<String> {
    if let Some(e) = explicit {
        return Ok(e.to_string());
    }
    if let Some(e) = config_editor() {
        if !e.trim().is_empty() {
            return Ok(e);
        }
    }
    if let Ok(v) = std::env::var("VISUAL") {
        if !v.trim().is_empty() {
            return Ok(v);
        }
    }
    if let Ok(e) = std::env::var("EDITOR") {
        if !e.trim().is_empty() {
            return Ok(e);
        }
    }
    if cfg!(windows) {
        Ok("notepad".to_string())
    } else {
        Ok("vi".to_string())
    }
}

/// Best-effort lookup of `[ui].editor` from the layered config. Any
/// I/O / parse failure silently falls through to env vars and the OS
/// default — `edit`/`create` must never refuse to open over a config
/// issue.
fn config_editor() -> Option<String> {
    let global = crate::paths::global_dir().ok()?;
    let global_cfg = global.join("config.toml");
    let pwd = std::env::current_dir().ok()?;
    let project_root = crate::paths::project_root_for(&pwd).ok().flatten();
    let project_cfg = project_root.map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).ok()?;
    cfg.ui.editor
}
