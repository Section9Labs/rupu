//! Which autoflows a directory sees ([`visible_autoflows`]), which ones a
//! tick runs ([`discover_tick_autoflows`]), and how one resolves — with its
//! repo binding, checkout and layered config — for an issue.

use crate::paths;
use crate::refs::{format_issue_ref, issue_ref_to_repo_ref, normalize_repo_filter};
use crate::Refusal;
use anyhow::{anyhow, bail};
use rupu_config::Config;
use rupu_orchestrator::Workflow;
use rupu_scm::checkout::{canonical_repo_ref, repo_of_checkout};
use rupu_scm::{EventSourceRef, IssueRef, IssueTracker, Platform};
use rupu_workspace::RepoRegistryStore;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tracing::warn;

/// An autoflow resolved for a repo: its workflow, the tracked checkout it
/// runs in and that checkout's layered config.
#[derive(Debug, Clone)]
pub struct ResolvedAutoflowWorkflow {
    pub scope: String,
    pub name: String,
    pub workflow: Workflow,
    pub workflow_path: PathBuf,
    pub project_root: Option<PathBuf>,
    pub repo_ref: String,
    pub preferred_checkout: PathBuf,
    pub cfg: Config,
}

impl ResolvedAutoflowWorkflow {
    pub fn autoflow(&self) -> anyhow::Result<&rupu_orchestrator::Autoflow> {
        self.workflow
            .autoflow
            .as_ref()
            .filter(|autoflow| autoflow.enabled)
            .ok_or_else(|| anyhow!("workflow `{}` is not autoflow-enabled", self.workflow.name))
    }
}

/// An autoflow a directory sees: a workflow with `autoflow.enabled`, where
/// it was found, and the repo it is bound to when one is known.
#[derive(Debug, Clone)]
pub struct VisibleAutoflowWorkflow {
    pub scope: String,
    pub name: String,
    pub workflow: Workflow,
    pub workflow_path: PathBuf,
    pub project_root: Option<PathBuf>,
    pub repo_ref: Option<String>,
    pub preferred_checkout: Option<PathBuf>,
}

impl VisibleAutoflowWorkflow {
    pub fn autoflow(&self) -> anyhow::Result<&rupu_orchestrator::Autoflow> {
        self.workflow
            .autoflow
            .as_ref()
            .filter(|autoflow| autoflow.enabled)
            .ok_or_else(|| anyhow!("workflow `{}` is not autoflow-enabled", self.workflow.name))
    }
}

/// A tracked repo an autoflow tick left out because its config (customer
/// layer included) failed to load. The tick records each as a
/// `cycle_failed` event naming the repo, so the skip is visible in the
/// autoflow history rather than only in a log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoConfigSkip {
    pub repo_ref: String,
    pub error: String,
}

/// Record (and log) that `repo_ref` was left out because its config failed
/// to load; each skip once.
pub fn record_repo_config_skip(
    skips: &mut Vec<RepoConfigSkip>,
    repo_ref: &str,
    error: &anyhow::Error,
) {
    let error = format!("{error:#}");
    warn!(repo_ref, error = %error, "skipping tracked repo this cycle: its config failed to load");
    let skip = RepoConfigSkip {
        repo_ref: repo_ref.to_string(),
        error,
    };
    if !skips.contains(&skip) {
        skips.push(skip);
    }
}

/// Global + customer + project config for `project_root`, strictly (a
/// dangling customer assignment is an error), with the customer looked up
/// from `project_root` itself. With no project this serves no project, so it
/// reads the global file alone. Prefer [`resolve_config_in`] wherever the
/// repo's own directory is known.
pub fn resolve_config(global: &Path, project_root: Option<&Path>) -> anyhow::Result<Config> {
    match project_root {
        Some(root) => resolve_config_in(global, Some(root), root),
        None => Ok(rupu_config::layer_files_locked(
            rupu_config::LayerPaths::global_only(&global.join("config.toml")),
        )?),
    }
}

/// [`resolve_config`] with the customer looked up from `run_dir` (a tracked
/// repo's checkout, the cwd) first — `project_root_for` resolves to `$HOME`
/// for a repo without its own `.rupu/`, which would miss the repo's
/// assignment (see [`rupu_workspace::config_paths`]).
pub fn resolve_config_in(
    global: &Path,
    project_root: Option<&Path>,
    run_dir: &Path,
) -> anyhow::Result<Config> {
    let cfg_paths = rupu_workspace::config_paths(global, project_root, run_dir)?;
    Ok(rupu_config::layer_files_locked(cfg_paths.layers())?)
}

