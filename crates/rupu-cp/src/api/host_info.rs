#![deny(clippy::all)]

use crate::{error::ApiResult, host::connector::HostCapabilities, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rupu_workspace::worker_store::WorkerStore;
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/host/info", get(get_host_info))
}

#[derive(Serialize)]
struct HostInfoResponse {
    version: String,
    capabilities: HostCapabilities,
    /// What this build honours as a host
    /// ([`crate::node::protocol::host_features`]) — e.g. request fields its
    /// launch endpoints take, so a coordinator can refuse to send one to a
    /// remote that would silently drop it (see
    /// `HttpHostConnector::require_feature`).
    features: Vec<String>,
    /// Explicit readiness; clients never infer Ask support from version strings.
    session_tool_approvals: serde_json::Value,
    children_paging_root_types: &'static [&'static str],
    retention_execution_kinds: &'static [&'static str],
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
        session_tool_approvals: {
            #[cfg(unix)]
            {
                super::session_approvals::status(&s)
            }
            #[cfg(not(unix))]
            {
                serde_json::json!({"feature":"session.tool_approvals.v1","enabled":false,"authenticated":s.token_set,"broker_ready":false,"reason":"unsupported_host","scope":"none","default_mode_changed":false})
            }
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
        children_paging_root_types: super::children::ROOT_TYPES,
        retention_execution_kinds: {
            #[cfg(unix)]
            {
                if s.launcher.is_some() {
                    super::retention_policy::KINDS
                } else {
                    &[]
                }
            }
            #[cfg(not(unix))]
            {
                &[]
            }
        },
        capabilities: HostCapabilities {
            backends: backends_vec,
            scm_hosts: scm_hosts_vec,
            permission_modes: permission_modes_vec,
        },
        // Shared host features apply to both HTTP and the SSH helper.
        features: {
            let mut features = crate::node::protocol::host_features();
            features.push(super::workflows::validation_projection::FEATURE.to_string());
            if s.agent_launcher
                .as_ref()
                .is_some_and(|l| l.supports_continuation_options())
            {
                features.push(rupu_runtime::continuation_options::FEATURE.to_string());
            }
            #[cfg(unix)]
            if s.launcher.is_some() {
                features.push(super::retention_policy::FEATURE.to_string());
                features.push(super::retention_policy::NETFLOW_FEATURE.to_string());
            }
            #[cfg(unix)]
            if super::session_approvals::available(&s) {
                features.push(super::session_approvals::FEATURE.to_string());
            }
            #[cfg(unix)]
            features.push(super::agentiflows::observation::FEATURE.to_string());
            #[cfg(unix)]
            features.push(super::workflow_observation::FEATURE.to_string());
            #[cfg(unix)]
            features.push(super::workflow_observation::PROCESS_LOG_FEATURE.to_string());
            #[cfg(unix)]
            if s.session_starter
                .as_ref()
                .is_some_and(|v| v.supports_exact_global_conversation())
            {
                features.push("agent.exact_global_conversation.v1".into());
            }
            #[cfg(unix)]
            if s.agent_launcher
                .as_ref()
                .is_some_and(|l| l.supports_exact_global_agent())
            {
                features.push(rupu_runtime::exact_agent::FEATURE.to_string());
            }
            #[cfg(unix)]
            features.push(super::workflow_observation::EVENT_EMITTER_FEATURE.to_string());
            #[cfg(unix)]
            features.push(super::workflow_observation::LIVE_FEATURE.to_string());
            #[cfg(unix)]
            features.push(super::workflow_observation::LATEST_PAGE_FEATURE.to_string());
            #[cfg(unix)]
            features.push(super::workflow_observation::DESCENDANT_FEATURE.to_string());
            #[cfg(unix)]
            features.push(super::agents::source::FEATURE.to_string());
            #[cfg(unix)]
            features.push(super::agentiflows::usage_pages::FEATURE.to_string());
            features.push("workflow.scoped_revision".to_string());
            features.push(super::agentiflow_defs::VALIDATION_DEFINITION_FEATURE.to_string());
            features.push(super::netflow::LOCAL_RUN_GRAPH_FEATURE.to_string());
            features.push("config.scoped_revision.v1".to_string());
            features.push("project.scm_context.v1".to_string());
            if super::transcripts::exact_delete_available(&s) {
                features.push(super::transcripts::EXACT_DELETE_FEATURE.to_string());
            }
            if super::forge_writes::available(&s) {
                features.push(super::forge_writes::FEATURE.to_string());
            }
            if s.repos.as_ref().and_then(|p| p.issue_reader()).is_some() {
                features.push("issues.operator_read.v1".to_string());
                if s.repos
                    .as_ref()
                    .and_then(|p| p.issue_reader())
                    .is_some_and(|p| p.supports_discovery())
                {
                    features.push(super::issues_discovery::FEATURE.to_string());
                }
                if s.repos
                    .as_ref()
                    .and_then(|p| p.issue_reader())
                    .is_some_and(|p| p.supports_discussion())
                {
                    features.push(super::issues_discussion::FEATURE.to_string());
                }
            }
            if s.repos.as_ref().is_some_and(|p| p.supports_review_pages()) {
                features.push(super::review_pages::FEATURE.to_string());
            }
            if s.repos.is_some() {
                features.push("project.scm_review_detail.v1".to_string());
            }
            features.push("workflow.session_history".to_string());
            features.push(super::session_workflows::HISTORY_QUERY_CAPABILITY.to_string());
            features.push(super::workflow_history_owner::FEATURE.to_string());
            features.push(super::session_capabilities::FEATURE.to_string());
            features.push(super::mcp_servers::FEATURE.to_string());
            if s.token_set && s.launcher.is_some() {
                features.push(super::mcp_credentials::FEATURE.to_string());
            }
            #[cfg(unix)]
            if s.launcher.is_some() {
                features.push(super::findings_import_review::FEATURE.to_string());
            }
            features.push(super::attempt_history::CAPABILITY.to_string());
            features.push(super::children::CAPABILITY.to_string());
            features.push(super::children::search::CAPABILITY.to_string());
            features.push(super::children::search::AGENTIFLOW_CAPABILITY.to_string());
            features.push(super::transcript::page::CAPABILITY.to_string());
            features.push(super::coordination_history::CAPABILITY.to_string());
            features.push(super::event_scan::CAPABILITY.to_string());
            if s.session_starter
                .as_ref()
                .is_some_and(|starter| starter.supports_reasoning_effort())
            {
                features.push("session.reasoning_effort.v1".to_string());
            }
            if s.launcher.is_some() {
                features.push("workflow.session_attach".to_string());
            }
            if super::project_session_start::available(&s) {
                features.push(super::project_session_start::CAP_BUILTIN_HARNESS.to_string());
            }
            if super::project_session_start::available(&s)
                && s.session_starter
                    .as_ref()
                    .is_some_and(|starter| starter.supports_session_scoped_reads())
            {
                features.push(rupu_agent::builtin::SESSION_READS_FEATURE.to_string());
            }
            if super::session_mcp::available(&s) {
                features.push(super::session_mcp::FEATURE.to_string());
            }
            if super::project_session_start::tool_selection_available(&s) {
                features.push(super::project_session_start::CAP_TOOL_SELECTION.to_string());
            }
            if super::project_session_start::available(&s)
                && s.session_starter
                    .as_ref()
                    .is_some_and(|starter| starter.supports_session_tool_ceiling())
            {
                features.push(rupu_tools::tool_ceiling::FEATURE.to_owned());
            }
            if super::project_session_start::idle_available(&s) {
                features.push(super::project_session_start::CAP_IDLE_HARNESS.to_string());
            }
            if s.launcher
                .as_ref()
                .is_some_and(|l| l.supports_session_workflows())
            {
                features.push("workflow.session_link".to_string());
                features.push(super::workflows::DRAFT_FEATURE.to_string());
            }
            if s.session_starter
                .as_ref()
                .is_some_and(|starter| starter.supports_session_capabilities())
            {
                features.push("session.skill_preset.v1".to_string());
                if super::project_session_start::available(&s)
                    && s.session_starter
                        .as_ref()
                        .is_some_and(|starter| starter.supports_full_skill_preset())
                {
                    features
                        .push(rupu_runtime::session_skill_preset::FULL_PRESET_FEATURE.to_string());
                }
            }
            features
        },
    }))
}
