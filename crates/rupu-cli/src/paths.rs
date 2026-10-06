//! `~/.rupu/` resolution + project `.rupu/` discovery.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Resolve the global rupu directory. Honors `$RUPU_HOME` if set
/// (used by tests + by users who want a non-default location);
/// otherwise falls back to `~/.rupu/`.
pub fn global_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("RUPU_HOME") {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate home directory"))?;
    Ok(home.join(".rupu"))
}

/// Walk up from `pwd` looking for the first `.rupu/` directory. Returns
/// `Some(path)` of the directory containing it, or `None` if not found.
pub fn project_root_for(pwd: &Path) -> Result<Option<PathBuf>> {
    let canonical = pwd
        .canonicalize()
        .with_context(|| format!("canonicalize {}", pwd.display()))?;
    let mut cursor: Option<&Path> = Some(&canonical);
    while let Some(dir) = cursor {
        if dir.join(".rupu").is_dir() {
            return Ok(Some(dir.to_path_buf()));
        }
        cursor = dir.parent();
    }
    Ok(None)
}

/// Pick the transcripts directory. Project-local when
/// `<project>/.rupu/transcripts/` exists; global default otherwise.
pub fn transcripts_dir(global: &Path, project_root: Option<&Path>) -> PathBuf {
    if let Some(p) = project_root {
        let local = p.join(".rupu/transcripts");
        if local.is_dir() {
            return local;
        }
    }
    global.join("transcripts")
}

/// Pick the netflow directory. Project-local when
/// `<project>/.rupu/netflow/` exists; global default otherwise.
///
/// Deliberately the same shape as [`transcripts_dir`] — a netflow ledger
/// has the same lifecycle as a transcript and the two resolutions must not
/// drift. The existence check is load-bearing: a repo that was never
/// `rupu init`'d falls back to global, so no ledger is ever written inside
/// a project that has not opted in.
///
/// Thin wrapper over `rupu_netflow::netflow_dir` — the actual rule now
/// lives there (a shared crate both `rupu-cli` and `rupu-orchestrator`
/// depend on) so the write side can never drift into two competing
/// copies of the same resolution logic. `rupu-cp`'s read side must
/// mirror this same rule; see `rupu_netflow::netflow_dir`'s doc comment.
pub fn netflow_dir(global: &Path, project_root: Option<&Path>) -> PathBuf {
    rupu_netflow::netflow_dir(global, project_root)
}

/// Global repo registry directory.
pub fn repos_dir(global: &Path) -> PathBuf {
    global.join("repos")
}

/// Global session state root.
pub fn sessions_dir(global: &Path) -> PathBuf {
    global.join("sessions")
}

/// Global archived session state root.
pub fn archived_sessions_dir(global: &Path) -> PathBuf {
    global.join("sessions-archive")
}

/// Archive directory nested under a transcript root.
pub fn archived_transcripts_dir(transcripts_dir: &Path) -> PathBuf {
    transcripts_dir.join("archive")
}

/// Global UI theme directory.
pub fn themes_dir(global: &Path) -> PathBuf {
    global.join("themes")
}

/// Project-local UI theme directory.
pub fn project_themes_dir(project_root: &Path) -> PathBuf {
    project_root.join(".rupu/themes")
}

/// Global autoflow state root.
pub fn autoflows_dir(global: &Path) -> PathBuf {
    global.join("autoflows")
}

/// Global autoflow claims directory.
pub fn autoflow_claims_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("claims")
}

/// Global autoflow worktrees directory.
pub fn autoflow_worktrees_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("worktrees")
}

/// Global autoflow worker registry directory.
pub fn autoflow_workers_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("workers")
}

/// Global autoflow event cursor directory.
pub fn autoflow_event_cursors_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("event-cursors")
}

/// Global autoflow wake queue root.
pub fn autoflow_wakes_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("wakes")
}

/// Global autoflow cycle/event history root.
pub fn autoflow_history_dir(global: &Path) -> PathBuf {
    autoflows_dir(global).join("history")
}

/// Global queued autoflow wake-record directory.
pub fn autoflow_wake_queue_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("queue")
}

/// Global processed autoflow wake-record directory.
pub fn autoflow_wake_processed_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("processed")
}

/// Global autoflow wake payload directory.
pub fn autoflow_wake_payloads_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("payloads")
}

/// Global autoflow wake dedupe marker directory.
pub fn autoflow_wake_dedupe_dir(global: &Path) -> PathBuf {
    autoflow_wakes_dir(global).join("dedupe")
}

/// Convenience: ensure a directory exists. Used to lazily create
/// `~/.rupu/cache/`, `~/.rupu/transcripts/`, etc. on first use.
pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("create_dir_all {}", p.display()))?;
    Ok(())
}

