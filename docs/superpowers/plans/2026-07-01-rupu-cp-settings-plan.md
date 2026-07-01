# CP Settings & Project Config (policy enforcement) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A CP settings surface — a global settings editor (fills the `Settings.tsx` stub) and a per-project config editor (a Config tab on `ProjectDetail`), both read-write, with a real policy-lock enforcement layer in `rupu-config` and a hybrid (typed form + raw TOML) editor.

**Architecture:** Three layers. (1) `rupu-config` gains a `[policy] lock` enforcement engine + `[cp]` section + a provenance-aware `resolve()` (locked global keys override project/env; enforced wherever config is read). (2) `cp serve` gains a write-safety module + a reloadable config snapshot on `AppState` + `api/config.rs` (GET effective+provenance+raw; PUT global/project/policy, launcher-gated, validate→backup→atomic→reload). (3) CP web fills `Settings.tsx` (global, sub-tabbed, form+raw+policy) and adds a Config tab to `ProjectDetail`.

**Tech Stack:** Rust 2021 (MSRV 1.88), serde/`toml`/`toml_edit`, axum, thiserror (libs) / `ApiError` (cp); React + vitest (web).

## Global Constraints

- Backward compatible: no `[policy]`/`[cp]` block ⇒ config resolution is byte-for-byte today; existing `layer_files` callers are unaffected.
- The CP read-only surface cannot write: config writes are launcher-gated (→ `ApiError::not_available`, 501, on a read-only deploy), mirroring host-add.
- No silent-noop: a successful write persists to disk **and** reloads the `AppState` config snapshot; settings that require a restart (bind/token) are flagged, never silently applied.
- Secrets are never displayed or written here: provider keys stay in the keychain (shown configured/not); the bearer `token` is masked (`••• set`).
- Atomic write + backup: validate → backup prior file to `<file>.bak` → temp write + rename → advisory file lock. A rejected/failed write never leaves a corrupt or partial file.
- Project config path is confined under the project's `.rupu/` (reuse `api/fs_safety`); no traversal.
- Validation rejects unknown-key / type errors before writing (`Config` is `#[serde(deny_unknown_fields)]` + has `validate()`).
- `#![deny(clippy::all)]`; no `unsafe`; libraries use `thiserror`; cp uses `ApiError`/`anyhow`.
- **Workspace deps only**: add `toml_edit` to root `[workspace.dependencies]`; reference via `{ workspace = true }`. `toml` is already pinned.
- Per-file `rustfmt` only (`rustfmt --edition 2021 <path>`); never a workspace-wide `cargo fmt` (`main` is fmt-dirty; a broad format has polluted ~16 files repeatedly). Before each commit run `git status --short` and `git restore` stray drift by name.
- Clippy `--no-deps`, scoped to changed crates. Pre-existing 1.95-only lints in untouched files (`rupu-orchestrator/src/runner.rs`, `node_tunnel.rs`) are unrelated. Web tasks: `cd crates/rupu-cp/web && npm test` (vitest) + `tsc` + `npm run build`.
- Hexagonal: `rupu-config` stays free of `rupu-cp`/`rupu-cli` deps — the enforcement engine is core.

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `crates/rupu-config/src/policy_config.rs` (new) | `PolicyConfig` (`[policy]`) + `CpConfig` (`[cp]`) | 1 |
| `crates/rupu-config/src/config.rs` | add `policy`/`cp` fields to `Config` | 1 |
| `crates/rupu-config/src/resolve.rs` (new) | `resolve()` + provenance + flatten/unflatten + lock precedence | 1 |
| `crates/rupu-config/src/lib.rs` | export the new items | 1 |
| `Cargo.toml` (root) | pin `toml_edit` | 3 |
| `crates/rupu-cp/src/config_write.rs` (new) | `validate_toml` + `write_atomic` + form-patch merge (`toml_edit`) | 2, 3 |
| `crates/rupu-cp/src/state.rs` | reloadable `config` holder + `reload_config` + effective `[cp]` limit | 2 |
| `crates/rupu-cp/src/host/connector.rs` (+ `api/workspace.rs`) | use configurable `max_workspace_bytes` at the boundary (const backstop) | 2 |
| `crates/rupu-cp/src/api/config.rs` (new) | GET/PUT config endpoints | 3 |
| `crates/rupu-cp/src/server.rs` | register `config::routes()` | 3 |
| `crates/rupu-cp/web/src/lib/api.ts` | config client types + calls | 4 |
| `crates/rupu-cp/web/src/pages/Settings.tsx` | global settings (form/raw/policy/status, sub-tabs) | 4, 5 |
| `crates/rupu-cp/web/src/pages/ProjectDetail.tsx` | per-project Config tab | 6 |
| `crates/rupu-cp/web/src/pages/*.test.tsx` | vitest | 4, 5, 6 |
| `crates/rupu-cp/tests/config_e2e.rs` (new) | e2e round-trip + lock enforcement | 7 |

---

## Task 1: `rupu-config` — policy-lock engine, `[cp]` section, `resolve()` + provenance

