//! The body of each catalog tool, built for one run from its
//! [`ToolContext`]. The agent loop builds its registry here, an `action:`
//! step its one tool and `rupu mcp serve` its listing: three callers, one
//! set of bodies (spec W4 §3.3).

use crate::tool::{Tool, ToolContext};
use crate::{
    assets, board, budget, coverage, findings, github, gitlab, goal, issues, msg, roster, scm,
    skills,
};
use crate::{
    AstGrepTool, BashTool, DispatchAgentTool, DispatchAgentsParallelTool, EditFileTool, GlobTool,
    GrepTool, ReadFileTool, WriteFileTool,
};
use std::sync::Arc;

/// The body of the catalog tool with canonical name `name` for the run `ctx`
/// describes, or `None` when this crate holds no body for it (the agentiflow
/// launch tools until W7) or the run lacks a service the tool can't run
/// without: a connector tool without an SCM registry, a coverage tool
/// without a concern catalog, `assets.mark` without an engagement, the
/// dispatch pair without a dispatcher, `board.*` / `msg.send` without a
/// message bus, `goal.*` / `budget.status` without a run status, the roster
/// without a catalog, `skills.load` without a granted skill the model may load.
/// [`crate::ToolServices::provided`] is the same accounting, so a grant
/// resolved over it never offers a tool this can't build.
pub fn body(name: &str, ctx: &ToolContext) -> Option<Arc<dyn Tool>> {
    let services = &ctx.services;
    let scm = services.scm.is_some();
    let coverage = services.coverage_catalog.is_some();
    let bus = services.message_bus.is_some();
    let status = services.run_status.is_some();
    let catalog = services.catalog.is_some();
    let skills = services.skills.as_ref().is_some_and(|s| s.has_loadable());
    let t: Arc<dyn Tool> = match name {
        // core
        "bash" => Arc::new(BashTool),
        "read_file" => Arc::new(ReadFileTool),
        "write_file" => Arc::new(WriteFileTool),
        "edit_file" => Arc::new(EditFileTool),
        "grep" => Arc::new(GrepTool),
        "glob" => Arc::new(GlobTool),
        "ast_grep" => Arc::new(AstGrepTool),
        // sub-agent dispatch
        "dispatch_agent" if services.dispatcher.is_some() => Arc::new(DispatchAgentTool),
        "dispatch_agents_parallel" if services.dispatcher.is_some() => {
            Arc::new(DispatchAgentsParallelTool)
        }
        // coverage ledger
        "coverage.mark" if coverage => Arc::new(coverage::CoverageMarkTool),
        "coverage.status" if coverage => Arc::new(coverage::CoverageStatusTool),
        "coverage.remaining" if coverage => Arc::new(coverage::CoverageRemainingTool),
        "coverage.concerns.search" if coverage => Arc::new(coverage::CoverageConcernsSearchTool),
        "coverage.concerns.detail" if coverage => Arc::new(coverage::CoverageConcernsDetailTool),
        // findings + assets
        "findings.report" => Arc::new(findings::report::FindingsReportTool::new(
            services.findings.clone().unwrap_or_default(),
        )),
        "findings.verify" => Arc::new(findings::verify::FindingsVerifyTool),
        "findings.query" => Arc::new(findings::query::FindingsQueryTool),
        "findings.tag" => Arc::new(findings::tag::FindingsTagTool),
        "assets.mark"
            if services
                .findings
                .as_ref()
                .is_some_and(|f| f.engagement.is_some()) =>
        {
            Arc::new(assets::mark::AssetsMarkTool)
        }
        // connectors
        "scm.repos.list" if scm => Arc::new(scm::repos::ReposListTool),
        "scm.repos.get" if scm => Arc::new(scm::repos::ReposGetTool),
        "scm.branches.list" if scm => Arc::new(scm::branches::BranchesListTool),
        "scm.branches.create" if scm => Arc::new(scm::branches::BranchesCreateTool),
        "scm.files.read" if scm => Arc::new(scm::files::FilesReadTool),
        "scm.prs.list" if scm => Arc::new(scm::prs::PrsListTool),
        "scm.prs.get" if scm => Arc::new(scm::prs::PrsGetTool),
        "scm.prs.diff" if scm => Arc::new(scm::prs::PrsDiffTool),
        "scm.prs.comment" if scm => Arc::new(scm::prs::PrsCommentTool),
        "scm.prs.create" if scm => Arc::new(scm::prs::PrsCreateTool),
        "issues.list" if scm => Arc::new(issues::IssuesListTool),
        "issues.get" if scm => Arc::new(issues::IssuesGetTool),
        "issues.comments" if scm => Arc::new(issues::IssuesCommentsTool),
        "issues.comment" if scm => Arc::new(issues::IssuesCommentTool),
        "issues.create" if scm => Arc::new(issues::IssuesCreateTool),
        "issues.update_state" if scm => Arc::new(issues::IssuesUpdateStateTool),
        "github.workflows_dispatch" if scm => Arc::new(github::WorkflowsDispatchTool),
        "gitlab.pipeline_trigger" if scm => Arc::new(gitlab::PipelineTriggerTool),
        // a flow's message bus
        "board.claim" if bus => Arc::new(board::BoardClaimTool),
        "board.release" if bus => Arc::new(board::BoardReleaseTool),
        "board.post" if bus => Arc::new(board::BoardPostTool),
        "board.read" if bus => Arc::new(board::BoardReadTool),
        "board.directive" if bus => Arc::new(board::BoardDirectiveTool),
        "board.retract" if bus => Arc::new(board::BoardRetractTool),
        "msg.send" if bus => Arc::new(msg::MsgSendTool),
        // the agent / workflow catalog
        "agents.list" if catalog => Arc::new(roster::AgentsListTool),
        "agents.get" if catalog => Arc::new(roster::AgentsGetTool),
        "workflows.list" if catalog => Arc::new(roster::WorkflowsListTool),
        "workflows.get" if catalog => Arc::new(roster::WorkflowsGetTool),
        "catalog.search" if catalog => Arc::new(roster::CatalogSearchTool),
        // a flow's live status
        "goal.status" if status => Arc::new(goal::GoalStatusTool),
        "goal.coverage" if status => Arc::new(goal::GoalCoverageTool),
        "budget.status" if status => Arc::new(budget::BudgetStatusTool),
        // a run's skills
        "skills.load" if skills => Arc::new(skills::SkillsLoadTool),
        _ => return None,
    };
    Some(t)
}
