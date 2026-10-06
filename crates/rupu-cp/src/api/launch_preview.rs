//! `POST /api/launch/preview` — what a launch would run as, before it runs:
//! the customer the launch directory belongs to and the provider, fallback
//! and SCM accounts the run would authenticate as, each with where the
//! choice came from.
//!
//! The preview resolves exactly like a launch: the launch directory
//! (`working_dir`, else the scope selector's directory, else the server's
//! cwd — as the launchers do), the strict shared layer paths
//! ([`rupu_workspace::config_paths`]) and `rupu_config::resolve`. A project
//! assigned to a customer that no longer exists is therefore a 409 here, as
//! it is a launch failure there. Spec:
//! `docs/superpowers/plans/2026-10-06-rupu-customers-plan-2a-cp-backend.md`
//! (Task 8).

use std::path::{Path as FsPath, PathBuf};

use axum::{routing::post, Json, Router};
use rupu_agent::AgentSpec;
use rupu_runtime::credential_manifest::{
    credential_manifest, AccountRole, AgentFacts, ManifestEntry,
};
use rupu_scm::rules::{expand_tilde, glob_matches, resolve_account, Resolution, Rule};
use rupu_scm::{AccountId, Platform};
use serde::{Deserialize, Serialize};

use crate::api::runs::blocking;
use crate::customers::{customer_ref, CustomerRef};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/launch/preview", post(preview))
}

