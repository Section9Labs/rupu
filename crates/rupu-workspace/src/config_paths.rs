//! Which config files one load layers — global, the project's customer,
//! and the project's `.rupu/config.toml` — shared by the CLI (every config
//! load) and the CP (Settings, the launch preview) so the two can never
//! resolve a run differently. Spec:
//! `docs/superpowers/specs/2026-10-06-rupu-customers-design.md` §1–2.

use crate::customers::{CustomerError, CustomerStore};
use std::path::{Path, PathBuf};

/// Walk up from `pwd` looking for the first `.rupu/` directory. Returns
/// `Some(path)` of the directory containing it, or `None` if not found.
///
/// NOTE: `~/.rupu` (the global dir) counts, so from any directory under
/// `$HOME` without its own `.rupu/` this returns `$HOME`. Never key
/// per-repo state off it; see [`config_paths`] for how the customer and
/// project layers cope.
pub fn project_root_for(pwd: &Path) -> std::io::Result<Option<PathBuf>> {
    let canonical = pwd.canonicalize()?;
    let mut cursor: Option<&Path> = Some(&canonical);
    while let Some(dir) = cursor {
        if dir.join(".rupu").is_dir() {
            return Ok(Some(dir.to_path_buf()));
        }
        cursor = dir.parent();
    }
    Ok(None)
}

/// The config files one load layers: global, the customer of the project
/// (if assigned), and the project's `.rupu/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub global: PathBuf,
    pub customer: Option<PathBuf>,
    /// The slug `customer` belongs to — what a run records as its customer.
    pub customer_slug: Option<String>,
    pub project: Option<PathBuf>,
}

impl ConfigPaths {
    pub fn layers(&self) -> rupu_config::LayerPaths<'_> {
        rupu_config::LayerPaths::new(
            Some(&self.global),
            self.customer.as_deref(),
            self.project.as_deref(),
        )
    }

    /// Global + project layers only (no customer lookup).
    pub fn without_customer(global: &Path, project_root: Option<&Path>) -> Self {
        Self {
            global: global.join("config.toml"),
            customer: None,
            customer_slug: None,
            project: project_config_path(global, project_root),
        }
    }
}

/// The project layer's `.rupu/` for `project_root`, or `None` when there is
/// no project root or its `.rupu/` IS the global dir (R5).
///
/// `project_root_for` treats `~/.rupu` as a project marker, so a repo
/// without its own `.rupu/` resolves to `$HOME`, whose "project layer" is
/// the global dir itself. Loading it a second time as the project layer let
/// global values outrank the customer layer (project beats customer), so the
/// layer is dropped there. Paths are compared canonicalized, or raw when
/// either side cannot be canonicalized.
pub fn project_layer_dir(global: &Path, project_root: Option<&Path>) -> Option<PathBuf> {
    let project_dir = project_root?.join(".rupu");
    let same = match (project_dir.canonicalize(), global.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => project_dir == global,
    };
    (!same).then_some(project_dir)
}

fn project_config_path(global: &Path, project_root: Option<&Path>) -> Option<PathBuf> {
    project_layer_dir(global, project_root).map(|d| d.join("config.toml"))
}

/// Layer paths for a load that serves a project. The customer is the one
/// assigned to the nearest ancestor of `run_dir` (the directory the run or
/// command works in); only when that walk finds no assignment is the
/// nearest assigned ancestor of `project_root` used. `run_dir` goes first
/// because `project_root_for` resolves to `$HOME` (which has `~/.rupu`) for
/// any repo without its own `.rupu/`, and a project-root-first lookup would
/// miss that repo's assignment.
///
/// **Strict** — for launch paths and for anything whose result drives a
/// provider, SCM-account or permission decision: a project assigned to a
/// customer that no longer exists is an error, never a silent fall back to
/// the global config. The error is the store's own (it names the project,
/// the customer and the fix), uncontexted, so a launch failure printed with
/// `{}` still shows it.
pub fn config_paths(
    home: &Path,
    project_root: Option<&Path>,
    run_dir: &Path,
) -> Result<ConfigPaths, CustomerError> {
    let store = CustomerStore::new(home);
    let mut slug = store.customer_for_dir(run_dir)?;
    if slug.is_none() {
        if let Some(root) = project_root {
            slug = store.customer_for_dir(root)?;
        }
    }
    Ok(ConfigPaths {
        customer: slug.as_deref().map(|s| store.config_path(s)),
        customer_slug: slug,
        ..ConfigPaths::without_customer(home, project_root)
    })
}

/// `e` and its sources as one line, `a: b: c` (anyhow's `{:#}`).
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

