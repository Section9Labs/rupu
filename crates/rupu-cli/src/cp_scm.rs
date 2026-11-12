//! Current SCM enrollment for a running CP. Each request pins an immutable
//! adapter; unchanged inputs reuse the exact Arc (and connector epochs).
use std::{path::PathBuf, sync::Arc};

use rupu_auth::{resolver::CredentialEnrollmentRevision, KeychainResolver};
use rupu_cp::repos::{RepoEntry, RepoListError, RepoLister};

pub struct ScmSnapshot {
    pub available: bool,
    pub config: rupu_config::Config,
    pub resolver: Arc<KeychainResolver>,
    pub registry: Arc<rupu_scm::Registry>,
    pub repos: Arc<dyn RepoLister>,
    pub generation: String,
    pub project_root: Option<PathBuf>,
    pub customer: Option<String>,
}

struct Inputs {
    config: rupu_config::Config,
    config_bytes: serde_json::Value,
    full_config: serde_json::Value,
    project_root: Option<PathBuf>,
    customer: Option<String>,
    credentials: Option<CredentialEnrollmentRevision>,
}
impl Inputs {
    fn same_revision(&self, other: &Self) -> bool {
        self.config_bytes == other.config_bytes && self.credentials == other.credentials
    }
}

pub struct CurrentScm {
    global: PathBuf,
    project: Option<PathBuf>,
    resolver: KeychainResolver,
    // Serializes discovery, not provider requests. No partially built registry
    // is published and canceled discovery never updates this cache.
    state: tokio::sync::Mutex<Option<(Inputs, Arc<ScmSnapshot>)>>,
    // These ports advertise availability only. Routes call snapshot_state
    // before invoking them, so no request uses this empty startup adapter.
    availability: crate::cp_repos::CpRepoLister,
}

fn unavailable() -> RepoListError {
    RepoListError::Backend("current SCM configuration or credentials unavailable".into())
}

impl CurrentScm {
    pub fn new(global: PathBuf, resolver: KeychainResolver) -> Self {
        Self {
            global,
            project: None,
            resolver,
            state: tokio::sync::Mutex::new(None),
            availability: crate::cp_repos::CpRepoLister {
                registry: Arc::new(rupu_scm::Registry::default()),
            },
        }
    }

    pub fn with_project(mut self, project: PathBuf) -> Self {
        self.project = Some(project);
        self
    }

    pub(crate) fn project_source(&self, path: PathBuf) -> Self {
        Self::new(self.global.clone(), self.resolver.clone()).with_project(path)
    }

