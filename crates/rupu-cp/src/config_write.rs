//! Config write-path safety: validate against the typed schema, then persist
//! atomically with a backup. Used by the `api/config` write endpoints.
//!
//! [`write_atomic_raw`] is the same backup+atomic mechanism minus the
//! TOML-schema gate, for callers editing a non-TOML file under a different
//! validator (currently `api::autoflows`'s enable/disable endpoint, which
//! validates workflow YAML via `rupu_orchestrator::Workflow::parse`).

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

/// Backup + atomic write for files `write_atomic`'s `validate_toml` gate does
/// not apply to (e.g. workflow YAML) — same backup/lock/rename mechanics,
/// minus the TOML-schema check. Callers own validating `contents` first (see
/// `api::autoflows`'s enable/disable endpoint, which validates via
/// `rupu_orchestrator::Workflow::parse`).
///
/// Backup/lock/temp siblings are named by *appending* `.bak` / `.lock` /
/// `.tmp` to the full file name (`nightly.yaml` -> `nightly.yaml.bak`),
/// unlike `write_atomic`'s TOML-specific `path.with_extension("toml.bak")`
/// scheme, which would rename-away a non-`.toml` extension.
pub fn write_atomic_raw(path: &Path, contents: &str) -> Result<(), ConfigWriteError> {
    let parent = path
        .parent()
        .ok_or_else(|| ConfigWriteError::Io("path has no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|e| ConfigWriteError::Io(e.to_string()))?;

    let append = |suffix: &str| {
        let mut s = path.as_os_str().to_os_string();
        s.push(".");
        s.push(suffix);
        std::path::PathBuf::from(s)
    };

    // Advisory lock (best-effort serialization), mirroring `write_atomic`.
    let lock_path = append("lock");
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
        let bak = append("bak");
        std::fs::copy(path, &bak).map_err(|e| ConfigWriteError::Io(e.to_string()))?;
    }
    // Temp write + rename.
    let tmp = append("tmp");
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

/// Inverse of `rupu_config::resolve`'s private `dotted()` encoding (kept in
/// lockstep with it, and with the frontend's `splitDottedKey` in
/// `ConfigEditor.tsx`): split a dotted key path on `.`, except inside a
/// `"…"`-quoted segment, where `\"` unescapes to `"`. A quoted segment must
/// span the whole segment between separators — e.g.
/// `pricing.oracle."/raid/models/zai-org/GLM-5.2-FP8".input_per_mtok` splits
/// to `["pricing", "oracle", "/raid/models/zai-org/GLM-5.2-FP8",
/// "input_per_mtok"]` — so a dotted model id round-trips through the write
/// path instead of being torn apart on every embedded `.`.
///
/// **Strict-write, lenient-read asymmetry (deliberate):** this is the WRITE
/// path's decoder, so it rejects anything that isn't an unambiguous encoding
/// of a real config key — malformed quoting (an unterminated quote, or a `"`
/// that doesn't span exactly one segment) AND any EMPTY segment (`.`, `a.`,
/// `.x`, `a..b` are all `Err`, including a trailing separator's *implied*
/// empty final segment — a real TOML key segment in a rupu config is never
/// empty, so treating one as valid here would silently accept a key that can
/// never correspond to an actual config field). This is intentionally
/// stricter than the frontend's `splitDottedKey`, which stays lenient
/// (never throws, degrades to a harmless failed `getPath` lookup) because it
/// only ever renders a UI field — see that function's doc comment. Never
/// silently drop a bogus segment either way; on the write side, refuse.
fn split_dotted_key(dotted: &str) -> Result<Vec<String>, ConfigWriteError> {
    let mut segments = Vec::new();
    let mut chars = dotted.chars().peekable();
    loop {
        let mut seg = String::new();
        // Whether this segment was terminated by consuming a `.` separator
        // (as opposed to running out of input) — if so, the separator
        // IMPLIES another segment follows (even if input is now exhausted,
        // in which case that implied segment is empty and gets rejected
        // below on the next iteration, rather than being silently dropped).
        let mut separator_consumed = false;
        if chars.peek() == Some(&'"') {
            chars.next(); // consume opening quote
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' if chars.peek() == Some(&'"') => {
                        chars.next();
                        seg.push('"');
                    }
                    other => seg.push(other),
                }
            }
            if !closed {
                return Err(ConfigWriteError::Validate(format!(
                    "malformed config key `{dotted}`: unterminated quoted segment"
                )));
            }
            // A quoted segment must span the whole segment: the next char
            // (if any) must be the `.` separator, never more content fused
            // onto the closing quote.
            match chars.peek() {
                None => {}
                Some('.') => {
                    chars.next();
                    separator_consumed = true;
                }
                Some(_) => {
                    return Err(ConfigWriteError::Validate(format!(
                        "malformed config key `{dotted}`: quoted segment must span the whole segment"
                    )));
                }
            }
        } else {
            loop {
                match chars.next() {
                    Some('.') => {
                        separator_consumed = true;
                        break;
                    }
                    Some('"') => {
                        return Err(ConfigWriteError::Validate(format!(
                            "malformed config key `{dotted}`: stray `\"` mid-segment"
                        )));
                    }
                    Some(c) => seg.push(c),
                    None => break,
                }
            }
        }
        if seg.is_empty() {
            return Err(ConfigWriteError::Validate(format!(
                "malformed config key `{dotted}`: empty key segment"
            )));
        }
        segments.push(seg);
        if !separator_consumed {
            break;
        }
    }
    Ok(segments)
}

