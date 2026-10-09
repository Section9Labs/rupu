//! Where a run's coverage, findings and asset records go: the ledger of its
//! findings scope in its workspace, mirrored to its run stream when it has
//! one. Every ledger tool derives its location here, from the call's
//! [`ToolContext`], so the agent loop, an `action:` step and `rupu mcp serve`
//! write the same bytes to the same place for the same identity.

use crate::tool::ToolContext;
use rupu_coverage::{target_id, CoveragePaths, RunStream, TagLog};

/// The run stream for records written under `scope`, when the run has one
/// (see [`crate::ToolServices::coverage_stream`]).
pub fn run_stream(ctx: &ToolContext, scope: &str) -> Option<RunStream> {
    ctx.services.coverage_stream.clone().map(|path| RunStream {
        path,
        scope_name: scope.to_string(),
    })
}

/// The coverage/findings ledger of the run's findings scope
/// ([`crate::RunIdentity::scope`]) in its workspace, with its run stream.
pub fn paths(ctx: &ToolContext) -> CoveragePaths {
    let scope = ctx.identity.scope();
    let workspace = &ctx.workspace.path;
    CoveragePaths::new(workspace, &target_id(workspace, scope))
        .with_run_stream(run_stream(ctx, scope))
}

/// The workspace-wide finding tag log, mirroring to the run stream when the
/// run has one.
pub fn tag_log(ctx: &ToolContext) -> TagLog {
    TagLog::for_workspace(&ctx.workspace.path)
        .with_run_stream(run_stream(ctx, ctx.identity.scope()))
}
