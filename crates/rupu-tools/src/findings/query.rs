//! `findings.query`: one page of the workspace's findings matching a query.

use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Instant;

// was `query_findings`
pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "findings.query",
    aliases: &[Alias::any("query_findings")],
    effect: Effect::Read,
    needs: &[Service::Findings],
    uses: &[],
    description: "List findings recorded in this project with a one-line query in `q` (e.g. \
     `severity>=high tag:needs-poc -has:poc`). Returns one page of slim rows (id, title, \
     severity, location, tags), `next_cursor`, `total`, and `tags_in_use` — reuse an \
     existing tag where it fits. `all: true` returns every match unpaged, however many: \
     page with `cursor` instead unless you need the whole set.",
    input_schema: rupu_coverage::query_input_schema,
};

pub struct FindingsQueryTool;

#[async_trait]
impl Tool for FindingsQueryTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let q: rupu_coverage::FindingQuery = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let workspace = ctx.workspace.path.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let records =
                rupu_coverage::read_workspace_findings(&workspace).map_err(|e| e.to_string())?;
            rupu_coverage::query_response(&records, &q).map_err(|e| e.to_string())
        })
        .await;
        match result {
            Ok(Ok(v)) => Ok(
                ok(serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())).timed(started),
            ),
            Ok(Err(e)) => Ok(failed(e).timed(started)),
            Err(join) => {
                Ok(failed(format!("findings.query did not complete: {join}")).timed(started))
            }
        }
    }
}