    async fn inputs(&self, credentials_required: bool) -> Result<Inputs, RepoListError> {
        let global = self.global.clone();
        let resolver = self.resolver.clone();
        let project = self.project.clone();
        tokio::task::spawn_blocking(move || {
            let global_path = global.join("config.toml");
            let project_root = project
                .as_deref()
                .map(crate::paths::project_root_for)
                .transpose()
                .map_err(|_| unavailable())?
                .flatten();
            let project_paths = project
                .as_deref()
                .map(|path| crate::paths::config_paths(&global, project_root.as_deref(), path))
                .transpose()
                .map_err(|_| unavailable())?;
            let layers = project_paths
                .as_ref()
                .map(|paths| paths.layers())
                .unwrap_or_else(|| rupu_config::LayerPaths::global_only(&global_path));
            let config = rupu_config::resolve::resolve_bounded(layers, 2 * 1024 * 1024)
                .map_err(|_| unavailable())?
                .config;
            // Legacy client constructors assume valid URLs and otherwise
            // panic. A live config edit must instead fail this request safely.
            for platform in config.scm.platforms.values() {
                if let Some(base) = &platform.base_url {
                    let parsed = url::Url::parse(base).map_err(|_| unavailable())?;
                    if !base.is_ascii()
                        || !matches!(parsed.scheme(), "http" | "https")
                        || parsed.host_str().is_none()
                        || parsed.cannot_be_a_base()
                        || !parsed.username().is_empty()
                        || parsed.password().is_some()
                        || parsed.query().is_some()
                        || parsed.fragment().is_some()
                    {
                        return Err(unavailable());
                    }
                }
            }
            let full_config = serde_json::to_value(&config).map_err(|_| unavailable())?;
            let customer = project_paths
                .as_ref()
                .and_then(|paths| paths.customer_slug.clone());
            let specs = crate::accounts::account_specs(&config);
            let resolver = resolver.with_accounts(specs.clone());
            let mut names =
                std::collections::BTreeSet::from(["github".to_string(), "gitlab".to_string()]);
            for (name, platform) in &config.scm.platforms {
                if platform
                    .kind
                    .as_deref()
                    .and_then(|kind| kind.parse::<rupu_scm::Platform>().ok())
                    .or_else(|| name.parse::<rupu_scm::Platform>().ok())
                    .is_some()
                {
                    names.insert(name.clone());
                }
            }
            let mut requests: Vec<_> = names.into_iter().map(|name| (name, None)).collect();
            requests.extend(
                ["linear", "jira"]
                    .into_iter()
                    .map(|name| (name.into(), Some(rupu_providers::AuthMode::ApiKey))),
            );
            let kinds: Vec<_> = requests
                .iter()
                .map(|(name, _)| {
                    (
                        name,
                        rupu_auth::account::resolve_provider_id(name, &specs)
                            .map(|kind| kind.as_str()),
                    )
                })
                .collect();
            let config_bytes = serde_json::to_value(&(&config.scm, &config.issues, kinds))
                .map_err(|_| unavailable())?;
            let credentials = if credentials_required {
                Some(
                    resolver
                        .enrollment_revision(&requests)
                        .map_err(|_| unavailable())?,
                )
            } else {
                None
            };
            Ok(Inputs {
                config,
                config_bytes,
                full_config,
                project_root,
                customer,
                credentials,
            })
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub async fn current(&self) -> Result<Arc<ScmSnapshot>, RepoListError> {
        // Bounds lock waiting + discovery; no retry of any forge request is
        // hidden here. Discovery only resolves credentials/builds connectors.
        tokio::time::timeout(std::time::Duration::from_secs(30), self.load())
            .await
            .map_err(|_| unavailable())?
    }

    /// Catalog tools may still serve non-SCM work while enrollment is broken.
    /// Mark the SCM service unavailable, never reuse the previous authorized
    /// snapshot. The empty registry is only for static catalog advertisement; a
    /// live tool context must omit the service when `available` is false. Operator SCM routes use `current` and return the refresh error.
    pub async fn for_tools(&self) -> Result<Arc<ScmSnapshot>, RepoListError> {
        match self.current().await {
            Ok(snapshot) => Ok(snapshot),
            Err(_) => {
                let inputs = self.inputs(false).await?;
                let registry = Arc::new(rupu_scm::Registry::default());
                Ok(Arc::new(ScmSnapshot {
                    available: false,
                    resolver: Arc::new(
                        self.resolver
                            .clone()
                            .with_accounts(crate::accounts::account_specs(&inputs.config)),
                    ),
                    config: inputs.config,
                    repos: Arc::new(crate::cp_repos::CpRepoLister {
                        registry: Arc::clone(&registry),
                    }),
                    registry,
                    generation: ulid::Ulid::new().to_string(),
                    project_root: inputs.project_root,
                    customer: inputs.customer,
                }))
            }
        }
    }

    async fn load(&self) -> Result<Arc<ScmSnapshot>, RepoListError> {
        let mut state = self.state.lock().await;
        for _ in 0..3 {
            let before = self.inputs(true).await?;
            if let Some((previous, snapshot)) = state.as_ref() {
                if previous.same_revision(&before) {
                    if previous.full_config == before.full_config
                        && previous.customer == before.customer
                        && previous.project_root == before.project_root
                    {
                        return Ok(Arc::clone(snapshot));
                    }
                    // Non-SCM preferences must reach new MCP calls without
                    // changing SCM cursor/write authority.
                    let updated = Arc::new(ScmSnapshot {
                        available: true,
                        config: before.config.clone(),
                        resolver: Arc::new(
                            self.resolver
                                .clone()
                                .with_accounts(crate::accounts::account_specs(&before.config)),
                        ),
                        registry: Arc::clone(&snapshot.registry),
                        repos: Arc::clone(&snapshot.repos),
                        generation: snapshot.generation.clone(),
                        project_root: before.project_root.clone(),
                        customer: before.customer.clone(),
                    });
                    *state = Some((before, Arc::clone(&updated)));
                    return Ok(updated);
                }
            }
            let resolver = Arc::new(
                self.resolver
                    .clone()
                    .with_accounts(crate::accounts::account_specs(&before.config)),
            );
            let enrollment = before
                .credentials
                .as_ref()
                .expect("credential inputs requested");
            let discovered = discover_complete(resolver.as_ref(), &before.config, enrollment).await;
            // Login/logout, config edits, or OAuth rotation during discovery
            // must not label a mixed registry as the newer revision.
            let after = self.inputs(true).await?;
            if !before.same_revision(&after)
                || !before
                    .credentials
                    .as_ref()
                    .expect("credential inputs requested")
                    .same_publication(
                        after
                            .credentials
                            .as_ref()
                            .expect("credential inputs requested"),
                    )
            {
                continue;
            }
            let registry = Arc::new(discovered?);
            let snapshot = Arc::new(ScmSnapshot {
                available: true,
                config: before.config.clone(),
                resolver,
                repos: Arc::new(crate::cp_repos::CpRepoLister {
                    registry: Arc::clone(&registry),
                }),
                registry,
                generation: ulid::Ulid::new().to_string(),
                project_root: before.project_root.clone(),
                customer: before.customer.clone(),
            });
            *state = Some((before, Arc::clone(&snapshot)));
            return Ok(snapshot);
        }
        Err(unavailable())
    }
}

// Registry::discover tolerates errors for one-shot CLI commands. A refresh
// source must distinguish "not enrolled" from "enrolled but discovery failed",
// or it could cache a transient OAuth failure until the next file edit.
async fn discover_complete(
    resolver: &dyn rupu_auth::CredentialResolver,
    config: &rupu_config::Config,
    enrollment: &CredentialEnrollmentRevision,
) -> Result<rupu_scm::Registry, RepoListError> {
    struct Checked<'a> {
        inner: &'a dyn rupu_auth::CredentialResolver,
        enrollment: &'a CredentialEnrollmentRevision,
        failed: std::sync::atomic::AtomicBool,
    }
    #[async_trait::async_trait]
    impl rupu_auth::CredentialResolver for Checked<'_> {
        async fn get(
            &self,
            account: &str,
            hint: Option<rupu_providers::AuthMode>,
        ) -> anyhow::Result<(
            rupu_providers::AuthMode,
            rupu_providers::auth::AuthCredentials,
        )> {
            if !self
                .enrollment
                .enrolled_accounts()
                .any(|name| name == account)
            {
                anyhow::bail!("SCM account is not enrolled in this snapshot");
            }
            match self.inner.get(account, hint).await {
                Ok((mode, credentials))
                    if self
                        .enrollment
                        .matches_resolution(account, mode, &credentials) =>
                {
                    Ok((mode, credentials))
                }
                _ => {
                    self.failed
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    anyhow::bail!("SCM credential unavailable or changed");
                }
            }
        }
        async fn refresh(
            &self,
            account: &str,
            mode: rupu_providers::AuthMode,
        ) -> anyhow::Result<rupu_providers::auth::AuthCredentials> {
            self.inner.refresh(account, mode).await
        }
        fn oauth_refresher(
            &self,
            account: &str,
            kind: &str,
        ) -> Option<Arc<dyn rupu_providers::credential_writes::OAuthRefresher>> {
            self.inner.oauth_refresher(account, kind)
        }
    }
    let checked = Checked {
        inner: resolver,
        enrollment,
        failed: std::sync::atomic::AtomicBool::new(false),
    };
    let registry =
        rupu_scm::Registry::discover(&checked, config, Arc::new(rupu_netflow::NullSink)).await;
    let issues = registry.all_issue_connectors();
    if checked.failed.load(std::sync::atomic::Ordering::Relaxed)
        || enrollment.enrolled_accounts().any(|name| {
            let account = rupu_scm::AccountId::new(name);
            let Some((_, tracker, _)) = issues.iter().find(|(id, _, _)| id == &account) else {
                return true;
            };
            matches!(
                tracker,
                rupu_scm::IssueTracker::Github | rupu_scm::IssueTracker::Gitlab
            ) && registry.repo_by_account(&account).is_none()
        })
    {
        return Err(unavailable());
    }
    Ok(registry)
}

#[async_trait::async_trait]
impl RepoLister for CurrentScm {
    async fn snapshot(&self) -> Result<Option<Arc<dyn RepoLister>>, RepoListError> {
        Ok(Some(Arc::clone(&self.current().await?.repos)))
    }
    async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError> {
        self.current().await?.repos.list().await
    }
    fn supports_review_pages(&self) -> bool {
        true
    }
    fn issue_reader(&self) -> Option<&dyn rupu_cp::issue_reader::IssueReader> {
        Some(&self.availability)
    }
    fn forge_writer(&self) -> Option<&dyn rupu_cp::forge_writer::ForgeWriter> {
        Some(&self.availability)
    }
}

#[cfg(test)]
mod tests;
