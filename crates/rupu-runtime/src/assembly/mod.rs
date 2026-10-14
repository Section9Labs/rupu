//! The run assembler (spec 2026-10-07 W3, P5/P6, D13/D14).
//!
//! A launch site says *what* it wants — an [`Origin`], an identity, a prompt,
//! a workspace ([`LaunchSpec`]). [`RunAssembler`] derives everything else
//! once: provider and its config, model limits, recovery, permission, the
//! tool grant over the services the run provides, findings options, netflow,
//! `[bash]`, the usage ledger, and the frontmatter pins. [`run_agent`] is the
//! one way a production run starts: assemble → admission → the agent loop.
//!
//! Adding a launch site = adding an [`Origin`] variant and its row in
//! [`defaults_for`]. Building `AgentRunOpts` by hand outside this module is
//! refused by the `no_hand_built_run_opts` test.

mod messaging;
mod origin;
mod pins;
mod spec;

pub use origin::{
    defaults_for, CoverageStream, NetflowStreamRoot, Origin, OriginDefaults, ParentRun, UnitKey,
    UsageRoot,
};
pub use messaging::{MessagingGrant, OffReason};
pub use pins::PinRule;
pub use spec::{LaunchServices, LaunchSpec, Overrides, WorkspaceBinding};

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use rupu_agent::{AgentRunOpts, Hooks, OnUsageCallback, RunExit};
use rupu_tools::{
    BashConfig, GrantError, LaunchPool, LaunchScope, ParentLink, PermissionMode, PermissionPolicy,
    RunIdentity, ToolContext, ToolServices, WorkspaceScope,
};

use crate::hop_builder::{recovery_opts, AgentOverrides};
use crate::model_limits::LimitsContext;
use crate::provider_factory::{
    self, AgentProviderSettings, FactoryError, OpenAiCompatibleParams, ProviderConfig,
};
use crate::usage_ledger::UsageLedger;

/// What a [`RunAssembler`] is built from: the config a process (or one root
/// run) runs under and the resources its runs share.
pub struct AssemblyContext {
    /// `RUPU_HOME`.
    pub global: PathBuf,
    /// The project the runs belong to (its `.rupu/agents`, its netflow
    /// directory).
    pub project_root: Option<PathBuf>,
    /// The layered config (global → customer → project).
    pub config: rupu_config::Config,
    /// The customer the config was layered with; recorded on every run.
    pub customer: Option<String>,
    pub resolver: Arc<rupu_auth::KeychainResolver>,
    /// The SCM/issue registry the root's runs share. `None` = no connector
    /// tools.
    pub scm: Option<Arc<rupu_scm::Registry>>,
    /// The `[findings]` base options plus the root's engagement; each run
    /// resolves its own profile on top.
    pub findings: rupu_coverage::FindingWriteOptions,
    /// The process-wide subprocess-capture backend.
    pub net_capture: Option<Arc<dyn rupu_netflow::SubprocessCapture>>,
    /// The agents and workflows the runs can draw on (`agents.*`,
    /// `workflows.list` / `workflows.get`, `catalog.search`): the site's
    /// `rupu_orchestrator::FsCatalog`. `None` = no `Service::Catalog`, so an
    /// agent naming a roster tool gets a `tool_unavailable` notice.
    pub catalog: Option<Arc<dyn rupu_tools::CatalogPort>>,
}

impl AssemblyContext {
    /// A context over the `RUPU_HOME` at `global` with the default config,
    /// its auth file, no SCM registry, no customer and the default findings
    /// options — a test's, or a site's starting point before it fills in
    /// what it has.
    pub fn minimal(global: PathBuf) -> Self {
        Self {
            resolver: Arc::new(rupu_auth::KeychainResolver::for_home(&global)),
            global,
            project_root: None,
            config: rupu_config::Config::default(),
            customer: None,
            scm: None,
            findings: rupu_coverage::FindingWriteOptions::default(),
            net_capture: None,
            catalog: None,
        }
    }
}

/// Why a run could not be assembled.
#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    /// The agent file is missing or does not parse. Raised by the site that
    /// loads it (a workflow step's factory), before there is a spec to
    /// assemble.
    #[error("agent `{agent}` not found or failed to load: {message}")]
    AgentLoad { agent: String, message: String },
    #[error(
        "provider '{0}' is not a built-in provider, is not a declared account (no \
         [providers.{0}] with a vendor `kind` — declare one with `rupu auth login --account {0} \
         --kind <vendor>`), and is not declared as [providers.{0}] with kind = \
         \"openai-compatible\" and a base_url in config.toml"
    )]
    UnknownProvider(String),
    #[error("build provider {provider} ({model}): {source}")]
    ProviderBuild {
        provider: String,
        model: String,
        #[source]
        source: FactoryError,
    },
    #[error("tool grant: {0}")]
    Grant(#[from] GrantError),
    /// The run's skills (the agent's and the launch's) don't resolve, or
    /// the skills directories can't be read. Typed, so an fd-exhaustion
    /// behind it stays classifiable. `looked_in` is the layers searched, so
    /// an unknown skill's message names where to install it.
    #[error("skills: {source}{}", source.install_hint(looked_in))]
    Skills {
        source: rupu_agent::SkillCheckError,
        looked_in: rupu_skills::LayerDirs,
    },
}

impl rupu_agent::fd_budget::FdExhaustion for AssembleError {
    fn is_fd_exhaustion(&self) -> bool {
        match self {
            AssembleError::Skills { source, .. } => source.is_fd_exhaustion(),
            _ => false,
        }
    }
}

impl AssembleError {
    /// The run failure this assembly error is, for a site that records it
    /// the way it records a failed run (`rupu run`'s `run.json`, a session
    /// turn).
    pub fn into_run_error(self) -> rupu_agent::RunError {
        match self {
            AssembleError::Grant(e) => rupu_agent::RunError::ToolGrant(e.to_string()),
            AssembleError::Skills { .. } => rupu_agent::RunError::Preflight(self.to_string()),
            other => rupu_agent::RunError::Provider(other.to_string()),
        }
    }

