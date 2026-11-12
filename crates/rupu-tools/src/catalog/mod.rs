//! The tool catalog: the descriptor of every tool rupu defines. Every tool
//! carries its descriptor next to its body in this crate ([`crate::bodies`]
//! builds the bodies).
//!
//! A [`ToolCatalog`] value adds static descriptors ([`ToolCatalog::with`])
//! and owned metadata/body bindings ([`ToolCatalog::with_owned`]) to that
//! list. Grant resolution retains the complete snapshot for dispatch.

use crate::descriptor::ToolDescriptor;
use crate::{BoundTool, CatalogError, ToolContext, ToolDefinition};
use std::sync::{Arc, OnceLock};

/// The unqualified core fs/shell tools (D11). Every other tool is
/// `namespace.verb`, apart from the launch pair `dispatch` / `join`.
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
    // child runs
    &crate::launch::dispatch::DESCRIPTOR,
    &crate::launch::join::DESCRIPTOR,
    &crate::launch::generate::DESCRIPTOR,
    // coverage ledger
    &crate::coverage::MARK,
    &crate::coverage::STATUS,
    &crate::coverage::REMAINING,
    &crate::coverage::CONCERNS_SEARCH,
    &crate::coverage::CONCERNS_DETAIL,
    // findings + assets
    &crate::findings::report::DESCRIPTOR,
    &crate::findings::verify::DESCRIPTOR,
    &crate::findings::query::DESCRIPTOR,
    &crate::findings::tag::DESCRIPTOR,
    &crate::assets::mark::DESCRIPTOR,
    // connectors
    &crate::scm::repos::LIST,
    &crate::scm::repos::GET,
    &crate::scm::branches::LIST,
    &crate::scm::branches::CREATE,
    &crate::scm::files::READ,
    &crate::scm::prs::LIST,
    &crate::scm::prs::GET,
    &crate::scm::prs::DIFF,
    &crate::scm::prs::COMMENT,
    &crate::scm::prs::CREATE,
    &crate::issues::LIST,
    &crate::issues::GET,
    &crate::issues::COMMENTS,
    &crate::issues::COMMENT,
    &crate::issues::CREATE,
    &crate::issues::UPDATE_STATE,
    &crate::github::WORKFLOWS_DISPATCH,
    &crate::gitlab::PIPELINE_TRIGGER,
    // a flow's message bus
    &crate::work_queue::POST,
    &crate::work_queue::LIST,
    &crate::work_queue::GET,
    &crate::work_queue::TIMELINE,
    &crate::work_queue::CLAIM,
    &crate::work_queue::START,
    &crate::work_queue::BLOCK,
    &crate::work_queue::RESUME,
    &crate::work_queue::COMPLETE,
    &crate::work_queue::FAIL,
    &crate::board::CLAIM,
    &crate::board::RELEASE,
    &crate::board::POST,
    &crate::board::READ,
    &crate::board::DIRECTIVE,
    &crate::board::RETRACT,
    &crate::msg::SEND,
    // the agent / workflow catalog
    &crate::roster::AGENTS_LIST,
    &crate::roster::AGENTS_GET,
    &crate::roster::WORKFLOWS_LIST,
    &crate::roster::WORKFLOWS_GET,
    &crate::roster::CATALOG_SEARCH,
    // a flow's live status
    &crate::goal::STATUS,
    &crate::goal::COVERAGE,
    &crate::budget::STATUS,
    // a run's skills
    &crate::skills::LOAD,
    &crate::session_reads::CONTEXT,
    &crate::session_reads::WORKFLOWS,
    &crate::session_reads::INSPECT,
    &crate::session_reads::NETFLOWS,
];

/// The catalog's queries. The associated functions ([`Self::all`],
/// [`Self::get`], …) cover the static list; a value ([`Self::builtin`],
/// extended with [`Self::with`]) also holds descriptors defined outside this
/// crate, and is what names are resolved and grants computed against.
#[derive(Clone, Debug, Default)]
pub struct ToolCatalog {
    extra: Vec<&'static ToolDescriptor>,
    owned: Vec<Arc<BoundTool>>,
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
            let shadows_owned = self.owned.iter().any(|have| {
                have.name() == d.name || d.aliases.iter().any(|alias| alias.name == have.name())
            });
            if !shadows_owned && !self.descriptors().any(|have| have.name == d.name) {
                self.extra.push(d);
            }
        }
        self
    }

    /// Add an owned tool without shadowing canonical names, aliases or remote identities.
    pub fn with_owned(mut self, tool: BoundTool) -> Result<Self, CatalogError> {
        let d = tool.descriptor();
        if self.definitions().any(|have| have.answers_to(&d.name)) {
            return Err(CatalogError::DuplicateName(d.name.clone()));
        }
        if let Some(id) = d.remote_identity() {
            if self
                .owned
                .iter()
                .any(|have| have.descriptor().remote_identity() == Some(id))
            {
                return Err(CatalogError::DuplicateIdentity {
                    server: id.server.clone(),
                    tool: id.tool.clone(),
                });
            }
        }
        self.owned.push(Arc::new(tool));
        Ok(self)
    }

    /// The complete immutable metadata snapshot, including owned tools.
    pub fn definitions(&self) -> impl Iterator<Item = Arc<ToolDefinition>> + '_ {
        static BUILTINS: OnceLock<Vec<Arc<ToolDefinition>>> = OnceLock::new();
        BUILTINS
            .get_or_init(|| ALL.iter().map(|d| ToolDefinition::builtin(d)).collect())
            .iter()
            .cloned()
            .chain(self.extra.iter().map(|d| ToolDefinition::builtin(d)))
            .chain(self.owned.iter().map(|t| t.definition().clone()))
    }

    pub fn definition(&self, name: &str) -> Option<Arc<ToolDefinition>> {
        self.definitions().find(|d| d.name == name)
    }

    /// Resolve the implementation from the same catalog that produced the grant.
    pub fn body(&self, name: &str, ctx: &ToolContext) -> Option<Arc<BoundTool>> {
        if let Some(tool) = self.owned.iter().find(|t| t.name() == name) {
            return Some(tool.clone());
        }
        let definition = self.definition(name)?;
        let tool = crate::bodies::body(name, ctx)?;
        Some(Arc::new(BoundTool::with_builtin(definition, tool)))
    }

    /// Static descriptors only: the builtin list, then static extras.
    /// Use `definitions` for a runtime snapshot that includes owned tools.
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
