//! The hidden `--launch-*` flags a parent run passes a process child (spec
//! W7 D18): its depth, permission ceiling, parent and root, the root's usage
//! ledger, and the caps on what it may launch and which connector tools it
//! may call. Rendered by `rupu_runtime::argv::LaunchLink`.

use std::path::PathBuf;

use rupu_runtime::argv::LaunchLink;
use rupu_tools::PermissionMode;

/// A comma-separated list of names; empty is an empty list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names(pub Vec<String>);

fn parse_names(s: &str) -> Result<Names, String> {
    Ok(Names(
        s.split(',')
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect(),
    ))
}

fn parse_mode(s: &str) -> Result<PermissionMode, String> {
    PermissionMode::parse(s).map_err(|e| e.to_string())
}

#[derive(clap::Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchLinkArgs {
    /// This run's launch depth: its launching parent's + 1. Internal.
    #[arg(
        long = "launch-depth",
        hide = true,
        value_name = "N",
        requires_all = ["launch_ceiling", "parent_run", "root_run", "launch_agents", "launch_workflows"]
    )]
    pub launch_depth: Option<u32>,
    /// The parent's permission ceiling. Internal.
    #[arg(long = "launch-ceiling", hide = true, value_name = "MODE", value_parser = parse_mode, requires = "launch_depth")]
    pub launch_ceiling: Option<PermissionMode>,
    /// The run that launched this one. Internal.
    #[arg(
        long = "parent-run",
        hide = true,
        value_name = "RUN_ID",
        requires = "launch_depth"
    )]
    pub parent_run: Option<String>,
    /// The root of the launch tree. Internal.
    #[arg(
        long = "root-run",
        hide = true,
        value_name = "RUN_ID",
        requires = "launch_depth"
    )]
    pub root_run: Option<String>,
    /// The root's usage ledger, charged for this run. Internal.
    #[arg(
        long = "usage-root",
        hide = true,
        value_name = "PATH",
        requires = "launch_depth"
    )]
    pub usage_root: Option<PathBuf>,
    /// The agents the parent may launch: this run's cap. Internal.
    #[arg(long = "launch-agents", hide = true, value_name = "NAMES", value_parser = parse_names, requires = "launch_depth")]
    pub launch_agents: Option<Names>,
    /// The workflows the parent may launch: this run's cap. Internal.
    #[arg(long = "launch-workflows", hide = true, value_name = "NAMES", value_parser = parse_names, requires = "launch_depth")]
    pub launch_workflows: Option<Names>,
    /// The connector tools the parent was narrowed to. Internal.
    #[arg(long = "launch-actions", hide = true, value_name = "NAMES", value_parser = parse_names, requires = "launch_depth")]
    pub launch_actions: Option<Names>,
}

impl LaunchLinkArgs {
    /// The link these flags carry; `None` for a run nobody launched.
    pub fn link(&self) -> Option<LaunchLink> {
        let depth = self.launch_depth?;
        Some(LaunchLink {
            depth,
            // clap requires the rest with `--launch-depth`; a hand-built
            // value missing one fails closed.
            ceiling: self.launch_ceiling.unwrap_or(PermissionMode::Readonly),
            parent_run: self.parent_run.clone().unwrap_or_default(),
            root_run: self.root_run.clone().unwrap_or_default(),
            usage_root: self.usage_root.clone(),
            agents: self.launch_agents.clone().unwrap_or_default().0,
            workflows: self.launch_workflows.clone().unwrap_or_default().0,
            actions: self.launch_actions.clone().map(|n| n.0),
        })
    }
}
