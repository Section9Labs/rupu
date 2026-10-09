//! `findings.tag`: add or remove tags on the workspace's findings, through
//! the one tag-log writer (`ledger::tags::apply`), mirrored to the run's
//! stream when it has one.

use crate::coverage_emit::attribution_from;
use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Instant;

// was `tag_findings`
pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "findings.tag",
    aliases: &[Alias::any("tag_findings")],
    effect: Effect::Record,
    needs: &[Service::Findings],
    uses: &[],
    description: "Add or remove tags on findings in this project, one or many at once. Tags are \
     free-form: lowercase a-z, 0-9 and . _ : / -, starting with a letter or digit (e.g. \
     class:sqli, needs-poc, status:triaged). Prefer tags already in use (findings.query \
     lists them). An unknown finding id rejects the whole call; adding a tag a finding \
     already has changes nothing. Returns each finding's tags before and after.",
    input_schema: rupu_coverage::tag_input_schema,
};

pub struct FindingsTagTool;

#[async_trait]
impl Tool for FindingsTagTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: rupu_coverage::TagChangeInput = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let change = match parsed.into_change() {
            Ok(c) => c,
            Err(e) => return Ok(failed(e.to_string()).timed(started)),
        };
        let by = rupu_coverage::TagActor::Agent(attribution_from(ctx));
        let log = crate::ledger::tag_log(ctx);
        let result =
            tokio::task::spawn_blocking(move || rupu_coverage::apply(&log, &change, &by)).await;
        match result {
            Ok(Ok(outcomes)) => Ok(ok(serde_json::to_string_pretty(
                &serde_json::json!({ "outcomes": outcomes }),
            )
            .unwrap_or_else(|_| "{}".into()))
            .timed(started)),
            Ok(Err(e)) => Ok(failed(e.to_string()).timed(started)),
            Err(join) => {
                Ok(failed(format!("findings.tag did not complete: {join}")).timed(started))
            }
        }
    }
}
