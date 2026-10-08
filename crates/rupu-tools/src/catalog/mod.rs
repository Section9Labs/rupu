//! The tool catalog: the descriptor of every tool rupu defines, wherever its
//! body still lives. Builtins carry their descriptor next to their body in
//! this crate; the coverage/findings tools (bodies in `rupu-agent`) and the
//! agentiflow tools (bodies in `rupu-agentiflow`) carry theirs here, so names,
//! aliases and effects are final now and W4/W5 only move bodies.
//!
//! The MCP connector tools (`scm.*`, `issues.*`, …) are not listed: their
//! descriptors are built from `rupu-mcp`'s catalog until W4 moves them in.
//! A [`ToolCatalog`] value is this static list plus such extra descriptors
//! ([`ToolCatalog::with`]); grant resolution ([`crate::grant`]) runs over a
//! value, so `rupu-agent` hands it the connector descriptors (and a run's
//! injected tools) until they live here.

pub mod coverage;
pub mod findings;
pub mod flow;

use crate::descriptor::ToolDescriptor;

/// The unqualified core fs/shell tools (D11). Every other tool is
/// `namespace.verb`, apart from the legacy-named dispatch tools W7 folds into
/// `dispatch`.
pub const CORE_NAMES: &[&str] = &[
    "bash",
    "read_file",
    "write_file",
    "edit_file",
    "grep",
    "glob",
    "ast_grep",
];

/// Every descriptor in the catalog, in a stable order.
pub static ALL: &[&ToolDescriptor] = &[
    // core
    &crate::bash::DESCRIPTOR,
    &crate::read_file::DESCRIPTOR,
    &crate::write_file::DESCRIPTOR,
    &crate::edit_file::DESCRIPTOR,
    &crate::grep::DESCRIPTOR,
    &crate::glob::DESCRIPTOR,
    &crate::ast_grep::DESCRIPTOR,
    // sub-agent dispatch (W7 folds these into `dispatch`)
    &crate::dispatch_agent::DESCRIPTOR,
    &crate::dispatch_agents_parallel::DESCRIPTOR,
    // coverage ledger
    &coverage::COVERAGE_MARK,
    &coverage::COVERAGE_STATUS,
    &coverage::COVERAGE_REMAINING,
    &coverage::COVERAGE_CONCERNS_SEARCH,
    &coverage::COVERAGE_CONCERNS_DETAIL,
    // findings + assets
    &findings::FINDINGS_REPORT,
    &findings::FINDINGS_VERIFY,
    &findings::FINDINGS_QUERY,
    &findings::FINDINGS_TAG,
    &findings::ASSETS_MARK,
    // agentiflow
    &flow::BOARD_CLAIM,
    &flow::BOARD_RELEASE,
    &flow::BOARD_POST,
    &flow::BOARD_READ,
    &flow::BOARD_DIRECTIVE,
    &flow::BOARD_RETRACT,
    &flow::MSG_SEND,
    &flow::AGENTS_LIST,
    &flow::AGENTS_GET,
    &flow::WORKFLOWS_LIST,
    &flow::WORKFLOWS_GET,
    &flow::CATALOG_SEARCH,
    &flow::GOAL_STATUS,
    &flow::GOAL_COVERAGE,
    &flow::BUDGET_STATUS,
    &flow::DISPATCH,
    &flow::RUN_WORKFLOW,
    &flow::WORKFLOWS_GENERATE,
    &flow::JOIN,
];

/// The catalog's queries. The associated functions ([`Self::all`],
/// [`Self::get`], …) cover the static list; a value ([`Self::builtin`],
/// extended with [`Self::with`]) also holds descriptors defined outside this
/// crate, and is what names are resolved and grants computed against.
#[derive(Clone, Debug, Default)]
pub struct ToolCatalog {
    extra: Vec<&'static ToolDescriptor>,
}

impl ToolCatalog {
    /// The static catalog, with nothing added.
    pub fn builtin() -> Self {
        Self::default()
    }

    /// This catalog plus `extra`. A descriptor whose canonical name the
    /// catalog already holds is ignored (the first one listed wins).
    pub fn with(mut self, extra: impl IntoIterator<Item = &'static ToolDescriptor>) -> Self {
        for d in extra {
            if !self.descriptors().any(|have| have.name == d.name) {
                self.extra.push(d);
            }
        }
        self
    }

    /// Every descriptor this value holds: the static list, then the extras
    /// in the order they were added.
    pub fn descriptors(&self) -> impl Iterator<Item = &'static ToolDescriptor> + '_ {
        ALL.iter().copied().chain(self.extra.iter().copied())
    }

    /// The descriptor this value holds under the canonical name `name`.
    pub fn descriptor(&self, name: &str) -> Option<&'static ToolDescriptor> {
        self.descriptors().find(|d| d.name == name)
    }

    /// Every descriptor.
    pub fn all() -> &'static [&'static ToolDescriptor] {
        ALL
    }

    /// The unqualified core fs/shell tool names.
    pub fn core_names() -> &'static [&'static str] {
        CORE_NAMES
    }

    /// The descriptor whose canonical name is `name`.
    pub fn get(name: &str) -> Option<&'static ToolDescriptor> {
        ALL.iter().copied().find(|d| d.name == name)
    }

    /// Every name — canonical or alias — that is a rupu tool and can't be a
    /// shell program: the non-core tools' dotted (`findings.report`) and
    /// underscored (`report_finding`) names. `bash` refuses to run one of
    /// these as a command and points the model at the tool call instead.
    /// Core tools (`grep`, `glob`) and plain-word names (`join`, `dispatch`)
    /// are left out: they are, or may be, real commands.
    pub fn non_shell_names() -> impl Iterator<Item = &'static str> {
        ALL.iter()
            .filter(|d| !d.is_core())
            .flat_map(|d| std::iter::once(d.name).chain(d.aliases.iter().map(|a| a.name)))
            .filter(|n| n.contains('.') || n.contains('_'))
    }
}