**Files:**
- Create: `crates/rupu-config/src/policy_config.rs`, `crates/rupu-config/src/resolve.rs`
- Modify: `crates/rupu-config/src/config.rs` (add `policy`/`cp` fields), `crates/rupu-config/src/lib.rs` (exports)
- Test: `crates/rupu-config/src/resolve.rs` (unit tests)

**Interfaces:**
- Produces:
  - `pub struct PolicyConfig { pub lock: Vec<String> }` (`#[serde(default, deny_unknown_fields)]`).
  - `pub struct CpConfig { pub max_workspace_bytes: Option<u64> }` (`#[serde(default, deny_unknown_fields)]`).
  - `Config.policy: PolicyConfig`, `Config.cp: CpConfig` (both `#[serde(default)]`).
  - `pub enum KeySource { Global, Project, Env, Default }`
  - `pub struct KeyProvenance { pub source: KeySource, pub locked: bool }`
  - `pub struct Resolved { pub config: Config, pub provenance: std::collections::BTreeMap<String, KeyProvenance> }`
  - `pub fn resolve(global: Option<&Path>, project: Option<&Path>, env: &BTreeMap<String, toml::Value>) -> Result<Resolved, LayerError>`

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-config/src/resolve.rs` with tests first:

```rust
//! Provenance-aware config resolution with policy-lock enforcement.
//! A key listed in the GLOBAL `[policy].lock` takes its GLOBAL value over
//! project + env: locked-global > env > project > global > default. Non-locked
//! keys keep env > project > global > default.

// (implementation added below)

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
        assert!(matches!(r.provenance.get("log_level").unwrap().source, KeySource::Env));
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
    fn no_policy_block_matches_layer_files() {
        let d = tempfile::tempdir().unwrap();
        let g = write_toml(d.path(), "g.toml", "default_model = \"m\"\nlog_level = \"info\"\n");
        let p = write_toml(d.path(), "p.toml", "log_level = \"debug\"\n");
        let via_layer = crate::layer_files(Some(&g), Some(&p)).unwrap();
        let via_resolve = resolve(Some(&g), Some(&p), &BTreeMap::new()).unwrap().config;
        assert_eq!(via_layer, via_resolve);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-config resolve`
Expected: FAIL — module/types not defined.

- [ ] **Step 3: Add `PolicyConfig` + `CpConfig`**

Create `crates/rupu-config/src/policy_config.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Global enforcement policy. Keys named here (dotted paths, e.g.
/// `"permission_mode"`, `"autoflow.max_active"`) are LOCKED: their GLOBAL value
/// overrides project + env at resolution. Only read from the global layer — a
/// project cannot declare its own locks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    pub lock: Vec<String>,
}

/// CP-runtime settings persistable in config (the `[cp]` section). Absent
/// fields fall back to the CP's compiled defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CpConfig {
    /// Max bytes for a workspace-sync payload/delta. `None` ⇒ the CP's
    /// `MAX_WORKSPACE_BYTES` default.
    pub max_workspace_bytes: Option<u64>,
}
```

In `crates/rupu-config/src/config.rs`, add to `Config` (after `storage`):

```rust
    #[serde(default)]
    pub policy: crate::policy_config::PolicyConfig,
    #[serde(default)]
    pub cp: crate::policy_config::CpConfig,
```

- [ ] **Step 4: Implement `resolve()` + provenance**

Add to `crates/rupu-config/src/resolve.rs` (above the test module):

```rust
use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use toml::Value;

use crate::config::Config;
use crate::layer::{read_optional_toml, LayerError}; // see note below

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

/// Flatten a TOML table to dotted leaf keys → scalar/array values. Tables
/// recurse; arrays and scalars are leaves (matching the "arrays replace"
/// merge semantics of `layer_files`).
fn flatten(prefix: &str, v: &Value, out: &mut BTreeMap<String, Value>) {
    match v {
        Value::Table(t) => {
            for (k, vv) in t {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, vv, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other.clone());
        }
    }
}

/// Rebuild a nested TOML table from dotted leaf keys.
fn unflatten(flat: &BTreeMap<String, Value>) -> Value {
    let mut root = toml::value::Table::new();
    for (dotted, val) in flat {
        let mut cur = &mut root;
        let parts: Vec<&str> = dotted.split('.').collect();
        for p in &parts[..parts.len() - 1] {
            cur = cur
                .entry(p.to_string())
                .or_insert_with(|| Value::Table(toml::value::Table::new()))
                .as_table_mut()
                .expect("intermediate must be a table");
        }
        cur.insert(parts[parts.len() - 1].to_string(), val.clone());
    }
    Value::Table(root)
}

