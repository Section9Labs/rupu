//! What every local workflow run is built on (spec 2026-10-07 W3, R12): the
//! run's assembler, the launcher its steps share (W7), the step factory, the
//! run's codename namer and what its `action:` steps call their tools with
//! (W4). One construction for `rupu workflow run`, its inline
//! approve-resume, and `rupu workflow resume` / reject cleanup.

use std::sync::Arc;

use rupu_runtime::argv::LaunchLink;
use rupu_runtime::assembly::{AssemblyContext, RunAssembler, WorkspaceBinding};
use rupu_tools::{Launcher, PermissionMode, WorkflowGenerator};

use crate::codenames::RunNaming;
use crate::executor::EventSink;
use crate::runner::ActionServices;
use crate::runs::RunStore;
use crate::step_factory::DefaultStepFactory;
use crate::workflow::Workflow;

/// What a workflow run's runtime is built from.
pub struct WorkflowRuntimeInputs {
    /// The run's config, SCM registry, findings base (with its engagement),
    /// capture backend and customer.
    pub context: AssemblyContext,
    pub workflow: Workflow,
    pub run_id: String,
    pub workspace: WorkspaceBinding,
    pub store: Arc<RunStore>,
    /// Where the launcher reports `DispatchStarted` / `DispatchCompleted`.
    pub events: Option<Arc<dyn EventSink>>,
    /// Builds the launcher the run's steps start children with
    /// (`rupu-launch`, which sits above this crate). `None` = the steps
    /// can't launch.
    pub launcher: Option<Arc<dyn LauncherFactory>>,
    /// The run is a process child of another run (`--launch-*`): every
    /// step carries the link (W7 D18).
    pub link: Option<LaunchLink>,
    pub mode: PermissionMode,
    /// The run's `## Run target` — recorded at launch, read back on resume
    /// (R8).
    pub system_prompt_suffix: Option<String>,
    /// An agentiflow-launched workflow unit's findings scope.
    pub scope_name_override: Option<String>,
    /// The codename the launching run minted for this run (`--codename`):
    /// the root its steps are named under. `None` = its own crew.
    pub codename: Option<rupu_codename::Codename>,
}

/// What [`LauncherFactory::for_workflow`] builds a workflow run's launcher
/// from.
pub struct WorkflowLauncherInputs<'a> {
    pub assembler: &'a Arc<RunAssembler>,
    pub store: &'a Arc<RunStore>,
    pub run_id: &'a str,
    pub workspace: &'a WorkspaceBinding,
    pub events: Option<Arc<dyn EventSink>>,
    /// The run's codename namer: children are `<step>><role>#n` from it.
    pub namer: rupu_codename::SharedNamer,
}

/// Builds the launcher (and workflow generator) a workflow run's steps
/// share. Implemented by `rupu-launch`, above this crate — the same port
/// shape as the catalog (W5 §3.1a).
pub trait LauncherFactory: Send + Sync {
    fn for_workflow(
        &self,
        inputs: WorkflowLauncherInputs<'_>,
    ) -> (Arc<dyn Launcher>, Arc<dyn WorkflowGenerator>);
}

/// Why a workflow run can't be built.
#[derive(Debug, thiserror::Error)]
pub enum WorkflowRuntimeError {
    /// A launched workflow (`--launch-*`) runs agents its parent may not
    /// dispatch. The parent vets this before it starts the child; the child
    /// checks again, against the workflow it actually loaded.
    #[error(
        "workflow '{workflow}' runs agents its launching run may not dispatch: {}",
        agents.join(", ")
    )]
    AgentsOutsideLaunchCap {
        workflow: String,
        agents: Vec<String>,
    },
}

/// A workflow run's assembler, launcher, step factory, namer and action
/// services.
pub struct WorkflowRuntime {
    pub assembler: Arc<RunAssembler>,
    pub launcher: Option<Arc<dyn Launcher>>,
    pub factory: Arc<DefaultStepFactory>,
    /// One codename namer for the whole run, shared by the orchestrator
    /// (static slots) and the launcher (`>role#n`), over the
    /// `<runs>/<run_id>` dir `run_workflow` uses, so both read and persist
    /// one `codenames.json`.
    pub naming: Arc<RunNaming>,
    /// What the run's `action:` steps (gate `notify:` hooks and `on_reject`
    /// cleanup steps too) call their catalog tool with.
    pub action_services: ActionServices,
}