    /// Record the run this error stopped as a failed run, and return its
    /// failure (W3 §3.6 — what the workflow step's error-stub providers used
    /// to produce by failing the run's first call). A failed grant already
    /// left its empty transcript ([`RunAssembler::assemble`]); everything
    /// else writes the unstarted run's transcript
    /// ([`rupu_agent::record_unstarted_run`]):
    ///
    /// - a missing agent, verbatim, with where it was looked for;
    /// - an undeclared provider, or skills that don't resolve, as its
    ///   config error;
    /// - a provider that did not build, as the auth error a first call on
    ///   it raised, with the `rupu auth login` hint.
    pub fn record_unstarted(self, run: rupu_agent::UnstartedRun<'_>) -> rupu_agent::RunError {
        use rupu_providers::ProviderError;
        let failure = match self {
            AssembleError::Grant(_) => return self.into_run_error(),
            AssembleError::AgentLoad { .. } => ProviderError::Preflight(format!(
                "{self}\n  Checked the project agents dir (.rupu/agents/) and the global agents dir."
            )),
            AssembleError::UnknownProvider(_) | AssembleError::Skills { .. } => {
                ProviderError::Preflight(self.to_string())
            }
            AssembleError::ProviderBuild {
                provider, source, ..
            } => ProviderError::AuthConfig(format!(
                "{provider}: {source}\n  Run: rupu auth login --provider {provider} --mode <api-key|sso>"
            )),
        };
        rupu_agent::record_unstarted_run(run, failure)
    }
}

/// A run's provider, built ahead of its assembly ([`RunAssembler::prepare_provider`]):
/// `rupu run` builds it before cloning a target repo, so a missing
/// credential fails the command before anything is written.
pub struct PreparedProvider {
    pub provider_name: String,
    pub model: String,
    provider: Box<dyn rupu_providers::LlmProvider>,
    pin: PinRule,
    /// Limits known ahead, used as they are ([`Self::injected`]).
    limits: Option<rupu_providers::model_limits::ModelLimits>,
}

impl PreparedProvider {
    /// A provider the site built itself — a test's mock provider — under
    /// `provider_name` / `model`, with `limits` used as they are (`None` =
    /// resolved through the provider like any run's). The agent's pins
    /// apply as for its own provider and model. Production sites build
    /// theirs with [`RunAssembler::prepare_provider`].
    pub fn injected(
        provider_name: impl Into<String>,
        model: impl Into<String>,
        provider: Box<dyn rupu_providers::LlmProvider>,
        agent: &rupu_agent::AgentSpec,
        limits: Option<rupu_providers::model_limits::ModelLimits>,
    ) -> Self {
        Self {
            provider_name: provider_name.into(),
            model: model.into(),
            provider,
            pin: PinRule::for_run(agent, false, false),
            limits,
        }
    }
}

/// A run ready to start: its options, and what the assembler opened for it.
pub struct AssembledRun {
    pub opts: AgentRunOpts,
    /// The run's netflow capture, when the assembler built the sink:
    /// finished when the run ends.
    netflow: Option<crate::netflow::RunNetflow>,
    /// The in-process parent this run borrows an admission slot from.
    parent_run_id: Option<String>,
    /// The run goes through admission ([`OriginDefaults::admission`]).
    admission: bool,
    /// The ledger the run's usage hook appends to, for the record.
    pub usage_ledger: Option<PathBuf>,
}

impl AssembledRun {
    /// Who the run is.
    pub fn identity(&self) -> &RunIdentity {
        self.opts.identity()
    }

    /// Take an admission slot (lending from an in-process parent) and run
    /// the agent loop; finish the run's netflow capture at the end.
    pub async fn run(self) -> RunExit {
        self.admit().await.run().await
    }

    /// Take the run's admission slot (lending from an in-process parent),
    /// waiting for one when the machine is at its ceiling. The returned run
    /// says how long it waited ([`AdmittedRun::permit`]).
    pub async fn admit(self) -> AdmittedRun {
        let AssembledRun {
            opts,
            netflow,
            parent_run_id,
            admission,
            ..
        } = self;
        let permit = if admission {
            Some(
                crate::admission::acquire_run(&opts.identity().run_id, parent_run_id.as_deref())
                    .await,
            )
        } else {
            None
        };
        if let Some(permit) = permit.as_ref().filter(|p| p.throttled()) {
            tracing::info!(
                run_id = %opts.identity().run_id,
                waited_for_slot_ms = permit.waited_for_slot.as_millis() as u64,
                waited_for_memory_ms = permit.waited_for_memory.as_millis() as u64,
                memory_timed_out = permit.memory_timed_out,
                "admission throttled the run"
            );
        }
        AdmittedRun {
            opts,
            netflow,
            permit,
        }
    }
}

/// An assembled run holding its admission slot ([`AssembledRun::admit`]).
pub struct AdmittedRun {
    opts: AgentRunOpts,
    netflow: Option<crate::netflow::RunNetflow>,
    permit: Option<crate::admission::JobPermit>,
}

impl AdmittedRun {
    /// The slot it holds — how long it waited for it. `None` for an origin
    /// that skips admission.
    pub fn permit(&self) -> Option<&crate::admission::JobPermit> {
        self.permit.as_ref()
    }

    /// Run the agent loop; release the slot and finish the run's netflow
    /// capture at the end.
    pub async fn run(self) -> RunExit {
        let AdmittedRun {
            opts,
            netflow,
            permit,
        } = self;
        let exit = rupu_agent::run_agent_full(opts).await;
        drop(permit);
        if let Some(n) = netflow {
            n.finish().await;
        }
        exit
    }
}

/// The one entry point for running an agent: assemble `spec`, take an
/// admission slot, run the loop (W3 §3.1).
pub async fn run_agent(
    assembler: &RunAssembler,
    spec: LaunchSpec,
) -> Result<RunExit, AssembleError> {
    Ok(assembler.assemble(spec).await?.run().await)
}

/// Turns [`LaunchSpec`]s into runs. Built once per process (or per root
/// run); caches the config-derived maps.
pub struct RunAssembler {
    ctx: AssemblyContext,
    openai_compatible: HashMap<String, OpenAiCompatibleParams>,
    limits_ctx: LimitsContext,
    bash: BashConfig,
}