pub fn resolve(
    global: Option<&Path>,
    project: Option<&Path>,
    env: &BTreeMap<String, Value>,
) -> Result<Resolved, LayerError> {
    let g = read_optional_toml(global)?; // Option<Value>
    let p = read_optional_toml(project)?;

    let mut fg = BTreeMap::new();
    if let Some(g) = &g {
        flatten("", g, &mut fg);
    }
    let mut fp = BTreeMap::new();
    if let Some(p) = &p {
        flatten("", p, &mut fp);
    }

    // Locks come from the GLOBAL layer only.
    let lock: Vec<String> = fg
        .iter()
        .filter(|(k, _)| k.as_str() == "policy.lock")
        .filter_map(|(_, v)| v.as_array())
        .flat_map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)))
        .collect();
    let is_locked = |key: &str| lock.iter().any(|l| l == key);

    let mut winners: BTreeMap<String, Value> = BTreeMap::new();
    let mut provenance: BTreeMap<String, KeyProvenance> = BTreeMap::new();
    let all_keys: std::collections::BTreeSet<String> = fg
        .keys()
        .chain(fp.keys())
        .chain(env.keys())
        .cloned()
        .collect();

    for key in all_keys {
        let locked = is_locked(&key);
        // Precedence: locked ⇒ global wins if present; else env > project > global.
        let (val, source) = if locked {
            if let Some(v) = fg.get(&key) {
                (Some(v.clone()), KeySource::Global)
            } else if let Some(v) = env.get(&key) {
                (Some(v.clone()), KeySource::Env)
            } else if let Some(v) = fp.get(&key) {
                (Some(v.clone()), KeySource::Project)
            } else {
                (None, KeySource::Default)
            }
        } else if let Some(v) = env.get(&key) {
            (Some(v.clone()), KeySource::Env)
        } else if let Some(v) = fp.get(&key) {
            (Some(v.clone()), KeySource::Project)
        } else if let Some(v) = fg.get(&key) {
            (Some(v.clone()), KeySource::Global)
        } else {
            (None, KeySource::Default)
        };
        if let Some(v) = val {
            winners.insert(key.clone(), v);
            provenance.insert(key, KeyProvenance { source, locked });
        }
    }

    let merged = unflatten(&winners);
    let config: Config = merged.clone().try_into().map_err(|source| LayerError::Layered {
        global_path: global.map(|p| p.display().to_string()),
        project_path: project.map(|p| p.display().to_string()),
        source: Box::new(source),
    })?;
    config.validate()?;
    Ok(Resolved { config, provenance })
}
```

> NOTE: `read_optional_toml` is currently private in `layer.rs`. Make it `pub(crate)` so `resolve.rs` can call it (a one-word visibility change). If its signature differs (e.g. returns `Result<Option<Value>, LayerError>`), match it exactly. `LayerError::Layered` is the variant `layer_files` uses — reuse it verbatim; if the variant name differs, use whatever `layer_files` returns on a `try_into` failure.

> `layer_files` is left as-is (backward compatible). `resolve` is additive. The `no_policy_block_matches_layer_files` test guards equivalence for the no-lock/no-env case.

- [ ] **Step 5: Export from lib.rs**

In `crates/rupu-config/src/lib.rs`, add:
```rust
pub mod policy_config;
pub mod resolve;
pub use policy_config::{CpConfig, PolicyConfig};
pub use resolve::{resolve, KeyProvenance, KeySource, Resolved};
```

- [ ] **Step 6: Run tests, format, lint, commit**

```bash
cargo test -p rupu-config
rustfmt --edition 2021 crates/rupu-config/src/policy_config.rs crates/rupu-config/src/resolve.rs crates/rupu-config/src/config.rs crates/rupu-config/src/lib.rs crates/rupu-config/src/layer.rs
cargo clippy -p rupu-config --all-targets --no-deps
git add crates/rupu-config
git commit -m "feat(cp-settings): policy-lock resolve() + [cp] section in rupu-config (T1)"
```
Expected: 6 resolve tests pass; full `rupu-config` suite green (additive serde-default fields ⇒ existing tests unaffected); clippy clean.

---

## Task 2: `rupu-cp` — write-safety module + reloadable config on `AppState`

**Files:**
- Modify: `Cargo.toml` (root — pin `toml_edit`), `crates/rupu-cp/Cargo.toml` (dep)
- Create: `crates/rupu-cp/src/config_write.rs`
- Modify: `crates/rupu-cp/src/state.rs` (reloadable config), `crates/rupu-cp/src/lib.rs` (declare module), `crates/rupu-cp/src/api/workspace.rs` (configurable limit at the boundary)
- Test: `crates/rupu-cp/src/config_write.rs`

**Interfaces:**
- Consumes: `rupu_config::{Config, resolve, CpConfig}` (T1); `MAX_WORKSPACE_BYTES` (connector).
- Produces:
  - `pub enum ConfigWriteError { Validate(String), Io(String), Locked(String) }` (thiserror).
  - `pub fn validate_toml(candidate: &str) -> Result<(), ConfigWriteError>`
  - `pub fn write_atomic(path: &std::path::Path, contents: &str) -> Result<(), ConfigWriteError>`
  - `pub fn apply_form_patch(existing_toml: &str, patch: &serde_json::Value) -> Result<String, ConfigWriteError>` (comment-preserving via `toml_edit`)
  - `AppState.config: std::sync::Arc<std::sync::RwLock<rupu_config::Config>>` + `AppState::reload_config(&self)`
  - `pub fn effective_max_workspace_bytes(cp: &rupu_config::CpConfig) -> usize`

- [ ] **Step 1: Pin `toml_edit`**

Root `Cargo.toml` `[workspace.dependencies]` (after `toml = "0.8"`):
```toml
toml_edit = "0.22"
```
`crates/rupu-cp/Cargo.toml` `[dependencies]` (both — `fs2` is a workspace dep but rupu-cp doesn't depend on it yet, and `write_atomic`'s advisory lock needs it):
```toml
toml_edit = { workspace = true }
fs2 = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Create `crates/rupu-cp/src/config_write.rs` with tests:

```rust
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
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "default_model = \"new\"\n");
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
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "default_model = \"x\"\n");
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
        let cp = rupu_config::CpConfig { max_workspace_bytes: Some(1024) };
        assert_eq!(effective_max_workspace_bytes(&cp), 1024);
        let cp_def = rupu_config::CpConfig::default();
        assert_eq!(
            effective_max_workspace_bytes(&cp_def),
            crate::host::connector::MAX_WORKSPACE_BYTES
        );
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-cp --lib config_write`
Expected: FAIL — module/functions not defined.

- [ ] **Step 4: Implement `config_write`**

Create `crates/rupu-cp/src/config_write.rs`:

```rust
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
/// write a temp sibling, fsync, and rename over the target. An advisory lock on
/// a `<file>.lock` sidecar serializes concurrent writers.
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
    cur.insert(parts[parts.len() - 1], item);
    Ok(())
}

fn json_to_toml_item(v: &serde_json::Value) -> Result<toml_edit::Item, ConfigWriteError> {
    use toml_edit::{value, Item};
    Ok(match v {
        serde_json::Value::String(s) => value(s.clone()),
        serde_json::Value::Bool(b) => value(*b),
        serde_json::Value::Number(n) if n.is_i64() => value(n.as_i64().unwrap()),
        serde_json::Value::Number(n) if n.is_f64() => value(n.as_f64().unwrap()),
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
            value(arr)
        }
        other => {
            return Err(ConfigWriteError::Validate(format!(
                "unsupported value type: {other}"
            )))
        }
    })
    .map(|i: toml_edit::Item| i)
    .or_else(|_: ConfigWriteError| Ok::<Item, ConfigWriteError>(Item::None))
}

/// The effective workspace-payload cap: the `[cp].max_workspace_bytes` config
/// override if set, else the compiled `MAX_WORKSPACE_BYTES` default.
pub fn effective_max_workspace_bytes(cp: &rupu_config::CpConfig) -> usize {
    cp.max_workspace_bytes
        .map(|v| v as usize)
        .unwrap_or(MAX_WORKSPACE_BYTES)
}
```

> `fs2` is already a workspace dep (used elsewhere for file locks — confirm; if not present, use a simpler lock or add it as a workspace dep). The `json_to_toml_item` last `.map/.or_else` shim is a defensive fallback — the implementer should simplify to a clean `match` that returns `Item`; keep the supported-type set (string/bool/int/float/array-of-scalars). Verify `toml_edit` 0.22's exact `DocumentMut`/`value()` API and adjust names if the pinned patch version differs.

- [ ] **Step 5: Add the reloadable config holder to `AppState`**

In `crates/rupu-cp/src/state.rs`, add a field:
```rust
    /// The resolved global config snapshot, reloaded after a config write so
    /// newly-started runs see updated values. Read via `config.read()`.
    pub config: std::sync::Arc<std::sync::RwLock<rupu_config::Config>>,
```
Add a method (in the `impl AppState` block, or a free helper if `AppState` has no impl):
```rust
    /// Re-resolve the global config from disk and swap it into the snapshot.
    /// Called after a successful global-config write.
    pub fn reload_config(&self) {
        let global = self.global_dir.join("config.toml");
        if let Ok(r) = rupu_config::resolve(Some(&global), None, &std::collections::BTreeMap::new()) {
            if let Ok(mut w) = self.config.write() {
                *w = r.config.clone();
            }
        }
    }
```
Update every `AppState { … }` construction site (find with `grep -rn "AppState {" crates/rupu-cp crates/rupu-cli`) to initialize `config`: at cp-serve startup build it from the resolved config; in tests use `Arc::new(RwLock::new(rupu_config::Config::default()))`. Declare `pub mod config_write;` in `crates/rupu-cp/src/lib.rs`.

- [ ] **Step 6: Wire the configurable limit at the workspace boundary**

In `crates/rupu-cp/src/api/workspace.rs`'s `stage_workspace` handler, replace the fixed `MAX_WORKSPACE_BYTES` guard with the configurable limit read from `AppState.config`'s `[cp]`:
```rust
    let limit = crate::config_write::effective_max_workspace_bytes(
        &s.config.read().map(|c| c.cp.clone()).unwrap_or_default(),
    );
    if body.len() > limit {
        return Err(ApiError::bad_request(format!(
            "workspace payload {} bytes exceeds limit {limit}",
            body.len()
        )));
    }
```
(The shared `stage_to_dir` keeps its `MAX_WORKSPACE_BYTES` const as a backstop; the config override applies at the HTTP boundary. Deeper in-process connectors keep the const — documented as v1 scope.)

