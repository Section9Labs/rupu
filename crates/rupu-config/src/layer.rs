//! Global+project config layering.
//!
//! Rules (locked by spec):
//! - Project overrides global key-by-key (deep merge for tables).
//! - Arrays REPLACE — never concatenate. This is what allows users to
//!   subtract entries by re-declaring the array in the project file.
//! - Missing files are treated as empty config (not an error). This
//!   lets users run rupu without writing any config at all.

use crate::Config;
use std::path::Path;
use thiserror::Error;
use toml::Value;

#[derive(Debug, Error)]
pub enum LayerError {
    #[error("io reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("layered config invalid: {0}")]
    Layered(toml::de::Error),
}

/// Layer global and project config files into a single [`Config`].
///
/// Either argument may be `None` (file not present); the other layer is
/// returned alone. If both are `None`, the default empty config is
/// returned. Files that exist on disk but are unreadable produce
/// [`LayerError::Io`]; files that parse-fail produce [`LayerError::Parse`].
///
/// Merge semantics:
///
/// - **Tables** merge key-by-key recursively.
/// - **Arrays** in `project` REPLACE arrays in `global` — they never
///   concatenate. This is deliberate: concatenation makes it impossible
///   for the project to subtract an entry from the global allow-list.
/// - **Scalars** in `project` overwrite scalars in `global`.
pub fn layer_files(global: Option<&Path>, project: Option<&Path>) -> Result<Config, LayerError> {
    let global_v = read_optional_toml(global)?;
    let project_v = read_optional_toml(project)?;

    let merged = match (global_v, project_v) {
        (Some(g), Some(p)) => deep_merge(g, p),
        (Some(g), None) => g,
        (None, Some(p)) => p,
        (None, None) => Value::Table(toml::value::Table::new()),
    };

    let cfg: Config = merged.try_into().map_err(LayerError::Layered)?;
    Ok(cfg)
}

fn read_optional_toml(path: Option<&Path>) -> Result<Option<Value>, LayerError> {
    let Some(path) = path else { return Ok(None) };
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| LayerError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let v: Value = toml::from_str(&text).map_err(|e| LayerError::Parse {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(Some(v))
}

/// Merge `overlay` into `base`. Tables merge key-by-key; everything else
/// (including arrays) is replaced wholesale by `overlay`.
fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Table(mut b), Value::Table(o)) => {
            for (k, v_overlay) in o {
                let merged = match b.remove(&k) {
                    Some(v_base) => deep_merge(v_base, v_overlay),
                    None => v_overlay,
                };
                b.insert(k, merged);
            }
            Value::Table(b)
        }
        // Anything else: overlay replaces base. Includes arrays.
        (_, overlay) => overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merge_replaces_arrays() {
        let base = toml::toml! {
            [t]
            arr = [1, 2, 3]
        };
        let overlay = toml::toml! {
            [t]
            arr = [9]
        };
        let merged = deep_merge(Value::Table(base), Value::Table(overlay));
        let arr = merged.get("t").unwrap().get("arr").unwrap();
        assert_eq!(arr, &Value::Array(vec![Value::Integer(9)]));
    }
}
