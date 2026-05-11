//! `rupu webhook serve`. Long-running HTTP receiver for SCM events.
//!
//! Secrets come from env vars (not config files, not the keychain —
//! webhook secrets are operational secrets that ought to be in
//! whatever process supervisor / systemd-unit / cron environment is
//! running this command):
//!
//!   RUPU_GITHUB_WEBHOOK_SECRET   (HMAC-SHA256 secret for GitHub)
//!   RUPU_GITLAB_WEBHOOK_TOKEN    (shared-secret token for GitLab)
//!   RUPU_LINEAR_WEBHOOK_SECRET   (HMAC-SHA256 secret for Linear)
//!   RUPU_JIRA_WEBHOOK_SECRET     (HMAC-SHA256 secret for Jira Cloud)
//!
//! Any may be unset; the corresponding endpoint then returns
//! 503 (service-unavailable) so the operator knows the route is
//! intentionally disabled rather than misconfigured.

use super::autoflow_wake::wake_requests_from_webhook;
use crate::paths;
use async_trait::async_trait;
use clap::Subcommand;
use rupu_auth::{CredentialResolver, KeychainResolver};
use rupu_orchestrator::Workflow;
use rupu_runtime::{WakeStore, WakeStoreError};
use rupu_scm::connectors::github::GithubClient;
use rupu_webhook::{
    serve, DispatchOutcome, GithubProjectsHydrator, WebhookConfig, WebhookEvent, WebhookObserver,
    WorkflowDispatcher,
};
use rupu_workspace::RepoRegistryStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::warn;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Run the webhook HTTP receiver in the foreground.
    Serve {
        /// Address to bind. Defaults to 127.0.0.1:8080. Use 0.0.0.0
        /// to expose externally (only do this behind a reverse proxy
        /// that terminates TLS — rupu does not).
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: SocketAddr,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Serve { addr } => serve_cmd(addr).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn serve_cmd(addr: SocketAddr) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let resolver = KeychainResolver::new();
    let cfg = load_cli_config();
    let github_secret = std::env::var("RUPU_GITHUB_WEBHOOK_SECRET")
        .ok()
        .map(|s| s.into_bytes());
    let gitlab_token = std::env::var("RUPU_GITLAB_WEBHOOK_TOKEN")
        .ok()
        .map(|s| s.into_bytes());
    let linear_secret = std::env::var("RUPU_LINEAR_WEBHOOK_SECRET")
        .ok()
        .map(|s| s.into_bytes());
    let jira_secret = std::env::var("RUPU_JIRA_WEBHOOK_SECRET")
        .ok()
        .map(|s| s.into_bytes());
    if github_secret.is_none()
        && gitlab_token.is_none()
        && linear_secret.is_none()
        && jira_secret.is_none()
    {
        anyhow::bail!(
            "none of RUPU_GITHUB_WEBHOOK_SECRET, RUPU_GITLAB_WEBHOOK_TOKEN, \
             RUPU_LINEAR_WEBHOOK_SECRET, or RUPU_JIRA_WEBHOOK_SECRET is set; \
             at least one webhook endpoint must be configured"
        );
    }

    let config = WebhookConfig {
        addr,
        github_secret,
        gitlab_token,
        linear_secret,
        jira_secret,
        github_projects_hydrator: maybe_build_github_projects_hydrator(&resolver, &cfg).await,
        workflow_loader: Arc::new(load_workflows),
        dispatcher: Arc::new(CliDispatcher),
        observer: Some(Arc::new(CliWebhookObserver { global })),
    };
    serve(config).await?;
    Ok(())
}

/// Production [`WorkflowDispatcher`] — dispatches the exact resolved
/// workflow candidate for the incoming event. Failures are returned to
/// the receiver, which records them in the JSON response so operators
/// can see what went wrong without tailing logs.
struct CliDispatcher;

