//! Shared editor-spawn helper used by `agent edit` and `workflow edit`.
//!
//! Resolves the editor binary in this order, matching POSIX convention:
//!   1. explicit `--editor <bin>` flag (passed through by the caller)
//!   2. `$VISUAL`
//!   3. `$EDITOR`
//!   4. fallback: `vi` on Unix, `notepad` on Windows
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