/// A run's `[bash]` settings from the layered config.
fn bash_config(config: &rupu_config::Config) -> BashConfig {
    BashConfig {
        env_allowlist: config.bash.env_allowlist.clone().unwrap_or_default(),
        timeout_secs: config
            .bash
            .timeout_secs
            .unwrap_or(BashConfig::DEFAULT_TIMEOUT_SECS),
    }
}

impl RunAssembler {
    pub fn new(ctx: AssemblyContext) -> Self {
        let openai_compatible = provider_factory::openai_compatible_map(&ctx.config.providers);
        let limits_ctx = LimitsContext::from_config(&ctx.config, &ctx.global);
        let bash = bash_config(&ctx.config);
        Self {
            ctx,
            openai_compatible,
            limits_ctx,
            bash,
        }
    }

    pub fn context(&self) -> &AssemblyContext {
        &self.ctx
    }

    /// The tool context of a caller that is not an agent run (spec W4
    /// §3.3): an `action:` workflow step or `rupu mcp serve`. `identity` is
    /// the caller's (an action step's workflow run, an MCP server session);
    /// the workspace carries the config's `[bash]`, and the services are what
    /// such a caller provides — the context's SCM registry, its findings base
    /// options and its customer. No launcher, coverage or prompter: there
    /// is no agent loop around these calls. `netflow_sink` is where a
    /// subprocess's flows go (with the context's capture backend); a caller
    /// with no run ledger to attribute them to passes
    /// `rupu_netflow::NullSink` (see `crate::netflow`).
    pub fn call_site_context(
        &self,
        identity: RunIdentity,
        workspace: &WorkspaceBinding,
        netflow_sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> ToolContext {
        ToolContext {
            identity: Arc::new(identity),
            workspace: WorkspaceScope {
                id: workspace.id.clone(),
                path: workspace.path.clone(),
                bash: self.bash.clone(),
                read_roots: Vec::new(),
            },
            services: ToolServices {
                scm: self.ctx.scm.clone(),
                findings: Some(self.ctx.findings.clone()),
                netflow_sink: Some(netflow_sink),
                net_capture: self.ctx.net_capture.clone(),
                customer: self.ctx.customer.clone(),
                ..Default::default()
            },
            call: Default::default(),
        }
    }

    /// This assembler with `findings` as its runs' base findings options —
    /// for a site that can only resolve its engagement once its workspace
    /// exists (`rupu run` clones it after building the provider).
    pub fn with_findings(mut self, findings: rupu_coverage::FindingWriteOptions) -> Self {
        self.ctx.findings = findings;
        self
    }

    fn providers(&self) -> &BTreeMap<String, rupu_config::ProviderConfig> {
        &self.ctx.config.providers
    }

    /// The provider and model a run of `agent` resolves to, with `overrides`.
    pub fn resolve_provider_model(
        &self,
        agent: &rupu_agent::AgentSpec,
        overrides: &Overrides,
    ) -> (String, String) {
        let cfg = &self.ctx.config;
        let provider = provider_factory::resolve_provider_name(
            overrides.provider.as_deref().or(agent.provider.as_deref()),
            cfg.default_provider.as_deref(),
        );
        let model = provider_factory::resolve_model(
            overrides.model.as_deref().or(agent.model.as_deref()),
            cfg.default_model.as_deref(),
            self.openai_compatible
                .get(&provider)
                .map(|p| p.default_model.as_str()),
        );
        (provider, model)
    }

    /// The provider and model the run `spec` will run on: the provider it
    /// was handed, else what it resolves to ([`Self::resolve_provider_model`]).
    pub fn provider_model_of(&self, spec: &LaunchSpec) -> (String, String) {
        match &spec.services.provider {
            Some(p) => (p.provider_name.clone(), p.model.clone()),
            None => self.resolve_provider_model(&spec.agent, &spec.overrides),
        }
    }

    /// The usage ledger and coverage stream of the run `run_id` of
    /// `origin`, per its row in [`defaults_for`].
    pub(crate) fn root_ledger_and_stream(
        &self,
        origin: &Origin,
        run_id: &str,
    ) -> (Option<UsageLedger>, Option<PathBuf>) {
        let d = defaults_for(origin);
        let runs = self.ctx.global.join("runs");
        let parent = match origin {
            Origin::SubAgent { parent } => Some(parent),
            _ => None,
        };
        let ledger = match &d.usage {
            UsageRoot::OwnRun => Some(UsageLedger::open(runs.join(run_id).join("usage.jsonl"))),
            // An in-memory workflow run (no run store, so no id) keeps none.
            UsageRoot::WorkflowRun(id) if id.is_empty() => None,
            UsageRoot::WorkflowRun(id) => {
                Some(UsageLedger::open(runs.join(id).join("usage.jsonl")))
            }
            UsageRoot::Parent => parent.and_then(|p| p.usage.clone()),
            UsageRoot::Dir(dir) => Some(UsageLedger::open(dir.join("usage.jsonl"))),
        };
        let stream = match d.coverage_stream {
            CoverageStream::None => None,
            CoverageStream::Own => Some(rupu_coverage::stream_path(&runs, run_id)),
            CoverageStream::Parent => parent.and_then(|p| p.coverage_stream.clone()),
        };
        (ledger, stream)
    }

    /// Resolve and build the provider a run of `agent` uses: overrides,
    /// else the agent, else config; the agent's pins as they apply to it;
    /// its config through [`ProviderConfig::for_agent`]; its requests into
    /// `sink` (the run's netflow sink).
    pub async fn prepare_provider(
        &self,
        agent: &rupu_agent::AgentSpec,
        overrides: &Overrides,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Result<PreparedProvider, AssembleError> {
        let (provider_name, model) = self.resolve_provider_model(agent, overrides);
        if !provider_factory::is_dispatchable_provider(&provider_name, self.providers()) {
            return Err(AssembleError::UnknownProvider(provider_name));
        }
        let (agent_provider, agent_model) =
            self.resolve_provider_model(agent, &Overrides::default());
        let pin = PinRule::for_run(agent, provider_name != agent_provider, model != agent_model);
        let provider_config = ProviderConfig::for_agent(
            &provider_name,
            self.providers(),
            AgentProviderSettings {
                oauth_prefix: agent.anthropic_oauth_prefix,
                prompt_cache: agent.anthropic_prompt_cache,
            },
            Some(self.ctx.config.recovery.server_side_fallback),
        );
        let built = provider_factory::build_for_provider_with_config(
            &provider_name,
            &model,
            pin.auth,
            self.ctx.resolver.as_ref(),
            &provider_config,
            sink,
        )
        .await;
        match built {
            Ok((_mode, provider)) => Ok(PreparedProvider {
                provider_name,
                model,
                provider,
                pin,
                limits: None,
            }),
            Err(source) => Err(AssembleError::ProviderBuild {
                provider: provider_name,
                model,
                source,
            }),
        }
    }

    /// The skills a run of `agent` with `overrides` is given (spec §2.4),
    /// resolved against this assembler's layers by [`resolve_agent_skills`]
    /// with the launch's own entries as reason `launch`. [`Self::assemble`]
    /// calls it unless the launch brings its own ([`LaunchServices::skills`]).
    pub async fn resolve_skills(
        &self,
        agent: &rupu_agent::AgentSpec,
        overrides: &Overrides,
    ) -> Result<Option<Arc<rupu_skills::ResolvedSkills>>, AssembleError> {
        let dirs = rupu_skills::LayerDirs::for_layers(
            &self.ctx.global,
            self.ctx
                .project_root
                .as_ref()
                .map(|p| p.join(".rupu"))
                .as_deref(),
        )
        .with_overlay(overrides.skill_overlay.clone());
        resolve_agent_skills(&dirs, agent, overrides, rupu_skills::SkillReason::Launch).await
    }

    /// [`Self::resolve_skills`] for the run whose transcript is
    /// `transcript_path`, recording a failure as [`Self::assemble`] does: as
    /// when its grant fails, the run's (empty) transcript exists, so whatever
    /// opens it finds the run that failed. A site that resolves a launch's
    /// skills before assembling it (`rupu run`, before a clone) calls this
    /// and hands the result to the launch ([`LaunchServices::skills`]).
    pub async fn resolve_skills_for_launch(
        &self,
        agent: &rupu_agent::AgentSpec,
        overrides: &Overrides,
        transcript_path: &std::path::Path,
    ) -> Result<Option<Arc<rupu_skills::ResolvedSkills>>, AssembleError> {
        let resolved = self.resolve_skills(agent, overrides).await;
        if resolved.is_err() {
            if let Err(w) = rupu_transcript::JsonlWriter::create(transcript_path) {
                tracing::warn!(error = %w, "create the transcript of a run whose skills failed");
            }
        }
        resolved
    }

    /// Build the run `spec` describes.
    pub async fn assemble(&self, mut spec: LaunchSpec) -> Result<AssembledRun, AssembleError> {
        let d = defaults_for(&spec.origin);
        let cfg = &self.ctx.config;
        let messaging = MessagingGrant::resolve(&spec, d.origin_grant.as_ref());
        if let MessagingGrant::Off(why) = &messaging {
            tracing::debug!(run = %spec.run_id, ?why, "messaging off");
        }

        // Before anything that needs cleaning up.
        let skills = match spec.services.skills.take() {
            Some(skills) => (!skills.is_empty()).then_some(skills),
            None => {
                self.resolve_skills_for_launch(&spec.agent, &spec.overrides, &spec.transcript_path)
                    .await?
            }
        };

        // The coordinator-launched root's netflow stream this run tees into
        // (spec 2026-10-14 remote netflow transport §N1), when that stream
        // has been begun: a process child's link names its root; else the
        // origin's own (`Standalone`), its workflow run's or its in-process
        // parent's. A site that built the sink itself owns that decision,
        // but the stream is still recorded for the run's children.
        let netflow_stream = self.netflow_stream(&spec, &d);
        // The run's netflow sink — built before the provider, which takes it.
        let (sink, netflow) = match spec.services.netflow.clone() {
            Some(sink) => (sink, None),
            None => {
                let netflow = crate::netflow::for_run(
                    &self.ctx.global,
                    self.ctx.project_root.as_deref(),
                    &spec.run_id,
                    &spec.transcript_path,
                    netflow_stream.clone(),
                );
                (netflow.sink.clone(), Some(netflow))
            }
        };
        let prepared = match spec.services.provider.take() {
            Some(p) => Ok(p),
            None => {
                self.prepare_provider(&spec.agent, &spec.overrides, sink.clone())
                    .await
            }
        };
        let PreparedProvider {
            provider_name,
            model,
            mut provider,
            pin: pins,
            limits: known_limits,
        } = match prepared {
            Ok(p) => p,
            Err(e) => {
                if let Some(n) = netflow {
                    n.finish().await;
                }
                return Err(e);
            }
        };
        let agent = &spec.agent;
        let settings = AgentProviderSettings {
            oauth_prefix: agent.anthropic_oauth_prefix,
            prompt_cache: agent.anthropic_prompt_cache,
        };

        // Limits: an injected provider's as they are, else a session's (or
        // the lead's previous round's) stored ones unless unresolved, else
        // resolved through the provider (agent pin → config → live model
        // list → unknown).
        let limits =
            match known_limits.or(spec.overrides.limits.clone().filter(|l| !l.is_unresolved())) {
                Some(l) => rupu_providers::model_limits::ModelLimits { note: None, ..l },
                None => {
                    crate::model_limits::resolve(
                        pins.limits,
                        &provider_name,
                        &model,
                        provider.as_mut(),
                        &self.limits_ctx,
                    )
                    .await
                }
            };
        let recovery = recovery_opts(
            &cfg.recovery,
            agent.fallbacks.as_deref(),
            self.ctx.resolver.clone(),
            cfg.providers.clone(),
            self.limits_ctx.clone(),
            sink.clone(),
            AgentOverrides {
                oauth_prefix: settings.oauth_prefix,
                prompt_cache: settings.prompt_cache,
                auth: pins.auth,
                origin_provider: provider_name.clone(),
            },
        );

        // Who launched this run: an in-process parent, or one across a
        // process boundary (`--launch-*`, D18).
        let (parent, depth) = match (&spec.origin, &spec.link) {
            (Origin::SubAgent { parent }, _) => (
                Some(ParentLink {
                    run_id: parent.identity.run_id.clone(),
                    codename: parent.identity.codename.clone(),
                    root_run_id: parent
                        .identity
                        .parent
                        .as_ref()
                        .map(|p| p.root_run_id.clone())
                        .unwrap_or_else(|| parent.identity.run_id.clone()),
                }),
                parent.identity.depth + 1,
            ),
            (_, Some(link)) => (
                Some(ParentLink {
                    run_id: link.parent_run.clone(),
                    codename: spec
                        .codename
                        .as_deref()
                        .and_then(|c| c.rsplit_once('>'))
                        .map(|(p, _)| p.to_string()),
                    root_run_id: link.root_run.clone(),
                }),
                link.depth,
            ),
            _ => (None, 0),
        };

        let (ledger, coverage_stream) = self.root_ledger_and_stream(&spec.origin, &spec.run_id);
        let (launch, connector_cap) = self.launch_scope(
            &spec,
            ledger.as_ref().map(|l| l.path().to_path_buf()),
            !messaging.takes_part(),
        );
        // A process child (or a run under one) also copies each call to the
        // launch tree's root ledger, for a budget over the tree (L5); its own
        // ledger stays what every report reads.
        let rollup = launch.rollup_root.clone().map(UsageLedger::open);
        let identity = RunIdentity {
            run_id: spec.run_id.clone(),
            codename: spec.codename.clone(),
            agent: agent.name.clone(),
            provider: provider_name.clone(),
            model: model.clone(),
            depth,
            parent,
            surface: d.surface,
            scope_name: d.scope_name.clone(),
            step_id: d.step_id.clone(),
            launch,
            launch_skills: spec.overrides.skills.clone(),
            launch_preload_skills: spec.overrides.preload_skills.clone(),
            launch_args: spec.overrides.launch_args.clone(),
        };

        let ceiling = spec.ceiling.or(spec.link.as_ref().map(|l| l.ceiling));
        let permission = match ceiling {
            Some(ceiling) => {
                PermissionPolicy::for_child(ceiling, declared_mode(agent), spec.prompter.clone())
            }
            None => PermissionPolicy::new(spec.mode, spec.prompter.clone()),
        };

        let message_bus = self.message_space(&spec);
        let findings =
            self.ctx
                .findings
                .clone()
                .with_profile(rupu_coverage::FindingProfile::resolve(
                    spec.overrides.findings_profile,
                    spec.overrides.findings_default,
                    agent.findings_profile,
                ));
        let mut tool_context = ToolContext {
            identity: Arc::new(identity),
            workspace: WorkspaceScope {
                id: spec.workspace.id.clone(),
                path: spec.workspace.path.clone(),
                bash: self.bash.clone(),
                read_roots: skills.as_ref().map(|s| s.read_roots()).unwrap_or_default(),
            },
            services: ToolServices {
                launcher: spec.services.launcher.clone().filter(|_| d.launcher),
                workflow_generator: spec
                    .services
                    .workflow_generator
                    .clone()
                    .filter(|_| d.launcher),
                scm: self.ctx.scm.clone().filter(|_| d.scm),
                findings: Some(findings),
                // The agent loop sets the writer and the concern catalog it
                // owns.
                coverage_writer: None,
                coverage_catalog: None,
                coverage_stream: coverage_stream.clone(),
                netflow_stream,
                netflow_sink: Some(sink),
                net_capture: self.ctx.net_capture.clone(),
                customer: self.ctx.customer.clone(),
                // The agent loop sets it from the permission policy.
                prompter: None,
                message_bus: message_bus.clone().filter(|_| messaging.takes_part()),
                message_space: message_bus.clone(),
                run_status: spec.services.run_status.clone(),
                catalog: spec
                    .services
                    .catalog
                    .clone()
                    .or_else(|| self.ctx.catalog.clone())
                    .filter(|_| d.catalog),
                skills: skills.clone(),
            },
            call: Default::default(),
        };

        let concerns = spec
            .overrides
            .concerns
            .clone()
            .or(agent.concerns.clone())
            .map(|block| rupu_agent::RunConcerns {
                block,
                full_mode_max_concerns: self
                    .ctx
                    .config
                    .coverage
                    .full_mode_max_concerns
                    .unwrap_or(rupu_coverage::DEFAULT_FULL_MODE_THRESHOLD),
            });
        let grant = rupu_agent::grant::resolve_run_grant(rupu_agent::grant::RunGrantInputs {
            declared: agent.tools.as_deref(),
            step_actions: &d.step_actions,
            connector_cap: connector_cap.as_deref(),
            concerns: concerns.is_some(),
            origin: d.origin_grant.as_ref(),
            messaging: messaging.ambient().as_ref(),
            tool_context: &tool_context,
            alias_scope: d.alias_scope,
        });
        let grant = match grant {
            Ok(g) => g,
            Err(e) => {
                // As when the loop resolved it: the run's transcript exists
                // (empty), so a parent, a session or the CP that opens it
                // finds the run that failed.
                if let Err(w) = rupu_transcript::JsonlWriter::create(&spec.transcript_path) {
                    tracing::warn!(error = %w, "create the transcript of a run whose grant failed");
                }
                if let Some(n) = netflow {
                    n.finish().await;
                }
                return Err(e.into());
            }
        };

        // The connector tools this run ended up narrowed to are its
        // children's cap.
        let launch = &mut Arc::make_mut(&mut tool_context.identity).launch;
        launch.connector_cap = grant
            .connector_cap
            .as_ref()
            .map(|c| c.iter().map(|n| n.to_string()).collect());
        launch.granted_connectors = grant
            .entries
            .values()
            .filter(|e| e.descriptor.is_connector())
            .map(|e| e.canonical.to_string())
            .collect();

        let usage_ledger = ledger.as_ref().map(|l| l.path().to_path_buf());
        let parent_run = spec
            .origin
            .parent_run_id()
            .or(spec.link.as_ref().map(|l| l.parent_run.as_str()))
            .map(str::to_string);
        let ledger_hook = ledger.map(|l| {
            l.hook(
                d.ledger_tag.clone(),
                spec.run_id.clone(),
                parent_run.clone(),
                spec.transcript_path.clone(),
                agent.name.clone(),
                None,
            )
        });
        // In the root's ledger a launched run is attributed through its
        // parent chain, never to a root step that shares its step id.
        let rollup_hook = rollup.map(|l| {
            l.hook(
                crate::usage_ledger::LedgerTag {
                    rollup: true,
                    ..Default::default()
                },
                spec.run_id.clone(),
                parent_run,
                spec.transcript_path.clone(),
                agent.name.clone(),
                None,
            )
        });
        let hooks = Hooks {
            on_tool_call: spec.hooks.on_tool_call.clone(),
            on_usage: chain_usage(
                ledger_hook,
                chain_usage(rollup_hook, spec.hooks.on_usage.clone()),
            ),
        };

        let system_prompt = system_prompt_for(agent, &spec.overrides, skills.as_deref());
        let parent_run_id = spec.origin.parent_run_id().map(str::to_string);
        let parent_run_id_for_space = tool_context.identity.parent.as_ref().map(|p| p.run_id.clone());
        let launched = spec.link.is_some();
        let spec_origin = spec.origin.clone();
        let LaunchSpec {
            agent,
            transcript_path,
            prompt,
            overrides,
            stream,
            pause,
            mut collectors,
            ..
        } = spec;
        // A run in a message space joins it and hears what is said in it
        // before every model call (spec F1a §3.1, §5).
        if let Some(bus) = &message_bus {
            let member = rupu_fleet::Member {
                codename: tool_context.identity.codename.clone(),
                agent: agent.name.clone(),
                run_id: tool_context.identity.run_id.clone(),
                step_id: tool_context.identity.step_id.clone(),
                unit: match &spec_origin {
                    Origin::WorkflowStep { unit: Some(u), .. } => {
                        Some(u.key.clone().unwrap_or_else(|| u.index.to_string()))
                    }
                    _ => None,
                },
                kind: crate::services::messaging::member_kind(&spec_origin, launched).into(),
                parent_run_id: parent_run_id_for_space.clone(),
                pid: Some(std::process::id()),
            };
            // Every run joins as it starts, so the operator can reach any of
            // them, messaging tools or not (F1b).
            let joining = Arc::clone(bus);
            match tokio::task::spawn_blocking(move || joining.join(member)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "could not join the run's message space")
                }
                Err(e) => tracing::warn!(error = %e, "could not join the run's message space"),
            }
            collectors.extend(crate::services::messaging::collectors(bus));
        }
        // A run that can launch hears about its unfinished children every
        // turn (W7 §3.6).
        if let Some(launcher) = &tool_context.services.launcher {
            collectors.push(Arc::new(crate::children::ChildrenCollector::new(
                Arc::clone(launcher),
                Arc::clone(&tool_context.identity),
            )));
        }

        Ok(AssembledRun {
            opts: AgentRunOpts {
                system_prompt,
                prompt,
                provider,
                limits,
                recovery,
                permission,
                grant,
                alias_scope: d.alias_scope,
                tool_context,
                pins: pins.pins,
                concerns,
                max_turns: overrides.max_turns.or(agent.max_turns).unwrap_or(50),
                stream,
                hooks,
                pause,
                collectors,
                transcript_path,
            },
            netflow,
            parent_run_id,
            admission: d.admission,
            usage_ledger,
        })
    }
}