#[derive(Debug, Deserialize)]
pub struct PreviewBody {
    /// Exactly one of `workflow` / `agent`.
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Same semantics as the launch bodies' scope selector.
    #[serde(default)]
    pub scope_kind: Option<String>,
    #[serde(default)]
    pub scope_id: Option<String>,
    /// Echoed. The preview resolves against THIS machine's configuration;
    /// a non-local host gets a warning saying so (the per-host shipping plan
    /// is Plan 3).
    #[serde(default)]
    pub host: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PreviewResponse {
    pub customer: Option<CustomerRef>,
    pub accounts: Vec<ManifestEntry>,
    /// Resolver warnings, unknown agents, an unresolvable SCM account.
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

async fn preview(
    axum::extract::State(s): axum::extract::State<AppState>,
    Json(body): Json<PreviewBody>,
) -> ApiResult<Json<PreviewResponse>> {
    let (kind, name) = match (non_empty(&body.workflow), non_empty(&body.agent)) {
        (Some(w), None) => (Target::Workflow, w.to_string()),
        (None, Some(a)) => (Target::Agent, a.to_string()),
        _ => {
            return Err(ApiError::bad_request(
                "name exactly one of `workflow` or `agent`",
            ))
        }
    };
    // The name is joined onto definition paths below: one that could leave
    // the definitions directory is refused before anything reads a file.
    if !rupu_workspace::is_safe_definition_name(&name) {
        return Err(ApiError::bad_request(format!(
            "invalid {} name `{name}`: no `/`, `\\` or `..`",
            match kind {
                Target::Workflow => "workflow",
                Target::Agent => "agent",
            }
        )));
    }
    if body.scope_kind.is_some() && body.working_dir.is_some() {
        return Err(ApiError::bad_request(
            "scope_kind and working_dir are mutually exclusive — the scope selector determines the working directory",
        ));
    }
    let host = body.host.clone().filter(|h| !h.is_empty() && h != "local");
    let response = blocking(move || {
        // Same launch directory a launch uses.
        let scope_dir = match kind {
            Target::Workflow => crate::api::workflows::resolve_launch_scope(
                &s,
                &name,
                body.scope_kind.as_deref(),
                body.scope_id.as_deref(),
            )?,
            Target::Agent => crate::api::agents::resolve_launch_scope(
                &s,
                &name,
                body.scope_kind.as_deref(),
                body.scope_id.as_deref(),
            )?,
        };
        let dir = match (body.working_dir.as_deref(), scope_dir) {
            (Some(w), _) => PathBuf::from(w),
            (None, Some(d)) => d,
            (None, None) => std::env::current_dir()
                .map_err(|e| ApiError::internal(format!("cannot read the server's cwd: {e}")))?,
        };
        build(&s.global_dir, &dir, kind, &name)
    })
    .await?;
    let mut response = response;
    if let Some(h) = host {
        response.warnings.push(format!(
            "this preview resolves against the machine running the control plane; \
             host `{h}` may resolve differently"
        ));
        response.host = Some(h);
    }
    Ok(Json(response))
}

#[derive(Debug, Clone, Copy)]
enum Target {
    Workflow,
    Agent,
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// The preview for a launch of `name` from `dir`. Blocking.
fn build(global: &FsPath, dir: &FsPath, target: Target, name: &str) -> ApiResult<PreviewResponse> {
    let project_root = rupu_workspace::project_root_for(dir)
        .map_err(|e| ApiError::bad_request(format!("cannot use {}: {e}", dir.display())))?;
    // Strict, as a launch is: an unresolvable assignment is the launch's own
    // failure, so it is the preview's.
    let paths = rupu_workspace::config_paths(global, project_root.as_deref(), dir)
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    let resolved = rupu_config::resolve(paths.layers())
        .map_err(|e| ApiError::conflict(format!("config does not load: {e}")))?;
    let cfg = &resolved.config;
    let mut warnings = resolved.warnings.clone();

    let customer = paths.customer_slug.as_deref().map(|slug| {
        match rupu_workspace::CustomerStore::new(global).get(slug) {
            Ok(c) => customer_ref(&c),
            // `config_paths` just confirmed the customer exists; a record
            // that still cannot be read is named by its slug, never hidden.
            Err(e) => {
                warnings.push(format!("customer `{slug}` record is unreadable: {e}"));
                crate::customers::CustomerRef {
                    slug: slug.to_string(),
                    name: slug.to_string(),
                    tint: crate::customers::tint_for(slug, None),
                    archived: false,
                }
            }
        }
    });

    let agent_names: Vec<String> = match target {
        Target::Agent => vec![name.to_string()],
        Target::Workflow => {
            let path = rupu_workspace::locate_workflow(global, project_root.as_deref(), name)
                .ok_or_else(|| ApiError::not_found(format!("workflow {name} not found")))?;
            let wf = rupu_orchestrator::Workflow::parse_file(&path)
                .map_err(|e| ApiError::conflict(format!("workflow {name} does not parse: {e}")))?;
            wf.dispatched_agents().into_iter().collect()
        }
    };
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
    let mut specs: Vec<AgentSpec> = Vec::new();
    for agent in &agent_names {
        match rupu_agent::loader::load_agent(global, project_agents_parent.as_deref(), agent) {
            Ok(spec) => specs.push(spec),
            Err(rupu_agent::loader::AgentLoadError::NotFound(_)) => {
                if matches!(target, Target::Agent) {
                    return Err(ApiError::not_found(format!("agent {agent} not found")));
                }
                warnings.push(format!(
                    "agent `{agent}` was not found; its accounts are not listed"
                ));
            }
            // Any other loader failure (a malformed agent file — the loader
            // reads the whole agents directory) fails the launch too, so it
            // fails the preview: 409 with the loader's message.
            Err(e) => {
                return Err(ApiError::conflict(format!(
                    "agent `{agent}` could not be loaded, so the launch would fail: {e}"
                )));
            }
        }
    }
    let facts: Vec<AgentFacts<'_>> = specs
        .iter()
        .map(|a| AgentFacts {
            name: &a.name,
            provider: a.provider.as_deref(),
            fallbacks: a.fallbacks.as_deref(),
            auth: a.auth,
        })
        .collect();

    let scm = scm_entry(global, cfg, dir, &mut warnings);
    let accounts = credential_manifest(cfg, &resolved.provenance, &facts, scm);
    Ok(PreviewResponse {
        customer,
        accounts,
        warnings,
        host: None,
    })
}

/// The SCM account the launch directory's `origin` repo resolves to, as the
/// registry would pick it. `None` (with a warning when the config is
/// ambiguous, broken, or the remote is not one rupu can resolve) when there
/// is nothing to report.
///
/// Candidates are the config-declared accounts of the repo's platform THAT
/// HAVE A CREDENTIAL — `Registry::discover` registers an account only when
/// its connector builds, so an uncredentialed `[scm.<name>]` table is not a
/// candidate at run time and must not make the choice look ambiguous here.
/// When the credential store cannot be read the account is kept (presence is
/// unknown) and the result says it was not checked.
fn scm_entry(
    global: &FsPath,
    cfg: &rupu_config::Config,
    dir: &FsPath,
    warnings: &mut Vec<String>,
) -> Option<ManifestEntry> {
    let remote = rupu_workspace::store::detect_repo_remote(dir)?;
    let Some(web) = rupu_scm::weburl::parse_repo_remote(&remote) else {
        warnings.push(format!(
            "origin `{remote}` is not a github.com/gitlab.com remote — \
             its SCM account can't be previewed"
        ));
        return None;
    };
    let repo = rupu_scm::RepoRef {
        platform: web.platform,
        owner: web.owner.clone(),
        repo: web.repo.clone(),
    };

    // The accounts the config declares for this platform: the bare vendor
    // names plus every `[scm.<name>]` whose declared `kind` (else its name)
    // is the platform — `rupu scm accounts`' own rule.
    let mut names: Vec<String> = vec!["github".to_string(), "gitlab".to_string()];
    for name in cfg.scm.platforms.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    let declared = names.into_iter().filter(|name| {
        let kind = cfg
            .scm
            .platforms
            .get(name)
            .and_then(|p| p.kind.as_deref())
            .and_then(|k| k.parse::<Platform>().ok())
            .or_else(|| name.parse::<Platform>().ok());
        kind == Some(repo.platform)
    });

    let resolver = rupu_auth::KeychainResolver::for_home(global);
    let mut unchecked = false;
    let mut candidates: Vec<AccountId> = Vec::new();
    for name in declared {
        match resolver.has_credential_named(&name) {
            Ok(true) => candidates.push(AccountId::new(name)),
            Ok(false) => {}
            Err(e) => {
                if !unchecked {
                    warnings.push(format!(
                        "the credential store could not be read ({e}); SCM accounts are \
                         listed as if their credentials exist"
                    ));
                }
                unchecked = true;
                candidates.push(AccountId::new(name));
            }
        }
    }
    let if_creds = if unchecked {
        " if its credentials exist"
    } else {
        ""
    };

    let rules: Vec<Rule> = cfg.scm.rules.iter().map(Rule::from_config).collect();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let source_of = |account: &AccountId, by_owner: bool| -> String {
        let hit = rules.iter().find(|r| {
            &r.account == account
                && if by_owner {
                    r.owner
                        .as_deref()
                        .is_some_and(|p| glob_matches(p, &repo.owner))
                } else {
                    r.path.as_deref().is_some_and(|p| {
                        glob_matches(&expand_tilde(p, home.as_deref()), &dir.to_string_lossy())
                    })
                }
        });
        match hit {
            Some(r) if by_owner => format!("rule owner = {}", r.owner.as_deref().unwrap_or("")),
            Some(r) => format!("rule path = {}", r.path.as_deref().unwrap_or("")),
            None => "rule".to_string(),
        }
    };
    let entry = |account: &AccountId, source: String| ManifestEntry {
        role: AccountRole::Scm,
        account: account.0.clone(),
        kind: Some(repo.platform.as_str().to_string()),
        auth_mode: None,
        agents: Vec::new(),
        source: if unchecked {
            format!("{source} · credentials not checked")
        } else {
            source
        },
    };
    match resolve_account(
        &rules,
        Some(&repo),
        Some(dir),
        home.as_deref(),
        None,
        &candidates,
        &candidates,
    ) {
        Resolution::Owner(a) => Some(entry(&a, source_of(&a, true))),
        Resolution::Path(a) => Some(entry(&a, source_of(&a, false))),
        Resolution::SoleAccount(a) => Some(entry(&a, format!("only {} account", repo.platform))),
        Resolution::Explicit(_) => None,
        Resolution::RuleTargetUnavailable { account, pattern } => {
            warnings.push(format!(
                "SCM rule `{pattern}` selects account `{}`, which is not configured \
                 or has no stored credentials; SCM calls for {}/{} will fail",
                account.0, repo.owner, repo.repo
            ));
            None
        }
        Resolution::NoMatch { candidates } => {
            let names: Vec<&str> = candidates.iter().map(|c| c.0.as_str()).collect();
            warnings.push(format!(
                "{} {} accounts are configured ({}) and no [[scm.rules]] entry matches {}/{}; \
                 SCM calls will fail until a rule selects one{if_creds}",
                names.len(),
                repo.platform,
                names.join(", "),
                repo.owner,
                repo.repo
            ));
            None
        }
        Resolution::NoAccounts => {
            warnings.push(format!(
                "no {} account has stored credentials, so SCM calls for {}/{} would fail",
                repo.platform, repo.owner, repo.repo
            ));
            None
        }
    }
}
