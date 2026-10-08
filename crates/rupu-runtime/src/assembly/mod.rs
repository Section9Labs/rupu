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

mod origin;
mod pins;
mod spec;

pub use origin::{
    defaults_for, CoverageStream, Origin, OriginDefaults, ParentRun, UnitKey, UsageRoot,
};
pub use pins::PinRule;
pub use spec::{LaunchServices, LaunchSpec, Overrides, WorkspaceBinding};

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use rupu_agent::{AgentRunOpts, Hooks, OnUsageCallback, RunExit};
use rupu_tools::{
    BashConfig, GrantError, ParentLink, PermissionMode, PermissionPolicy, RunIdentity, ToolContext,
    ToolServices, WorkspaceScope,
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
}

impl AssembleError {
    /// The run failure this assembly error is, for a site that records it
    /// the way it records a failed run (`rupu run`'s `run.json`, a session
    /// turn).
    pub fn into_run_error(self) -> rupu_agent::RunError {
        match self {
            AssembleError::Grant(e) => rupu_agent::RunError::ToolGrant(e.to_string()),
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
    /// - an undeclared provider, as its config error;
    /// - a provider that did not build, as the auth error a first call on
    ///   it raised, with the `rupu auth login` hint.
    pub fn record_unstarted(self, run: rupu_agent::UnstartedRun<'_>) -> rupu_agent::RunError {
        use rupu_providers::ProviderError;
        let failure = match self {
            AssembleError::Grant(_) => return self.into_run_error(),
            AssembleError::AgentLoad { .. } => ProviderError::Preflight(format!(
                "{self}\n  Checked the project agents dir (.rupu/agents/) and the global agents dir."
            )),
            AssembleError::UnknownProvider(_) => ProviderError::Preflight(self.to_string()),
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
    /// The run's netflow ledger writer, when the assembler built the sink:
    /// shut down when the run ends.
    netflow: Option<rupu_netflow::NetflowWriterHandle>,
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
    /// the agent loop; flush the run's netflow ledger at the end.
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
    netflow: Option<rupu_netflow::NetflowWriterHandle>,
    permit: Option<crate::admission::JobPermit>,
}

impl AdmittedRun {
    /// The slot it holds — how long it waited for it. `None` for an origin
    /// that skips admission.
    pub fn permit(&self) -> Option<&crate::admission::JobPermit> {
        self.permit.as_ref()
    }

    /// Run the agent loop; release the slot and flush the run's netflow
    /// ledger at the end.
    pub async fn run(self) -> RunExit {
        let AdmittedRun {
            opts,
            netflow,
            permit,
        } = self;
        let exit = rupu_agent::run_agent_full(opts).await;
        drop(permit);
        if let Some(h) = netflow {
            h.shutdown().await;
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

impl RunAssembler {
    pub fn new(ctx: AssemblyContext) -> Self {
        let openai_compatible = provider_factory::openai_compatible_map(&ctx.config.providers);
        let limits_ctx = LimitsContext::from_config(&ctx.config, &ctx.global);
        let bash = BashConfig {
            env_allowlist: ctx.config.bash.env_allowlist.clone().unwrap_or_default(),
            timeout_secs: ctx
                .config
                .bash
                .timeout_secs
                .unwrap_or(BashConfig::DEFAULT_TIMEOUT_SECS),
        };
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

    /// Build the run `spec` describes.
    pub async fn assemble(&self, mut spec: LaunchSpec) -> Result<AssembledRun, AssembleError> {
        let d = defaults_for(&spec.origin);
        let cfg = &self.ctx.config;

        // The run's netflow sink — built before the provider, which takes it.
        let (sink, netflow) = match spec.services.netflow.clone() {
            Some(sink) => (sink, None),
            None => crate::netflow::for_run(
                &self.ctx.global,
                self.ctx.project_root.as_deref(),
                &spec.run_id,
                &spec.transcript_path,
            ),
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
                if let Some(h) = netflow {
                    h.shutdown().await;
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

        let parent = match &spec.origin {
            Origin::SubAgent { parent } => Some(ParentLink {
                run_id: parent.identity.run_id.clone(),
                codename: parent.identity.codename.clone(),
                root_run_id: parent
                    .identity
                    .parent
                    .as_ref()
                    .map(|p| p.root_run_id.clone())
                    .unwrap_or_else(|| parent.identity.run_id.clone()),
            }),
            _ => None,
        };
        let depth = match &spec.origin {
            Origin::SubAgent { parent } => parent.identity.depth + 1,
            _ => 0,
        };
        let identity = Arc::new(RunIdentity {
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
            dispatchable_agents: agent.dispatchable_agents.clone(),
        });

        let permission = match spec.ceiling {
            Some(ceiling) => {
                PermissionPolicy::for_child(ceiling, declared_mode(agent), spec.prompter.clone())
            }
            None => PermissionPolicy::new(spec.mode, spec.prompter.clone()),
        };

        let (ledger, coverage_stream) = self.root_ledger_and_stream(&spec.origin, &spec.run_id);
        let findings =
            self.ctx
                .findings
                .clone()
                .with_profile(rupu_coverage::FindingProfile::resolve(
                    spec.overrides.findings_profile,
                    spec.overrides.findings_default,
                    agent.findings_profile,
                ));
        let tool_context = ToolContext {
            identity: Arc::clone(&identity),
            workspace: WorkspaceScope {
                id: spec.workspace.id.clone(),
                path: spec.workspace.path.clone(),
                bash: self.bash.clone(),
            },
            services: ToolServices {
                dispatcher: spec.services.dispatcher.clone().filter(|_| d.launcher),
                scm: self.ctx.scm.clone().filter(|_| d.scm),
                findings: Some(findings),
                coverage_writer: None,
                coverage_stream: coverage_stream.clone(),
                netflow_sink: Some(sink),
                net_capture: self.ctx.net_capture.clone(),
                customer: self.ctx.customer.clone(),
                // The agent loop sets it from the permission policy.
                prompter: None,
            },
            call: Default::default(),
        };

        let concerns = spec.overrides.concerns.clone().or(agent.concerns.clone());
        let grant = rupu_agent::grant::resolve_run_grant(rupu_agent::grant::RunGrantInputs {
            declared: agent.tools.as_deref(),
            step_actions: &d.step_actions,
            concerns: concerns.is_some(),
            injected: &spec.services.extra_tools,
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
                if let Some(h) = netflow {
                    h.shutdown().await;
                }
                return Err(e.into());
            }
        };

        let usage_ledger = ledger.as_ref().map(|l| l.path().to_path_buf());
        let ledger_hook = ledger.map(|l| {
            l.hook(
                d.ledger_tag.clone(),
                spec.run_id.clone(),
                spec.origin.parent_run_id().map(str::to_string),
                spec.transcript_path.clone(),
                agent.name.clone(),
                None,
            )
        });
        let hooks = Hooks {
            on_tool_call: spec.hooks.on_tool_call.clone(),
            on_usage: chain_usage(ledger_hook, spec.hooks.on_usage.clone()),
        };

        let system_prompt = system_prompt_for(agent, &spec.overrides);
        let parent_run_id = spec.origin.parent_run_id().map(str::to_string);
        let LaunchSpec {
            agent,
            transcript_path,
            prompt,
            overrides,
            stream,
            pause,
            services,
            collectors,
            ..
        } = spec;

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
                extra_tools: services.extra_tools,
                transcript_path,
            },
            netflow,
            parent_run_id,
            admission: d.admission,
            usage_ledger,
        })
    }
}

/// The system prompt a run of `agent` sends: the agent's, with the site's
/// `## Run target` section appended.
pub fn system_prompt_for(agent: &rupu_agent::AgentSpec, overrides: &Overrides) -> String {
    match overrides.system_prompt_suffix.as_deref() {
        Some(suffix) => format!("{}\n\n## Run target\n\n{suffix}", agent.system_prompt),
        None => agent.system_prompt.clone(),
    }
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