impl RunAssembler {
    /// The netflow stream the run `spec` tees its ledger lines into (spec
    /// 2026-10-14 remote netflow transport §N1), if its root has begun one.
    /// The candidate, in order: a `LaunchLink`'s root (its usage root's
    /// directory); then by origin, the run's own directory (`Standalone`),
    /// its workflow run's, or its in-process parent's stream under this
    /// run's id. A session turn, a flow lead or a flow unit without a link
    /// tees nowhere. `None` unless the candidate has a `begin` line.
    fn netflow_stream(
        &self,
        spec: &LaunchSpec,
        d: &OriginDefaults,
    ) -> Option<rupu_netflow::NetflowStream> {
        let runs = self.ctx.global.join("runs");
        match (&spec.link, d.netflow_stream, &spec.origin) {
            (Some(link), _, _) => crate::netflow::link_stream(link, &spec.run_id),
            (None, NetflowStreamRoot::Own, _) => {
                crate::netflow::stream_for(&runs.join(&spec.run_id), &spec.run_id)
            }
            (
                None,
                NetflowStreamRoot::WorkflowRun,
                Origin::WorkflowStep {
                    workflow_run_id, ..
                },
            ) if !workflow_run_id.is_empty() => {
                crate::netflow::stream_for(&runs.join(workflow_run_id), &spec.run_id)
            }
            (None, NetflowStreamRoot::Parent, Origin::SubAgent { parent }) => parent
                .netflow_stream
                .as_ref()
                .map(|s| rupu_netflow::NetflowStream {
                    path: s.path.clone(),
                    ledger_id: spec.run_id.clone(),
                }),
            _ => None,
        }
    }