- [ ] **Step 7: Run tests, format, lint, commit**

```bash
cargo test -p rupu-cp --lib config_write state
rustfmt --edition 2021 crates/rupu-cp/src/config_write.rs crates/rupu-cp/src/state.rs crates/rupu-cp/src/lib.rs crates/rupu-cp/src/api/workspace.rs
cargo clippy -p rupu-cp --no-deps
git add Cargo.toml Cargo.lock crates/rupu-cp/Cargo.toml crates/rupu-cp/src
git commit -m "feat(cp-settings): config write-safety + reloadable snapshot + [cp] limit (T2)"
```
Expected: 7 config_write tests pass; rupu-cp lib compiles with the new AppState field (all construction sites updated); clippy clean. Commit Cargo.lock (new dep).

---

## Task 3: `rupu-cp` — `api/config.rs` (GET/PUT endpoints)

**Files:**
- Create: `crates/rupu-cp/src/api/config.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs` (declare `pub mod config;`), `crates/rupu-cp/src/server.rs` (register `.merge(crate::api::config::routes())`)
- Test: `crates/rupu-cp/src/api/config.rs`

**Interfaces:**
- Consumes: `rupu_config::{resolve, Config, KeyProvenance}` (T1); `config_write::{validate_toml, write_atomic, apply_form_patch}` (T2); `AppState.{global_dir, launcher, config, reload_config}`; `api/fs_safety` confinement; `ApiError`.
- Produces the routes:
  - `GET /api/config` (+ `?project=<id>`) → `ConfigView` JSON.
  - `PUT /api/config/global` (body `ConfigWriteBody`) → persist + reload.
  - `PUT /api/config/project/:id` (body `ConfigWriteBody`) → persist project `.rupu/config.toml`.
  - `PUT /api/config/policy` (body `{ lock: Vec<String> }`) → set global `[policy].lock`.
- `ConfigWriteBody { raw: Option<String>, patch: Option<serde_json::Value> }` (raw editor sends `raw`; form sends `patch`).
- `ConfigView { effective: serde_json::Value, provenance: BTreeMap<String, KeyProvenance>, raw_global: String, raw_project: Option<String>, cp: serde_json::Value, status: RuntimeStatus }`; `RuntimeStatus { bind: String, token_set: bool, restart_required_keys: Vec<String> }`.

- [ ] **Step 1: Write the failing tests**

Add to `api/config.rs` a `#[cfg(test)] mod tests` (mirror the harness in `api/workspace.rs` tests — `AppState` builder, `oneshot`/direct handler calls):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // Build an AppState with a temp global_dir; helper mirrors api/workspace.rs tests.

    #[tokio::test]
    async fn get_config_returns_effective_and_masks_token() {
        // write a global config.toml with default_model + a token-ish field is NOT
        // in config (token is runtime); assert ConfigView.effective has default_model,
        // provenance marks it Global, status.token_set reflects the runtime, and no
        // secret value is present in the JSON.
    }

    #[tokio::test]
    async fn put_global_persists_and_reloads() {
        // launcher present (writable). PUT raw "default_model = \"sonnet\"" →
        // 200; file on disk updated; AppState.config snapshot reloaded to sonnet.
    }

    #[tokio::test]
    async fn put_global_rejects_unknown_key() {
        // PUT raw "bogus = 1" → 400, file unchanged.
    }

    #[tokio::test]
    async fn put_without_launcher_is_501() {
        // launcher None (read-only deploy). PUT → ApiError::not_available (501).
    }

    #[tokio::test]
    async fn put_project_rejects_locked_key() {
        // global locks "permission_mode"; PUT project raw sets permission_mode →
        // 400 "enforced by global policy".
    }

    #[tokio::test]
    async fn put_project_confines_path() {
        // project id resolving outside .rupu (traversal) → rejected.
    }
}
```

The implementer fills these against the real `AppState` test-builder (copy it from `api/workspace.rs`'s tests) and the real handler signatures.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp --lib api::config`
Expected: FAIL — module not defined.

- [ ] **Step 3: Implement the handlers**

Create `crates/rupu-cp/src/api/config.rs`. Sketch (fill against the real `ApiError`, `AppState`, `fs_safety`, and project-path resolution — see `api/projects.rs` for how a project id maps to its dir):