#[async_trait]
impl WorkflowDispatcher for CliDispatcher {
    async fn dispatch(
        &self,
        workflow_key: &str,
        event: &serde_json::Value,
    ) -> anyhow::Result<DispatchOutcome> {
        let dispatch = decode_dispatch_key(workflow_key)?;
        let summary = super::workflow::run_by_path(
            dispatch.workflow_path,
            dispatch.project_root,
            dispatch.workspace_path,
            Vec::new(),
            None,
            Some(event.clone()),
        )
        .await?;
        Ok(DispatchOutcome {
            run_id: summary.run_id,
            awaiting_step_id: summary.awaiting_step_id,
        })
    }
}

fn load_cli_config() -> rupu_config::Config {
    let Ok(global_dir) = paths::global_dir() else {
        return rupu_config::Config::default();
    };
    let global_cfg_path = global_dir.join("config.toml");
    let pwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_root = paths::project_root_for(&pwd).ok().flatten();
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

async fn maybe_build_github_projects_hydrator(
    resolver: &dyn CredentialResolver,
    cfg: &rupu_config::Config,
) -> Option<Arc<dyn GithubProjectsHydrator>> {
    let creds = resolver.get("github", None).await.ok()?.1;
    let token = match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => key,
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access,
    };
    let base_url = cfg
        .scm
        .platforms
        .get("github")
        .and_then(|platform| platform.base_url.clone());
    Some(Arc::new(CliGithubProjectsHydrator {
        client: GithubClient::new(token, base_url, Some(2)),
    }))
}

struct CliWebhookObserver {
    global: PathBuf,
}

#[async_trait]
impl WebhookObserver for CliWebhookObserver {
    async fn observe(&self, event: &WebhookEvent) -> anyhow::Result<()> {
        let Some(requests) = wake_requests_from_webhook(event) else {
            return Ok(());
        };
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&self.global),
        };
        let Some(first_request) = requests.first() else {
            return Ok(());
        };
        if repo_store.load(&first_request.repo_ref)?.is_none() {
            return Ok(());
        }
        let store = WakeStore::new(paths::autoflow_wakes_dir(&self.global));
        for request in requests {
            match store.enqueue(request) {
                Ok(_) => {}
                Err(WakeStoreError::DuplicateDedupeKey(_)) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }
}

/// Walk global + visible tracked-repo workflow directories and return
/// every successfully-parsed workflow candidate. Called fresh per
/// request so authors can edit workflow files without restarting the
/// receiver.
fn load_workflows() -> Vec<(String, Workflow)> {
    let global = match paths::global_dir() {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "global dir lookup failed");
            return Vec::new();
        }
    };
    let pwd = std::env::current_dir().ok();
    let project_root = pwd
        .as_deref()
        .and_then(|p| paths::project_root_for(p).ok().flatten());
    let default_workspace = pwd.clone().unwrap_or_else(|| global.clone());
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };

    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    push_workflows(
        &global.join("workflows"),
        None,
        default_workspace.clone(),
        &mut seen,
        &mut candidates,
    );
    if let Some(p) = &project_root {
        push_workflows(
            &p.join(".rupu/workflows"),
            Some(p.clone()),
            p.clone(),
            &mut seen,
            &mut candidates,
        );
    }
    if let Ok(tracked) = repo_store.list() {
        for repo in tracked {
            let preferred_checkout = PathBuf::from(&repo.preferred_path);
            if !preferred_checkout.exists() {
                continue;
            }
            let tracked_project_root = paths::project_root_for(&preferred_checkout)
                .ok()
                .flatten()
                .or_else(|| {
                    preferred_checkout
                        .join(".rupu")
                        .is_dir()
                        .then_some(preferred_checkout.clone())
                });
            let Some(tracked_project_root) = tracked_project_root else {
                continue;
            };
            push_workflows(
                &tracked_project_root.join(".rupu/workflows"),
                Some(tracked_project_root.clone()),
                preferred_checkout,
                &mut seen,
                &mut candidates,
            );
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    candidates
}

fn push_workflows(
    dir: &Path,
    project_root: Option<PathBuf>,
    workspace_path: PathBuf,
    seen: &mut BTreeSet<PathBuf>,
    into: &mut Vec<(String, Workflow)>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let ext = p.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let canonical = match p.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let body = match std::fs::read_to_string(&canonical) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(wf) = Workflow::parse(&body) else {
            continue;
        };
        let Ok(key) = encode_dispatch_key(DispatchKey {
            workflow_path: canonical,
            project_root: project_root.clone(),
            workspace_path: workspace_path.clone(),
        }) else {
            continue;
        };
        into.push((key, wf));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DispatchKey {
    workflow_path: PathBuf,
    #[serde(default)]
    project_root: Option<PathBuf>,
    workspace_path: PathBuf,
}

fn encode_dispatch_key(key: DispatchKey) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&key)?)
}

