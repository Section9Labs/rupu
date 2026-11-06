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
    /// A write refused before anything was written (an existing file that
    /// isn't TOML, a request that names no rule); the message stands alone.
    #[error("{0}")]
    Refused(String),
    /// The existing file could not be read.
    #[error("{0}")]
    Read(String),
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
    let lock = lock_config(path)?;
    let written = replace_locked(path, contents);
    let _ = fs2::FileExt::unlock(&lock);
    written
}

/// Take the `<file>.lock` advisory lock [`write_atomic`] writes under.
fn lock_config(path: &Path) -> Result<std::fs::File, ConfigWriteError> {
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
    Ok(lock_file)
}

/// Back up `path` and rename `contents` over it; the caller holds the lock.
fn replace_locked(path: &Path, contents: &str) -> Result<(), ConfigWriteError> {
    // Backup existing.
    if path.exists() {
        let bak = path.with_extension("toml.bak");
        std::fs::copy(path, &bak).map_err(|e| ConfigWriteError::Io(e.to_string()))?;
    }
    // Temp write + rename.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, contents).map_err(|e| ConfigWriteError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| ConfigWriteError::Io(e.to_string()))?;
    Ok(())
}

/// `rupu scm bind`: append one `[[scm.rules]]` entry selecting `account`
/// for repos under the owner glob `owner`, or checkouts under the path
/// glob `path` (exactly one of the two), to the config file at `cfg_path`.
///
/// Comments and layout survive (`toml_edit`), and the whole result is
/// validated before it lands, like any [`write_atomic`]. The file is read
/// and replaced under its lock, so two binds at once both land. An
/// existing file that is not valid TOML is refused untouched: replacing it
/// would discard whatever it holds. Every value is written as a TOML
/// value, never parsed as a key, so an account name with dots
/// (`gh.work`) stays one name.
pub fn append_scm_rule(
    cfg_path: &Path,
    owner: Option<&str>,
    path: Option<&str>,
    account: &str,
) -> Result<rupu_config::ScmRule, ConfigWriteError> {
    let rule = match (owner, path) {
        (Some(o), None) => rupu_config::ScmRule {
            owner: Some(o.to_string()),
            path: None,
            account: account.to_string(),
        },
        (None, Some(p)) => rupu_config::ScmRule {
            owner: None,
            path: Some(p.to_string()),
            account: account.to_string(),
        },
        _ => {
            return Err(ConfigWriteError::Refused(
                "a rule takes exactly one of `owner` or `path`".into(),
            ))
        }
    };
    if account.trim().is_empty() {
        return Err(ConfigWriteError::Refused("`account` is empty".into()));
    }
    let lock = lock_config(cfg_path)?;
    let written = append_scm_rule_locked(cfg_path, &rule);
    let _ = fs2::FileExt::unlock(&lock);
    written.map(|()| rule)
}

