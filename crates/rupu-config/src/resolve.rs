//! Provenance-aware config resolution with policy-lock enforcement.
//! A key listed in the GLOBAL `[policy].lock` takes its GLOBAL value over
//! project + env: locked-global > env > project > global > default. Non-locked
//! keys keep env > project > global > default.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use toml::Value;

use crate::config::Config;
use crate::layer::{read_optional_toml, LayerError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    Global,
    Project,
    Env,
    Default,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyProvenance {
    pub source: KeySource,
    pub locked: bool,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub config: Config,
    pub provenance: BTreeMap<String, KeyProvenance>,
}

/// Flatten a TOML table to leaf keys → scalar/array values, where each key is
/// the list of raw key SEGMENTS from root to leaf. Tables recurse; arrays and
/// scalars are leaves (matching the "arrays replace" merge semantics of
/// `layer_files`).
///
/// The path is a `Vec<String>` of segments — never a dot-joined string —
/// because a TOML key segment can itself contain a `.` (a quoted table name
/// like `[pricing.<provider>."claude-3.5-sonnet"]` or
/// `[pricing.oracle."…/GLM-5.2-FP8"]`). Dot-joining then dot-splitting such a
/// segment silently tears it in two (`GLM-5.2-FP8` → `GLM-5` / `2-FP8`),
/// corrupting the rebuilt config and making it fail to deserialize. Keeping
/// segments intact round-trips any key through [`unflatten`] unchanged.
fn flatten(prefix: &[String], v: &Value, out: &mut BTreeMap<Vec<String>, Value>) {
    match v {
        Value::Table(t) => {
            for (k, vv) in t {
                let mut key = prefix.to_vec();
                key.push(k.clone());
                flatten(&key, vv, out);
            }
        }
        other => {
            out.insert(prefix.to_vec(), other.clone());
        }
    }
}

/// Rebuild a nested TOML table from segment-path leaf keys.
///
/// Fallible: when the same top-level name appears as a scalar leaf in one
/// layer and a table parent in another (e.g. global `default_model = "x"`
/// vs project `[default_model]\nk = "y"`), the winners map holds both
/// `["default_model"]` and `["default_model", "k"]`. Rebuilding then tries to
/// descend through a scalar, which is a structural conflict — return a
/// `LayerError` instead of panicking on user-editable config.
fn unflatten(flat: &BTreeMap<Vec<String>, Value>) -> Result<Value, LayerError> {
    let mut root = toml::value::Table::new();
    for (parts, val) in flat {
        if parts.is_empty() {
            continue;
        }
        let mut cur = &mut root;
        for p in &parts[..parts.len() - 1] {
            let entry = cur
                .entry(p.clone())
                .or_insert_with(|| Value::Table(toml::value::Table::new()));
            cur = entry.as_table_mut().ok_or_else(|| {
                LayerError::Invalid(format!(
                    "config key `{}` conflicts: `{p}` is used as both a value and a table",
                    parts.join(".")
                ))
            })?;
        }
        cur.insert(parts[parts.len() - 1].clone(), val.clone());
    }
    Ok(Value::Table(root))
}

/// Dotted display form of a segment path — used for the public provenance
/// map keys and for comparing against the (dotted) `[policy].lock` entries.
/// This is purely informational: the merge itself keys off segment paths, so
/// the theoretical ambiguity of joining segments that contain dots never
/// affects which value wins.
fn dotted(parts: &[String]) -> String {
    parts.join(".")
}

pub fn resolve(
    global: Option<&Path>,
    project: Option<&Path>,
    env: &BTreeMap<String, Value>,
) -> Result<Resolved, LayerError> {
    let g = read_optional_toml(global)?; // Option<Value>
    let p = read_optional_toml(project)?;

    let mut fg: BTreeMap<Vec<String>, Value> = BTreeMap::new();
    if let Some(g) = &g {
        flatten(&[], g, &mut fg);
    }
    let mut fp: BTreeMap<Vec<String>, Value> = BTreeMap::new();
    if let Some(p) = &p {
        flatten(&[], p, &mut fp);
    }
    // Env overrides arrive keyed by dotted config path (simple keys — env never
    // carries a dotted model id), so splitting them into segments is safe and
    // lets them share the segment-path key space with `fg`/`fp`.
    let fe: BTreeMap<Vec<String>, Value> = env
        .iter()
        .map(|(k, v)| {
            (
                k.split('.').map(String::from).collect::<Vec<_>>(),
                v.clone(),
            )
        })
        .collect();

    // Locks come from the GLOBAL layer only. `[policy].lock` is a two-segment key.
    let lock: Vec<String> = fg
        .iter()
        .filter(|(k, _)| dotted(k) == "policy.lock")
        .filter_map(|(_, v)| v.as_array())
        .flat_map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)))
        .collect();
    let is_locked = |key: &str| lock.iter().any(|l| l == key);

    let mut winners: BTreeMap<Vec<String>, Value> = BTreeMap::new();
    let mut provenance: BTreeMap<String, KeyProvenance> = BTreeMap::new();
    let all_keys: std::collections::BTreeSet<Vec<String>> = fg
        .keys()
        .chain(fp.keys())
        .chain(fe.keys())
        .cloned()
        .collect();

    for key in all_keys {
        let key_dotted = dotted(&key);
        let locked = is_locked(&key_dotted);
        // Precedence: locked ⇒ global wins if present; else env > project > global.
        let (val, source) = if locked {
            if let Some(v) = fg.get(&key) {
                (Some(v.clone()), KeySource::Global)
            } else if let Some(v) = fe.get(&key) {
                (Some(v.clone()), KeySource::Env)
            } else if let Some(v) = fp.get(&key) {
                (Some(v.clone()), KeySource::Project)
            } else {
                (None, KeySource::Default)
            }
        } else if let Some(v) = fe.get(&key) {
            (Some(v.clone()), KeySource::Env)
        } else if let Some(v) = fp.get(&key) {
            (Some(v.clone()), KeySource::Project)
        } else if let Some(v) = fg.get(&key) {
            (Some(v.clone()), KeySource::Global)
        } else {
            (None, KeySource::Default)
        };
        if let Some(v) = val {
            winners.insert(key, v);
            provenance.insert(key_dotted, KeyProvenance { source, locked });
        }
    }

    let merged = unflatten(&winners)?;
    let mut config: Config = merged.try_into().map_err(|source| LayerError::Layered {
        global_path: global.map(|p| p.display().to_string()),
        project_path: project.map(|p| p.display().to_string()),
        source: Box::new(source),
    })?;

    // The `policy.lock` list is itself an unlocked key, so a project's
    // `[policy].lock` would otherwise land in the resolved config and mislead
    // consumers (e.g. the CP UI reading `config.policy.lock` for lock badges,
    // or a project appearing to clear locks). Pin the resolved lock list to
    // the GLOBAL-derived enforced list and mark its provenance Global so no
    // consumer ever trusts a project-supplied lock list.
    config.policy.lock = lock.clone();
    let policy_lock_key = vec!["policy".to_string(), "lock".to_string()];
    if winners.contains_key(&policy_lock_key) || !lock.is_empty() {
        provenance.insert(
            "policy.lock".to_string(),
            KeyProvenance {
                source: KeySource::Global,
                locked: is_locked("policy.lock"),
            },
        );
    }

    config.validate()?;
    Ok(Resolved { config, provenance })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Write;

    fn write_toml(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn unlocked_key_project_overrides_global() {
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(d.path(), "g.toml", "default_model = \"global-m\"\n");
        let p = write_toml(d.path(), "p.toml", "default_model = \"project-m\"\n");
        let r = resolve(Some(&g), Some(&p), &BTreeMap::new()).unwrap();
        assert_eq!(r.config.default_model.as_deref(), Some("project-m"));
        let prov = r.provenance.get("default_model").unwrap();
        assert!(matches!(prov.source, KeySource::Project));
        assert!(!prov.locked);
    }

    #[test]
    fn locked_key_global_overrides_project() {
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(
            d.path(),
            "g.toml",
            "permission_mode = \"ask\"\n[policy]\nlock = [\"permission_mode\"]\n",
        );
        let p = write_toml(d.path(), "p.toml", "permission_mode = \"bypass\"\n");
        let r = resolve(Some(&g), Some(&p), &BTreeMap::new()).unwrap();
        // Locked: the global value wins over the project override.
        assert_eq!(r.config.permission_mode.as_deref(), Some("ask"));
        let prov = r.provenance.get("permission_mode").unwrap();
        assert!(matches!(prov.source, KeySource::Global));
        assert!(prov.locked);
    }

    #[test]
    fn locked_nested_key_global_wins() {
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(
            d.path(),
            "g.toml",
            "[autoflow]\nmax_active = 2\n[policy]\nlock = [\"autoflow.max_active\"]\n",
        );
        let p = write_toml(d.path(), "p.toml", "[autoflow]\nmax_active = 99\n");
        let r = resolve(Some(&g), Some(&p), &BTreeMap::new()).unwrap();
        assert_eq!(r.config.autoflow.max_active, Some(2));
        assert!(r.provenance.get("autoflow.max_active").unwrap().locked);
    }

    #[test]
    fn env_overrides_project_when_unlocked() {
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(d.path(), "g.toml", "log_level = \"info\"\n");
        let p = write_toml(d.path(), "p.toml", "log_level = \"debug\"\n");
        let mut env = BTreeMap::new();
        env.insert("log_level".to_string(), toml::Value::String("trace".into()));
        let r = resolve(Some(&g), Some(&p), &env).unwrap();
        assert_eq!(r.config.log_level.as_deref(), Some("trace"));
        assert!(matches!(
            r.provenance.get("log_level").unwrap().source,
            KeySource::Env
        ));
    }

    #[test]
    fn cp_section_parses_and_defaults() {
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(d.path(), "g.toml", "[cp]\nmax_workspace_bytes = 1048576\n");
        let r = resolve(Some(&g), None, &BTreeMap::new()).unwrap();
        assert_eq!(r.config.cp.max_workspace_bytes, Some(1_048_576));
        // absent ⇒ None
        let g2 = write_toml(d.path(), "g2.toml", "default_model = \"x\"\n");
        let r2 = resolve(Some(&g2), None, &BTreeMap::new()).unwrap();
        assert_eq!(r2.config.cp.max_workspace_bytes, None);
    }

    #[test]
    fn resolve_preserves_config_keys_containing_dots() {
        // A `[pricing.<provider>."<model>"]` table name with a dot in it — a
        // quoted model id like `GLM-5.2-FP8` — must survive provenance
        // resolution. The old dot-join/dot-split flatten tore `GLM-5.2-FP8`
        // into `GLM-5` / `2-FP8`, rebuilding a bogus nested `2-FP8` table that
        // failed to deserialize ("unknown field `2-FP8`").
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(
            d.path(),
            "g.toml",
            "[pricing.oracle.\"/raid/models/zai-org/GLM-5.2-FP8\"]\n\
             input_per_mtok = 1.42\n\
             output_per_mtok = 1.42\n\
             cached_input_per_mtok = 0.82\n",
        );
        let r = resolve(Some(&g), None, &BTreeMap::new())
            .expect("dotted model key must resolve, not error");
        let mp = r
            .config
            .pricing
            .models
            .get("oracle")
            .and_then(|m| m.get("/raid/models/zai-org/GLM-5.2-FP8"))
            .copied()
            .expect("the dotted model key must survive intact");
        assert_eq!(mp.input_per_mtok, 1.42);
        assert_eq!(mp.cached_input_per_mtok, Some(0.82));
        // The provenance display key retains the full (dotted) path.
        assert!(r.provenance.keys().any(|k| k.contains("GLM-5.2-FP8")));
    }

    #[test]
    fn resolve_scalar_vs_table_conflict_errors_not_panics() {
        // Global uses `default_model` as a scalar; project redeclares it as a
        // table. The winners map then holds both `default_model` and
        // `default_model.k`, which cannot be rebuilt into one TOML tree.
        // resolve must return Err rather than panic on user-editable config.
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(d.path(), "g.toml", "default_model = \"x\"\n");
        let p = write_toml(d.path(), "p.toml", "[default_model]\nk = \"y\"\n");
        let r = resolve(Some(&g), Some(&p), &BTreeMap::new());
        assert!(r.is_err(), "expected Err, got {r:?}");
    }

    #[test]
    fn project_cannot_override_resolved_lock_list() {
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(
            d.path(),
            "g.toml",
            "permission_mode = \"ask\"\n[policy]\nlock = [\"permission_mode\"]\n",
        );
        // Project attempts to clear the lock list AND override the locked key.
        let p = write_toml(
            d.path(),
            "p.toml",
            "permission_mode = \"bypass\"\n[policy]\nlock = []\n",
        );
        let r = resolve(Some(&g), Some(&p), &BTreeMap::new()).unwrap();
        // The resolved lock list reflects the GLOBAL list, not the project's.
        assert_eq!(r.config.policy.lock, vec!["permission_mode".to_string()]);
        // Provenance for policy.lock must be Global.
        assert!(matches!(
            r.provenance.get("policy.lock").unwrap().source,
            KeySource::Global
        ));
        // Enforcement still holds: permission_mode is locked to the global value.
        assert_eq!(r.config.permission_mode.as_deref(), Some("ask"));
        assert!(r.provenance.get("permission_mode").unwrap().locked);
    }

    #[test]
    fn no_policy_block_matches_layer_files() {
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(
            d.path(),
            "g.toml",
            "default_model = \"m\"\nlog_level = \"info\"\n",
        );
        let p = write_toml(d.path(), "p.toml", "log_level = \"debug\"\n");
        let via_layer = crate::layer_files(Some(&g), Some(&p)).unwrap();
        let via_resolve = resolve(Some(&g), Some(&p), &BTreeMap::new())
            .unwrap()
            .config;
        assert_eq!(via_layer, via_resolve);
    }
}
