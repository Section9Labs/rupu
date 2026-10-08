//! Adapter so MCP-backed tools satisfy the rupu_tools::Tool trait.
//!
//! Wraps a single MCP tool and forwards `invoke(input, ctx)` to the
//! `ToolDispatcher` shared with the in-process MCP server. Its descriptor is
//! built from the MCP `ToolSpec`, whose `effect()` is the one `ToolKind` →
//! `Effect` mapping. W4 deletes this adapter when the connector tools move
//! into the `rupu-tools` catalog.

use async_trait::async_trait;
use rupu_mcp::{McpError, ToolDispatcher, ToolSpec};
use rupu_tools::{Service, Tool, ToolContext, ToolDescriptor, ToolError, ToolOutput};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// One input-schema function per MCP tool. A descriptor's schema is a plain
/// `fn() -> Value`, so it can't capture the spec's runtime-generated schema;
/// each function reads its tool's schema from [`mcp_schema`] instead.
/// `every_mcp_tool_has_a_schema_fn` holds this list in lockstep with
/// `rupu_mcp::tool_catalog()`.
macro_rules! mcp_schema_fns {
    ($($fn_name:ident => $tool:literal),* $(,)?) => {
        $(fn $fn_name() -> Value { mcp_schema($tool) })*

        fn schema_fn_for(name: &str) -> Option<fn() -> Value> {
            match name {
                $($tool => Some($fn_name as fn() -> Value),)*
                _ => None,
            }
        }
    };
}

mcp_schema_fns! {
    s_scm_repos_list => "scm.repos.list",
    s_scm_repos_get => "scm.repos.get",
    s_scm_branches_list => "scm.branches.list",
    s_scm_branches_create => "scm.branches.create",
    s_scm_files_read => "scm.files.read",
    s_scm_prs_list => "scm.prs.list",
    s_scm_prs_get => "scm.prs.get",
    s_scm_prs_diff => "scm.prs.diff",
    s_scm_prs_comment => "scm.prs.comment",
    s_scm_prs_create => "scm.prs.create",
    s_issues_list => "issues.list",
    s_issues_get => "issues.get",
    s_issues_comments => "issues.comments",
    s_issues_comment => "issues.comment",
    s_issues_create => "issues.create",
    s_issues_update_state => "issues.update_state",
    s_github_workflows_dispatch => "github.workflows_dispatch",
    s_gitlab_pipeline_trigger => "gitlab.pipeline_trigger",
    s_findings_record => "findings.record",
    s_findings_query => "findings.query",
    s_findings_tag => "findings.tag",
}

/// The MCP catalog's specs by name, generated once.
fn specs() -> &'static HashMap<&'static str, ToolSpec> {
    static SPECS: OnceLock<HashMap<&'static str, ToolSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        rupu_mcp::tool_catalog()
            .into_iter()
            .map(|s| (s.name, s))
            .collect()
    })
}

fn mcp_schema(name: &str) -> Value {
    specs()
        .get(name)
        .map(|s| s.input_schema.clone())
        .unwrap_or(Value::Null)
}

/// The descriptor of the MCP tool `name`, built once per process from its
/// spec. `None` for a name outside the MCP catalog, or one with no schema
/// function (a lockstep test keeps that from shipping).
pub fn descriptor_for(name: &str) -> Option<&'static ToolDescriptor> {
    static DESCRIPTORS: OnceLock<HashMap<&'static str, &'static ToolDescriptor>> = OnceLock::new();
    DESCRIPTORS
        .get_or_init(|| {
            specs()
                .values()
                .filter_map(|spec| {
                    let input_schema = schema_fn_for(spec.name)?;
                    let needs: &'static [Service] = if spec.name.starts_with("findings.") {
                        &[Service::Findings]
                    } else {
                        &[Service::Scm]
                    };
                    // Leaked once per catalog entry (a fixed set of ~20) so
                    // the descriptor is `'static` like every other tool's.
                    let d: &'static ToolDescriptor = Box::leak(Box::new(ToolDescriptor {
                        name: spec.name,
                        aliases: &[],
                        effect: spec.effect(),
                        needs,
                        uses: &[],
                        description: spec.description,
                        input_schema,
                    }));
                    Some((spec.name, d))
                })
                .collect()
        })
        .get(name)
        .copied()
}

/// The descriptors of the MCP connector tools an agent run can be offered:
/// the MCP catalog minus `findings.*`, which the agent loop serves through
/// the catalog's own findings tools (the in-process MCP dispatcher has no
/// run context to record findings with).
pub fn connector_descriptors() -> Vec<&'static ToolDescriptor> {
    let mut out: Vec<_> = rupu_mcp::tool_catalog()
        .into_iter()
        .filter(|spec| !spec.name.starts_with("findings."))
        .filter_map(|spec| descriptor_for(spec.name))
        .collect();
    out.sort_by_key(|d| d.name);
    out
}

pub struct McpToolAdapter {
    descriptor: &'static ToolDescriptor,
    dispatcher: Arc<ToolDispatcher>,
}

impl McpToolAdapter {
    /// An adapter for the MCP tool `name`; `None` when [`descriptor_for`]
    /// has no descriptor for it.
    pub fn new(name: &str, dispatcher: Arc<ToolDispatcher>) -> Option<Self> {
        Some(Self {
            descriptor: descriptor_for(name)?,
            dispatcher,
        })
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn descriptor(&self) -> &'static ToolDescriptor {
        self.descriptor
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let name = self.descriptor.name;
        match self.dispatcher.call(name, input.clone()).await {
            Ok(text) => {
                // Emit a FileTouchEvent for user-declared tool mappings so
                // MCP/custom tools contribute to coverage even though they
                // don't self-instrument like built-in tools do.
                if let Some(event) = rupu_tools::coverage_emit::mapped_touch(ctx, name, &input) {
                    rupu_tools::coverage_emit::emit(ctx, event).await;
                }
                Ok(ToolOutput {
                    stdout: text,
                    error: None,
                    duration_ms: 0,
                    derived: None,
                    structured: None,
                })
            }
            Err(e) => match e {
                McpError::PermissionDenied { .. } => Err(ToolError::PermissionDenied),
                other => Err(ToolError::Execution(other.to_string())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_tools::Effect;

    #[test]
    fn every_mcp_tool_has_a_schema_fn() {
        for spec in rupu_mcp::tool_catalog() {
            let d = descriptor_for(spec.name)
                .unwrap_or_else(|| panic!("no schema fn for MCP tool `{}`", spec.name));
            assert_eq!((d.input_schema)(), spec.input_schema, "{}", spec.name);
            assert_eq!(d.effect, spec.effect(), "{}", spec.name);
        }
    }

    #[test]
    fn mcp_writes_are_external_except_findings_writes() {
        assert_eq!(
            descriptor_for("scm.prs.create").unwrap().effect,
            Effect::External
        );
        assert_eq!(
            descriptor_for("issues.comment").unwrap().effect,
            Effect::External
        );
        assert_eq!(descriptor_for("scm.prs.get").unwrap().effect, Effect::Read);
        assert_eq!(
            descriptor_for("findings.record").unwrap().effect,
            Effect::Record
        );
        assert_eq!(
            descriptor_for("findings.tag").unwrap().effect,
            Effect::Record
        );
        assert_eq!(
            descriptor_for("findings.query").unwrap().effect,
            Effect::Read
        );
    }
}
