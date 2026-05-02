//! Tool registry — name → Box<dyn Tool>. Real impl lands in Task 6.

use rupu_tools::Tool;
use std::collections::HashMap;
use std::sync::Arc;

/// A registry mapping tool names to their implementations.
pub struct ToolRegistry {
    #[allow(dead_code)]
    tools: HashMap<String, Arc<dyn Tool>>,
}

/// Construct the default [`ToolRegistry`] populated with all six built-in tools.
pub fn default_tool_registry() -> ToolRegistry {
    todo!("default_tool_registry lands in Task 6")
}

impl ToolRegistry {
    /// Look up a tool by name, returning `None` if not registered.
    pub fn get(&self, _name: &str) -> Option<Arc<dyn Tool>> {
        todo!("get lands in Task 6")
    }
}
