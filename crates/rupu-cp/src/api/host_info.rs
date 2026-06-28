#![deny(clippy::all)]

use crate::{
    error::ApiResult,
    host::connector::HostCapabilities,
    state::AppState,
};
use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use rupu_workspace::worker_store::WorkerStore;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/host/info", get(get_host_info))
}

#[derive(Serialize)]
struct HostInfoResponse {
    version: String,
    capabilities: HostCapabilities,
}

async fn get_host_info(State(s): State<AppState>) -> ApiResult<Json<HostInfoResponse>> {
    // Load the worker store and aggregate capabilities
    let worker_store = WorkerStore {
        root: s.global_dir.join("autoflows").join("workers"),
    };

    let mut backends = std::collections::HashSet::new();
    let mut scm_hosts = std::collections::HashSet::new();
    let mut permission_modes = std::collections::HashSet::new();

    // Aggregate capabilities from all workers
    if let Ok(workers) = worker_store.list() {
        for worker in workers {
            for b in &worker.capabilities.backends {
                backends.insert(b.clone());
            }
            for s in &worker.capabilities.scm_hosts {
                scm_hosts.insert(s.clone());
            }
            for m in &worker.capabilities.permission_modes {
                permission_modes.insert(m.clone());
            }
        }
    }

    // Convert to sorted vecs for consistent ordering
    let mut backends_vec: Vec<String> = backends.into_iter().collect();
    let mut scm_hosts_vec: Vec<String> = scm_hosts.into_iter().collect();
    let mut permission_modes_vec: Vec<String> = permission_modes.into_iter().collect();

    backends_vec.sort();
    scm_hosts_vec.sort();
    permission_modes_vec.sort();

    Ok(Json(HostInfoResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: HostCapabilities {
            backends: backends_vec,
            scm_hosts: scm_hosts_vec,
            permission_modes: permission_modes_vec,
        },
    }))
}
