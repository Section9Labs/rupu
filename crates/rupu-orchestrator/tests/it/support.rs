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

/// Action services over the SCM registry `reg`, deciding calls under `mode`.
pub fn action_services(
    reg: rupu_scm::Registry,
    mode: rupu_tools::PermissionMode,
) -> rupu_orchestrator::runner::ActionServices {
    let mut context = rupu_tools::ToolContext::default();
    context.services.scm = Some(Arc::new(reg));
    rupu_orchestrator::runner::ActionServices { context, mode }
}

/// Action services of the workflow run `run_id` recording findings in
/// `workspace` under scope `scope` (the run default profile, `full`), as the
/// CLI builds them minus the codename and provider.
pub fn findings_action_services(
    workspace: &Path,
    scope: &str,
    run_id: &str,
    model: &str,
) -> rupu_orchestrator::runner::ActionServices {
    let mut s = action_services(
        rupu_scm::Registry::empty(),
        rupu_tools::PermissionMode::Bypass,
    );
    let ctx = &mut s.context;
    ctx.workspace.path = workspace.to_path_buf();
    let id = ctx.identity_mut();
    id.run_id = run_id.into();
    id.model = model.into();
    id.surface = rupu_tools::Surface::Workflow;
    id.scope_name = Some(scope.into());
    ctx.services.findings = Some(rupu_coverage::FindingWriteOptions::default());
    s
}