    /// The run's bus on its message space (spec F1a): the one the site
    /// handed it (an agentiflow's lead and units, a child sharing its
    /// parent's), else one on its origin's own space. Whether the run takes
    /// part — is offered the messaging tools — is [`MessagingGrant`]'s call
    /// (spec F1d); either way the bus is where its children and the
    /// operator reach it.
    fn message_space(&self, spec: &LaunchSpec) -> Option<Arc<rupu_fleet::Bus>> {
        if let Some(bus) = &spec.services.message_bus {
            return Some(Arc::clone(bus));
        }
        let space =
            crate::services::messaging::space_dir(&self.ctx.global, &spec.origin, &spec.run_id)?;
        let address =
            crate::services::messaging::address_of(spec.codename.as_deref(), &spec.run_id);
        let bus = rupu_fleet::Bus::open(&space, address)
            .with_caps(crate::services::messaging::caps(&self.ctx.config));
        // A session's agent stays reachable between its turns.
        let bus = match spec.origin {
            Origin::SessionTurn { .. } => bus.staying(),
            _ => bus,
        };
        Some(Arc::new(bus))
    }

    /// What the run `spec` may launch and where its children are recorded
    /// (W7 §3.4), and the connector cap its grant is narrowed by. A child's
    /// caps are what its parent may launch — in process the parent's own
    /// scope, across one the `--launch-*` link. `messaging_off` is the
    /// run's own (spec F1d D6), which its children inherit.
    fn launch_scope(
        &self,
        spec: &LaunchSpec,
        usage_root: Option<PathBuf>,
        messaging_off: bool,
    ) -> (LaunchScope, Option<Vec<String>>) {
        let flow_dir = match &spec.origin {
            Origin::FlowLead { flow_dir, .. } | Origin::FlowUnit { flow_dir, .. } => {
                Some(flow_dir.clone())
            }
            _ => None,
        };
        let (cap, pool, flow, connector_cap, rollup_root) = match (&spec.origin, &spec.link) {
            (Origin::SubAgent { parent }, _) => {
                let p = &parent.identity.launch;
                (
                    Some((p.agents.clone(), p.workflows.clone())),
                    p.pool.clone(),
                    p.flow.clone(),
                    p.connector_cap.clone(),
                    p.rollup_root.clone(),
                )
            }
            (_, Some(link)) => (
                Some((link.agents.clone(), link.workflows.clone())),
                // A unit of a flow: the pool reached it as its parent's set.
                flow_dir.as_ref().map(|_| LaunchPool {
                    agents: link.agents.clone(),
                    workflows: link.workflows.clone(),
                }),
                flow_dir.clone(),
                link.actions.clone(),
                link.usage_root.clone(),
            ),
            (Origin::FlowLead { .. }, None) => (
                None,
                spec.services.flow_pool.clone(),
                flow_dir.clone(),
                None,
                None,
            ),
            _ => (None, None, flow_dir.clone(), None, None),
        };
        let agents = allowed(
            spec.agent.dispatchable_agents.as_deref(),
            pool.as_ref().map(|p| p.agents.as_slice()),
            cap.as_ref().map(|c| c.0.as_slice()),
        );
        let workflows = allowed(
            spec.agent.dispatchable_workflows.as_deref(),
            pool.as_ref().map(|p| p.workflows.as_slice()),
            cap.as_ref().map(|c| c.1.as_slice()),
        );
        let children_dir = match &spec.origin {
            Origin::FlowLead { flow_dir, .. } => Some(flow_dir.clone()),
            _ if spec.run_id.is_empty() => None,
            _ => Some(self.ctx.global.join("runs").join(&spec.run_id)),
        };
        (
            LaunchScope {
                agents,
                workflows,
                pool,
                connector_cap: None,
                // Set from the grant, once it is resolved.
                granted_connectors: Vec::new(),
                children_dir,
                usage_root,
                rollup_root,
                flow,
                messaging_off,
            },
            connector_cap,
        )
    }
}