fn set_dotted(
    doc: &mut toml_edit::DocumentMut,
    dotted: &str,
    val: &serde_json::Value,
) -> Result<(), ConfigWriteError> {
    let item = json_to_toml_item(val)?;
    let parts = split_dotted_key(dotted)?;
    if parts.is_empty() {
        return Err(ConfigWriteError::Validate(format!(
            "malformed config key `{dotted}`: empty"
        )));
    }
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
    // toml_edit re-quotes/escapes the raw segment itself when serializing the
    // key, so inserting it unescaped here (e.g. `/raid/models/zai-org/GLM-5.2-FP8`)
    // still produces valid, round-trippable TOML.
    cur[parts[parts.len() - 1].as_str()] = item;
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
    fn split_dotted_key_simple() {
        assert_eq!(
            split_dotted_key("autoflow.max_active").unwrap(),
            vec!["autoflow".to_string(), "max_active".to_string()]
        );
        assert_eq!(
            split_dotted_key("default_model").unwrap(),
            vec!["default_model".to_string()]
        );
    }

    #[test]
    fn split_dotted_key_quoted_segment() {
        let parts = split_dotted_key(
            "pricing.oracle.\"/raid/models/zai-org/GLM-5.2-FP8\".input_per_mtok",
        )
        .unwrap();
        assert_eq!(
            parts,
            vec![
                "pricing".to_string(),
                "oracle".to_string(),
                "/raid/models/zai-org/GLM-5.2-FP8".to_string(),
                "input_per_mtok".to_string(),
            ]
        );
    }

    #[test]
    fn split_dotted_key_rejects_unterminated_quote() {
        let err = split_dotted_key("pricing.oracle.\"unterminated").unwrap_err();
        assert!(matches!(err, ConfigWriteError::Validate(_)));
    }

    #[test]
    fn split_dotted_key_rejects_quote_mid_segment() {
        let err = split_dotted_key("pricing.oracle.\"a\"b.input_per_mtok").unwrap_err();
        assert!(matches!(err, ConfigWriteError::Validate(_)));
    }

    #[test]
    fn split_dotted_key_rejects_empty_segments() {
        // Every one of these implies at least one empty segment — a bare
        // `.`, a leading/trailing separator, an internal double separator,
        // or a quoted segment immediately followed by a trailing separator.
        // None of these can ever be a real config key, so the write path
        // must reject them outright rather than silently dropping the
        // implied-empty segment (the bug this test set closes).
        for bad in [".", "a.", ".x", "a..b", "\"x\"."] {
            let err = split_dotted_key(bad)
                .expect_err(&format!("expected Err for {bad:?}, got Ok"));
            assert!(matches!(err, ConfigWriteError::Validate(_)), "{bad:?}: {err:?}");
        }
    }

    #[test]
    fn form_patch_writes_dotted_model_key_as_raw_table_not_split() {
        // A pricing patch key for a model id containing a literal `.`
        // (quoted in the canonical dotted-key encoding) must land as ONE
        // raw-segment table, not get torn into a bogus nested `GLM-5`/`2-FP8`
        // table by a naive `split('.')`.
        let existing = "default_model = \"opus\"\n";
        let patch = serde_json::json!({
            "pricing.oracle.\"/raid/models/zai-org/GLM-5.2-FP8\".input_per_mtok": 1.5
        });
        let out = apply_form_patch(existing, &patch).unwrap();
        let parsed: toml::Value = toml::from_str(&out).expect("valid toml");
        let val = parsed
            .get("pricing")
            .and_then(|v| v.get("oracle"))
            .and_then(|v| v.get("/raid/models/zai-org/GLM-5.2-FP8"))
            .and_then(|v| v.get("input_per_mtok"))
            .and_then(|v| v.as_float())
            .expect("dotted model key must round-trip intact");
        assert_eq!(val, 1.5);
        // No bogus split-table artifact.
        assert!(
            parsed
                .get("pricing")
                .and_then(|v| v.get("oracle"))
                .and_then(|v| v.get("GLM-5"))
                .is_none(),
            "must not have split the model id into a `GLM-5` table: {out}"
        );
    }

    #[test]
    fn form_patch_simple_keys_unaffected() {
        let existing = "default_model = \"opus\"\n";
        let patch = serde_json::json!({ "autoflow.max_active": 3 });
        let out = apply_form_patch(existing, &patch).unwrap();
        let parsed: toml::Value = toml::from_str(&out).unwrap();
        assert_eq!(
            parsed
                .get("autoflow")
                .and_then(|v| v.get("max_active"))
                .and_then(|v| v.as_integer()),
            Some(3)
        );
    }

    #[test]
    fn form_patch_rejects_malformed_dotted_key() {
        let existing = "default_model = \"opus\"\n";
        let patch = serde_json::json!({ "pricing.oracle.\"unterminated": 1.0 });
        let err = apply_form_patch(existing, &patch).unwrap_err();
        assert!(matches!(err, ConfigWriteError::Validate(_)));
    }

    #[test]
    fn effective_limit_uses_config_then_default() {
        let cp = rupu_config::CpConfig {
            max_workspace_bytes: Some(1024),
            ..rupu_config::CpConfig::default()
        };
        assert_eq!(effective_max_workspace_bytes(&cp), 1024);
        let cp_def = rupu_config::CpConfig::default();
        assert_eq!(
            effective_max_workspace_bytes(&cp_def),
            crate::host::connector::MAX_WORKSPACE_BYTES
        );
    }
}