/// The config files one load layers: global, the customer of the project
/// (if assigned), and the project's `.rupu/config.toml`. Spec:
/// `docs/superpowers/specs/2026-10-06-rupu-customers-design.md` §2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub global: PathBuf,
    pub customer: Option<PathBuf>,
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

    fn without_customer(global: &Path, project_root: Option<&Path>) -> Self {
        Self {
            global: global.join("config.toml"),
            customer: None,
            project: project_config_path(global, project_root),
        }
    }
}

/// The project layer's config file for `project_root`, or `None` when there
/// is no project root or its `.rupu/` IS the global dir.
///
/// `project_root_for` treats `~/.rupu` as a project marker, so a repo
/// without its own `.rupu/` resolves to `$HOME`, whose "project config" is
/// the global `config.toml` itself. Loading that file a second time as the
/// project layer let global values outrank the customer layer (project beats
/// customer), so the layer is dropped there. Paths are compared
/// canonicalized, or raw when either side cannot be canonicalized.
fn project_config_path(global: &Path, project_root: Option<&Path>) -> Option<PathBuf> {
    let root = project_root?;
    let project_dir = root.join(".rupu");
    let same = match (project_dir.canonicalize(), global.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => project_dir == global,
    };
    if same {
        None
    } else {
        Some(project_dir.join("config.toml"))
    }
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
    global: &Path,
    project_root: Option<&Path>,
    run_dir: &Path,
) -> Result<ConfigPaths> {
    let store = rupu_workspace::CustomerStore::new(global);
    let mut customer = store.customer_config_for_dir(run_dir)?;
    if customer.is_none() {
        if let Some(root) = project_root {
            customer = store.customer_config_for_dir(root)?;
        }
    }
    Ok(ConfigPaths {
        customer,
        ..ConfigPaths::without_customer(global, project_root)
    })
}

/// [`config_paths`] for display-only reads (UI preferences, pricing tables,
/// listings): a failed customer lookup is logged and the customer layer
/// left out, so a broken assignment never stops `rupu transcript list`.
/// Never use this where the config picks a provider, account or permission.
pub fn config_paths_for_display(
    global: &Path,
    project_root: Option<&Path>,
    run_dir: &Path,
) -> ConfigPaths {
    config_paths(global, project_root, run_dir).unwrap_or_else(|e| {
        tracing::warn!(error = %format!("{e:#}"), "customer config layer skipped for this display");
        ConfigPaths::without_customer(global, project_root)
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
    global: &Path,
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
    let paths = config_paths_for_display(global, project_root, run_dir);
    let first = match load(&paths) {
        Ok(cfg) => return cfg,
        Err(e) => e,
    };
    if paths.customer.is_some() {
        tracing::warn!(
            error = %format!("{first:#}"),
            "config failed to load for this display; retrying without the customer layer"
        );
        match load(&ConfigPaths::without_customer(global, project_root)) {
            Ok(cfg) => return cfg,
            Err(e) => tracing::warn!(
                error = %format!("{e:#}"),
                "config failed to load for this display; using defaults"
            ),
        }
    } else {
        tracing::warn!(
            error = %format!("{first:#}"),
            "config failed to load for this display; using defaults"
        );
    }
    rupu_config::Config::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netflow_dir_prefers_an_existing_project_local_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".rupu/netflow")).unwrap();

        assert_eq!(
            netflow_dir(&global, Some(&project)),
            project.join(".rupu/netflow")
        );
    }

    #[test]
    fn netflow_dir_falls_back_to_global_when_the_project_dir_does_not_exist() {
        // Load-bearing: a repo that was never `rupu init`'d must never get a
        // ledger written inside it. This is what closes the git-leak class
        // structurally rather than by patching ensure_dir.
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        assert_eq!(netflow_dir(&global, Some(&project)), global.join("netflow"));
    }

    #[test]
    fn netflow_dir_falls_back_to_global_with_no_project_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        assert_eq!(netflow_dir(&global, None), global.join("netflow"));
    }
}

#[cfg(test)]
mod customer_layer_tests {
    use super::*;
    use rupu_workspace::{CustomerStore, NewCustomer, ProjectRef};

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(project.join(".rupu")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        (tmp, home, project)
    }

    fn assign(home: &Path, project: &Path) {
        let store = CustomerStore::new(home);
        store
            .create(
                "acme",
                &NewCustomer {
                    name: "Acme".into(),
                    ..NewCustomer::default()
                },
            )
            .unwrap();
        store.assign("acme", ProjectRef::Path(project)).unwrap();
    }

    #[test]
    fn config_paths_includes_the_customer_of_the_project_root() {
        let (_t, home, project) = setup();
        assign(&home, &project);
        let p = config_paths(&home, Some(&project), Path::new("/")).unwrap();
        assert_eq!(p.global, home.join("config.toml"));
        assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
        assert_eq!(p.project, Some(project.join(".rupu/config.toml")));
    }

