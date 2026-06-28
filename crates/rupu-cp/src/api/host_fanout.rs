//! Shared fan-out helpers used by list endpoints that spread queries across
//! all registered hosts.
//!
//! Extracted from `run_streams` (Task 3) so that `sessions` (Task 4) and any
//! future list handlers can reuse the same tolerant-fan-out pattern without
//! duplication.

#![deny(clippy::all)]

use futures_util::future::join_all;
use std::sync::Arc;

/// Concurrently proxy `GET list_path` to every registered **remote** host,
/// tag each returned row JSON object with `"host_id": "<that host's id>"`,
/// and return the combined list (`local_values` + all remote rows).
///
/// `local_values` should already have `"host_id": "local"` set on each element.
///
/// Per-host failures emit a `tracing::warn` and contribute nothing — the
/// caller always gets a 200 even when some hosts are offline.
pub(crate) async fn fan_out_rows(
    hosts: &Arc<crate::host::registry::HostRegistry>,
    list_path: &str,
    local_values: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let all_hosts = hosts.list_hosts();
    let remote_hosts: Vec<_> = all_hosts.into_iter().filter(|h| h.id != "local").collect();

    if remote_hosts.is_empty() {
        return local_values;
    }

    let futs: Vec<_> = remote_hosts
        .into_iter()
        .map(|h| {
            let registry = Arc::clone(hosts);
            let path = list_path.to_string();
            async move {
                let conn = match registry.resolve(&h.id) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(
                            host_id = %h.id,
                            error = %e,
                            "fan_out_rows: could not resolve connector; skipping"
                        );
                        return Vec::<serde_json::Value>::new();
                    }
                };
                match conn.proxy_get_json(&path).await {
                    Ok(v) => {
                        let arr = match v.as_array() {
                            Some(a) => a.clone(),
                            None => {
                                tracing::warn!(
                                    host_id = %h.id,
                                    "fan_out_rows: remote returned non-array JSON; skipping"
                                );
                                return Vec::new();
                            }
                        };
                        let host_id = h.id;
                        arr.into_iter()
                            .map(|mut row| {
                                row["host_id"] = serde_json::json!(&host_id);
                                row
                            })
                            .collect()
                    }
                    Err(e) => {
                        tracing::warn!(
                            host_id = %h.id,
                            error = %e,
                            "fan_out_rows: proxy_get_json failed; skipping"
                        );
                        Vec::new()
                    }
                }
            }
        })
        .collect();

    let remote_results = join_all(futs).await;
    let mut all = local_values;
    all.extend(remote_results.into_iter().flatten());
    all
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