/// What a run may launch (W7 §3.4): its own allowlist, its flow's pool when
/// it is in one (intersected when both are set), within the cap its parent
/// passed down. Neither declared nor in a flow = nothing.
fn allowed(
    declared: Option<&[String]>,
    pool: Option<&[String]>,
    cap: Option<&[String]>,
) -> Vec<String> {
    let mut out: Vec<String> = match (declared, pool) {
        (Some(d), Some(p)) => d.iter().filter(|a| p.contains(a)).cloned().collect(),
        (Some(d), None) => d.to_vec(),
        (None, Some(p)) => p.to_vec(),
        (None, None) => Vec::new(),
    };
    if let Some(c) = cap {
        out.retain(|a| c.contains(a));
    }
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|a| seen.insert(a.clone()));
    out
}

/// The system prompt a run of `agent` sends: the agent's, with the site's
/// `## Run target` section, then the run's skills section (spec §3.1): the
/// frozen one in [`Overrides::skill_section`] when the site has one.
pub fn system_prompt_for(
    agent: &rupu_agent::AgentSpec,
    overrides: &Overrides,
    skills: Option<&rupu_skills::ResolvedSkills>,
) -> String {
    let mut prompt = match overrides.system_prompt_suffix.as_deref() {
        Some(suffix) => format!("{}\n\n## Run target\n\n{suffix}", agent.system_prompt),
        None => agent.system_prompt.clone(),
    };
    let section = match &overrides.skill_section {
        Some(frozen) => (!frozen.is_empty()).then(|| frozen.clone()),
        None => skills.and_then(|s| s.prompt_section()),
    };
    if let Some(section) = section {
        prompt.push_str("\n\n");
        prompt.push_str(&section);
    }
    prompt
}

