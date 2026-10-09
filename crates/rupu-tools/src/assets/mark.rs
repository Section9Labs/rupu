//! `assets.mark`: record how deeply an engagement asset has been examined.
//! Offered only under an active engagement profile set.

use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_coverage::{asset_mark, AssetMarkInput};
use serde_json::Value;
use std::time::Instant;

// was `asset_mark`
pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "assets.mark",
    aliases: &[Alias::any("asset_mark")],
    effect: Effect::Record,
    needs: &[Service::Findings, Service::Engagement],
    uses: &[],
    description: "Record how deeply an engagement asset has been examined, as a rung of \
     its profile's coverage depth ladder (monotonic — a shallower rung after \
     a deeper one keeps the deeper one). The effective rung is returned.",
    input_schema: schema,
};

fn schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["kind", "depth"],
        "properties": {
            "kind": { "type": "string", "description": "Profile-namespaced asset kind, e.g. \"network:service\"." },
            "coordinates": {
                "type": "array",
                "description": "Locator coordinates pinning the asset, each {\"t\": <tag>, \"v\": <value>}.",
                "items": { "type": "object" }
            },
            "depth": { "type": "string", "description": "The depth-ladder rung reached, e.g. \"tested\"." },
            "label": { "type": "string" }
        }
    })
}

pub struct AssetsMarkTool;

#[async_trait]
impl Tool for AssetsMarkTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: AssetMarkInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let Some(engagement) = ctx
            .services
            .findings
            .as_ref()
            .and_then(|f| f.engagement.clone())
        else {
            return Err(ToolError::Execution(
                "assets.mark needs an active engagement profile, and this run has none".into(),
            ));
        };
        let paths = crate::ledger::paths(ctx);
        let res = tokio::task::spawn_blocking(move || asset_mark(&paths, parsed, &engagement))
            .await
            .map_err(|join| {
                ToolError::Execution(format!("assets.mark did not complete: {join}"))
            })?;
        match res {
            Ok(out) => Ok(ok(format!(
                "asset {} is at depth `{}`",
                out.id, out.effective_depth
            ))
            .timed(started)),
            Err(e) => Ok(failed(e.to_string()).timed(started)),
        }
    }
}