/// Every autoflow `pwd` sees: its project's and the global ones (bound to
/// the project's repo), else the global ones (bound to `[autoflow].repo`),
/// plus every tracked repo's ([`discover_tick_autoflows`]). Sorted by repo,
/// name, scope, path.
pub fn visible_autoflows(
    global: &Path,
    pwd: &Path,
) -> anyhow::Result<Vec<VisibleAutoflowWorkflow>> {
    let project_root = paths::project_root_for(pwd)?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(global),
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();

    if let Some(project_root) = &project_root {
        let cfg = resolve_config_in(global, Some(project_root), pwd)?;
        let repo_ref = cfg.autoflow.repo.clone().or_else(|| {
            repo_of_checkout(project_root)
                .ok()
                .map(|repo| canonical_repo_ref(&repo))
        });
        push_visible_workflow_paths(
            &project_root.join(".rupu/workflows"),
            "project",
            Some(project_root),
            repo_ref.clone(),
            Some(project_root.clone()),
            &mut seen,
            &mut out,
        )?;
        push_visible_workflow_paths(
            &global.join("workflows"),
            "global",
            Some(project_root),
            repo_ref,
            Some(project_root.clone()),
            &mut seen,
            &mut out,
        )?;
    } else {
        let global_cfg = resolve_config(global, None)?;
        push_visible_workflow_paths(
            &global.join("workflows"),
            "global",
            None,
            global_cfg.autoflow.repo.clone(),
            None,
            &mut seen,
            &mut out,
        )?;
    }

    for resolved in discover_tick_autoflows(global, &repo_store)? {
        let key = visible_autoflow_key(
            Some(&resolved.repo_ref),
            &resolved.scope,
            &resolved.workflow_path,
        );
        if !seen.insert(key) {
            continue;
        }
        out.push(VisibleAutoflowWorkflow {
            scope: resolved.scope,
            name: resolved.name,
            workflow_path: resolved.workflow_path,
            workflow: resolved.workflow,
            project_root: resolved.project_root,
            repo_ref: Some(resolved.repo_ref),
            preferred_checkout: Some(resolved.preferred_checkout),
        });
    }

    out.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.workflow_path.cmp(&right.workflow_path))
    });
    Ok(out)
}

pub fn push_visible_workflow_paths(
    dir: &Path,
    scope: &str,
    preferred_checkout: Option<&Path>,
    repo_ref: Option<String>,
    project_root: Option<PathBuf>,
    seen: &mut BTreeSet<String>,
    into: &mut Vec<VisibleAutoflowWorkflow>,
) -> anyhow::Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in rd {
        let entry = entry?;
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let body = std::fs::read_to_string(&path)?;
        let workflow = Workflow::parse(&body)?;
        if !workflow
            .autoflow
            .as_ref()
            .map(|a| a.enabled)
            .unwrap_or(false)
        {
            continue;
        }
        let key = visible_autoflow_key(repo_ref.as_deref(), scope, &path);
        if !seen.insert(key) {
            continue;
        }
        into.push(VisibleAutoflowWorkflow {
            scope: scope.to_string(),
            name: workflow.name.clone(),
            workflow,
            workflow_path: path,
            project_root: project_root.clone(),
            repo_ref: repo_ref.clone(),
            preferred_checkout: preferred_checkout.map(Path::to_path_buf),
        });
    }
    Ok(())
}

pub fn visible_autoflow_key(repo_ref: Option<&str>, scope: &str, path: &Path) -> String {
    format!("{}|{}|{}", repo_ref.unwrap_or("-"), scope, path.display())
}

pub fn discover_tick_autoflows(
    global: &Path,
    repo_store: &RepoRegistryStore,
) -> anyhow::Result<Vec<ResolvedAutoflowWorkflow>> {
    // Skips are logged by `record_repo_config_skip`; callers that keep an
    // autoflow history use `discover_tick_autoflows_reporting`.
    discover_tick_autoflows_reporting(global, repo_store, &mut Vec::new())
}

