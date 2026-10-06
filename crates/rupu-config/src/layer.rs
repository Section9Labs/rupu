//! Global + customer + project config layering.
//!
//! Rules (locked by spec):
//! - Layer order (lowest first): global, customer, project. Only the
//!   global layer's `[policy].lock` survives into the merged config (the
//!   customer's lock list is enforced by `resolve` and reported in
//!   `Resolved.customer_lock`).
//! - A higher layer overrides a lower one key-by-key (deep merge for tables).
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
    #[error(
        "layered config invalid (merging {global_path:?} + {customer_path:?} + {project_path:?}): {source}"
    )]
    Layered {
        global_path: Option<String>,
        customer_path: Option<String>,
        project_path: Option<String>,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// The config files one load layers, lowest first: global, customer,
/// project. Any may be `None` (or name a missing file) — that layer is
/// empty. Spec: `docs/superpowers/specs/2026-10-06-rupu-customers-design.md`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LayerPaths<'a> {
    pub global: Option<&'a Path>,
    pub customer: Option<&'a Path>,
    pub project: Option<&'a Path>,
}

impl<'a> LayerPaths<'a> {
    pub fn new(
        global: Option<&'a Path>,
        customer: Option<&'a Path>,
        project: Option<&'a Path>,
    ) -> Self {
        Self {
            global,
            customer,
            project,
        }
    }

    /// Just the global file — for loads that serve no project by design.
    pub fn global_only(global: &'a Path) -> Self {
        Self {
            global: Some(global),
            ..Self::default()
        }
    }

    pub(crate) fn layered_error(&self, source: toml::de::Error) -> LayerError {
        let show = |p: Option<&Path>| p.map(|p| p.display().to_string());
        LayerError::Layered {
            global_path: show(self.global),
            customer_path: show(self.customer),
            project_path: show(self.project),
            source: Box::new(source),
        }
    }
}

/// `[policy].lock` of one raw layer, as written.
pub(crate) fn policy_lock_of(layer: Option<&Value>) -> Vec<String> {
    layer
        .and_then(|v| v.get("policy"))
        .and_then(|p| p.get("lock"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Layer global, customer and project config files into a single
/// [`Config`]: plain layering, no lock enforcement — correct for non-policy
/// reads only (see [`layer_files_locked`]).
///
/// Merge semantics:
///
/// - **Tables** merge key-by-key recursively.
/// - **Arrays** in a higher layer REPLACE arrays below it — they never
///   concatenate, so a layer can subtract an entry from a lower allow-list.
/// - **Scalars** in a higher layer overwrite lower ones.
/// - `policy.lock` is pinned to the GLOBAL layer's list: a customer's or
///   project's lock list never appears in the merged config.
pub fn layer_files(paths: LayerPaths<'_>) -> Result<Config, LayerError> {
    let g = read_optional_toml(paths.global)?;
    let c = read_optional_toml(paths.customer)?;
    let p = read_optional_toml(paths.project)?;
    let global_lock = policy_lock_of(g.as_ref());

    let merged = [g, c, p]
        .into_iter()
        .flatten()
        .reduce(deep_merge)
        .unwrap_or_else(|| Value::Table(toml::value::Table::new()));

    let mut cfg: Config = merged
        .try_into()
        .map_err(|source| paths.layered_error(source))?;
    cfg.policy.lock = global_lock;
    cfg.attach_provider_kinds();
    cfg.validate()?;
    Ok(cfg)
}

/// Like [`layer_files`], but honouring `[policy].lock`: a key the GLOBAL
/// lock names keeps its global value, and a key the CUSTOMER lock names
/// keeps its customer value against the project.
///
/// Every path that honors operator policy must use this — see ISSUES.md
/// I-7. This delegates entirely to `resolve()` for lock precedence and
/// dotted-key handling rather than reimplementing them, and logs
/// `resolve`'s warnings (e.g. a customer lock naming a key the customer
/// does not set).
pub fn layer_files_locked(paths: LayerPaths<'_>) -> Result<Config, LayerError> {
    let resolved = crate::resolve::resolve(paths)?;
    for w in &resolved.warnings {
        tracing::warn!("{w}");
    }
    Ok(resolved.config)
}

fn read_toml_file(path: &Path) -> Result<Value, LayerError> {
    let text = std::fs::read_to_string(path).map_err(|e| LayerError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    toml::from_str(&text).map_err(|e| LayerError::Parse {
        path: path.display().to_string(),
        source: e,
    })
}

pub(crate) fn read_optional_toml(path: Option<&Path>) -> Result<Option<Value>, LayerError> {
    let Some(path) = path else { return Ok(None) };
    match read_toml_file(path) {
        Ok(v) => Ok(Some(v)),
        Err(LayerError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            // File genuinely absent — treat as an empty layer. This is the
            // expected path on a fresh install where ~/.rupu/config.toml
            // doesn't exist yet.
            Ok(None)
        }
        Err(e) => Err(e),
    }
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
    fn layer_files_attaches_provider_account_kinds_to_pricing() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path().join("global.toml");
        std::fs::write(
            &g,
            "[providers.openai-oracle]\nkind = \"openai\"\n\n[providers.anthropic]\nmax_retries = 3\n",
        )
        .unwrap();
        let cfg = layer_files(LayerPaths::global_only(&g)).unwrap();
        assert_eq!(
            cfg.pricing
                .provider_kinds
                .get("openai-oracle")
                .map(String::as_str),
            Some("openai")
        );
        // An account with no `kind` IS the vendor; nothing to map.
        assert!(!cfg.pricing.provider_kinds.contains_key("anthropic"));

        // End to end: the account name prices through the kind's table.
        let p =
            crate::pricing::lookup(&cfg.pricing, "openai-oracle", "gpt-5.6-cyber", "x").unwrap();
        assert_eq!(p.input_per_mtok, 12.50);
    }

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
