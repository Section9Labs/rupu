//! Shared fixtures for the orchestrator integration tests.

use std::path::Path;
use std::sync::Arc;

use rupu_orchestrator::runner::StepFactory;
use rupu_orchestrator::{DefaultStepFactory, Workflow};

/// The production step factory over a throwaway `RUPU_HOME` at `global`
/// (agents under `<global>/agents`, no credentials, the default config).
pub fn step_factory(global: &Path, workflow: Workflow) -> Arc<dyn StepFactory> {
    Arc::new(DefaultStepFactory {
        workflow,
        assembler: Arc::new(rupu_runtime::assembly::RunAssembler::new(
            rupu_runtime::assembly::AssemblyContext::minimal(global.to_path_buf()),
        )),
        mode: rupu_tools::PermissionMode::Bypass,
        system_prompt_suffix: None,
        dispatcher: None,
        scope_name_override: None,
    })
}
