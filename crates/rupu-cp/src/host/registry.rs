//! `HostRegistry` — resolves `host_id` → `Arc<dyn HostConnector>`, with
//! add/remove and a connector cache (hot-reload, no `cp serve` restart needed).

#![deny(clippy::all)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rupu_orchestrator::runs::RunStore;
use rupu_workspace::{
    delete_host_token, get_host_token, set_host_token, Host, HostStore, HostStoreError,
    HostTransport,
};
use ulid::Ulid;

use crate::{
    host::{
        connector::{HostConnector, HostConnectorError},
        http::HttpHostConnector,
        ssh::{SshExec, SshHostConnector},
        tunnel::TunnelHostConnector,
    },
    node::{NodeMirror, NodeRegistry},
};

// ── Error mapping ─────────────────────────────────────────────────────────────

impl From<HostStoreError> for HostConnectorError {
    fn from(e: HostStoreError) -> Self {
        HostConnectorError::Invalid(e.to_string())
    }
}

// ── HostRegistry ──────────────────────────────────────────────────────────────

/// Resolves a `host_id` string to a live `HostConnector`.
///
/// - `"local"` always maps to the in-process `LocalHostConnector`.
/// - `Tunnel` hosts resolve to a [`TunnelHostConnector`] when tunnel deps have
///   been wired in via [`HostRegistry::with_tunnel_deps`]; otherwise they
///   return an [`HostConnectorError::Invalid`] placeholder until Task 7 wires
///   the deps.
/// - All other IDs are looked up in the [`HostStore`], and an
///   [`HttpHostConnector`] is built from the stored transport + keychain token.
/// - Resolved connectors are cached in a `Mutex<HashMap>` so repeated calls
///   reuse the same `reqwest::Client`. Cache entries are invalidated on
///   `add_host` / `remove_host`.
pub struct HostRegistry {
    store: HostStore,
    local: Arc<dyn HostConnector>,
    cache: Mutex<HashMap<String, Arc<dyn HostConnector>>>,
    /// Deps required to build a [`TunnelHostConnector`].  Set via
    /// [`Self::with_tunnel_deps`]; `None` until wired by the caller (Task 7).
    node_registry: Option<Arc<NodeRegistry>>,
    node_mirror: Option<Arc<NodeMirror>>,
    run_store: Option<Arc<RunStore>>,
    pricing: rupu_config::PricingConfig,
}

impl HostRegistry {
    /// Create a new registry.
    ///
    /// `local` is the host[0] connector (always a `LocalHostConnector` in
    /// production; may be any `HostConnector` in tests).  Tunnel deps are not
    /// present by default; call [`Self::with_tunnel_deps`] to enable tunnel
    /// host resolution.
    pub fn new(store: HostStore, local: Arc<dyn HostConnector>) -> Self {
        Self {
            store,
            local,
            cache: Mutex::new(HashMap::new()),
            node_registry: None,
            node_mirror: None,
            run_store: None,
            pricing: rupu_config::PricingConfig::default(),
        }
    }

    /// Wire the deps needed to build [`TunnelHostConnector`] instances.
    ///
    /// Call this once after construction to enable resolution of `Tunnel`
    /// transport hosts.  Can be chained: `HostRegistry::new(…).with_tunnel_deps(…)`.
    pub fn with_tunnel_deps(
        mut self,
        node_registry: Arc<NodeRegistry>,
        node_mirror: Arc<NodeMirror>,
        run_store: Arc<RunStore>,
        pricing: rupu_config::PricingConfig,
    ) -> Self {
        self.node_registry = Some(node_registry);
        self.node_mirror = Some(node_mirror);
        self.run_store = Some(run_store);
        self.pricing = pricing;
        self
    }

    /// Resolve `host_id` to a connector.
    ///
    /// - `"local"` → the local connector (never goes to the store or cache).
    /// - Otherwise: check the in-memory cache, then load from the store. Unknown
    ///   ids return [`HostConnectorError::NotFound`].
    pub fn resolve(&self, host_id: &str) -> Result<Arc<dyn HostConnector>, HostConnectorError> {
        if host_id == "local" {
            return Ok(Arc::clone(&self.local));
        }

        // Fast path: already cached.
        {
            let guard = self.cache.lock().unwrap();
            if let Some(conn) = guard.get(host_id) {
                return Ok(Arc::clone(conn));
            }
        }

        // Load from store — read each call so hot-reloaded entries are picked up
        // immediately without a restart.
        let host = self
            .store
            .load(host_id)?
            .ok_or_else(|| HostConnectorError::NotFound(host_id.to_string()))?;

        let conn = self.build_connector(&host)?;

        {
            let mut guard = self.cache.lock().unwrap();
            guard.insert(host_id.to_string(), Arc::clone(&conn));
        }

        Ok(conn)
    }

    /// List all known hosts: local (host[0]) first, then every persisted host
    /// from the store in sorted order.
    pub fn list_hosts(&self) -> Vec<Host> {
        let mut out = vec![Host::local()];
        match self.store.list() {
            Ok(hosts) => out.extend(hosts),
            Err(e) => {
                tracing::warn!(error = %e, "host_registry: failed to list persisted hosts");
            }
        }
        out
    }