fn append_scm_rule_locked(
    cfg_path: &Path,
    rule: &rupu_config::ScmRule,
) -> Result<(), ConfigWriteError> {
    let text = match std::fs::read_to_string(cfg_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(ConfigWriteError::Read(e.to_string())),
    };
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        ConfigWriteError::Refused(format!(
            "refusing to write: {} is not valid TOML ({e}). \
             Fix or move the file first — writing would discard its contents.",
            cfg_path.display()
        ))
    })?;

    let root = doc.as_table_mut();
    if root.get("scm").is_none() {
        root.insert("scm", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let scm = root
        .get_mut("scm")
        .and_then(|item| item.as_table_mut())
        .ok_or_else(|| ConfigWriteError::Refused("`scm` is already a value, not a table".into()))?;
    if scm.get("rules").is_none() {
        scm.insert(
            "rules",
            toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()),
        );
    }
    let rules = scm
        .get_mut("rules")
        .and_then(|item| item.as_array_of_tables_mut())
        .ok_or_else(|| {
            ConfigWriteError::Refused(
                "`scm.rules` is already declared as something other than an array of tables".into(),
            )
        })?;

    let mut entry = toml_edit::Table::new();
    if let Some(o) = &rule.owner {
        entry["owner"] = toml_edit::value(o.as_str());
    }
    if let Some(p) = &rule.path {
        entry["path"] = toml_edit::value(p.as_str());
    }
    entry["account"] = toml_edit::value(rule.account.as_str());
    rules.push(entry);

    let contents = doc.to_string();
    validate_toml(&contents)?;
    replace_locked(cfg_path, &contents)
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
///
/// Also used by `rupu customer show` to walk provenance keys — one decoder,
/// not a fifth lockstep site.
pub fn split_dotted_key(dotted: &str) -> Result<Vec<String>, ConfigWriteError> {
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

    #[test]
    fn append_scm_rule_writes_owner_form() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        append_scm_rule(&path, Some("acme/*"), None, "gh-work").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        let rules = v["scm"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["owner"].as_str(), Some("acme/*"));
        assert_eq!(rules[0]["account"].as_str(), Some("gh-work"));
        assert!(rules[0].get("path").is_none());
    }

    #[test]
    fn append_scm_rule_writes_path_form() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        append_scm_rule(&path, None, Some("~/Code/work/*"), "gh-work").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        let rules = v["scm"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["path"].as_str(), Some("~/Code/work/*"));
        assert_eq!(rules[0]["account"].as_str(), Some("gh-work"));
    }

    /// Binds are additive: three binds, three entries, in order.
    #[test]
    fn append_scm_rule_appends_rather_than_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        append_scm_rule(&path, Some("acme/*"), None, "gh-work").unwrap();
        append_scm_rule(&path, Some("MrBrutti/*"), None, "gh-personal").unwrap();
        append_scm_rule(&path, None, Some("~/Code/work/*"), "gh-work").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        let rules = v["scm"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0]["account"].as_str(), Some("gh-work"));
        assert_eq!(rules[1]["account"].as_str(), Some("gh-personal"));
        assert_eq!(rules[2]["path"].as_str(), Some("~/Code/work/*"));
    }

    /// Concurrent binds each read and replace the file under its lock, so
    /// none is lost.
    #[test]
    fn concurrent_appends_all_land() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::thread::scope(|s| {
            for i in 0..8 {
                let path = &path;
                s.spawn(move || {
                    append_scm_rule(path, Some(&format!("o{i}/*")), None, "gh-work").unwrap();
                });
            }
        });
        let v: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["scm"]["rules"].as_array().unwrap().len(), 8);
    }

    #[test]
    fn append_scm_rule_preserves_a_comment_and_a_dotted_account_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# do not remove this note\ndefault_provider = \"anthropic\"\n\n[scm.\"gh.work\"]\nkind = \"github\"\n",
        )
        .unwrap();

        append_scm_rule(&path, Some("acme/*"), None, "gh.work").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# do not remove this note"),
            "comment was dropped by the config write:\n{text}"
        );
        let cfg: rupu_config::Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.scm.platforms["gh.work"].kind.as_deref(), Some("github"));
        assert_eq!(cfg.scm.rules[0].account, "gh.work");
        assert_eq!(cfg.scm.rules[0].owner.as_deref(), Some("acme/*"));
    }

    #[test]
    fn append_scm_rule_refuses_to_clobber_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not = = toml").unwrap();

        let err = append_scm_rule(&path, Some("acme/*"), None, "gh-work").unwrap_err();
        // `rupu scm bind` prints this as it is: no "invalid config: " prefix.
        assert!(
            err.to_string().starts_with(&format!(
                "refusing to write: {} is not valid TOML (",
                path.display()
            )),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is not = = toml"
        );
    }

    /// The whole document is re-validated: an existing rule with both
    /// `owner` and `path` fails `Config::validate`, so the append is refused.
    #[test]
    fn append_scm_rule_rejects_a_result_that_would_fail_config_validate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[scm.rules]]\nowner = \"a/*\"\npath = \"~/x\"\naccount = \"gh-work\"\n",
        )
        .unwrap();

        assert!(append_scm_rule(&path, Some("acme/*"), None, "gh-personal").is_err());
    }

    #[test]
    fn append_scm_rule_takes_exactly_one_selector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(append_scm_rule(&path, None, None, "gh").is_err());
        assert!(append_scm_rule(&path, Some("a"), Some("b"), "gh").is_err());
        assert!(append_scm_rule(&path, Some("a"), None, " ").is_err());
        assert!(!path.exists());
    }
}