fn decode_dispatch_key(value: &str) -> anyhow::Result<DispatchKey> {
    Ok(serde_json::from_str(value)?)
}

struct CliGithubProjectsHydrator {
    client: GithubClient,
}

#[async_trait]
impl GithubProjectsHydrator for CliGithubProjectsHydrator {
    async fn hydrate(&self, payload: &Value) -> anyhow::Result<Value> {
        if !github_projects_payload_needs_hydration(payload) {
            return Ok(payload.clone());
        }
        hydrate_github_projects_payload(&self.client, payload).await
    }
}

const GITHUB_PROJECTS_ITEM_QUERY: &str = r#"
query RupuProjectItemHydration($itemId: ID!) {
  item: node(id: $itemId) {
    __typename
    ... on ProjectV2Item {
      content {
        __typename
        ... on Issue {
          id
          number
          url
          repository { nameWithOwner }
        }
        ... on PullRequest {
          id
          number
          url
          repository { nameWithOwner }
        }
        ... on DraftIssue {
          id
          title
        }
      }
      fieldValues(first: 50) {
        nodes {
          __typename
          ... on ProjectV2ItemFieldSingleSelectValue {
            optionId
            name
            field {
              __typename
              ... on ProjectV2FieldCommon {
                id
                name
                dataType
              }
              ... on ProjectV2SingleSelectField {
                options {
                  id
                  name
                }
              }
            }
          }
          ... on ProjectV2ItemFieldIterationValue {
            iterationId
            title
            field {
              __typename
              ... on ProjectV2FieldCommon {
                id
                name
                dataType
              }
              ... on ProjectV2IterationField {
                configuration {
                  iterations {
                    id
                    title
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;

const GITHUB_PROJECTS_QUERY: &str = r#"
query RupuProjectHydration($projectId: ID!) {
  project: node(id: $projectId) {
    __typename
    ... on ProjectV2 {
      id
      title
    }
  }
}
"#;

async fn hydrate_github_projects_payload(
    client: &GithubClient,
    payload: &Value,
) -> anyhow::Result<Value> {
    let item_id = github_projects_item_id(payload);
    let project_id = github_projects_project_id(payload);
    if item_id.is_none() && project_id.is_none() {
        return Ok(payload.clone());
    }

    let item_data = if let Some(item_id) = item_id.as_deref() {
        client
            .graphql_json(GITHUB_PROJECTS_ITEM_QUERY, json!({ "itemId": item_id }))
            .await
            .ok()
    } else {
        None
    };
    let project_data = if let Some(project_id) = project_id.as_deref() {
        client
            .graphql_json(GITHUB_PROJECTS_QUERY, json!({ "projectId": project_id }))
            .await
            .ok()
    } else {
        None
    };

    Ok(apply_github_projects_hydration(
        payload,
        item_data.as_ref(),
        project_data.as_ref(),
    ))
}

fn github_projects_payload_needs_hydration(payload: &Value) -> bool {
    let item = payload
        .get("projects_v2_item")
        .or_else(|| payload.get("project_v2_item"));
    let content = item.and_then(|item| item.get("content"));
    let project = payload.get("projects_v2");
    let current = item.and_then(|item| item.get("field_value"));
    let previous = payload
        .get("changes")
        .and_then(|changes| changes.get("field_value"));

    content
        .and_then(|content| content.get("number"))
        .and_then(Value::as_u64)
        .is_none()
        || content
            .and_then(|content| content.get("repository"))
            .and_then(|repo| repo.get("full_name"))
            .and_then(Value::as_str)
            .is_none()
        || project
            .and_then(|project| project.get("title").or_else(|| project.get("name")))
            .and_then(Value::as_str)
            .is_none()
        || current
            .and_then(|value| value.get("name").or_else(|| value.get("title")))
            .and_then(Value::as_str)
            .is_none()
        || current
            .and_then(|value| {
                value
                    .get("field")
                    .and_then(|field| field.get("name"))
                    .or_else(|| value.get("field_name"))
            })
            .and_then(Value::as_str)
            .is_none()
        || previous
            .and_then(|value| value.get("name").or_else(|| value.get("title")))
            .and_then(Value::as_str)
            .is_none()
}

fn apply_github_projects_hydration(
    payload: &Value,
    item_data: Option<&Value>,
    project_data: Option<&Value>,
) -> Value {
    let mut hydrated = payload.clone();
    if let Some(project) = project_data.and_then(|data| data.get("project")) {
        merge_project_title(&mut hydrated, project);
    }
    if let Some(item) = item_data.and_then(|data| data.get("item")) {
        merge_project_content(&mut hydrated, item);
        merge_project_field_values(&mut hydrated, item);
    }
    hydrated
}

fn merge_project_title(payload: &mut Value, project: &Value) {
    let title = project.get("title").and_then(Value::as_str);
    let Some(title) = title else {
        return;
    };
    let Some(root) = payload.as_object_mut() else {
        return;
    };
    let projects = root
        .entry("projects_v2")
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(projects) = projects.as_object_mut() else {
        return;
    };
    if projects.get("title").is_none() && projects.get("name").is_none() {
        projects.insert("title".into(), Value::String(title.to_string()));
    }
}

fn merge_project_content(payload: &mut Value, item: &Value) {
    let Some(content) = item.get("content") else {
        return;
    };
    let Some(item_obj) = github_projects_item_object_mut(payload) else {
        return;
    };
    let target = item_obj
        .entry("content")
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(target_obj) = target.as_object_mut() else {
        return;
    };

    merge_value_if_missing(target_obj, "__typename", content.get("__typename"));
    merge_value_if_missing(target_obj, "node_id", content.get("id"));
    merge_value_if_missing(target_obj, "number", content.get("number"));
    merge_value_if_missing(target_obj, "html_url", content.get("url"));
    merge_value_if_missing(target_obj, "url", content.get("url"));

    if let Some(repo_name) = content
        .get("repository")
        .and_then(|repo| repo.get("nameWithOwner"))
        .and_then(Value::as_str)
    {
        let repo = target_obj
            .entry("repository")
            .or_insert_with(|| Value::Object(Default::default()));
        if let Some(repo_obj) = repo.as_object_mut() {
            if repo_obj.get("full_name").is_none() {
                repo_obj.insert("full_name".into(), Value::String(repo_name.to_string()));
            }
        }
    }
}

fn merge_project_field_values(payload: &mut Value, item: &Value) {
    let Some(nodes) = item
        .get("fieldValues")
        .and_then(|field_values| field_values.get("nodes"))
        .and_then(Value::as_array)
    else {
        return;
    };
    let Some(item_obj) = github_projects_item_object_mut(payload) else {
        return;
    };

    let Some(current) = item_obj.get_mut("field_value") else {
        return;
    };
    let Some(matched) = find_matching_field_value(current, nodes) else {
        return;
    };
    hydrate_field_value(current, matched, None);

    if let Some(previous) = payload
        .get_mut("changes")
        .and_then(|changes| changes.get_mut("field_value"))
    {
        hydrate_field_value(previous, matched, previous_field_name(matched, previous));
    }
}

fn previous_field_name(matched: &Value, previous: &Value) -> Option<String> {
    previous
        .get("field")
        .and_then(|field| field.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            matched
                .get("field")
                .and_then(|field| field.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn hydrate_field_value(
    field_value: &mut Value,
    matched: &Value,
    field_name_override: Option<String>,
) {
    let Some(target) = field_value.as_object_mut() else {
        return;
    };
    if let Some(field_type) = matched
        .get("__typename")
        .and_then(Value::as_str)
        .and_then(graphql_field_value_type)
    {
        target
            .entry("field_type")
            .or_insert_with(|| Value::String(field_type.to_string()));
    }

    if let Some(field) = matched.get("field") {
        let field_entry = target
            .entry("field")
            .or_insert_with(|| Value::Object(Default::default()));
        if let Some(field_obj) = field_entry.as_object_mut() {
            merge_value_if_missing(field_obj, "id", field.get("id"));
            if let Some(name) = field_name_override.or_else(|| {
                field
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }) {
                field_obj
                    .entry("name")
                    .or_insert_with(|| Value::String(name.clone()));
                target
                    .entry("field_name")
                    .or_insert_with(|| Value::String(name));
            }
        }
    }

    match matched.get("__typename").and_then(Value::as_str) {
        Some("ProjectV2ItemFieldSingleSelectValue") => {
            merge_value_if_missing(target, "optionId", matched.get("optionId"));
            if let Some(name) = lookup_single_select_option_name(target, matched) {
                target.entry("name").or_insert_with(|| Value::String(name));
            } else if let Some(name) = matched.get("name").and_then(Value::as_str) {
                target
                    .entry("name")
                    .or_insert_with(|| Value::String(name.to_string()));
            }
        }
        Some("ProjectV2ItemFieldIterationValue") => {
            merge_value_if_missing(target, "iterationId", matched.get("iterationId"));
            if let Some(title) = lookup_iteration_title(target, matched) {
                target
                    .entry("title")
                    .or_insert_with(|| Value::String(title.clone()));
                target.entry("name").or_insert_with(|| Value::String(title));
            } else if let Some(title) = matched.get("title").and_then(Value::as_str) {
                target
                    .entry("title")
                    .or_insert_with(|| Value::String(title.to_string()));
                target
                    .entry("name")
                    .or_insert_with(|| Value::String(title.to_string()));
            }
        }
        _ => {}
    }
}

fn lookup_single_select_option_name(
    target: &serde_json::Map<String, Value>,
    matched: &Value,
) -> Option<String> {
    let option_id = target
        .get("optionId")
        .or_else(|| target.get("option_id"))
        .and_then(value_as_scalar_string)?;
    matched
        .get("field")
        .and_then(|field| field.get("options"))
        .and_then(Value::as_array)?
        .iter()
        .find(|option| option.get("id").and_then(Value::as_str) == Some(option_id.as_str()))
        .and_then(|option| option.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn lookup_iteration_title(
    target: &serde_json::Map<String, Value>,
    matched: &Value,
) -> Option<String> {
    let iteration_id = target
        .get("iterationId")
        .or_else(|| target.get("iteration_id"))
        .and_then(value_as_scalar_string)?;
    matched
        .get("field")
        .and_then(|field| field.get("configuration"))
        .and_then(|configuration| configuration.get("iterations"))
        .and_then(Value::as_array)?
        .iter()
        .find(|iteration| {
            iteration.get("id").and_then(Value::as_str) == Some(iteration_id.as_str())
        })
        .and_then(|iteration| iteration.get("title"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn find_matching_field_value<'a>(current: &Value, nodes: &'a [Value]) -> Option<&'a Value> {
    let field_id = current
        .get("field")
        .and_then(|field| field.get("id").or_else(|| field.get("node_id")))
        .and_then(value_as_scalar_string);
    let option_id = current
        .get("optionId")
        .or_else(|| current.get("option_id"))
        .and_then(value_as_scalar_string);
    let iteration_id = current
        .get("iterationId")
        .or_else(|| current.get("iteration_id"))
        .and_then(value_as_scalar_string);
    let field_name = current
        .get("field")
        .and_then(|field| field.get("name"))
        .or_else(|| current.get("field_name"))
        .and_then(Value::as_str);

    nodes.iter().find(|node| {
        if let Some(expected) = field_id.as_deref() {
            let actual = node
                .get("field")
                .and_then(|field| field.get("id"))
                .and_then(Value::as_str);
            if actual == Some(expected) {
                return true;
            }
        }
        if let Some(expected) = option_id.as_deref() {
            if node.get("optionId").and_then(Value::as_str) == Some(expected) {
                return true;
            }
        }
        if let Some(expected) = iteration_id.as_deref() {
            if node.get("iterationId").and_then(Value::as_str) == Some(expected) {
                return true;
            }
        }
        if let Some(expected) = field_name {
            return node
                .get("field")
                .and_then(|field| field.get("name"))
                .and_then(Value::as_str)
                == Some(expected);
        }
        false
    })
}

fn github_projects_item_id(payload: &Value) -> Option<String> {
    payload
        .get("projects_v2_item")
        .or_else(|| payload.get("project_v2_item"))
        .and_then(|item| item.get("id"))
        .and_then(value_as_scalar_string)
}

fn github_projects_item_object_mut(
    payload: &mut Value,
) -> Option<&mut serde_json::Map<String, Value>> {
    let map = payload.as_object_mut()?;
    if map.contains_key("projects_v2_item") {
        map.get_mut("projects_v2_item")?.as_object_mut()
    } else {
        map.get_mut("project_v2_item")?.as_object_mut()
    }
}

fn github_projects_project_id(payload: &Value) -> Option<String> {
    payload
        .get("projects_v2")
        .and_then(|project| project.get("node_id").or_else(|| project.get("id")))
        .and_then(value_as_scalar_string)
        .or_else(|| {
            payload
                .get("projects_v2_item")
                .or_else(|| payload.get("project_v2_item"))
                .and_then(|item| {
                    item.get("project_node_id")
                        .or_else(|| item.get("project_id"))
                })
                .and_then(value_as_scalar_string)
        })
}

fn merge_value_if_missing(
    object: &mut serde_json::Map<String, Value>,
    key: &str,
    value: Option<&Value>,
) {
    if object.get(key).is_none() {
        if let Some(value) = value.cloned() {
            object.insert(key.to_string(), value);
        }
    }
}

fn graphql_field_value_type(typename: &str) -> Option<&'static str> {
    match typename {
        "ProjectV2ItemFieldSingleSelectValue" => Some("single_select"),
        "ProjectV2ItemFieldIterationValue" => Some("iteration"),
        _ => None,
    }
}

fn value_as_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use httpmock::prelude::*;
    use rupu_webhook::{WebhookEvent, WebhookSource};
    use serde_json::json;

    #[tokio::test]
    async fn observer_queues_only_tracked_repo_events() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        let repo_path = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &repo_path,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("main"),
            )
            .unwrap();

        let observer = CliWebhookObserver {
            global: global.clone(),
        };
        let tracked_event = WebhookEvent {
            source: WebhookSource::Github,
            event_id: "github.issue.labeled".into(),
            delivery_id: Some("delivery-123".into()),
            payload: json!({
                "issue": { "number": 42 },
                "repository": {
                    "name": "rupu",
                    "owner": { "login": "Section9Labs" }
                }
            }),
        };
        observer.observe(&tracked_event).await.unwrap();
        observer
            .observe(&WebhookEvent {
                source: WebhookSource::Github,
                event_id: "github.issue.labeled".into(),
                delivery_id: Some("delivery-456".into()),
                payload: json!({
                    "issue": { "number": 7 },
                    "repository": {
                        "name": "other",
                        "owner": { "login": "Section9Labs" }
                    }
                }),
            })
            .await
            .unwrap();

        let queued = WakeStore::new(paths::autoflow_wakes_dir(&global))
            .list_due(chrono::Utc::now())
            .unwrap();
        let expected = crate::cmd::autoflow_wake::wake_requests_from_webhook(&tracked_event)
            .unwrap()
            .len();
        assert_eq!(queued.len(), expected);
        assert!(queued
            .iter()
            .all(|wake| wake.repo_ref == "github:Section9Labs/rupu"));
    }

    #[tokio::test]
    async fn observer_dedupes_replayed_webhook_deliveries() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        let repo_path = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &repo_path,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("main"),
            )
            .unwrap();

        let observer = CliWebhookObserver {
            global: global.clone(),
        };
        let replay = WebhookEvent {
            source: WebhookSource::Github,
            event_id: "github.issue.labeled".into(),
            delivery_id: Some("delivery-123".into()),
            payload: json!({
                "issue": { "number": 42 },
                "repository": {
                    "name": "rupu",
                    "owner": { "login": "Section9Labs" }
                }
            }),
        };

        observer.observe(&replay).await.unwrap();
        observer.observe(&replay).await.unwrap();

        let queued = WakeStore::new(paths::autoflow_wakes_dir(&global))
            .list_due(chrono::Utc::now())
            .unwrap();
        let expected = crate::cmd::autoflow_wake::wake_requests_from_webhook(&replay)
            .unwrap()
            .len();
        assert_eq!(queued.len(), expected);
        assert!(queued
            .iter()
            .all(|wake| wake.event.delivery_id.as_deref() == Some("delivery-123")));
    }

    #[test]
    fn dispatch_key_round_trips() {
        let key = DispatchKey {
            workflow_path: PathBuf::from("/tmp/rupu/.rupu/workflows/review.yaml"),
            project_root: Some(PathBuf::from("/tmp/rupu")),
            workspace_path: PathBuf::from("/tmp/rupu"),
        };
        let encoded = encode_dispatch_key(key.clone()).unwrap();
        assert_eq!(decode_dispatch_key(&encoded).unwrap(), key);
    }

    #[test]
    fn load_workflows_includes_tracked_repo_project_workflows() {
        let _guard = ENV_LOCK.blocking_lock();
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let tracked_repo = tmp.path().join("tracked");
        std::fs::create_dir_all(tracked_repo.join(".rupu/workflows")).unwrap();
        std::fs::write(
            tracked_repo.join(".rupu/workflows/review.yaml"),
            r#"name: repo-review
trigger:
  on: event
  event: github.pr.opened
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
"#,
        )
        .unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        std::fs::create_dir_all(&global).unwrap();
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &tracked_repo,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("main"),
            )
            .unwrap();

        let old_home = std::env::var_os("RUPU_HOME");
        let old_cwd = std::env::current_dir().unwrap();
        std::env::set_var("RUPU_HOME", &global);
        std::env::set_current_dir(tmp.path()).unwrap();

        let loaded = load_workflows();

        std::env::set_current_dir(old_cwd).unwrap();
        match old_home {
            Some(value) => std::env::set_var("RUPU_HOME", value),
            None => std::env::remove_var("RUPU_HOME"),
        }

        let (_, workflow) = loaded
            .iter()
            .find(|(_, workflow)| workflow.name == "repo-review")
            .expect("tracked repo workflow");
        assert_eq!(workflow.name, "repo-review");
    }

    #[test]
    fn apply_github_projects_hydration_restores_sparse_names() {
        let payload = json!({
            "action": "edited",
            "projects_v2_item": {
                "id": "PVTI_1",
                "project_node_id": "PVT_1",
                "content_type": "Issue",
                "field_value": {
                    "optionId": "opt-ready"
                }
            },
            "changes": {
                "field_value": {
                    "optionId": "opt-progress"
                }
            }
        });
        let item_data = json!({
            "item": {
                "content": {
                    "__typename": "Issue",
                    "id": "I_1",
                    "number": 42,
                    "url": "https://github.com/Section9Labs/rupu/issues/42",
                    "repository": { "nameWithOwner": "Section9Labs/rupu" }
                },
                "fieldValues": {
                    "nodes": [{
                        "__typename": "ProjectV2ItemFieldSingleSelectValue",
                        "optionId": "opt-ready",
                        "name": "Ready For Review",
                        "field": {
                            "id": "field-status",
                            "name": "Status",
                            "dataType": "SINGLE_SELECT",
                            "options": [
                                { "id": "opt-progress", "name": "In Progress" },
                                { "id": "opt-ready", "name": "Ready For Review" }
                            ]
                        }
                    }]
                }
            }
        });
        let project_data = json!({
            "project": {
                "id": "PVT_1",
                "title": "Delivery"
            }
        });

        let hydrated =
            apply_github_projects_hydration(&payload, Some(&item_data), Some(&project_data));

        assert_eq!(hydrated["projects_v2"]["title"], "Delivery");
        assert_eq!(hydrated["projects_v2_item"]["content"]["number"], 42);
        assert_eq!(
            hydrated["projects_v2_item"]["content"]["repository"]["full_name"],
            "Section9Labs/rupu"
        );
        assert_eq!(
            hydrated["projects_v2_item"]["field_value"]["field"]["name"],
            "Status"
        );
        assert_eq!(
            hydrated["projects_v2_item"]["field_value"]["name"],
            "Ready For Review"
        );
        assert_eq!(hydrated["changes"]["field_value"]["name"], "In Progress");
    }

    #[tokio::test]
    async fn github_projects_hydrator_graphql_round_trip() {
        let server = MockServer::start();
        let item_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/graphql")
                .header("authorization", "Bearer ghp_test")
                .body_contains("\"itemId\":\"PVTI_1\"");
            then.status(200).json_body(json!({
                "data": {
                    "item": {
                        "__typename": "ProjectV2Item",
                        "content": {
                            "__typename": "Issue",
                            "id": "I_1",
                            "number": 42,
                            "url": "https://github.com/Section9Labs/rupu/issues/42",
                            "repository": { "nameWithOwner": "Section9Labs/rupu" }
                        },
                        "fieldValues": {
                            "nodes": [{
                                "__typename": "ProjectV2ItemFieldSingleSelectValue",
                                "optionId": "opt-ready",
                                "name": "Ready For Review",
                                "field": {
                                    "id": "field-status",
                                    "name": "Status",
                                    "dataType": "SINGLE_SELECT",
                                    "options": [
                                        { "id": "opt-progress", "name": "In Progress" },
                                        { "id": "opt-ready", "name": "Ready For Review" }
                                    ]
                                }
                            }]
                        }
                    }
                }
            }));
        });
        let project_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/graphql")
                .header("authorization", "Bearer ghp_test")
                .body_contains("\"projectId\":\"PVT_1\"");
            then.status(200).json_body(json!({
                "data": {
                    "project": {
                        "__typename": "ProjectV2",
                        "id": "PVT_1",
                        "title": "Delivery"
                    }
                }
            }));
        });

        let hydrator = CliGithubProjectsHydrator {
            client: GithubClient::new("ghp_test".into(), Some(server.base_url()), Some(2)),
        };
        let payload = json!({
            "action": "edited",
            "projects_v2_item": {
                "id": "PVTI_1",
                "project_node_id": "PVT_1",
                "content_type": "Issue",
                "field_value": { "optionId": "opt-ready" }
            },
            "changes": {
                "field_value": { "optionId": "opt-progress" }
            }
        });

        let hydrated = hydrator.hydrate(&payload).await.unwrap();

        item_mock.assert();
        project_mock.assert();
        assert_eq!(hydrated["projects_v2"]["title"], "Delivery");
        assert_eq!(hydrated["changes"]["field_value"]["name"], "In Progress");
    }
}
