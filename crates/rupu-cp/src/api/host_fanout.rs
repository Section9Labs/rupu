//! Shared fan-out helpers used by list endpoints that spread queries across
//! all registered hosts.
//!
//! Extracted from `run_streams` (Task 3) so that `sessions` (Task 4) and any
//! future list handlers can reuse the same tolerant-fan-out pattern without
//! duplication.

#![deny(clippy::all)]

use futures_util::future::join_all;
use std::sync::Arc;

/// Generic tolerant fan-out over structured connector methods: run `f(conn)`
/// on every remote host, tag each returned row with its `host_id`, and merge
/// with `local_values`. Per-host failures warn (tagged `what`) and contribute
/// nothing. Used by the run-list views (agents / autoflows / autoflow events)
/// so SSH hosts — which can't serve `proxy_get_json` — still contribute rows.
pub(crate) async fn fan_out_via<F, Fut>(
    hosts: &Arc<crate::host::registry::HostRegistry>,
    local_values: Vec<serde_json::Value>,
    what: &'static str,
    f: F,
) -> Vec<serde_json::Value>
where
    F: Fn(Arc<dyn crate::host::connector::HostConnector>) -> Fut + Clone + Send + Sync,
    Fut: std::future::Future<
            Output = Result<Vec<serde_json::Value>, crate::host::connector::HostConnectorError>,
        > + Send,
{
    fan_out_via_filtered(hosts, local_values, what, f, None)
        .await
        .0
}

/// [`fan_out_via`] under an optional customer filter: each remote host's
/// rows are filtered by their `customer` key
/// ([`crate::customers::filter_remote_rows`]); a host whose rows carry no
/// such key (a peer too old to report customers) contributes nothing and is
/// returned in the second element, for the caller's
/// [`crate::customers::HOSTS_WITHOUT_CUSTOMER_HEADER`]. `local_values` are
/// taken as given — the caller filters them by attribution.
pub(crate) async fn fan_out_via_filtered<F, Fut>(
    hosts: &Arc<crate::host::registry::HostRegistry>,
    local_values: Vec<serde_json::Value>,
    what: &'static str,
    f: F,
    filter: Option<&crate::customers::CustomerFilter>,
) -> (Vec<serde_json::Value>, Vec<String>)
where
    F: Fn(Arc<dyn crate::host::connector::HostConnector>) -> Fut + Clone + Send + Sync,
    Fut: std::future::Future<
            Output = Result<Vec<serde_json::Value>, crate::host::connector::HostConnectorError>,
        > + Send,
{
    let remote_hosts: Vec<_> = hosts
        .list_hosts()
        .into_iter()
        .filter(|h| h.id != "local")
        .collect();
    if remote_hosts.is_empty() {
        return (local_values, Vec::new());
    }
    let futs: Vec<_> = remote_hosts
        .into_iter()
        .map(|h| {
            let registry = Arc::clone(hosts);
            let f = f.clone();
            async move {
                let conn = match registry.resolve(&h.id) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(host_id = %h.id, error = %e, "fan_out_via({what}): resolve failed; skipping");
                        return HostRows::default();
                    }
                };
                match f(conn).await {
                    Ok(rows) => {
                        let host_id = h.id;
                        let rows = rows
                            .into_iter()
                            .map(|mut r| {
                                r["host_id"] = serde_json::json!(&host_id);
                                r
                            })
                            .collect();
                        HostRows::filtered(host_id, rows, filter)
                    }
                    Err(e) => {
                        tracing::warn!(host_id = %h.id, error = %e, "fan_out_via({what}): fetch failed; skipping");
                        HostRows::default()
                    }
                }
            }
        })
        .collect();
    HostRows::merge(local_values, join_all(futs).await)
}

/// One remote host's contribution to a fan-out list.
#[derive(Default)]
struct HostRows {
    rows: Vec<serde_json::Value>,
    /// Set when a customer filter was asked of a host that cannot report
    /// customers.
    without_customer: Option<String>,
}

impl HostRows {
    fn filtered(
        host_id: String,
        rows: Vec<serde_json::Value>,
        filter: Option<&crate::customers::CustomerFilter>,
    ) -> Self {
        let Some(f) = filter else {
            return Self {
                rows,
                without_customer: None,
            };
        };
        match crate::customers::filter_remote_rows(rows, f) {
            Some(rows) => Self {
                rows,
                without_customer: None,
            },
            None => Self {
                rows: Vec::new(),
                without_customer: Some(host_id),
            },
        }
    }

    fn merge(
        local_values: Vec<serde_json::Value>,
        hosts: Vec<HostRows>,
    ) -> (Vec<serde_json::Value>, Vec<String>) {
        let mut all = local_values;
        let mut skipped = Vec::new();
        for h in hosts {
            all.extend(h.rows);
            skipped.extend(h.without_customer);
        }
        (all, skipped)
    }
}

/// Like [`fan_out_via`] but specialized for sessions: calls the structured
/// `list_sessions(scope)` on each remote host instead of `proxy_get_json`, so
/// SSH hosts (which can't serve a generic GET) contribute their sessions
/// instead of being silently skipped. Per-host failures warn and contribute
/// nothing; the caller always gets a 200.
pub(crate) async fn fan_out_sessions(
    hosts: &Arc<crate::host::registry::HostRegistry>,
    scope: Option<&str>,
    local_values: Vec<serde_json::Value>,
    filter: Option<&crate::customers::CustomerFilter>,
) -> (Vec<serde_json::Value>, Vec<String>) {
    let remote_hosts: Vec<_> = hosts
        .list_hosts()
        .into_iter()
        .filter(|h| h.id != "local")
        .collect();
    if remote_hosts.is_empty() {
        return (local_values, Vec::new());
    }

    let scope_owned = scope.map(|s| s.to_string());
    let futs: Vec<_> = remote_hosts
        .into_iter()
        .map(|h| {
            let registry = Arc::clone(hosts);
            let scope_owned = scope_owned.clone();
            async move {
                let conn = match registry.resolve(&h.id) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(host_id = %h.id, error = %e, "fan_out_sessions: could not resolve connector; skipping");
                        return HostRows::default();
                    }
                };
                match conn.list_sessions(scope_owned.as_deref()).await {
                    Ok(rows) => {
                        let host_id = h.id;
                        let rows = rows
                            .into_iter()
                            .map(|mut row| {
                                row["host_id"] = serde_json::json!(&host_id);
                                crate::codename::inject_codename_row(
                                    &mut row,
                                    "session_id",
                                    Some("agent_name"),
                                );
                                row
                            })
                            .collect();
                        HostRows::filtered(host_id, rows, filter)
                    }
                    Err(e) => {
                        tracing::warn!(host_id = %h.id, error = %e, "fan_out_sessions: list_sessions failed; skipping");
                        HostRows::default()
                    }
                }
            }
        })
        .collect();

    HostRows::merge(local_values, join_all(futs).await)
}

/// Sort a `Vec<Value>` newest-first using the string field named `time_field`.
/// Missing / null values sort after present values.
pub(crate) fn sort_values_newest_first(values: &mut [serde_json::Value], time_field: &str) {
    values.sort_by(|a, b| {
        let ta = a[time_field].as_str().unwrap_or("");
        let tb = b[time_field].as_str().unwrap_or("");
        tb.cmp(ta)
    });
}
