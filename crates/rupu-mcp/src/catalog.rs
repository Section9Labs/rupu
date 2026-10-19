//! The `rupu-tools` catalog as a [`ToolSource`]: every tool the factory's
//! base context can serve, each called in a context built for the call's
//! workspace.

use crate::error::ServeError;
use crate::source::{CallCx, ToolDef, ToolHints, ToolResult, ToolSource};
use async_trait::async_trait;
use rupu_tools::call::{call_shared, CallOutcome};
use rupu_tools::{
    AliasScope, AllowAlways, Effect, GrantInputs, PermissionMode, PermissionPolicy, Service,
    ToolCatalog, ToolContext, ToolDescriptor,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Tools the operator surface serves itself, so the catalog's versions are
/// not listed.
pub const HIDDEN_BY_OPERATOR_TOOLS: &[&str] = &[
    "agents.list",
    "agents.get",
    "workflows.list",
    "workflows.get",
    "findings.query",
    "findings.tag",
];

const WORKSPACE_DESCRIPTION: &str =
    "Project id, or the absolute path of a registered project, the tool acts on.";

pub trait ContextFactory: Send + Sync {
    /// A tool context for one call. `workspace` is a project id or the
    /// absolute path of a registered project; `None` for tools that act on
    /// no workspace (connectors). `Err` text is returned to the caller as
    /// the tool's error.
    fn context(&self, workspace: Option<&str>) -> Result<ToolContext, String>;
}

pub struct CatalogSource {
    factory: Arc<dyn ContextFactory>,
    /// Served descriptors by canonical name.
    served: BTreeMap<&'static str, &'static ToolDescriptor>,
    defs: Vec<ToolDef>,
    policy: PermissionPolicy,
    always: Mutex<AllowAlways>,
}

/// A tool acts on a workspace when it is core (fs/shell) or needs findings.
fn needs_workspace(d: &ToolDescriptor) -> bool {
    d.is_core() || d.needs.contains(&Service::Findings)
}

fn hints_of(effect: Effect) -> ToolHints {
    match effect {
        Effect::Read => ToolHints {
            read_only: true,
            ..ToolHints::default()
        },
        Effect::Record => ToolHints::default(),
        Effect::Write => ToolHints::default(),
        Effect::External | Effect::Spawn => ToolHints {
            open_world: true,
            ..ToolHints::default()
        },
    }
}

fn with_workspace(mut schema: Value) -> Value {
    if !schema.is_object() {
        schema = json!({"type": "object"});
    }
    let obj = schema.as_object_mut().expect("object");
    let props = obj
        .entry("properties")
        .or_insert_with(|| json!({}))
        .as_object_mut();
    if let Some(props) = props {
        props.insert(
            "workspace".into(),
            json!({"type": "string", "description": WORKSPACE_DESCRIPTION}),
        );
    }
    let required = obj.entry("required").or_insert_with(|| json!([]));
    if let Some(arr) = required.as_array_mut() {
        arr.push(json!("workspace"));
    }
    schema
}

impl CatalogSource {
    /// Every catalog tool the factory's base context can serve
    /// (`ctx.services.provided(false)`, grant `*`), minus
    /// [`HIDDEN_BY_OPERATOR_TOOLS`].
    pub fn new(factory: Arc<dyn ContextFactory>) -> Result<Self, ServeError> {
        let base = factory
            .context(None)
            .map_err(|e| ServeError::Filter(format!("catalog base context: {e}")))?;
        let grant = ToolCatalog::builtin().resolve_grant(GrantInputs {
            declared: Some(&["*".to_string()]),
            step_actions: &[],
            connector_cap: None,
            ambient: &[],
            available: &base.services.provided(false),
            alias_scope: AliasScope::Everywhere,
        })?;
        let mut served = BTreeMap::new();
        let mut defs = Vec::new();
        for d in ToolCatalog::all() {
            if !grant.entries.contains_key(d.name) || HIDDEN_BY_OPERATOR_TOOLS.contains(&d.name) {
                continue;
            }
            let body = rupu_tools::bodies::body(d.name, &base)
                .ok_or_else(|| ServeError::NoBody(d.name.to_string()))?;
            let schema = if needs_workspace(d) {
                with_workspace(body.input_schema())
            } else {
                body.input_schema()
            };
            defs.push(ToolDef {
                name: d.name.to_string(),
                description: body.description().to_string(),
                input_schema: schema,
                hints: hints_of(d.effect),
            });
            served.insert(d.name, *d);
        }
        Ok(Self {
            factory,
            served,
            defs,
            policy: PermissionPolicy::unattended(PermissionMode::Bypass),
            always: Mutex::new(AllowAlways::default()),
        })
    }
}

#[async_trait]
impl ToolSource for CatalogSource {
    fn tools(&self) -> Vec<ToolDef> {
        self.defs.clone()
    }

    async fn call(&self, name: &str, args: Value, _cx: &CallCx) -> Option<ToolResult> {
        let d = ToolCatalog::builtin().resolve_name(name, AliasScope::Everywhere)?;
        let d = *self.served.get(d.name)?;
        let mut args = args;
        let workspace = if needs_workspace(d) {
            let ws = args
                .as_object_mut()
                .and_then(|o| o.remove("workspace"))
                .and_then(|v| v.as_str().map(String::from));
            match ws {
                Some(ws) => Some(ws),
                None => {
                    return Some(ToolResult::Error(
                        "missing required argument `workspace`".into(),
                    ))
                }
            }
        } else {
            None
        };
        let ctx = match self.factory.context(workspace.as_deref()) {
            Ok(c) => c,
            Err(e) => return Some(ToolResult::Error(e)),
        };
        let Some(tool) = rupu_tools::bodies::body(d.name, &ctx) else {
            return Some(ToolResult::Error(format!(
                "tool `{}` is not available in this context",
                d.name
            )));
        };
        Some(
            match call_shared(&*tool, args, &ctx, &self.policy, &self.always).await {
                CallOutcome::Done(text) => ToolResult::Text(text),
                CallOutcome::Denied(reason) => {
                    ToolResult::Error(format!("permission denied: {reason}"))
                }
                CallOutcome::Failed(text) => ToolResult::Error(text),
            },
        )
    }
}
