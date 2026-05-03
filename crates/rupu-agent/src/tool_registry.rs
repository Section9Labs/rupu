//! Tool registry — maps tool name (as it appears in agent files and
//! provider tool-call payloads) to a `Box<dyn Tool>` for dispatch.
//!
//! The default registry contains the six v0 tools; agents can opt
//! into a subset via the frontmatter `tools:` list ([`Self::filter_to`]).

use rupu_tools::{BashTool, EditFileTool, GlobTool, GrepTool, ReadFileTool, Tool, WriteFileTool};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Tool name → boxed implementation.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, name: impl Into<String>, tool: Arc<dyn Tool>) {
        self.tools.insert(name.into(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Sorted list of registered tool names.
    pub fn known_tools(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Convert each registered tool into the `ToolDefinition` shape the
    /// LLM provider expects in `LlmRequest.tools`. Used by the agent
    /// loop to tell the model what tools are available before sending
    /// the request.
    pub fn to_tool_definitions(&self) -> Vec<rupu_providers::ToolDefinition> {
        self.tools
            .iter()
            .map(|(name, tool)| rupu_providers::ToolDefinition {
                name: name.clone(),
                description: tool.description().to_string(),
                input_schema: tool.input_schema(),
            })
            .collect()
    }

    /// New registry containing only the entries whose names are in
    /// `whitelist`. Used to honor an agent's frontmatter `tools:` field.
    pub fn filter_to(&self, whitelist: &[String]) -> Self {
        let mut out = Self::new();
        for n in whitelist {
            if let Some(t) = self.tools.get(n) {
                out.tools.insert(n.clone(), t.clone());
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

/// All six v0 tools wired up.
pub fn default_tool_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.insert("bash", Arc::new(BashTool));
    r.insert("read_file", Arc::new(ReadFileTool));
    r.insert("write_file", Arc::new(WriteFileTool));
    r.insert("edit_file", Arc::new(EditFileTool));
    r.insert("grep", Arc::new(GrepTool));
    r.insert("glob", Arc::new(GlobTool));
    r
}
