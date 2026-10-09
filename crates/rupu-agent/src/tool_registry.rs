//! Tool registry — the tools one run offers, keyed by canonical name.
//!
//! A run's registry is built from its resolved grant
//! ([`rupu_tools::ResolvedGrant`]) and nothing else, each body from
//! `rupu_tools::bodies` (or an injected tool). The model sees canonical names
//! ([`ToolRegistry::to_tool_definitions`]); a call by a legacy alias
//! (`report_finding` for `findings.report`) still resolves
//! ([`ToolRegistry::resolve`]).

use rupu_tools::{AliasScope, Tool};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Canonical tool name → implementation.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<&'static str, Arc<dyn Tool>>,
    alias_scope: AliasScope,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            alias_scope: AliasScope::Everywhere,
        }
    }

    /// A registry whose alias lookups resolve in `scope` (see
    /// [`Self::resolve`]).
    pub fn with_alias_scope(scope: AliasScope) -> Self {
        Self {
            tools: BTreeMap::new(),
            alias_scope: scope,
        }
    }

    /// Register `tool` under its canonical name, replacing any tool already
    /// registered under it.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name(), tool);
    }

    /// The tool `name` resolves to: inside a flow lead a lead-scoped alias
    /// first (`coverage.status` → `goal.coverage`), then a canonical name,
    /// then any registered tool's alias (an alias scoped to a flow lead only
    /// ever belongs to a tool registered in a lead).
    pub fn resolve(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if self.alias_scope == AliasScope::FlowLead {
            let scoped = self.tools.values().find(|t| {
                t.descriptor()
                    .aliases
                    .iter()
                    .any(|a| a.name == name && a.scope == AliasScope::FlowLead)
            });
            if let Some(t) = scoped {
                return Some(t.clone());
            }
        }
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
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