/// [`discover_tick_autoflows`], also returning (in `skips`) each tracked
/// repo left out because its config failed to load — one broken repo (a
/// dangling customer assignment, a malformed layer) never stops discovery
/// for the others.
pub fn discover_tick_autoflows_reporting(
    global: &Path,
    repo_store: &RepoRegistryStore,
    skips: &mut Vec<RepoConfigSkip>,
) -> anyhow::Result<Vec<ResolvedAutoflowWorkflow>> {
    let mut out = Vec::new();
    let global_cfg = resolve_config(global, None)?;
    if global_cfg.autoflow.enabled.unwrap_or(true) {
        if let Some(repo_ref) = global_cfg.autoflow.repo.clone() {
            if let Some(tracked) = repo_store.load(&repo_ref)? {
                let preferred_checkout = PathBuf::from(&tracked.preferred_path);
                let project_root = paths::project_root_for(&preferred_checkout)?
                    .or_else(|| Some(preferred_checkout.clone()));
                push_resolved_autoflow_paths(
                    &global.join("workflows"),
                    "global",
                    global,
                    project_root,
                    repo_ref,
                    preferred_checkout,
                    global_cfg.clone(),
                    &mut out,
                )?;
            } else {
                warn!(
                    repo_ref,
                    "skipping global autoflows because repo is not tracked"
                );
            }
        }
    }

    for tracked in repo_store.list()? {
        let preferred_checkout = PathBuf::from(&tracked.preferred_path);
        if !preferred_checkout.exists() {
            warn!(path = %preferred_checkout.display(), repo_ref = %tracked.repo_ref, "skipping tracked repo because preferred checkout is missing");
            continue;
        }
        let project_root = paths::project_root_for(&preferred_checkout)?.or_else(|| {
            preferred_checkout
                .join(".rupu")
                .is_dir()
                .then_some(preferred_checkout.clone())
        });
        let Some(project_root) = project_root else {
            continue;
        };
        let cfg = match resolve_config_in(global, Some(&project_root), &preferred_checkout) {
            Ok(cfg) => cfg,
            Err(e) => {
                record_repo_config_skip(skips, &tracked.repo_ref, &e);
                continue;
            }
        };
        if cfg.autoflow.enabled == Some(false) {
            continue;
        }
        push_resolved_autoflow_paths(
            &project_root.join(".rupu/workflows"),
            "project",
            global,
            Some(project_root),
            tracked.repo_ref.clone(),
            preferred_checkout,
            cfg,
            &mut out,
        )?;
    }

    out.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.scope.cmp(&right.scope))
    });
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub fn push_resolved_autoflow_paths(
    dir: &Path,
    scope: &str,
    global: &Path,
    project_root: Option<PathBuf>,
    repo_ref: String,
    preferred_checkout: PathBuf,
    cfg: Config,
    into: &mut Vec<ResolvedAutoflowWorkflow>,
) -> anyhow::Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in rd {
        let entry = entry?;
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let body = std::fs::read_to_string(&path)?;
        let workflow = Workflow::parse(&body)?;
        if !workflow
            .autoflow
            .as_ref()
            .map(|autoflow| autoflow.enabled)
            .unwrap_or(false)
        {
            continue;
        }
        let resolved = resolve_autoflow_from_path(
            global,
            path,
            scope.to_string(),
            project_root.clone(),
            repo_ref.clone(),
            preferred_checkout.clone(),
            cfg.clone(),
        )?;
        into.push(resolved);
    }
    Ok(())
}

pub fn resolve_autoflow_from_path(
    _global: &Path,
    workflow_path: PathBuf,
    scope: String,
    project_root: Option<PathBuf>,
    repo_ref: String,
    preferred_checkout: PathBuf,
    cfg: Config,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let body = std::fs::read_to_string(&workflow_path)?;
    let workflow = Workflow::parse(&body)?;
    let enabled = workflow
        .autoflow
        .as_ref()
        .map(|autoflow| autoflow.enabled)
        .unwrap_or(false);
    if !enabled {
        bail!(
            "workflow `{}` does not declare `autoflow.enabled = true`",
            workflow.name
        );
    }
    Ok(ResolvedAutoflowWorkflow {
        scope,
        name: workflow.name.clone(),
        workflow,
        workflow_path,
        project_root,
        repo_ref,
        preferred_checkout,
        cfg,
    })
}

pub fn resolve_visible_autoflow_workflow(
    global: &Path,
    entry: &VisibleAutoflowWorkflow,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let repo_ref = entry
        .repo_ref
        .clone()
        .ok_or_else(|| anyhow!("autoflow `{}` is missing a repo binding", entry.name))?;
    let preferred_checkout = entry
        .preferred_checkout
        .clone()
        .or_else(|| entry.project_root.clone())
        .ok_or_else(|| anyhow!("autoflow `{}` is missing a preferred checkout", entry.name))?;
    let cfg = resolve_config_in(global, entry.project_root.as_deref(), &preferred_checkout)?;
    resolve_autoflow_from_path(
        global,
        entry.workflow_path.clone(),
        entry.scope.clone(),
        entry.project_root.clone(),
        repo_ref,
        preferred_checkout,
        cfg,
    )
}

