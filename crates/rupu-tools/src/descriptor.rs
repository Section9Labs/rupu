//! What a tool *is*: its name, legacy aliases, the effect it has on the world
//! and the services it needs. Every [`crate::Tool`] returns one
//! [`ToolDescriptor`]; permission ([`crate::permission::PermissionPolicy`])
//! is a pure function of the descriptor's [`Effect`] and the run's mode, and
//! never reads a tool's name.
//!
//! Spec: `docs/superpowers/specs/2026-10-07-rupu-tool-and-launch-architecture/W1-tool-catalog-and-permissions.md`.

use serde::Serialize;
use serde_json::Value;

/// What a tool does to the world. Exactly one per tool.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Observes only: files, ledgers, catalogs, status. (`read_file`, `grep`,
    /// `findings.query`)
    Read,
    /// Writes rupu's own run bookkeeping, never the user's workspace or the
    /// outside world. (`findings.report`, `coverage.mark`, `board.post`,
    /// `msg.send`)
    Record,
    /// Mutates the workspace or runs arbitrary code. (`write_file`,
    /// `edit_file`, `bash`)
    Write,
    /// Acts on a system outside this machine. (`scm.prs.create`,
    /// `issues.comment`, `github.workflows_dispatch`)
    External,
    /// Starts another run or a model call. The child inherits a permission
    /// ceiling. (`dispatch`, `workflows.generate`)
    Spawn,
}

impl Effect {
    /// The wire word (`read`, `record`, …), as serialized.
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::Record => "record",
            Effect::Write => "write",
            Effect::External => "external",
            Effect::Spawn => "spawn",
        }
    }
}

/// A service a tool reads from the run. W3 introduces the `ToolServices`
/// struct that carries them; W1 declares the enum so descriptors are final.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    Launcher,
    Scm,
    Findings,
    Coverage,
    MessageBus,
    RunStatus,
    Catalog,
    WorkflowGenerator,
    Netflow,
}

/// Where a legacy alias is accepted.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AliasScope {
    /// Accepted wherever the tool is offered.
    Everywhere,
    /// Accepted only for an agent loaded inside an agentiflow lead. The one
    /// case is `coverage.status`: agentiflow's goal-coverage tool used that
    /// name before it became `goal.coverage`, and outside a lead the name is
    /// the ledger tool's canonical one.
    FlowLead,
}

/// A legacy name a tool still answers to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Alias {
    pub name: &'static str,
    pub scope: AliasScope,
}

impl Alias {
    /// An alias accepted wherever the tool is offered.
    pub const fn any(name: &'static str) -> Self {
        Self {
            name,
            scope: AliasScope::Everywhere,
        }
    }

    /// An alias accepted only inside an agentiflow lead.
    pub const fn flow_lead(name: &'static str) -> Self {
        Self {
            name,
            scope: AliasScope::FlowLead,
        }
    }
}

/// One tool's declaration. Static data: a tool's identity never depends on
/// the run it is offered to.
#[derive(Debug)]
pub struct ToolDescriptor {
    /// Canonical name the model sees: `namespace.verb` (`findings.report`), or
    /// an unqualified core fs/shell name (`bash`, `read_file`, …).
    pub name: &'static str,
    /// Legacy names, accepted forever in `tools:` and in model calls.
    pub aliases: &'static [Alias],
    pub effect: Effect,
    pub needs: &'static [Service],
    pub description: &'static str,
    /// The input JSON Schema. A tool whose schema depends on its run (the
    /// findings profile picks `findings.report`'s) overrides
    /// [`crate::Tool::input_schema`]; this is then its default.
    pub input_schema: fn() -> Value,
}

impl ToolDescriptor {
    /// True when `name` is this tool's canonical name or one of its aliases
    /// (any scope).
    pub fn answers_to(&self, name: &str) -> bool {
        self.name == name || self.aliases.iter().any(|a| a.name == name)
    }

    /// True for the unqualified core fs/shell tools (D11).
    pub fn is_core(&self) -> bool {
        crate::catalog::CORE_NAMES.contains(&self.name)
    }
}