impl WorkflowRuntime {
    pub fn new(inputs: WorkflowRuntimeInputs) -> Result<Self, WorkflowRuntimeError> {
        let WorkflowRuntimeInputs {
            context,
            workflow,
            run_id,
            workspace,
            store,
            events,
            launcher,
            link,
            mode,
            system_prompt_suffix,
            scope_name_override,
            codename,
        } = inputs;
        if let Some(link) = &link {
            let outside: Vec<String> = workflow
                .dispatched_agents()
                .into_iter()
                .filter(|a| !link.agents.contains(a))
                .collect();
            if !outside.is_empty() {
                return Err(WorkflowRuntimeError::AgentsOutsideLaunchCap {
                    workflow: workflow.name.clone(),
                    agents: outside,
                });
            }
        }
        // Every run of the workflow can read the agent/workflow catalog.
        let mut context = context;
        if context.catalog.is_none() {
            context.catalog = Some(Arc::new(crate::catalog::FsCatalog::for_project_root(
                &context.global,
                context.project_root.as_deref(),
            )));
        }
        let assembler = Arc::new(RunAssembler::new(context));
        let naming = Arc::new(RunNaming::open_under(
            &workflow,
            &run_id,
            Some(&store.root.join(&run_id)),
            codename,
        ));
        let mut action_services = action_services(
            &assembler,
            &workflow,
            &run_id,
            naming.crew(),
            &workspace,
            link.as_ref().map_or(mode, |l| mode.min(l.ceiling)),
            scope_name_override.as_deref(),
        );
        if let Some(link) = &link {
            action_services.context.identity_mut().launch.connector_cap = link.actions.clone();
        }
        let (launcher, workflow_generator) = match launcher {
            Some(f) => {
                let (l, g) = f.for_workflow(WorkflowLauncherInputs {
                    assembler: &assembler,
                    store: &store,
                    run_id: &run_id,
                    workspace: &workspace,
                    events,
                    namer: naming.namer(),
                });
                (Some(l), Some(g))
            }
            None => (None, None),
        };
        // A process child's ceiling caps every step (D7 across the boundary).
        let mode = link.as_ref().map_or(mode, |l| mode.min(l.ceiling));
        let factory = Arc::new(DefaultStepFactory {
            workflow,
            assembler: Arc::clone(&assembler),
            mode,
            system_prompt_suffix,
            launcher: launcher.clone(),
            workflow_generator,
            link,
            scope_name_override,
        });
        Ok(Self {
            assembler,
            launcher,
            factory,
            naming,
            action_services,
        })
    }
}

/// The action services of the workflow run `run_id`: its identity — the
/// run id, its root codename (an action step is no agent instance), the
/// configured default model/provider, the workflow surface, the findings
/// scope (the workflow's name unless an agentiflow unit pools it) — over the
/// assembler's call-site context (workspace, SCM registry, findings base,
/// customer). Each call's findings profile is the step's
/// (`Workflow::action_findings_profile`); the run default seeds it here.
fn action_services(
    assembler: &RunAssembler,
    workflow: &Workflow,
    run_id: &str,
    root: &rupu_codename::Codename,
    workspace: &WorkspaceBinding,
    mode: PermissionMode,
    scope_name_override: Option<&str>,
) -> ActionServices {
    let config = &assembler.context().config;
    let identity = rupu_tools::RunIdentity {
        run_id: run_id.to_string(),
        codename: Some(root.to_string()),
        model: config.default_model.clone().unwrap_or_default(),
        provider: config.default_provider.clone().unwrap_or_default(),
        surface: rupu_tools::Surface::Workflow,
        scope_name: Some(
            scope_name_override
                .map(str::to_string)
                .unwrap_or_else(|| workflow.name.clone()),
        ),
        ..Default::default()
    };
    // No subprocess runs here (`bash` is not action-eligible), and the
    // connector tools' HTTP is attributed by the SCM registry's own sink.
    let mut context =
        assembler.call_site_context(identity, workspace, Arc::new(rupu_netflow::NullSink));
    if let Some(findings) = context.services.findings.as_mut() {
        findings.profile =
            rupu_coverage::FindingProfile::resolve(None, workflow.defaults.findings_profile, None);
    }
    ActionServices { context, mode }
}