/// The one autoflow named `name` that `pwd` sees for `issue_ref`, resolved:
/// none is a [`Refusal::NotFound`], several (pass `--repo`) a
/// [`Refusal::Conflict`].
pub fn resolve_autoflow_workflow_for_issue(
    global: &Path,
    pwd: &Path,
    name: &str,
    issue_ref: &IssueRef,
    repo: Option<&str>,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let matches = visible_autoflow_matches_for_issue(global, pwd, issue_ref, repo)?
        .into_iter()
        .filter(|entry| entry.name == name)
        .collect::<Vec<_>>();
    let entry = match matches.as_slice() {
        [] => {
            let issue_ref_text = format_issue_ref(issue_ref);
            return Err(Refusal::NotFound(match repo {
                Some(repo) => format!(
                    "no autoflow named `{name}` is visible for issue `{issue_ref_text}` and repo `{repo}`"
                ),
                None => format!("no autoflow named `{name}` is visible for issue `{issue_ref_text}`"),
            })
            .into());
        }
        [entry] => entry.clone(),
        many => {
            let mut options = many
                .iter()
                .map(|entry| {
                    format!(
                        "- {} ({}, {})",
                        entry.repo_ref.as_deref().unwrap_or("-"),
                        entry.scope,
                        entry.workflow_path.display()
                    )
                })
                .collect::<Vec<_>>();
            options.sort();
            return Err(Refusal::Conflict(format!(
                "multiple autoflows named `{name}` are visible for issue `{}`:\n{}\npass `--repo <platform>:<owner>/<repo>` to disambiguate",
                format_issue_ref(issue_ref),
                options.join("\n")
            ))
            .into());
        }
    };
    resolve_visible_autoflow_workflow(global, &entry)
}

/// The autoflows named `name` that `pwd` sees, limited to `repo`.
pub fn visible_autoflow_matches(
    global: &Path,
    pwd: &Path,
    name: &str,
    repo: Option<&str>,
) -> anyhow::Result<Vec<VisibleAutoflowWorkflow>> {
    let repo_filter = normalize_repo_filter(repo)?;
    let mut entries = visible_autoflows(global, pwd)?
        .into_iter()
        .filter(|entry| entry.name == name)
        .collect::<Vec<_>>();
    entries = filter_visible_autoflows(entries, repo_filter.as_deref());
    entries.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.workflow_path.cmp(&right.workflow_path))
    });
    Ok(entries)
}

/// The autoflows `pwd` sees whose source covers `issue_ref`, limited to
/// `repo`.
pub fn visible_autoflow_matches_for_issue(
    global: &Path,
    pwd: &Path,
    issue_ref: &IssueRef,
    repo: Option<&str>,
) -> anyhow::Result<Vec<VisibleAutoflowWorkflow>> {
    let repo_filter = normalize_repo_filter(repo)?;
    let mut entries = visible_autoflows(global, pwd)?
        .into_iter()
        .filter(|entry| issue_matches_visible_source(issue_ref, entry))
        .collect::<Vec<_>>();
    entries = filter_visible_autoflows(entries, repo_filter.as_deref());
    entries.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.workflow_path.cmp(&right.workflow_path))
    });
    Ok(entries)
}

pub fn issue_matches_visible_source(issue_ref: &IssueRef, entry: &VisibleAutoflowWorkflow) -> bool {
    let Ok(autoflow) = entry.autoflow() else {
        return false;
    };
    if let Some(source) = autoflow.source.as_deref() {
        return source_matches_issue_ref(source, issue_ref);
    }
    entry
        .repo_ref
        .as_deref()
        .is_some_and(|repo_ref| repo_ref_matches_issue_ref(repo_ref, issue_ref))
}