```rust
use axum::{
    extract::{Path as AxPath, Query, State},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    config_write::{apply_form_patch, validate_toml, write_atomic},
    error::{ApiError, ApiResult},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/config", get(get_config))
        .route("/api/config/global", put(put_global))
        .route("/api/config/project/:id", put(put_project))
        .route("/api/config/policy", put(put_policy))
}

#[derive(Deserialize)]
struct ProjectQuery { project: Option<String> }

#[derive(Serialize)]
struct RuntimeStatus { bind: String, token_set: bool, restart_required_keys: Vec<String> }

#[derive(Serialize)]
struct ConfigView {
    effective: serde_json::Value,
    provenance: BTreeMap<String, rupu_config::KeyProvenance>,
    raw_global: String,
    raw_project: Option<String>,
    cp: serde_json::Value,
    status: RuntimeStatus,
}

async fn get_config(
    State(s): State<AppState>,
    Query(q): Query<ProjectQuery>,
) -> ApiResult<Json<ConfigView>> {
    let global = s.global_dir.join("config.toml");
    let project_path = match &q.project {
        Some(id) => Some(project_config_path(&s, id)?), // confined under <project>/.rupu/config.toml
        None => None,
    };
    let resolved = rupu_config::resolve(Some(&global), project_path.as_deref(), &BTreeMap::new())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let raw_global = std::fs::read_to_string(&global).unwrap_or_default();
    let raw_project = project_path
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok());
    Ok(Json(ConfigView {
        effective: serde_json::to_value(&resolved.config).unwrap_or(serde_json::Value::Null),
        provenance: resolved.provenance,
        raw_global,
        raw_project,
        cp: serde_json::to_value(&resolved.config.cp).unwrap_or(serde_json::Value::Null),
        status: RuntimeStatus {
            bind: s.bind_display(), // add a display helper or thread bind into AppState
            token_set: s.token_is_set(),
            restart_required_keys: vec!["bind".into(), "token".into()],
        },
    }))
}

#[derive(Deserialize)]
struct ConfigWriteBody { raw: Option<String>, patch: Option<serde_json::Value> }

fn require_writable(s: &AppState) -> ApiResult<()> {
    s.launcher
        .as_ref()
        .map(|_| ())
        .ok_or_else(|| ApiError::not_available("editing config requires `rupu cp serve`"))
}

/// Materialize the body to candidate TOML (form patch merged into `existing`,
/// or the raw text), then validate.
fn candidate_toml(body: &ConfigWriteBody, existing: &str) -> ApiResult<String> {
    let cand = match (&body.raw, &body.patch) {
        (Some(raw), _) => raw.clone(),
        (None, Some(patch)) => {
            apply_form_patch(existing, patch).map_err(|e| ApiError::bad_request(e.to_string()))?
        }
        (None, None) => return Err(ApiError::bad_request("body needs `raw` or `patch`")),
    };
    validate_toml(&cand).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(cand)
}

async fn put_global(State(s): State<AppState>, Json(body): Json<ConfigWriteBody>) -> ApiResult<Json<serde_json::Value>> {
    require_writable(&s)?;
    let path = s.global_dir.join("config.toml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let cand = candidate_toml(&body, &existing)?;
    write_atomic(&path, &cand).map_err(|e| ApiError::internal(e.to_string()))?;
    s.reload_config();
    Ok(Json(serde_json::json!({ "ok": true, "restart_required": [] })))
}

async fn put_project(State(s): State<AppState>, AxPath(id): AxPath<String>, Json(body): Json<ConfigWriteBody>) -> ApiResult<Json<serde_json::Value>> {
    require_writable(&s)?;
    let path = project_config_path(&s, &id)?; // fs_safety-confined
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let cand = candidate_toml(&body, &existing)?;
    // Reject setting a globally-LOCKED key from a project layer.
    reject_locked_project_keys(&s, &cand)?;
    write_atomic(&path, &cand).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn put_policy(State(s): State<AppState>, Json(body): Json<PolicyBody>) -> ApiResult<Json<serde_json::Value>> {
    require_writable(&s)?;
    let path = s.global_dir.join("config.toml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let patch = serde_json::json!({ "policy.lock": body.lock });
    let cand = apply_form_patch(&existing, &patch).map_err(|e| ApiError::bad_request(e.to_string()))?;
    validate_toml(&cand).map_err(|e| ApiError::bad_request(e.to_string()))?;
    write_atomic(&path, &cand).map_err(|e| ApiError::internal(e.to_string()))?;
    s.reload_config();
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct PolicyBody { lock: Vec<String> }
```

Implement the helpers:
- `project_config_path(&AppState, id) -> ApiResult<PathBuf>` — resolve the project's dir via `rupu_workspace::WorkspaceStore { root: s.global_dir.join("workspaces") }` (load by `ws_id` as `api/projects.rs` does), take the workspace's path, join `.rupu/config.toml`, and **confine** the result under the workspace path via `api/fs_safety` (reject traversal / a config path escaping the workspace).
- `reject_locked_project_keys(&AppState, candidate_toml) -> ApiResult<()>` — read the global `[policy].lock`; flatten the candidate project TOML; if any flattened key ∈ lock, return `ApiError::bad_request("key `<k>` is enforced by global policy")`.
- `bind_display()` / `token_is_set()` — thread the serve `bind` string and whether a token was set into `AppState` (add fields at startup; tests default them). `token_is_set` returns a bool only — never the token value.

Register: `crates/rupu-cp/src/api/mod.rs` add `pub mod config;`; `server.rs` add `.merge(crate::api::config::routes())` in the merge chain.

- [ ] **Step 4: Run tests, format, lint, commit**

