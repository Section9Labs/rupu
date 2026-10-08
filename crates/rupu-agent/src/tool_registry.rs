//! Tool registry — the tools one run offers, keyed by canonical name.
//!
//! The model sees canonical names ([`Self::to_tool_definitions`]); a call by
//! a legacy alias (`report_finding` for `findings.report`) still resolves
//! ([`Self::resolve`]). The default registry holds the builtins; agents opt
//! into a subset via the frontmatter `tools:` list ([`Self::filter_to`]).

use rupu_tools::{
    AstGrepTool, BashTool, DispatchAgentTool, DispatchAgentsParallelTool, EditFileTool, GlobTool,
    GrepTool, ReadFileTool, Tool, WriteFileTool,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Canonical tool name → implementation.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<&'static str, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    /// Register `tool` under its canonical name, replacing any tool already
    /// registered under it.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name(), tool);
    }

    /// The tool `name` resolves to: its canonical name, else any registered
    /// tool's alias (an alias scoped to a flow lead only ever belongs to a
    /// tool registered in a lead).
    pub fn resolve(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if let Some(t) = self.tools.get(name) {
            return Some(t.clone());
        }
        self.tools
            .values()
            .find(|t| t.descriptor().answers_to(name))
            .cloned()
    }

    /// Alias-aware lookup; see [`Self::resolve`].
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.resolve(name)
    }

    /// Sorted list of registered canonical tool names.
    pub fn known_tools(&self) -> Vec<String> {
        self.tools.keys().map(|k| k.to_string()).collect()
    }

    /// Convert each registered tool into the `ToolDefinition` shape the
    /// LLM provider expects in `LlmRequest.tools`. Used by the agent
    /// loop to tell the model what tools are available before sending
    /// the request.
    pub fn to_tool_definitions(&self) -> Vec<rupu_providers::ToolDefinition> {
        self.tools
            .iter()
            .map(|(name, tool)| rupu_providers::ToolDefinition {
                name: name.to_string(),
                description: tool.description().to_string(),
                input_schema: tool.input_schema(),
            })
            .collect()
    }

    /// New registry containing only the entries `whitelist` names, by
    /// canonical name or alias. Used to honor an agent's frontmatter
    /// `tools:` field.
    pub fn filter_to(&self, whitelist: &[String]) -> Self {
        let mut out = Self::new();
        for n in whitelist {
            if let Some(t) = self.resolve(n) {
                out.insert(t);
            }
        }
        out
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// All builtin tools + sub-agent dispatch wired up.
pub fn default_tool_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.insert(Arc::new(BashTool));
    r.insert(Arc::new(ReadFileTool));
    r.insert(Arc::new(WriteFileTool));
    r.insert(Arc::new(EditFileTool));
    r.insert(Arc::new(AstGrepTool));
    r.insert(Arc::new(GrepTool));
    r.insert(Arc::new(GlobTool));
    r.insert(Arc::new(DispatchAgentTool));
    r.insert(Arc::new(DispatchAgentsParallelTool));
    r
}
