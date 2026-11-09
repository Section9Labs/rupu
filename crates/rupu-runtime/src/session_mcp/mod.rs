//! Frozen, session-owned configured MCP tools. Discovery supplies metadata;
//! the existing grant and permission path remains the only dispatch path.
use async_trait::async_trait;
use rupu_config::mcp::{Registry, ToolEffect};
use rupu_mcp_client::{Capture, Credentials, Deadlines, DiscoveredTool, HttpToolsClient};
use rupu_tools::{
    BoundTool, Effect, RemoteToolIdentity, Surface, ToolCatalog, ToolContext, ToolDefinition,
    ToolError, ToolInvoker, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_TOOLS: usize = 256;
const DISCOVERY_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRef {
    pub server: String,
    pub tool: String,
}
impl ToolRef {
    fn validate(&self) -> Result<(), Error> {
        if self.server.is_empty()
            || self.server.len() > 64
            || !self
                .server
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || self.tool.is_empty()
            || self.tool.len() > 256
            || self.tool.trim() != self.tool
            || self.tool.chars().any(char::is_control)
        {
            return Err(Error::InvalidSelection);
        }
        Ok(())
    }
    /// Stable provider-safe name; exact identity is carried separately and
    /// never recovered by splitting or decoding this name.
    pub fn local_name(&self) -> String {
        let digest = digest(&json!([self.server, self.tool]));
        format!("mcp.t{}", &digest[7..55])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenTool {
    pub identity: ToolRef,
    /// Includes trusted endpoint/auth-reference/policy and discovered metadata,
    /// never the credential value. Opaque to clients.
    pub fingerprint: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenSelection {
    pub tools: Vec<FrozenTool>,
}
impl FrozenSelection {
    pub fn validate(&self) -> Result<(), Error> {
        if self.tools.len() > MAX_TOOLS {
            return Err(Error::InvalidSelection);
        }
        let mut identities = BTreeSet::new();
        let mut names = BTreeSet::new();
        for entry in &self.tools {
            entry.identity.validate()?;
            if !valid_digest(&entry.fingerprint)
                || !identities.insert(&entry.identity)
                || !names.insert(entry.identity.local_name())
            {
                return Err(Error::InvalidSelection);
            }
        }
        Ok(())
    }
    /// Apply after builtin settings. None is the builtin default, so preserve
    /// it as DEFAULT_GRANT before adding exact remote names; `*` itself never
    /// grants owned tools. Explicit builtin [] remains empty apart from these
    /// separately selected MCP tools and existing ambient grants.
    pub fn append_to(&self, declared: &mut Option<Vec<String>>) {
        if self.tools.is_empty() {
            return;
        }
        let names = declared.get_or_insert_with(|| {
            rupu_tools::DEFAULT_GRANT
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
        });
        for tool in &self.tools {
            let name = tool.identity.local_name();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CatalogTool {
    pub server: String,
    pub server_label: String,
    pub tool: String,
    pub name: String,
    pub description: String,
    pub effect: ToolEffect,
    pub fingerprint: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct CatalogSnapshot {
    pub revision: String,
    pub tools: Vec<CatalogTool>,
}
impl CatalogSnapshot {
    /// None means the trusted host's current configured preset at creation;
    /// Some([]) is an explicit empty selection. Only explicit nonempty choices
    /// require the discovery revision that the editor actually saw.
    pub fn freeze(
        &self,
        requested: Option<&[ToolRef]>,
        expected: Option<&str>,
    ) -> Result<FrozenSelection, Error> {
        let selected = match requested {
            None => self
                .tools
                .iter()
                .map(|t| ToolRef {
                    server: t.server.clone(),
                    tool: t.tool.clone(),
                })
                .collect(),
            Some(tools) => {
                if !tools.is_empty() && expected != Some(self.revision.as_str()) {
                    return Err(Error::Stale);
                }
                tools.to_vec()
            }
        };
        if selected.len() > MAX_TOOLS {
            return Err(Error::InvalidSelection);
        }
        let mut frozen = Vec::new();
        for identity in selected {
            identity.validate()?;
            let tool = self
                .tools
                .iter()
                .find(|t| t.server == identity.server && t.tool == identity.tool)
                .ok_or(Error::Stale)?;
            frozen.push(FrozenTool {
                identity,
                fingerprint: tool.fingerprint.clone(),
            });
        }
        frozen.sort_by(|a, b| a.identity.cmp(&b.identity));
        let result = FrozenSelection { tools: frozen };
        result.validate()?;
        Ok(result)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid MCP tool selection")]
    InvalidSelection,
    #[error("MCP catalog changed; reload the available tools before selecting again")]
    Stale,
    #[error("MCP discovery exceeded its deadline")]
    Deadline,
    #[error(transparent)]
    Registry(#[from] rupu_config::mcp::RegistryError),
    #[error(transparent)]
    Client(#[from] rupu_mcp_client::Error),
    #[error(transparent)]
    Catalog(#[from] rupu_tools::CatalogError),
}

fn digest(value: &Value) -> String {
    // Sort recursively even if a dependency enables serde_json/preserve_order.
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted: BTreeMap<_, _> = map.iter().collect();
                Value::Object(
                    sorted
                        .into_iter()
                        .map(|(key, value)| (key.clone(), canonical(value)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(value)).expect("JSON value serializes"))
    )
}
fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn entry(registry: &Registry, tool: &DiscoveredTool) -> Result<CatalogTool, Error> {
    let config = registry.enabled_server(tool.server_id())?;
    let identity = ToolRef {
        server: tool.server_id().into(),
        tool: tool.name().into(),
    };
    identity.validate()?;
    Ok(CatalogTool {
        server: identity.server.clone(),
        server_label: config.label.clone(),
        tool: identity.tool.clone(),
        name: identity.local_name(),
        description: tool.metadata()["description"]
            .as_str()
            .unwrap_or_default()
            .into(),
        effect: tool.local_effect(),
        fingerprint: digest(
            &json!({"version":1,"server":identity.server,"config":config,"tool":tool.metadata()}),
        ),
    })
}
fn snapshot(mut tools: Vec<CatalogTool>) -> Result<CatalogSnapshot, Error> {
    if tools.len() > MAX_TOOLS {
        return Err(Error::InvalidSelection);
    }
    tools.sort_by(|a, b| (&a.server, &a.tool).cmp(&(&b.server, &b.tool)));
    let revision = digest(
        &json!({"version":1,"tools":tools.iter().map(|t| (&t.server,&t.tool,&t.fingerprint)).collect::<Vec<_>>()}),
    );
    Ok(CatalogSnapshot { revision, tools })
}

/// Read-only metadata discovery; never invokes a tool or opens a provider.
/// Disabled servers are not contacted and their credentials are not read.
pub async fn discover(
    registry: &Registry,
    credentials: &dyn Credentials,
) -> Result<CatalogSnapshot, Error> {
    registry.validate()?;
    tokio::time::timeout(DISCOVERY_DEADLINE, async {
        let mut tools = Vec::new();
        for (server, config) in &registry.servers {
            if !config.enabled {
                continue;
            }
            let client = HttpToolsClient::connect(
                registry,
                server,
                credentials,
                Deadlines::default(),
                Capture {
                    context: rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Mcp(
                        server.clone(),
                    )),
                    sink: Arc::new(rupu_netflow::NullSink),
                },
            )
            .await?;
            let discovered = client.discover().await;
            // Teardown is best effort. Failure never turns a valid metadata
            // snapshot into permission to dispatch or replay anything.
            let _ = client.close().await;
            for tool in discovered? {
                tools.push(entry(registry, &tool)?);
            }
            if tools.len() > MAX_TOOLS {
                return Err(Error::InvalidSelection);
            }
        }
        snapshot(tools)
    })
    .await
    .map_err(|_| Error::Deadline)?
}

/// Validate caller input before any metadata or credential request.
pub fn validate_request(
    requested: Option<&[ToolRef]>,
    expected: Option<&str>,
) -> Result<(), Error> {
    let Some(tools) = requested else {
        return if expected.is_some() {
            Err(Error::InvalidSelection)
        } else {
            Ok(())
        };
    };
    if tools.len() > MAX_TOOLS {
        return Err(Error::InvalidSelection);
    }
    let mut seen = BTreeSet::new();
    for identity in tools {
        identity.validate()?;
        if !seen.insert(identity) {
            return Err(Error::InvalidSelection);
        }
    }
    if !tools.is_empty() && expected.is_none_or(str::is_empty) {
        return Err(Error::Stale);
    }
    Ok(())
}

/// Creation preflight. Explicit empty avoids registry/credential/network work;
/// absence chooses the enabled host preset exactly once.
pub async fn freeze(
    registry: &Registry,
    credentials: &dyn Credentials,
    requested: Option<&[ToolRef]>,
    expected: Option<&str>,
) -> Result<FrozenSelection, Error> {
    validate_request(requested, expected)?;
    if requested.is_some_and(<[ToolRef]>::is_empty) {
        return Ok(FrozenSelection::default());
    }
    discover(registry, credentials)
        .await?
        .freeze(requested, expected)
}

/// A client and all its bindings belong to exactly this session turn. Remote
/// effects are trusted host declarations, not claims of workspace isolation.
#[derive(Clone, Debug)]
pub struct Owner {
    pub run_id: String,
    pub workspace_id: String,
    pub workspace_path: PathBuf,
}
impl Owner {
    fn matches(&self, ctx: &ToolContext) -> bool {
        ctx.identity.surface == Surface::Session
            && ctx.identity.parent.is_none()
            && ctx.identity.run_id == self.run_id
            && ctx.workspace.id == self.workspace_id
            && ctx.workspace.path == self.workspace_path
    }
    fn capture(&self, sink: Arc<dyn rupu_netflow::FlowSink>) -> Capture {
        let mut context = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System);
        context.run_id = Some(self.run_id.clone());
        context.workspace_id = Some(self.workspace_id.clone());
        Capture { context, sink }
    }
}
struct RemoteBody {
    client: Arc<HttpToolsClient>,
    tool: DiscoveredTool,
    owner: Owner,
}
#[async_trait]
impl ToolInvoker for RemoteBody {
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        if !self.owner.matches(ctx) {
            return Err(ToolError::PermissionDenied);
        }
        let started = Instant::now();
        let result = self
            .client
            .call(&self.tool, input)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        // Content blocks remain data. No resource URL is fetched, and no
        // returned metadata changes grants, local effects or dispatch bodies.
        let payload = json!({"content":result.content,"structuredContent":result.structured_content,"isError":result.is_error});
        Ok(ToolOutput {
            stdout: serde_json::to_string(&payload).expect("JSON value serializes"),
            error: result
                .is_error
                .then(|| "MCP tool reported an execution error".into()),
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            derived: None,
            structured: Some(payload),
        })
    }
}

/// Keep initialized sessions alive for their exact tool bindings, then attempt
/// bounded teardown after the turn (including provider/preparation failures).
#[derive(Debug)]
pub struct Assembled {
    pub catalog: ToolCatalog,
    clients: Vec<Arc<HttpToolsClient>>,
}
impl std::ops::Deref for Assembled {
    type Target = ToolCatalog;
    fn deref(&self) -> &Self::Target {
        &self.catalog
    }
}
impl Assembled {
    pub fn cleanup(&self) -> Cleanup {
        Cleanup(self.clients.clone())
    }
}
/// A turn-owned teardown handle; it cannot invoke tools or enlarge authority.
pub struct Cleanup(Vec<Arc<HttpToolsClient>>);
impl Cleanup {
    pub async fn close(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            for client in &self.0 {
                let _ = client.close().await;
            }
        })
        .await;
    }
}

/// Reconnect only selected servers, verify frozen metadata and policy, then
/// bind bodies. Call before provider preparation. A new server/tool cannot
/// expand an old session; changed selected metadata fails closed.
pub async fn assemble(
    registry: &Registry,
    credentials: &dyn Credentials,
    selection: &FrozenSelection,
    owner: Owner,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> Result<Assembled, Error> {
    selection.validate()?;
    if selection.tools.is_empty() {
        return Ok(Assembled {
            catalog: ToolCatalog::builtin(),
            clients: Vec::new(),
        });
    }
    if owner.run_id.is_empty()
        || owner.workspace_id.is_empty()
        || !owner.workspace_path.is_absolute()
    {
        return Err(Error::InvalidSelection);
    }
    registry.validate()?;
    tokio::time::timeout(DISCOVERY_DEADLINE, async {
        let servers: BTreeSet<_> = selection
            .tools
            .iter()
            .map(|t| t.identity.server.as_str())
            .collect();
        let mut clients = BTreeMap::new();
        for server in servers {
            let client = Arc::new(
                HttpToolsClient::connect(
                    registry,
                    server,
                    credentials,
                    Deadlines::default(),
                    owner.capture(sink.clone()),
                )
                .await?,
            );
            let discovered = client.discover().await?;
            clients.insert(server, (client, discovered));
        }
        let mut catalog = ToolCatalog::builtin();
        for frozen in &selection.tools {
            let (client, tools) = &clients[frozen.identity.server.as_str()];
            let tool = tools
                .iter()
                .find(|t| t.name() == frozen.identity.tool)
                .ok_or(Error::Stale)?;
            let metadata = entry(registry, tool)?;
            if metadata.fingerprint != frozen.fingerprint {
                return Err(Error::Stale);
            }
            let definition = ToolDefinition::remote(
                metadata.name,
                RemoteToolIdentity {
                    server: metadata.server,
                    tool: metadata.tool,
                },
                metadata.description,
                tool.metadata()["inputSchema"].clone(),
                match metadata.effect {
                    ToolEffect::Read => Effect::Read,
                    ToolEffect::Write => Effect::Write,
                    ToolEffect::External => Effect::External,
                },
            )?;
            catalog = catalog.with_owned(BoundTool::owned(
                definition,
                Arc::new(RemoteBody {
                    client: client.clone(),
                    tool: tool.clone(),
                    owner: owner.clone(),
                }),
            )?)?;
        }
        Ok(Assembled {
            catalog,
            clients: clients.into_values().map(|(client, _)| client).collect(),
        })
    })
    .await
    .map_err(|_| Error::Deadline)?
}

#[cfg(test)]
mod tests;