    #[test]
    fn the_global_dir_is_never_also_the_project_layer() {
        // `project_root_for` resolves a repo without its own `.rupu/` to
        // `$HOME`, whose `.rupu` is the global dir: its config.toml must not
        // be loaded a second time as the project layer.
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join(".rupu");
        std::fs::create_dir_all(&global).unwrap();
        let p = config_paths(&global, Some(tmp.path()), tmp.path()).unwrap();
        assert_eq!(p.global, global.join("config.toml"));
        assert_eq!(p.project, None);
        // Also through a non-canonical spelling of the same directory.
        let dotted = tmp.path().join("sub/..");
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let p = config_paths(&global, Some(&dotted), tmp.path()).unwrap();
        assert_eq!(p.project, None);
        // A root whose `.rupu` is a symlink to the global dir (canonical
        // compare, not a textual one).
        #[cfg(unix)]
        {
            let alias = tmp.path().join("alias");
            std::fs::create_dir_all(&alias).unwrap();
            std::os::unix::fs::symlink(&global, alias.join(".rupu")).unwrap();
            let p = config_paths(&global, Some(&alias), &alias).unwrap();
            assert_eq!(p.project, None);
        }
        // A normal project root still yields its own config.
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join(".rupu")).unwrap();
        let p = config_paths(&global, Some(&proj), &proj).unwrap();
        assert_eq!(p.project, Some(proj.join(".rupu/config.toml")));
    }

    #[test]
    fn config_paths_falls_back_to_the_run_dir_without_a_project_root() {
        let (_t, home, project) = setup();
        assign(&home, &project);
        let p = config_paths(&home, None, &project).unwrap();
        assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
        assert_eq!(p.project, None);
    }

    #[test]
    fn config_paths_looks_up_from_the_run_dir_before_the_project_root() {
        // `project_root_for` resolves to $HOME (which has `~/.rupu`) for any
        // repo without its own `.rupu/`; the repo's own assignment must win.
        let (_t, home, fake_home) = setup();
        let repo = fake_home.join("code/repo");
        std::fs::create_dir_all(&repo).unwrap();
        assign(&home, &repo);
        let p = config_paths(&home, Some(&fake_home), &repo).unwrap();
        assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
        assert_eq!(p.project, Some(fake_home.join(".rupu/config.toml")));

        // Even when the project root has a customer of its own.
        let store = CustomerStore::new(&home);
        store
            .create(
                "homeco",
                &NewCustomer {
                    name: "Home".into(),
                    ..NewCustomer::default()
                },
            )
            .unwrap();
        store
            .assign("homeco", ProjectRef::Path(&fake_home))
            .unwrap();
        let p = config_paths(&home, Some(&fake_home), &repo).unwrap();
        assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
        // An unassigned run dir falls back to the project root's customer.
        let elsewhere = fake_home.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let p = config_paths(&home, Some(&fake_home), &elsewhere).unwrap();
        assert_eq!(p.customer, Some(home.join("customers/homeco/config.toml")));
    }

    #[test]
    fn display_loader_drops_a_malformed_customer_layer_and_keeps_the_rest() {
        let (_t, home, project) = setup();
        assign(&home, &project);
        std::fs::write(
            home.join("config.toml"),
            "default_model = \"global-model\"\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".rupu/config.toml"),
            "[ui]\ntheme = \"project-theme\"\n",
        )
        .unwrap();
        std::fs::write(
            home.join("customers/acme/config.toml"),
            "this is = = not toml",
        )
        .unwrap();
        for locked in [true, false] {
            let cfg = load_config_for_display(&home, Some(&project), &project, locked);
            assert_eq!(cfg.default_model.as_deref(), Some("global-model"));
            assert_eq!(cfg.ui.theme.as_deref(), Some("project-theme"));
        }
    }

    #[test]
    fn display_loader_reads_the_customer_layer_when_it_is_fine() {
        let (_t, home, project) = setup();
        assign(&home, &project);
        std::fs::write(
            home.join("config.toml"),
            "default_model = \"global-model\"\n",
        )
        .unwrap();
        std::fs::write(
            home.join("customers/acme/config.toml"),
            "default_model = \"acme-model\"\n",
        )
        .unwrap();
        let cfg = load_config_for_display(&home, Some(&project), &project, false);
        assert_eq!(cfg.default_model.as_deref(), Some("acme-model"));
    }

    #[test]
    fn strict_errors_and_display_degrades_on_a_dangling_assignment() {
        let (_t, home, project) = setup();
        assign(&home, &project);
        std::fs::remove_dir_all(home.join("customers/acme")).unwrap();
        let err = config_paths(&home, Some(&project), &project).unwrap_err();
        // Launch failures print `{}` — the actionable cause must be in it.
        let shown = err.to_string();
        assert!(shown.contains("acme"), "{shown}");
        assert!(shown.contains("does not exist"), "{shown}");
        let p = config_paths_for_display(&home, Some(&project), &project);
        assert_eq!(p.customer, None);
        assert_eq!(p.project, Some(project.join(".rupu/config.toml")));
    }
}
