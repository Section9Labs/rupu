//! Config write-path safety: validate against the typed schema, then persist
//! atomically with a backup. Used by the `api/config` write endpoints.

use std::path::Path;

use crate::host::connector::MAX_WORKSPACE_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum ConfigWriteError {
    #[error("invalid config: {0}")]
    Validate(String),
    #[error("config io: {0}")]
    Io(String),
    #[error("config busy: {0}")]
    Locked(String),
}

/// Parse `candidate` into the typed `rupu_config::Config` (deny_unknown_fields
/// rejects unknown keys) and run its `validate()`. Does not touch disk.
pub fn validate_toml(candidate: &str) -> Result<(), ConfigWriteError> {
    let cfg: rupu_config::Config =
        toml::from_str(candidate).map_err(|e| ConfigWriteError::Validate(e.to_string()))?;
    cfg.validate()
        .map_err(|e| ConfigWriteError::Validate(e.to_string()))?;
    Ok(())
}

/// Validate then persist atomically: back up the prior file to `<file>.bak`,
/// write a temp sibling, and rename over the target. An advisory lock on a
/// `<file>.lock` sidecar serializes concurrent writers.
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), ConfigWriteError> {
    validate_toml(contents)?;
    let parent = path
        .parent()
        .ok_or_else(|| ConfigWriteError::Io("config path has no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|e| ConfigWriteError::Io(e.to_string()))?;

    // Advisory lock (best-effort serialization).
    let lock_path = path.with_extension("lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)
        .map_err(|e| ConfigWriteError::Io(e.to_string()))?;
    fs2::FileExt::lock_exclusive(&lock_file)
        .map_err(|e| ConfigWriteError::Locked(e.to_string()))?;

    // Backup existing.
    if path.exists() {
        let bak = path.with_extension("toml.bak");
        std::fs::copy(path, &bak).map_err(|e| ConfigWriteError::Io(e.to_string()))?;
    }
    // Temp write + rename.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, contents).map_err(|e| ConfigWriteError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| ConfigWriteError::Io(e.to_string()))?;

    let _ = fs2::FileExt::unlock(&lock_file);
    Ok(())
}

/// Apply a flat JSON object of `dotted.key -> value` edits onto the existing
/// TOML text, preserving comments and layout (`toml_edit`). The caller then
/// runs the result through `write_atomic` (which re-validates).
pub fn apply_form_patch(
    existing_toml: &str,
    patch: &serde_json::Value,
) -> Result<String, ConfigWriteError> {
    let mut doc: toml_edit::DocumentMut = existing_toml
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigWriteError::Validate(e.to_string()))?;
    let obj = patch
        .as_object()
        .ok_or_else(|| ConfigWriteError::Validate("patch must be a JSON object".into()))?;
    for (dotted, val) in obj {
        set_dotted(&mut doc, dotted, val)?;
    }
    Ok(doc.to_string())
}

fn set_dotted(
    doc: &mut toml_edit::DocumentMut,
    dotted: &str,
    val: &serde_json::Value,
) -> Result<(), ConfigWriteError> {
    let item = json_to_toml_item(val)?;
    let parts: Vec<&str> = dotted.split('.').collect();
    // Navigate/create intermediate tables.
    let mut cur = doc.as_table_mut();
    for p in &parts[..parts.len() - 1] {
        if cur.get(p).is_none() {
            cur.insert(p, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        cur = cur
            .get_mut(p)
            .and_then(|i| i.as_table_mut())
            .ok_or_else(|| ConfigWriteError::Validate(format!("`{p}` is not a table")))?;
    }
    // Index-assign rather than `Table::insert`: `insert` normalizes
    // (`Key::fmt`) the key formatting on an existing entry, which strips any
    // leading comment attached to it. Index assignment (`entry(..).or_insert`
    // under the hood) only replaces the value, preserving key decor.
    cur[parts[parts.len() - 1]] = item;
    Ok(())
}

/// Convert a JSON scalar/array into a `toml_edit::Item`. Supported types:
/// string, bool, integer, float, and arrays of those scalars. Anything else
/// (object, null, mixed/unsupported array element) is rejected — callers only
/// ever patch flat config-form fields, never arbitrary nested structures.
fn json_to_toml_item(v: &serde_json::Value) -> Result<toml_edit::Item, ConfigWriteError> {
    use toml_edit::value;
    match v {
        serde_json::Value::String(s) => Ok(value(s.clone())),
        serde_json::Value::Bool(b) => Ok(value(*b)),
        serde_json::Value::Number(n) if n.is_i64() => Ok(value(n.as_i64().unwrap())),
        serde_json::Value::Number(n) if n.is_f64() => Ok(value(n.as_f64().unwrap())),
        serde_json::Value::Array(a) => {
            let mut arr = toml_edit::Array::new();
            for e in a {
                match e {
                    serde_json::Value::String(s) => arr.push(s.clone()),
                    serde_json::Value::Bool(b) => arr.push(*b),
                    serde_json::Value::Number(n) if n.is_i64() => arr.push(n.as_i64().unwrap()),
                    other => {
                        return Err(ConfigWriteError::Validate(format!(
                            "unsupported array element: {other}"
                        )))
                    }
                }
            }
            Ok(value(arr))
        }
        other => Err(ConfigWriteError::Validate(format!(
            "unsupported value type: {other}"
        ))),
    }
}

/// The effective workspace-payload cap: the `[cp].max_workspace_bytes` config
/// override if set, else the compiled `MAX_WORKSPACE_BYTES` default.
pub fn effective_max_workspace_bytes(cp: &rupu_config::CpConfig) -> usize {
    cp.max_workspace_bytes
        .map(|v| v as usize)
        .unwrap_or(MAX_WORKSPACE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_good_config() {
        assert!(validate_toml("default_model = \"opus\"\n").is_ok());
    }

    #[test]
    fn validate_rejects_unknown_key() {
        // Config is deny_unknown_fields.
        let err = validate_toml("not_a_real_key = 1\n").unwrap_err();
        assert!(matches!(err, ConfigWriteError::Validate(_)));
    }

    #[test]
    fn validate_rejects_type_error() {
        let err = validate_toml("default_model = 123\n").unwrap_err();
        assert!(matches!(err, ConfigWriteError::Validate(_)));
    }

    #[test]
    fn write_atomic_backs_up_and_replaces() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        std::fs::write(&f, "default_model = \"old\"\n").unwrap();
        write_atomic(&f, "default_model = \"new\"\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "default_model = \"new\"\n"
        );
        // prior content backed up
        assert_eq!(
            std::fs::read_to_string(f.with_extension("toml.bak")).unwrap(),
            "default_model = \"old\"\n"
        );
    }

    #[test]
    fn write_atomic_creates_new_file_without_backup() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        write_atomic(&f, "default_model = \"x\"\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "default_model = \"x\"\n"
        );
    }

    #[test]
    fn form_patch_preserves_comments() {
        let existing = "# my config\ndefault_model = \"opus\"\n";
        let patch = serde_json::json!({ "default_model": "sonnet" });
        let out = apply_form_patch(existing, &patch).unwrap();
        assert!(out.contains("# my config"), "comment preserved: {out}");
        assert!(out.contains("default_model = \"sonnet\""));
    }

    #[test]
    fn effective_limit_uses_config_then_default() {
        let cp = rupu_config::CpConfig {
            max_workspace_bytes: Some(1024),
        };
        assert_eq!(effective_max_workspace_bytes(&cp), 1024);
        let cp_def = rupu_config::CpConfig::default();
        assert_eq!(
            effective_max_workspace_bytes(&cp_def),
            crate::host::connector::MAX_WORKSPACE_BYTES
        );
    }
}