```bash
cargo test -p rupu-cp --lib api::config
cargo test -p rupu-cp --lib
rustfmt --edition 2021 crates/rupu-cp/src/api/config.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/server.rs crates/rupu-cp/src/state.rs
cargo clippy -p rupu-cp --no-deps
git add crates/rupu-cp/src
git commit -m "feat(cp-settings): api/config GET+PUT (global/project/policy) endpoints (T3)"
```
Expected: the config API tests pass; full rupu-cp lib green; clippy clean.

---

## Task 4: web — config API client + global Settings **Form**

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (config types + calls)
- Modify: `crates/rupu-cp/web/src/pages/Settings.tsx` (replace the stub)
- Test: `crates/rupu-cp/web/src/pages/Settings.test.tsx` (new)

**Interfaces:**
- Consumes: `GET /api/config`, `PUT /api/config/global`, `PUT /api/config/policy` (T3).
- Produces: `getConfig(project?)`, `putGlobalConfig(body)`, `putPolicy(lock)` in api.ts; a tabbed `Settings` page whose **Form** tab renders typed sections.

- [ ] **Step 1: Add the api.ts client**

Add types mirroring `ConfigView`/`ConfigWriteBody` and functions `getConfig(project?: string)`, `putGlobalConfig(body: { raw?: string; patch?: Record<string, unknown> })`, `putPolicy(lock: string[])`. Follow the existing api.ts patterns (fetch + error handling used by other pages).

- [ ] **Step 2: Write the failing vitest**

`Settings.test.tsx`: mock `getConfig` to return an effective config + provenance (e.g. `default_model` from Project, `permission_mode` locked from Global) + masked status; assert the Form renders the value, shows a provenance badge ("project"), shows a 🔒 on the locked key, renders `token` as masked (never the value), and that clicking Save calls `putGlobalConfig` with a patch. Assert a validation error from the API surfaces inline.

- [ ] **Step 3: Implement the Form**

Replace `Settings.tsx`'s stub with a page that: fetches `getConfig()`; renders sub-tabs *General · Providers · Autoflow · SCM/Issues · Pricing · CP-Runtime* (Raw + Policy tabs are Task 5); each typed field shows its value + a provenance badge (source) + a 🔒 lock toggle (calls `putPolicy` with the updated lock list); secret/token fields render `••• set` / "not configured" (from `status.token_set`) and are never editable here; a Save button submits changed fields as a `patch` to `putGlobalConfig`; API validation errors render inline. Match the existing page styling/components (see `Hosts.tsx`/`ProjectDetail.tsx` for form + panel patterns).

- [ ] **Step 4: Run web checks + commit**

```bash
cd crates/rupu-cp/web && npm test -- Settings && npx tsc --noEmit && npm run build && cd -
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/Settings.tsx crates/rupu-cp/web/src/pages/Settings.test.tsx
git commit -m "feat(cp-settings): global Settings form + config api client (T4)"
```
Expected: Settings vitest passes; tsc + build clean.

---

## Task 5: web — Raw TOML tab, Policy tab, Runtime status

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/Settings.tsx`
- Test: `crates/rupu-cp/web/src/pages/Settings.test.tsx`

- [ ] **Step 1: Write the failing vitest**

Extend `Settings.test.tsx`: the **Raw** tab shows `raw_global` in the highlight component and on Save submits `{ raw }` to `putGlobalConfig`; a server validation error (400) renders (raw editor shows the message). The **Policy** tab lists lockable keys with checkboxes reflecting `provenance[key].locked` and Save calls `putPolicy` with the new list. The **Runtime status** panel shows `bind` and `token` masked with a "requires restart" note.

- [ ] **Step 2: Implement**

Add three tabs to `Settings.tsx`: **Raw** (reuse the existing syntax-highlight component for TOML; an editable textarea + Validate/Save that posts `{ raw }`; render API validation errors), **Policy** (render the union of typed key paths with a lock checkbox each, seeded from `provenance`; Save posts `putPolicy`), **Runtime** (read-only panel from `status`: bind, `token_set` → `••• set`/"not set", `restart_required_keys` note). No secret values ever rendered.

- [ ] **Step 3: Run web checks + commit**

```bash
cd crates/rupu-cp/web && npm test -- Settings && npx tsc --noEmit && npm run build && cd -
git add crates/rupu-cp/web/src/pages/Settings.tsx crates/rupu-cp/web/src/pages/Settings.test.tsx
git commit -m "feat(cp-settings): raw TOML + policy + runtime-status tabs (T5)"
```

---

## Task 6: web — per-project Config tab on `ProjectDetail`

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/ProjectDetail.tsx`, `crates/rupu-cp/web/src/lib/api.ts` (`putProjectConfig`)
- Test: `crates/rupu-cp/web/src/pages/ProjectDetail.test.tsx`

- [ ] **Step 1: Write the failing vitest**

Extend `ProjectDetail.test.tsx`: a **Config** tab calls `getConfig(projectId)`; renders the project's config (Form + Raw) with provenance showing values inherited from global; a **locked** key renders read-only with 🔒 + "enforced by global policy" and no editable control; Save posts to `putProjectConfig(id, body)`; a locked-key write attempt surfaces the API rejection.

