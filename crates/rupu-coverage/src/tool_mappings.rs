//! User-declared mappings that teach the coverage harness how to extract
//! a file path from an otherwise-unrecognized (e.g. MCP-provided) tool's
//! input, so it can emit FileTouchEvents. Loaded from
//! `.rupu/coverage/tool-mappings.yaml`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One tool's path-extraction rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMapping {
    /// JSON key in the tool's input object that holds the file path.
    pub path_arg: String,
    /// Touch kind to record (defaults to "read").
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "read".to_string()
}

/// Map of tool name → extraction rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolMappings {
    pub tools: BTreeMap<String, ToolMapping>,
}

impl ToolMappings {
    pub fn get(&self, tool: &str) -> Option<&ToolMapping> {
        self.tools.get(tool)
    }
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// Load `.rupu/coverage/tool-mappings.yaml` from a workspace. Returns an
/// empty mapping (not an error) when the file is absent.
pub fn load_tool_mappings(workspace: &Path) -> Result<ToolMappings, serde_yaml::Error> {
    let path = workspace
        .join(".rupu")
        .join("coverage")
        .join("tool-mappings.yaml");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(ToolMappings::default());
    };
    serde_yaml::from_str(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_file_yields_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(load_tool_mappings(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn parses_mappings_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join(".rupu/coverage");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tool-mappings.yaml"),
            "cat_file:\n  path_arg: path\nread_doc:\n  path_arg: file\n  kind: read\n",
        )
        .unwrap();
        let m = load_tool_mappings(tmp.path()).unwrap();
        assert_eq!(m.get("cat_file").unwrap().path_arg, "path");
        assert_eq!(m.get("cat_file").unwrap().kind, "read"); // default applied
        assert_eq!(m.get("read_doc").unwrap().path_arg, "file");
    }
}
