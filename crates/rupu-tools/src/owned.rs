//! Owned tool metadata and invocation bindings. This is a runtime seam, not
//! an MCP client: callers must supply an already scoped, trusted definition
//! and body. Remote annotations are never converted into permission effects.
use crate::{Alias, Effect, Service, Tool, ToolContext, ToolDescriptor, ToolError, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// The exact remote identity behind a locally named tool. Kept separately
/// from the provider-visible canonical name; never reconstructed from it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RemoteToolIdentity {
    pub server: String,
    pub tool: String,
}

/// An immutable catalog definition. Builtin metadata is copied from its
/// static declaration; owned definitions retain their data for the run.
#[derive(Debug)]
pub struct ToolMetadata {
    pub name: String,
    pub aliases: Vec<Alias>,
    pub effect: Effect,
    pub needs: Vec<Service>,
    pub uses: Vec<Service>,
    pub wildcard: bool,
    pub description: String,
}

#[derive(Debug)]
pub struct ToolDefinition {
    metadata: ToolMetadata,
    schema: Value,
    builtin: Option<&'static ToolDescriptor>,
    remote: Option<RemoteToolIdentity>,
}

impl std::ops::Deref for ToolDefinition {
    type Target = ToolMetadata;
    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

impl ToolDefinition {
    pub fn builtin(d: &'static ToolDescriptor) -> Arc<Self> {
        Arc::new(Self {
            metadata: ToolMetadata {
                name: d.name.into(),
                aliases: d.aliases.to_vec(),
                effect: d.effect,
                needs: d.needs.to_vec(),
                uses: d.uses.to_vec(),
                wildcard: d.wildcard,
                description: d.description.into(),
            },
            schema: (d.input_schema)(),
            builtin: Some(d),
            remote: None,
        })
    }

    /// A remote definition with a locally assigned effect. No aliases or
    /// implicit wildcard grant: adding a server cannot enlarge `*` authority.
    /// Record/Spawn require Rupu-owned bookkeeping/child-ceiling contracts.
    pub fn remote(
        name: String,
        identity: RemoteToolIdentity,
        description: String,
        schema: Value,
        effect: Effect,
    ) -> Result<Arc<Self>, CatalogError> {
        let valid_id = |s: &str| {
            !s.is_empty() && s.len() <= 256 && s.trim() == s && !s.chars().any(char::is_control)
        };
        if !name.starts_with("mcp.")
            || name.len() + name.bytes().filter(|b| *b == b'.').count() * 6 > 64
            || name.len() <= 4
            || name.contains("__dot__")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !valid_id(&identity.server)
            || !valid_id(&identity.tool)
            || !schema.is_object()
            || description.len() > 16_384
            || !matches!(effect, Effect::Read | Effect::Write | Effect::External)
        {
            return Err(CatalogError::InvalidDefinition(name));
        }
        Ok(Arc::new(Self {
            metadata: ToolMetadata {
                name,
                aliases: Vec::new(),
                effect,
                needs: Vec::new(),
                uses: Vec::new(),
                wildcard: false,
                description,
            },
            schema,
            builtin: None,
            remote: Some(identity),
        }))
    }
    pub fn remote_identity(&self) -> Option<&RemoteToolIdentity> {
        self.remote.as_ref()
    }
    pub fn static_descriptor(&self) -> Option<&'static ToolDescriptor> {
        self.builtin
    }
    pub fn input_schema(&self) -> Value {
        self.schema.clone()
    }
    pub fn answers_to(&self, name: &str) -> bool {
        self.name == name || self.aliases.iter().any(|a| a.name == name)
    }
    pub fn is_core(&self) -> bool {
        self.builtin.is_some_and(ToolDescriptor::is_core)
    }
    pub fn is_connector(&self) -> bool {
        self.needs.contains(&Service::Scm)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("invalid owned tool definition `{0}`")]
    InvalidDefinition(String),
    #[error("tool name or alias `{0}` already belongs to this catalog")]
    DuplicateName(String),
    #[error("remote tool `{server}` / `{tool}` already belongs to this catalog")]
    DuplicateIdentity { server: String, tool: String },
}

/// The body port only. Catalog metadata, grants and permission decisions
/// belong to the caller; an implementation cannot replace them at dispatch.
#[async_trait]
pub trait ToolInvoker: Send + Sync {
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
}

/// Metadata and implementation captured together. Both builtin and owned
/// tools pass through this binding in the agent registry.
#[derive(Clone)]
pub struct BoundTool {
    definition: Arc<ToolDefinition>,
    body: Body,
}
#[derive(Clone)]
enum Body {
    Builtin(Arc<dyn Tool>),
    Owned(Arc<dyn ToolInvoker>),
}
impl std::fmt::Debug for BoundTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundTool")
            .field("definition", &self.definition)
            .finish_non_exhaustive()
    }
}
impl BoundTool {
    pub fn owned(
        definition: Arc<ToolDefinition>,
        body: Arc<dyn ToolInvoker>,
    ) -> Result<Self, CatalogError> {
        if definition.remote_identity().is_none() {
            return Err(CatalogError::InvalidDefinition(definition.name.clone()));
        }
        Ok(Self {
            definition,
            body: Body::Owned(body),
        })
    }
    pub fn builtin(tool: Arc<dyn Tool>) -> Self {
        Self {
            definition: ToolDefinition::builtin(tool.descriptor()),
            body: Body::Builtin(tool),
        }
    }
    pub(crate) fn with_builtin(definition: Arc<ToolDefinition>, tool: Arc<dyn Tool>) -> Self {
        Self {
            definition,
            body: Body::Builtin(tool),
        }
    }
    pub fn definition(&self) -> &Arc<ToolDefinition> {
        &self.definition
    }
    pub fn descriptor(&self) -> &ToolDefinition {
        &self.definition
    }
    pub fn name(&self) -> &str {
        &self.definition.name
    }
    pub fn description(&self) -> &str {
        match &self.body {
            Body::Builtin(tool) => tool.description(),
            Body::Owned(_) => &self.definition.description,
        }
    }
    pub fn input_schema(&self) -> Value {
        match &self.body {
            Body::Builtin(t) => t.input_schema(),
            Body::Owned(_) => self.definition.input_schema(),
        }
    }
    pub async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        match &self.body {
            Body::Builtin(t) => t.invoke(input, ctx).await,
            Body::Owned(t) => t.invoke(input, ctx).await,
        }
    }
}
