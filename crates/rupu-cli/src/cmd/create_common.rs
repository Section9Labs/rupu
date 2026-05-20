//! Shared prompts and target-path resolution for the `create` family of
//! commands (`agent create`, `workflow create`, `autoflow create`). The
//! philosophy mirrors `edit`: scope picker → name → write template → open
//! the user's editor. These helpers handle the first three steps; each
//! caller plugs in its own template + post-save validation.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// What is being created. Used in user-visible prompts.
pub fn prompt_scope(kind: &str, project_root: Option<&Path>) -> anyhow::Result<String> {
    if project_root.is_none() {
        // No `.rupu` directory walking up from cwd → project scope isn't
        // available. Don't bother asking, just use global.
        eprintln!("note: no project root detected — using global scope");
        return Ok("global".to_string());
    }
    loop {
        eprint!("scope for new {kind} [global/project]: ");
        io::stderr().flush().ok();
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let trimmed = line.trim().to_ascii_lowercase();
        match trimmed.as_str() {
            "g" | "global" => return Ok("global".to_string()),
            "p" | "project" => return Ok("project".to_string()),
            "" => continue,
            _ => eprintln!("  unrecognized — type `global` or `project`"),
        }
    }
}

pub fn prompt_name(kind: &str) -> anyhow::Result<String> {
    loop {
        eprint!("{kind} name: ");
        io::stderr().flush().ok();
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            eprintln!("  name cannot be empty");
            continue;
        }
        if let Err(e) = validate_name(&trimmed) {
            eprintln!("  {e}");
            continue;
        }
        return Ok(trimmed);
    }
}

/// Names are used as filenames (`<name>.md`, `<name>.yaml`). Be strict so
/// we don't have to escape anything later: lowercase letters, digits,
/// dash, underscore; must start with a letter.
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("name cannot be empty");
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphabetic() {
        anyhow::bail!("name must start with a letter (got `{first}`)");
    }
    for c in name.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            anyhow::bail!(
                "name may only contain letters, digits, `-`, and `_` (got `{c}`)"
            );
        }
    }
    Ok(())
}

/// Resolve `<root>/<subdir>` for the chosen scope. `subdir` is the per-kind
/// folder (`agents` or `workflows`). Returns the directory, not the file —
/// the caller composes the filename.
pub fn target_dir(
    scope: &str,
    global: &Path,
    project_root: Option<&Path>,
    subdir: &str,
) -> anyhow::Result<PathBuf> {
    match scope {
        "global" => Ok(global.join(subdir)),
        "project" => {
            let root = project_root.ok_or_else(|| {
                anyhow::anyhow!(
                    "no project root detected; cannot create at project scope"
                )
            })?;
            Ok(root.join(".rupu").join(subdir))
        }
        other => anyhow::bail!("invalid scope `{other}` (expected `global` or `project`)"),
    }
}
