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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_info_fixture_is_current() {
        let value = HostInfoResponse {
            version: "0.71.0".to_string(),
            capabilities: HostCapabilities {
                backends: vec!["claude".into(), "codex".into()],
                scm_hosts: vec!["github.com".into()],
                permission_modes: vec!["ask".into(), "bypass".into(), "readonly".into()],
            },
        };
        // Same helper contract as tests/macos_fixtures.rs (duplicated: unit tests
        // can't share code with integration tests without a public module).
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/rupu-macos/Fixtures");
        let path = dir.join("host_info.json");
        let rendered = serde_json::to_string_pretty(&value).expect("serialize");
        if std::env::var_os("REGEN_FIXTURES").is_some() {
            std::fs::write(&path, rendered + "\n").expect("write fixture");
            return;
        }
        let on_disk = std::fs::read_to_string(&path)
            .expect("missing fixture host_info.json; run `make macos-fixtures`");
        assert_eq!(
            on_disk.trim_end(),
            rendered,
            "host_info fixture drifted; run `make macos-fixtures`"
        );
    }
}