/// [`config_paths`] for display-only reads (UI preferences, pricing tables,
/// listings): a failed customer lookup is logged and the customer layer
/// left out, so a broken assignment never stops `rupu transcript list`.
/// Never use this where the config picks a provider, account or permission.
pub fn config_paths_for_display(
    home: &Path,
    project_root: Option<&Path>,
    run_dir: &Path,
) -> ConfigPaths {
    config_paths(home, project_root, run_dir).unwrap_or_else(|e| {
        tracing::warn!(error = %error_chain(&e), "customer config layer skipped for this display");
        ConfigPaths::without_customer(home, project_root)
    })
}

/// The config for a display-only read (UI preferences, pricing, listings),
/// never failing: [`config_paths_for_display`] layers, loaded with
/// `layer_files_locked` when `locked` (a policy-bearing value such as
/// `[ui].editor` or pricing) else `layer_files`. A layer that fails to load
/// is logged and the load retried without the customer layer; if that fails
/// too, it is logged and `Config::default()` returned. Never use this where
/// the config picks a provider, account or permission — use
/// [`config_paths`] there.
pub fn load_config_for_display(
    home: &Path,
    project_root: Option<&Path>,
    run_dir: &Path,
    locked: bool,
) -> rupu_config::Config {
    let load = |p: &ConfigPaths| {
        if locked {
            rupu_config::layer_files_locked(p.layers())
        } else {
            rupu_config::layer_files(p.layers())
        }
    };
    let paths = config_paths_for_display(home, project_root, run_dir);
    let first = match load(&paths) {
        Ok(cfg) => return cfg,
        Err(e) => e,
    };
    if paths.customer.is_some() {
        tracing::warn!(
            error = %error_chain(&first),
            "config failed to load for this display; retrying without the customer layer"
        );
        match load(&ConfigPaths::without_customer(home, project_root)) {
            Ok(cfg) => return cfg,
            Err(e) => tracing::warn!(
                error = %error_chain(&e),
                "config failed to load for this display; using defaults"
            ),
        }
    } else {
        tracing::warn!(
            error = %error_chain(&first),
            "config failed to load for this display; using defaults"
        );
    }
    rupu_config::Config::default()
}

/// Layers for a known customer `slug` (resume by the recorded slug).
/// `Some(slug)` with no customer directory is [`CustomerError::NotFound`].
pub fn config_paths_for_customer(
    home: &Path,
    slug: Option<&str>,
    project_root: Option<&Path>,
) -> Result<ConfigPaths, CustomerError> {
    let store = CustomerStore::new(home);
    let customer = match slug {
        Some(s) => {
            store.get(s)?; // NotFound when the customer directory is gone
            Some(store.config_path(s))
        }
        None => None,
    };
    Ok(ConfigPaths {
        customer,
        customer_slug: slug.map(str::to_string),
        ..ConfigPaths::without_customer(home, project_root)
    })
}

/// Whether `name` can name a definition file (`<dir>/<name>.yaml` /
/// `<name>.md`) without leaving that directory: non-empty, and no `/`, `\`,
/// `..` or NUL. Checked before a caller-supplied name is joined onto a path.
pub fn is_safe_definition_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\0']) && !name.contains("..")
}

/// Workflow definition file extensions, in preference order: in one
/// directory `<name>.yaml` wins over `<name>.yml`. Held in lockstep with
/// `rupu_orchestrator::catalog::WORKFLOW_EXTENSIONS` (the two crates don't
/// depend on each other).
pub const WORKFLOW_EXTENSIONS: [&str; 2] = ["yaml", "yml"];

/// `<dir>/<name>.yaml`, else `<dir>/<name>.yml` — whichever is a file.
pub fn workflow_file(dir: &Path, name: &str) -> Option<PathBuf> {
    WORKFLOW_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|p| p.is_file())
}

/// The workflow file `name` resolves to from a project: the project's
/// `.rupu/workflows/<name>.yaml` (else `.yml`) first, then
/// `<global>/workflows/<name>.yaml` (else `.yml`) — `rupu workflow run`'s
/// lookup, shared by the CLI and the CP (launch preview, definition reads,
/// autoflow toggles) so they can never find different files. `None` when
/// none exists, and for a name that is not [`is_safe_definition_name`] (it
/// could only resolve outside the workflow directories).
pub fn locate_workflow(global: &Path, project_root: Option<&Path>, name: &str) -> Option<PathBuf> {
    if !is_safe_definition_name(name) {
        return None;
    }
    project_root
        .map(|r| r.join(".rupu").join("workflows"))
        .into_iter()
        .chain(std::iter::once(global.join("workflows")))
        .find_map(|dir| workflow_file(&dir, name))
}