/// The skills a run of `agent` with `overrides` is given (spec §2.4): the
/// agent's keys and the launch's own, recorded with `launch_reason` (a
/// session's `--skill` entries are `Session`), resolved in one pass against
/// `dirs`. `None` when the run has none. Fails closed: a run never starts
/// without a skill it was given. Discovery reads directories and a preload
/// hashes files, so it runs on the blocking pool, and an fd exhaustion in a
/// wide fan-out is retried the way the agent's load is.
pub async fn resolve_agent_skills(
    dirs: &rupu_skills::LayerDirs,
    agent: &rupu_agent::AgentSpec,
    overrides: &Overrides,
    launch_reason: rupu_skills::SkillReason,
) -> Result<Option<Arc<rupu_skills::ResolvedSkills>>, AssembleError> {
    let agent_grants = agent.skills.clone().unwrap_or_default();
    let agent_preload = agent.preload_skills.clone().unwrap_or_default();
    let o = overrides;
    resolve_skill_requests(
        dirs,
        &agent.name,
        vec![
            (rupu_skills::SkillReason::Agent, agent_grants, agent_preload),
            (launch_reason, o.skills.clone(), o.preload_skills.clone()),
        ],
    )
    .await
}

/// Resolve `requests` (reason, grants, preload) in one pass against `dirs`
/// on the blocking pool, retrying an fd exhaustion under `label`. `None`
/// when nothing is requested (the layers are not even read) or nothing
/// resolved.
pub async fn resolve_skill_requests(
    dirs: &rupu_skills::LayerDirs,
    label: &str,
    requests: Vec<(rupu_skills::SkillReason, Vec<String>, Vec<String>)>,
) -> Result<Option<Arc<rupu_skills::ResolvedSkills>>, AssembleError> {
    if requests
        .iter()
        .all(|(_, g, p)| g.is_empty() && p.is_empty())
    {
        return Ok(None);
    }
    let requests = Arc::new(requests);
    let resolved = rupu_agent::fd_budget::retry_on_fd_exhaustion(
        label,
        &rupu_agent::fd_budget::Policy::default(),
        rupu_agent::fd_budget::grow,
        || {
            let (dirs, requests) = (dirs.clone(), Arc::clone(&requests));
            async move {
                tokio::task::spawn_blocking(move || resolve_run_skills(dirs, &requests[..]))
                    .await
                    .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
            }
        },
    )
    .await?;
    Ok((!resolved.is_empty()).then(|| Arc::new(resolved)))
}