pub fn source_matches_issue_ref(source: &str, issue_ref: &IssueRef) -> bool {
    let Ok(source_ref) = source.parse::<EventSourceRef>() else {
        return false;
    };
    match source_ref {
        EventSourceRef::Repo { repo } => match issue_ref.tracker {
            IssueTracker::Github => {
                repo.platform == Platform::Github
                    && issue_ref.project == format!("{}/{}", repo.owner, repo.repo)
            }
            IssueTracker::Gitlab => {
                repo.platform == Platform::Gitlab
                    && issue_ref.project == format!("{}/{}", repo.owner, repo.repo)
            }
            IssueTracker::Linear | IssueTracker::Jira => false,
        },
        EventSourceRef::TrackerProject {
            tracker, project, ..
        } => {
            tracker == issue_ref.tracker
                && match tracker {
                    IssueTracker::Jira => {
                        project == issue_ref.project
                            || (!project.contains('/')
                                && issue_ref
                                    .project
                                    .rsplit('/')
                                    .next()
                                    .is_some_and(|suffix| suffix == project))
                    }
                    IssueTracker::Github | IssueTracker::Gitlab | IssueTracker::Linear => {
                        project == issue_ref.project
                    }
                }
        }
    }
}

pub fn repo_ref_matches_issue_ref(repo_ref: &str, issue_ref: &IssueRef) -> bool {
    issue_ref_to_repo_ref(&format_issue_ref(issue_ref))
        .map(|expected| expected == repo_ref)
        .unwrap_or(false)
}

/// The repo `issue_ref`'s claim binds to: `repo` when given, the issue's
/// own repo for GitHub / GitLab, else the one repo the autoflows `pwd` sees
/// for it are bound to.
pub fn resolve_repo_binding_for_issue(
    global: &Path,
    pwd: &Path,
    issue_ref: &IssueRef,
    repo: Option<&str>,
) -> anyhow::Result<String> {
    let issue_ref_text = format_issue_ref(issue_ref);
    if let Some(repo) = normalize_repo_filter(repo)? {
        return Ok(repo);
    }
    if let Ok(repo_ref) = issue_ref_to_repo_ref(&issue_ref_text) {
        return Ok(repo_ref);
    }
    let matches = visible_autoflow_matches_for_issue(global, pwd, issue_ref, None)?;
    let repos = matches
        .iter()
        .filter_map(|entry| entry.repo_ref.clone())
        .collect::<BTreeSet<_>>();
    match repos.len() {
        0 => bail!("no visible autoflow is bound to tracker issue `{issue_ref_text}`"),
        1 => Ok(repos.into_iter().next().expect("one repo binding")),
        _ => bail!(
            "multiple repo bindings are visible for tracker issue `{issue_ref_text}`: {}\npass `--repo <platform>:<owner>/<repo>` to disambiguate",
            repos.into_iter().collect::<Vec<_>>().join(", ")
        ),
    }
}

pub fn issue_ref_for_autoflow(
    issue_ref: &IssueRef,
    resolved: &ResolvedAutoflowWorkflow,
) -> anyhow::Result<IssueRef> {
    let autoflow = resolved.autoflow()?;
    let Some(source) = autoflow.source.as_deref() else {
        return Ok(issue_ref.clone());
    };
    if issue_ref.tracker != IssueTracker::Jira || !source_matches_issue_ref(source, issue_ref) {
        return Ok(issue_ref.clone());
    }
    let Ok(EventSourceRef::TrackerProject {
        tracker, project, ..
    }) = source.parse::<EventSourceRef>()
    else {
        return Ok(issue_ref.clone());
    };
    if tracker != IssueTracker::Jira {
        return Ok(issue_ref.clone());
    }
    let mut adjusted = issue_ref.clone();
    adjusted.project = project;
    Ok(adjusted)
}

pub fn filter_visible_autoflows(
    entries: Vec<VisibleAutoflowWorkflow>,
    repo_filter: Option<&str>,
) -> Vec<VisibleAutoflowWorkflow> {
    match repo_filter {
        Some(repo_ref) => entries
            .into_iter()
            .filter(|entry| entry.repo_ref.as_deref() == Some(repo_ref))
            .collect(),
        None => entries,
    }
}

pub fn resolve_autoflow_permission_mode(
    mode_override: Option<&str>,
    config_mode: Option<&str>,
) -> anyhow::Result<String> {
    let mode = mode_override
        .map(ToOwned::to_owned)
        .or_else(|| config_mode.map(ToOwned::to_owned))
        .unwrap_or_else(|| "bypass".to_string());
    match mode.as_str() {
        "bypass" | "readonly" => Ok(mode),
        "ask" => {
            bail!("autoflow does not support `ask` permission mode; use `bypass` or `readonly`")
        }
        _ => bail!("invalid autoflow permission mode `{mode}`; expected `bypass` or `readonly`"),
    }
}
