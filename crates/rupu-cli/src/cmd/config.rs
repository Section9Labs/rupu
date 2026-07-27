//! `rupu config get | set <key> [value]`. Scoped to ~/.rupu/config.toml.

use crate::paths;
use clap::Subcommand;
use std::process::ExitCode;
use toml::Value;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Print the value of a key. Dotted keys (`ui.theme`) descend into
    /// nested tables.
    Get { key: String },
    /// Set a key. Dotted keys (`ui.theme`) descend into nested tables,
    /// creating them as needed. The value is parsed as a TOML scalar
    /// (string / integer / bool).
    Set { key: String, value: String },
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::Get { key } => match get(&key).await {
            Ok(v) => {
                println!("{v}");
                ExitCode::from(0)
            }
            Err(e) => crate::output::diag::fail(e),
        },
        Action::Set { key, value } => match set(&key, &value).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
        },
    }
}

async fn get(key: &str) -> anyhow::Result<String> {
    let global = paths::global_dir()?;
    let path = global.join("config.toml");
    if !path.exists() {
        anyhow::bail!("config file does not exist: {}", path.display());
    }
    let text = std::fs::read_to_string(&path)?;
    let v: Value = toml::from_str(&text)?;
    let val = get_path(&v, key).ok_or_else(|| anyhow::anyhow!("key not set: {key}"))?;
    Ok(format!("{val}"))
}

async fn set(key: &str, value: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let path = global.join("config.toml");
    let mut v: Value = if path.exists() {
        let text = std::fs::read_to_string(&path)?;
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
    let parsed: Value = toml::from_str(&format!("__v = {value}"))
        .map(|t: Value| {
            t.get("__v")
                .cloned()
                .unwrap_or(Value::String(value.to_string()))
        })
        .unwrap_or(Value::String(value.to_string()));
    set_path(&mut v, key, parsed)?;
    let serialized = toml::to_string_pretty(&v)?;
    std::fs::write(&path, serialized)?;
    Ok(())
}

/// Read a dotted key (`ui.theme`) by descending nested tables.
fn get_path<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let mut cur = v;
    for seg in key.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// Write a dotted key (`ui.theme`), creating intermediate tables as needed.
/// Refuses to replace an existing non-table with a table — that would silently
/// discard the user's value.
fn set_path(v: &mut Value, key: &str, val: Value) -> anyhow::Result<()> {
    let segs: Vec<&str> = key.split('.').collect();
    let (last, parents) = segs.split_last().expect("split always yields one segment");
    let mut cur = v;
    for seg in parents {
        if !matches!(cur.get(*seg), Some(Value::Table(_))) {
            if cur.get(*seg).is_some() {
                anyhow::bail!("cannot set `{key}`: `{seg}` is already a value, not a table");
            }
            let table = cur.as_table_mut().ok_or_else(|| {
                anyhow::anyhow!("cannot set `{key}`: `{seg}` has no parent table")
            })?;
            table.insert((*seg).to_string(), Value::Table(Default::default()));
        }
        cur = cur.get_mut(*seg).expect("just inserted");
    }
    cur.as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("cannot set `{key}`: parent is not a table"))?
        .insert((*last).to_string(), val);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_path_descends_into_a_nested_table() {
        let mut v: Value = toml::from_str("[ui]\ntheme = \"old\"\n").unwrap();
        set_path(&mut v, "ui.theme", Value::String("dracula".into())).unwrap();
        let rendered = toml::to_string_pretty(&v).unwrap();
        // The dotted key must NOT appear as a literal top-level key.
        assert!(
            !rendered.contains("\"ui.theme\""),
            "wrote a literal dotted key:\n{rendered}"
        );
        assert_eq!(
            v.get("ui")
                .and_then(|u| u.get("theme"))
                .and_then(|t| t.as_str()),
            Some("dracula")
        );
    }

    #[test]
    fn set_path_creates_missing_intermediate_tables() {
        let mut v = Value::Table(Default::default());
        set_path(&mut v, "bash.timeout_secs", Value::Integer(30)).unwrap();
        assert_eq!(
            v.get("bash")
                .and_then(|b| b.get("timeout_secs"))
                .and_then(|t| t.as_integer()),
            Some(30)
        );
    }

    #[test]
    fn set_path_refuses_to_overwrite_a_scalar_with_a_table() {
        let mut v: Value = toml::from_str("log_level = \"warn\"\n").unwrap();
        let err = set_path(&mut v, "log_level.nested", Value::Integer(1)).unwrap_err();
        assert!(err.to_string().contains("log_level"), "got: {err}");
    }

    #[test]
    fn get_path_reads_a_nested_key() {
        let v: Value = toml::from_str("[ui]\ntheme = \"dracula\"\n").unwrap();
        assert_eq!(
            get_path(&v, "ui.theme").and_then(|t| t.as_str()),
            Some("dracula")
        );
        assert!(get_path(&v, "ui.missing").is_none());
        assert!(get_path(&v, "nope.nope").is_none());
    }

    #[test]
    fn a_top_level_key_still_works() {
        let mut v = Value::Table(Default::default());
        set_path(&mut v, "log_level", Value::String("debug".into())).unwrap();
        assert_eq!(
            get_path(&v, "log_level").and_then(|t| t.as_str()),
            Some("debug")
        );
    }
}
