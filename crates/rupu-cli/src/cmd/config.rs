//! `rupu config get | set <key> [value]`. Scoped to ~/.rupu/config.toml.

use crate::paths;
use clap::Subcommand;
use std::process::ExitCode;
use toml::Value;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Print the value of a top-level key.
    Get { key: String },
    /// Set a top-level key. The value is parsed as a TOML scalar
    /// (string / integer / bool); to set a table or array, hand-edit
    /// the file at `~/.rupu/config.toml`.
    Set { key: String, value: String },
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::Get { key } => match get(&key).await {
            Ok(v) => {
                println!("{v}");
                ExitCode::from(0)
            }
            Err(e) => crate::output::diag::fail(e)
        },
        Action::Set { key, value } => match set(&key, &value).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e)
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
    let val = v
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("key not set: {key}"))?;
    Ok(format!("{val}"))
}

async fn set(key: &str, value: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let path = global.join("config.toml");
    let mut v: Value = if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        toml::from_str(&text).unwrap_or_else(|_| Value::Table(Default::default()))
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
    if let Value::Table(t) = &mut v {
        t.insert(key.to_string(), parsed);
    }
    let serialized = toml::to_string_pretty(&v)?;
    std::fs::write(&path, serialized)?;
    Ok(())
}
