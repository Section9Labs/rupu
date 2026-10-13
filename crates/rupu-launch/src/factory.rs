//! [`Launchers`]: what a launch site needs to give its runs a launcher — the
//! process backend, the runtime children run on, the concurrency cap and the
//! generation provider — and the orchestrator's [`LauncherFactory`] port over
//! it, so a workflow run's steps share one [`RunLauncher`].

use std::path::Path;
use std::sync::Arc;

use rupu_orchestrator::{LauncherFactory, RunStore, WorkflowLauncherInputs};
use rupu_runtime::assembly::RunAssembler;
use rupu_tools::{Launcher, WorkflowGenerator};

use crate::generate::Generation;
use crate::launcher::{LaunchEnv, RunLauncher, DEFAULT_MAX_CONCURRENT_CHILDREN};
use crate::process::{ProcessBackend, Subprocess};

/// See the module docs.
#[derive(Clone)]
pub struct Launchers {
    pub backend: Arc<dyn ProcessBackend>,
    pub runtime: tokio::runtime::Handle,
    pub max_concurrent: usize,
    pub generation: Option<Generation>,
}

impl Launchers {
    /// Process children are this binary (`rupu`) under `global` as
    /// `RUPU_HOME`, on the current tokio runtime, at most
    /// `[dispatch].max_concurrent_children` at once per parent.
    pub fn local(global: &Path, config: &rupu_config::Config) -> std::io::Result<Self> {
        Ok(Self {
            backend: Arc::new(Subprocess::new(std::env::current_exe()?, global)),
            runtime: tokio::runtime::Handle::current(),
            max_concurrent: config
                .dispatch
                .max_concurrent_children
                .unwrap_or(DEFAULT_MAX_CONCURRENT_CHILDREN),
            generation: None,
        })
    }

    /// A launcher for one root run's descendants.
    pub fn launcher(
        &self,
        assembler: Arc<RunAssembler>,
        store: Arc<RunStore>,
        events: Option<Arc<dyn rupu_orchestrator::executor::EventSink>>,
        namer: Option<rupu_codename::SharedNamer>,
    ) -> Arc<RunLauncher> {
        RunLauncher::new(LaunchEnv {
            assembler,
            store,
            events,
            namer,
            backend: Arc::clone(&self.backend),
            runtime: self.runtime.clone(),
            max_concurrent: self.max_concurrent,
            generation: self.generation.clone(),
            project: None,
        })
    }
}

impl LauncherFactory for Launchers {
    fn for_workflow(
        &self,
        inputs: WorkflowLauncherInputs<'_>,
    ) -> (Arc<dyn Launcher>, Arc<dyn WorkflowGenerator>) {
        let l = self.launcher(
            Arc::clone(inputs.assembler),
            Arc::clone(inputs.store),
            inputs.events,
            Some(inputs.namer),
        );
        (l.clone(), l)
    }
}