    /// Persist a new remote host record and (optionally) store its token in the
    /// system keychain.
    ///
    /// Returns the newly created [`Host`] (with its generated id).
    /// Keychain write failure is logged as a warning rather than returned as an
    /// error so that this method succeeds on platforms / environments (CI) where
    /// the system keychain is unavailable.
    pub fn add_host(
        &self,
        name: &str,
        base_url: &str,
        token: Option<&str>,
    ) -> Result<Host, HostConnectorError> {
        let id = format!("host_{}", Ulid::new());
        let host = Host {
            id: id.clone(),
            name: name.to_string(),
            transport: HostTransport::HttpCp {
                base_url: base_url.to_string(),
            },
            created_at: chrono::Utc::now().to_rfc3339(),
            last_seen_at: None,
            token_hash: None,
        };

        // Spec §Errors+security: warn when the transport is unencrypted.
        if base_url.starts_with("http://") {
            tracing::warn!(
                %base_url,
                "host_registry: adding host with unencrypted http:// URL; \
                 consider using https:// to protect tokens in transit"
            );
        }

        self.store.save(&host)?;

        if let Some(tok) = token {
            set_host_token(&id, tok)
                .map_err(|e| HostConnectorError::Invalid(format!("could not store token for host {id}: {e}")))?;
        }

        // Invalidate any stale cache entry for this id (shouldn't exist on add,
        // but safe to remove anyway).
        self.cache.lock().unwrap().remove(&id);

        Ok(host)
    }

    /// Enroll a new tunnel node.
    ///
    /// Delegates to [`rupu_workspace::enroll_node`]: generates a `node_id`, a
    /// cryptographically random one-time token, and persists a `Tunnel` [`Host`]
    /// whose `token_hash` is the SHA-256 of the token.  Returns
    /// `(host, plaintext_token)`.
    ///
    /// The plaintext token is returned **once** and never stored or logged.
    /// Callers must surface it to the operator over a secure channel and discard
    /// it immediately.
    pub fn enroll_node(&self, name: &str) -> Result<(Host, String), HostConnectorError> {
        let (host, token) = rupu_workspace::enroll_node(&self.store, name)?;
        // Invalidate any stale cache entry (no entry should exist for a new id,
        // but safe to remove anyway).
        self.cache.lock().unwrap().remove(&host.id);
        Ok((host, token))
    }

    /// Remove a persisted host, its keychain token, and its cache entry.
    ///
    /// Refuses `"local"` with [`HostConnectorError::Invalid`].
    pub fn remove_host(&self, host_id: &str) -> Result<(), HostConnectorError> {
        if host_id == "local" {
            return Err(HostConnectorError::Invalid(
                "cannot remove the built-in local host".to_string(),
            ));
        }

        self.store.delete(host_id)?;

        // Best-effort: warn but don't propagate keychain failures.
        if let Err(e) = delete_host_token(host_id) {
            tracing::warn!(host_id, error = %e, "host_registry: could not delete token from keychain");
        }

        self.cache.lock().unwrap().remove(host_id);

        Ok(())
    }

    // ── Private ───────────────────────────────────────────────────────────────

    /// Build a connector from a persisted `Host` record.
    ///
    /// The keychain read is best-effort: if it fails (e.g. on CI), the
    /// connector is built without a token rather than returning an error.
    fn build_connector(&self, host: &Host) -> Result<Arc<dyn HostConnector>, HostConnectorError> {
        match &host.transport {
            HostTransport::HttpCp { base_url } => {
                let token = match get_host_token(&host.id) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(
                            host_id = %host.id,
                            error = %e,
                            "host_registry: keychain unavailable; connecting without token"
                        );
                        None
                    }
                };
                Ok(Arc::new(HttpHostConnector::new(base_url.clone(), token)))
            }
            HostTransport::Local => Ok(Arc::clone(&self.local)),
            HostTransport::Tunnel { node_id } => {
                match (&self.node_registry, &self.node_mirror, &self.run_store) {
                    (Some(reg), Some(mir), Some(store)) => {
                        Ok(Arc::new(TunnelHostConnector::new(
                            node_id.clone(),
                            Arc::clone(reg),
                            Arc::clone(mir),
                            Arc::clone(store),
                            self.pricing.clone(),
                        )))
                    }
                    _ => Err(HostConnectorError::Invalid(
                        "tunnel deps not wired (call HostRegistry::with_tunnel_deps)".to_string(),
                    )),
                }
            }
            HostTransport::Ssh {
                host: ssh_host,
                port,
                identity_file,
            } => match (&self.node_mirror, &self.run_store) {
                (Some(mir), Some(store)) => {
                    let exec = Arc::new(SshExec {
                        host: ssh_host.clone(),
                        port: *port,
                        identity_file: identity_file.clone(),
                    });
                    Ok(Arc::new(SshHostConnector::new(
                        host.id.clone(),
                        exec,
                        Arc::clone(mir),
                        Arc::clone(store),
                        self.pricing.clone(),
                    )))
                }
                _ => Err(HostConnectorError::Invalid(
                    "ssh deps not wired (call HostRegistry::with_tunnel_deps; \
                     mirror + run_store are shared with tunnel hosts)"
                        .into(),
                )),
            },
        }
    }
}
