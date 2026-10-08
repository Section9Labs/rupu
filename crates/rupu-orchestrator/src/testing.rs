//! Test support: step launches over mock providers.
//!
//! A test's [`StepFactory`](crate::StepFactory) describes the run it wants
//! with a [`MockRun`] — the agent, its scripted provider and the few settings
//! tests vary — and [`MockRun::launch`] turns it into the
//! [`StepLaunch`](crate::StepLaunch) the orchestrator assembles, like a
//! production step's: `Origin::WorkflowStep`, through a real
//! [`RunAssembler`]. Not used by production code.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use rupu_agent::{AgentSpec, MockProvider};
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::LlmProvider;
use rupu_runtime::assembly::{AssembleError, AssemblyContext, PreparedProvider, RunAssembler};
use rupu_tools::{AgentDispatcher, PermissionMode};

use crate::runner::{StepLaunch, StepRequest};

/// An assembler over a throwaway `RUPU_HOME` shared by the whole test
/// process (its netflow ledgers and usage ledgers land there).
pub fn scratch_assembler() -> Arc<RunAssembler> {
    static SCRATCH: OnceLock<Arc<RunAssembler>> = OnceLock::new();
    SCRATCH
        .get_or_init(|| {
            let global = std::env::temp_dir().join(format!(
                "rupu-orchestrator-test-home-{}",
                std::process::id()
            ));
            assembler_at(&global)
        })
        .clone()
}

/// An assembler over the `RUPU_HOME` at `global` (`<global>/runs` is where
/// a workflow run's usage ledger goes) with the default config.
pub fn assembler_at(global: &Path) -> Arc<RunAssembler> {
    Arc::new(RunAssembler::new(AssemblyContext::minimal(
        global.to_path_buf(),
    )))
}

/// A parsed agent named `name` with `system_prompt`.
pub fn agent(name: &str, system_prompt: &str) -> AgentSpec {
    AgentSpec::parse(&format!("---\nname: {name}\n---\n{system_prompt}\n"))
        .expect("test agent parses")
}

/// One step's run on a scripted provider, as a test describes it.
pub struct MockRun {
    pub agent_name: String,
    pub agent_system_prompt: String,
    /// The agent's `tools:`; `None` = the default grant.
    pub agent_tools: Option<Vec<String>>,
    pub dispatchable_agents: Option<Vec<String>>,
    pub provider: Box<dyn LlmProvider>,
    pub provider_name: String,
    pub model: String,
    pub max_turns: u32,
    /// Used as they are (no model-list lookup).
    pub limits: ModelLimits,
    pub concerns: Option<rupu_coverage::ConcernsBlock>,
    /// The step's `actions:`.
    pub step_actions: Vec<String>,
    /// The in-process sub-agent dispatcher the run is offered.
    pub dispatcher: Option<Arc<dyn AgentDispatcher>>,
    pub mode: PermissionMode,
    pub no_stream: bool,
    pub suppress_stream_stdout: bool,
    /// `None` = [`scratch_assembler`].
    pub assembler: Option<Arc<RunAssembler>>,
}

impl Default for MockRun {
    fn default() -> Self {
        Self {
            agent_name: "agent".into(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            dispatchable_agents: None,
            provider: Box::new(MockProvider::new(Vec::new())),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            max_turns: 5,
            limits: ModelLimits::unknown(),
            concerns: None,
            step_actions: Vec::new(),
            dispatcher: None,
            mode: PermissionMode::Bypass,
            no_stream: false,
            suppress_stream_stdout: false,
            assembler: None,
        }
    }
}

impl MockRun {
    /// The launch of `request` as this run.
    pub fn launch(self, request: StepRequest) -> Result<StepLaunch, AssembleError> {
        let mut agent = agent(&self.agent_name, &self.agent_system_prompt);
        agent.tools = self.agent_tools;
        agent.dispatchable_agents = self.dispatchable_agents;
        agent.max_turns = Some(self.max_turns);
        agent.concerns = self.concerns;
        let provider = PreparedProvider::injected(
            self.provider_name,
            self.model,
            self.provider,
            &agent,
            Some(self.limits),
        );
        let mut spec = request.into_spec(agent, self.step_actions, None, self.mode);
        spec.stream.no_stream = self.no_stream;
        spec.stream.suppress_stdout = self.suppress_stream_stdout;
        spec.services.provider = Some(provider);
        spec.services.dispatcher = self.dispatcher;
        Ok(StepLaunch {
            assembler: self.assembler.unwrap_or_else(scratch_assembler),
            spec,
        })
    }
}