/// A session's frozen skill set (spec 2026-10-12 §5.1) as installed now in
/// `dirs`, the layers it was resolved in: the set (`None` when nothing in
/// it can be given) and the entries that can't be, for the turn to report
/// ([`rupu_agent::UserTurn::missing_skills`]). Fails only when the skills
/// directories can't be read.
pub async fn resolve_frozen_skills(
    dirs: &rupu_skills::LayerDirs,
    frozen: &[rupu_skills::FrozenSkill],
) -> Result<
    (
        Option<Arc<rupu_skills::ResolvedSkills>>,
        Vec<rupu_skills::MissingSkill>,
    ),
    AssembleError,
> {
    if frozen.is_empty() {
        return Ok((None, Vec::new()));
    }
    let (dirs, frozen) = (dirs.clone(), frozen.to_vec());
    let (resolved, missing) = tokio::task::spawn_blocking(move || {
        let catalog =
            rupu_skills::SkillCatalog::discover(&dirs).map_err(|e| AssembleError::Skills {
                source: e.into(),
                looked_in: dirs.clone(),
            })?;
        Ok::<_, AssembleError>(rupu_skills::resolve_frozen(&catalog, &frozen))
    })
    .await
    .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))?;
    Ok(((!resolved.is_empty()).then(|| Arc::new(resolved)), missing))
}

/// An operator's invocations for a session turn (spec 2026-10-12 §5.2),
/// each entry resolved against `dirs`, the session's layers: any valid
/// installed skill, granted or not. `held` is the session's frozen set; one
/// resolved twice is returned once. A wildcard, an unknown or ambiguous
/// name, a name held from the other layer, or one invoked from both layers
/// fails the whole call.
pub async fn resolve_operator_skills(
    dirs: &rupu_skills::LayerDirs,
    held: &[rupu_skills::FrozenSkill],
    entries: &[String],
) -> Result<Vec<rupu_skills::FrozenSkill>, AssembleError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let (dirs, held, entries) = (dirs.clone(), held.to_vec(), entries.to_vec());
    tokio::task::spawn_blocking(move || {
        let failed = |source: rupu_agent::SkillCheckError| AssembleError::Skills {
            source,
            looked_in: dirs.clone(),
        };
        let catalog = rupu_skills::SkillCatalog::discover(&dirs).map_err(|e| failed(e.into()))?;
        let mut invoked: Vec<rupu_skills::FrozenSkill> = Vec::new();
        for entry in &entries {
            let f = rupu_skills::resolve_operator(&catalog, &held, entry)
                .map_err(|e| failed(e.into()))?;
            if invoked
                .iter()
                .any(|i| i.name == f.name && i.layer != f.layer)
            {
                let both = rupu_skills::ResolveError::BothLayers { name: f.name };
                return Err(failed(
                    rupu_skills::RunSkillsError::Resolve(vec![both]).into(),
                ));
            }
            if !invoked.contains(&f) {
                invoked.push(f);
            }
        }
        Ok(invoked)
    })
    .await
    .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
}

/// Discover the layers and resolve `requests` against them (blocking IO).
fn resolve_run_skills(
    dirs: rupu_skills::LayerDirs,
    requests: &[(rupu_skills::SkillReason, Vec<String>, Vec<String>)],
) -> Result<rupu_skills::ResolvedSkills, AssembleError> {
    let failed = |source: rupu_agent::SkillCheckError| AssembleError::Skills {
        source,
        looked_in: dirs.clone(),
    };
    let catalog = rupu_skills::SkillCatalog::discover(&dirs).map_err(|e| failed(e.into()))?;
    let requests: Vec<rupu_skills::SkillRequest<'_>> = requests
        .iter()
        .map(|(reason, grants, preload)| rupu_skills::SkillRequest {
            reason: *reason,
            grants,
            preload,
        })
        .collect();
    rupu_skills::resolve_run_skills(&catalog, &requests).map_err(|e| failed(e.into()))
}

/// The agent's own `permissionMode`. An unreadable one can't lower a
/// ceiling: it caps the run at readonly rather than run it at the parent's
/// full mode.
fn declared_mode(agent: &rupu_agent::AgentSpec) -> Option<PermissionMode> {
    let word = agent.permission_mode.as_deref()?;
    match PermissionMode::parse(word) {
        Ok(mode) => Some(mode),
        Err(e) => {
            tracing::warn!(agent = %agent.name, error = %e, "capping the run at readonly");
            Some(PermissionMode::Readonly)
        }
    }
}

/// Both hooks, the ledger's first.
fn chain_usage(a: Option<OnUsageCallback>, b: Option<OnUsageCallback>) -> Option<OnUsageCallback> {
    match (a, b) {
        (Some(a), Some(b)) => Some(Arc::new(move |u: &rupu_agent::UsageTurn| {
            a(u);
            b(u);
        })),
        (a, b) => a.or(b),
    }
}