- [ ] **Step 2: Implement**

Add a `putProjectConfig(id, body)` to api.ts (`PUT /api/config/project/:id`). Add a **Config** tab to `ProjectDetail.tsx` reusing the Form + Raw components from Settings (extract shared components if cleanly reusable, else mirror), parameterized by `project=<id>`; locked keys are read-only with the 🔒/enforced note; Save posts the project body; API errors render inline.

- [ ] **Step 3: Run web checks + commit**

```bash
cd crates/rupu-cp/web && npm test -- ProjectDetail && npx tsc --noEmit && npm run build && cd -
git add crates/rupu-cp/web/src/pages/ProjectDetail.tsx crates/rupu-cp/web/src/pages/ProjectDetail.test.tsx crates/rupu-cp/web/src/lib/api.ts
git commit -m "feat(cp-settings): per-project config tab on ProjectDetail (T6)"
```

---

## Task 7: e2e — round-trip + lock enforcement

**Files:**
- Create: `crates/rupu-cp/tests/config_e2e.rs`
- Test: that file

- [ ] **Step 1: Write the e2e test**

Mirror the harness in `crates/rupu-cp/tests/` (or an in-`api/config.rs` integration mod if internals are needed). Two tests:

```rust
// pseudocode shape — fill against the real AppState builder + router
#[tokio::test]
async fn edit_persists_reloads_and_takes_effect() {
    // 1. temp global_dir with config.toml default_model="opus"; writable AppState.
    // 2. PUT /api/config/global raw default_model="sonnet" → 200.
    // 3. GET /api/config → effective.default_model == "sonnet".
    // 4. rupu_config::resolve(global,...) on disk == "sonnet" (took effect).
    // 5. also exercise a form patch { "log_level": "debug" } → persists + comment-safe.
}

#[tokio::test]
async fn global_lock_overrides_project_at_resolution() {
    // global: permission_mode="ask", [policy] lock=["permission_mode"].
    // project .rupu/config.toml: permission_mode="bypass".
    // resolve(global, project) → permission_mode == "ask" (locked); provenance locked.
    // PUT project permission_mode="bypass" → 400 enforced-by-policy.
}
```

- [ ] **Step 2: Run, format, lint, commit**

```bash
cargo test -p rupu-cp --test config_e2e
cargo test -p rupu-cp --lib
rustfmt --edition 2021 crates/rupu-cp/tests/config_e2e.rs
cargo clippy -p rupu-cp --all-targets --no-deps
git add crates/rupu-cp/tests/config_e2e.rs
git commit -m "test(cp-settings): e2e config round-trip + lock enforcement (T7)"
```

---

## Self-Review

**Spec coverage:**
- `[policy]` lock engine + `[cp]` section + provenance `resolve()` → T1. ✅
- Write-safety (validate/backup/atomic/lock) + reloadable snapshot + configurable `max_workspace_bytes` → T2. ✅
- GET/PUT config API (global/project/policy), launcher-gated, fs_safety-confined, locked-key rejection, token masking, snapshot reload → T3. ✅
- Global Form (provenance + lock toggle + secret masking) → T4; Raw + Policy + Runtime status → T5. ✅
- Per-project Config tab (inherited/locked read-only) → T6. ✅
- Hybrid (form + raw) both validate + write same file → T2 (`candidate_toml`) + T4/T5. ✅
- Backward compat (no `[policy]`/`[cp]` ⇒ today) → T1 test `no_policy_block_matches_layer_files`; e2e → T7. ✅
- Errors/security (no silent-noop reload, secrets never echoed, atomic+backup, confinement, restart-flagged) → T2/T3, asserted in T3/T7. ✅

**Placeholder scan:** The Rust core (T1 `resolve`, T2 `config_write`) is complete code. T3 handlers are near-complete with the helper contracts spelled out (project-path resolution + locked-key rejection + bind/token status), pointing at `api/projects.rs`/`api/workspace.rs` for the exact AppState-builder/id-mapping — deliberate, since those are existing-pattern lookups, not inventions. Web tasks (T4–T6) give exact API shapes, component responsibilities, and test cases, building React from existing page patterns (as prior CP plans did). No "TBD"/vague-error placeholders.

**Type consistency:** `resolve(global, project, env) -> Resolved{config, provenance}` + `KeyProvenance{source,locked}` + `KeySource` (T1) are consumed by T3's `ConfigView` and the web client; `PolicyConfig{lock}`/`CpConfig{max_workspace_bytes}` (T1) used in T2 (`effective_max_workspace_bytes`) + T3 (`put_policy`); `config_write::{validate_toml, write_atomic, apply_form_patch}` (T2) consumed by T3; `AppState.config` + `reload_config` (T2) consumed by T3; `ConfigWriteBody{raw,patch}` consistent T3↔T4/T5/T6. Names align across tasks.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-01-rupu-cp-settings-plan.md`. Build via subagent-driven-development: fresh implementer per task, task review (spec + quality) after each, a broad whole-branch review at the end, then a single PR to `main` (no self-merge — matt reviews, and validates the CP UI before merge).
