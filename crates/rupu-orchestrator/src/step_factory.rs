//! Default [`StepFactory`] implementation: a step's launch, built by the
//! run's assembler.
//!
//! `DefaultStepFactory` resolves each step's `agent:` against the project-
//! and global-scope `agents/` dirs and returns its `LaunchSpec`
//! (`Origin::WorkflowStep`); the run's [`RunAssembler`] — built once per
//! workflow run ([`crate::workflow_runtime::WorkflowRuntime`]) over the
//! run's config, SCM registry, engagement and capture backend — derives the
//! provider, limits, recovery, grant, findings, netflow, `[bash]` and usage
//! ledger from it (spec 2026-10-07 W3).

use crate::runner::{StepFactory, StepLaunch, StepRequest};
use crate::workflow::Workflow;
use async_trait::async_trait;
use rupu_runtime::assembly::{AssembleError, RunAssembler};
use rupu_tools::{AgentDispatcher, PermissionMode};
use std::sync::Arc;

/// `StepFactory` impl that loads each step's `agent:` from the project-
/// and global-scope `agents/` dirs and launches it through the run's
/// assembler.
pub struct DefaultStepFactory {
    pub workflow: Workflow,
    /// The run's assembler: its config, SCM registry, findings base (with
    /// the run's engagement), capture backend and customer.
    pub assembler: Arc<RunAssembler>,
    /// The mode the workflow run was launched under; every agent step runs
    /// under it (unattended: no operator prompter).
    pub mode: PermissionMode,
    /// Formatted `## Run target` text to append to each step's system prompt.
    /// `None` when no `--target` was supplied at workflow invocation.
    pub system_prompt_suffix: Option<String>,
    /// The in-process sub-agent dispatcher every step's run is offered
    /// (`dispatch_agent`). `None` = the steps can't dispatch.
    pub dispatcher: Option<Arc<dyn AgentDispatcher>>,
    /// Coverage/findings scope every step reports under. `None` (the
    /// default for every ordinary workflow run) keeps the historical
    /// per-workflow scope — the workflow's own name. `Some(name)` is set
    /// only by an agentiflow-launched workflow unit (`rupu workflow run
    /// --fleet-run-dir`), so its steps' findings pool into the agentiflow's
    /// scope instead of the workflow's.
    pub scope_name_override: Option<String>,
}

#[async_trait]
impl StepFactory for DefaultStepFactory {
    async fn launch_for_step(&self, request: StepRequest) -> Result<StepLaunch, AssembleError> {
        // We still verify the parent step exists in the workflow so
        // unknown step ids surface clearly, but we drive the agent
        // load off `agent_name` (which differs from the parent's
        // `agent:` for `parallel:` sub-steps).
        //
        // An `on_reject:` cleanup sub-step's id is NOT in `workflow.steps`
        // — it lives nested under its gate's `approval.on_reject`
        // (`crate::workflow::Approval::on_reject`), the same way a
        // `parallel:`/`for_each:` sub-step's agent differs from its
        // parent's. `run_reject_cleanup` dispatches those sub-steps
        // through this same factory (`dispatch_one` with the sub-step's
        // own id), so the lookup falls back to searching every gate's
        // cleanup chain before giving up.
        let step = self
            .workflow
            .steps
            .iter()
            .find(|s| s.id == request.step_id)
            .or_else(|| {
                self.workflow.steps.iter().find_map(|s| {
                    s.approval
                        .as_ref()
                        .and_then(|a| a.on_reject.iter().find(|sub| sub.id == request.step_id))
                })
            })
            .expect(
                "step_id from orchestrator must match a workflow step or an on_reject cleanup sub-step",
            );

        // The agent loader takes the parent of `agents/`. For the
        // project layer that's `<project>/.rupu`; the global layer is
        // `<global>` directly (which already contains `agents/`).
        let ctx = self.assembler.context();
        let project_agents_parent = ctx.project_root.as_ref().map(|p| p.join(".rupu"));
        // Admission-paced: under fd pressure (a wide fan-out) this grows the
        // open-file limit or waits for running agents to release descriptors
        // instead of failing with EMFILE. See `rupu_agent::fd_budget`.
        //
        // A missing or unparseable agent file fails the step loudly — never a
        // run on the default provider/model (a step naming a nonexistent
        // agent once ran on `anthropic` and billed it). A present agent that
        // merely omits `provider:`/`model:` still defaults.
        let agent = rupu_agent::load_agent_admitted(
            &ctx.global,
            project_agents_parent.as_deref(),
            &request.agent_name,
        )
        .await
        .map_err(|e| AssembleError::AgentLoad {
            agent: request.agent_name.clone(),
            message: e.to_string(),
        })?;

        let mut spec = request.into_spec(
            agent,
            step.actions.clone(),
            self.scope_name_override.clone(),
            self.mode,
        );
        spec.overrides.system_prompt_suffix = self.system_prompt_suffix.clone();
        spec.overrides.findings_profile = step.findings_profile;
        spec.overrides.findings_default = self.workflow.defaults.findings_profile;
        // Workflow-level concerns take precedence over agent-level concerns:
        // when the workflow declares `concerns:`, every step uses it.
        spec.overrides.concerns = self.workflow.concerns.clone();
        spec.services.dispatcher = self.dispatcher.clone();
        Ok(StepLaunch {
            assembler: Arc::clone(&self.assembler),
            spec,
        })
    }

    fn permission_mode(&self) -> Option<&str> {
        Some(self.mode.as_str())
    }

    fn customer(&self) -> Option<&str> {
        self.assembler.context().customer.as_deref()
    }

    fn system_prompt_suffix(&self) -> Option<&str> {
        self.system_prompt_suffix.as_deref()
    }

    fn engagement_profiles(&self) -> Vec<String> {
        self.assembler
            .context()
            .findings
            .engagement
            .as_deref()
            .map(|set| set.ids().into_iter().map(str::to_string).collect())
            .unwrap_or_default()
    }
}

/// The real factory against agents on disk: what each step's launch carries,
/// and what the run's assembler derives from it (spec 2026-10-07 W3). The
/// provider is injected (a mock), so nothing here needs credentials.
#[cfg(test)]
mod tests {
    use super::DefaultStepFactory;
    use crate::runner::{StepFactory, StepLaunch, StepRequest};
    use crate::workflow::Workflow;
    use rupu_runtime::assembly::{
        AssembleError, AssembledRun, AssemblyContext, PreparedProvider, RunAssembler,
        WorkspaceBinding,
    };
    use std::sync::Arc;

    const WF: &str = r#"
name: w
steps:
  - id: narrowed
    agent: ag
    prompt: p
    actions: ["issues.list"]
  - id: unrestricted
    agent: ag
    prompt: p
    actions: []
"#;

    fn context(global: &std::path::Path) -> AssemblyContext {
        AssemblyContext::minimal(global.to_path_buf())
    }

    fn factory_with(wf: &str, ctx: AssemblyContext) -> DefaultStepFactory {
        DefaultStepFactory {
            workflow: Workflow::parse(wf).expect("workflow must parse"),
            assembler: Arc::new(RunAssembler::new(ctx)),
            mode: rupu_tools::PermissionMode::Bypass,
            system_prompt_suffix: None,
            dispatcher: None,
            scope_name_override: None,
        }
    }

    fn write(global: &std::path::Path, name: &str, frontmatter: &str) {
        let agents_dir = global.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join(format!("{name}.md")),
            format!("---\nname: {name}\n{frontmatter}---\nDo the thing.\n"),
        )
        .unwrap();
    }

    fn request(global: &std::path::Path, step: &str, agent: &str) -> StepRequest {
        StepRequest {
            step_id: step.into(),
            agent_name: agent.into(),
            rendered_prompt: "prompt".into(),
            run_id: "run1".into(),
            workflow_run_id: String::new(),
            workflow_name: "ignored-the-factory-is-told".into(),
            unit: None,
            workspace: WorkspaceBinding {
                id: "ws1".into(),
                path: global.to_path_buf(),
            },
            transcript_path: global.join(format!("{step}.jsonl")),
            on_tool_call: None,
        }
    }

    async fn launch(
        f: &DefaultStepFactory,
        global: &std::path::Path,
        step: &str,
        agent: &str,
    ) -> StepLaunch {
        f.launch_for_step(request(global, step, agent))
            .await
            .expect("the agent loads")
    }

    /// The launch assembled on a mock provider (no credentials, no
    /// model-list lookup).
    async fn assemble(launch: StepLaunch) -> AssembledRun {
        let StepLaunch {
            assembler,
            mut spec,
        } = launch;
        spec.services.provider = Some(PreparedProvider::injected(
            "anthropic",
            "claude-test-1",
            Box::new(rupu_agent::MockProvider::new(Vec::new())),
            &spec.agent,
            Some(rupu_providers::model_limits::ModelLimits::unknown()),
        ));
        match assembler.assemble(spec).await {
            Ok(run) => run,
            Err(e) => panic!("assembles: {e}"),
        }
    }

    #[tokio::test]
    async fn a_missing_agent_is_an_agent_load_error_not_a_default_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let f = factory_with(WF, context(tmp.path()));
        let err = f
            .launch_for_step(request(tmp.path(), "narrowed", "oracle-enumerator-glm"))
            .await
            .err()
            .expect("a missing agent must fail the launch");
        assert!(
            matches!(&err, AssembleError::AgentLoad { agent, .. } if agent == "oracle-enumerator-glm"),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .contains("agent `oracle-enumerator-glm` not found or failed to load"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn step_actions_reach_the_run_with_the_agent_grant() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ag", "tools: [issues.list, issues.create]\n");
        let f = factory_with(WF, context(tmp.path()));

        let narrowed = launch(&f, tmp.path(), "narrowed", "ag").await;
        assert_eq!(
            narrowed.spec.agent.tools,
            Some(vec!["issues.list".to_string(), "issues.create".to_string()]),
            "the agent's own grant, un-narrowed: the assembler narrows it"
        );
        match &narrowed.spec.origin {
            rupu_runtime::assembly::Origin::WorkflowStep { step_actions, .. } => {
                assert_eq!(step_actions, &vec!["issues.list".to_string()])
            }
            _ => panic!("a step's origin"),
        }

        let unrestricted = launch(&f, tmp.path(), "unrestricted", "ag").await;
        match &unrestricted.spec.origin {
            rupu_runtime::assembly::Origin::WorkflowStep { step_actions, .. } => {
                assert!(step_actions.is_empty(), "actions: [] narrows nothing")
            }
            _ => panic!("a step's origin"),
        }
    }

    #[tokio::test]
    async fn step_scope_is_the_workflow_name_unless_overridden() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ag", "");
        let mut req = request(tmp.path(), "unrestricted", "ag");
        req.workflow_name = "w".into();

        // Default (`None`): the historical per-workflow scope.
        let f = factory_with(WF, context(tmp.path()));
        let run = assemble(f.launch_for_step(req).await.unwrap()).await;
        assert_eq!(run.identity().scope_name.as_deref(), Some("w"));

        // An agentiflow-launched workflow unit pools into the agentiflow scope.
        let mut f = factory_with(WF, context(tmp.path()));
        f.scope_name_override = Some("af_123".to_string());
        let mut req = request(tmp.path(), "unrestricted", "ag");
        req.workflow_name = "w".into();
        let run = assemble(f.launch_for_step(req).await.unwrap()).await;
        assert_eq!(run.identity().scope_name.as_deref(), Some("af_123"));
    }

    #[tokio::test]
    async fn findings_profile_precedence_table() {
        use rupu_coverage::FindingProfile::{self, Full, Summary};
        let tmp = tempfile::tempdir().unwrap();
        for (name, line) in [
            ("fp_none", ""),
            ("fp_full", "findingsProfile: full\n"),
            ("fp_summary", "findingsProfile: summary\n"),
        ] {
            write(
                tmp.path(),
                name,
                &format!("tools: [report_finding]\n{line}"),
            );
        }
        let name = |p: Option<FindingProfile>| match p {
            None => None,
            Some(Full) => Some("full"),
            Some(Summary) => Some("summary"),
        };
        // (step, workflow defaults, agent frontmatter) → resolved.
        type Row = (
            Option<FindingProfile>,
            Option<FindingProfile>,
            Option<FindingProfile>,
            FindingProfile,
        );
        let table: &[Row] = &[
            // The step wins over everything.
            (Some(Summary), Some(Full), Some(Full), Summary),
            (Some(Full), Some(Summary), Some(Summary), Full),
            (Some(Summary), None, None, Summary),
            // Then the workflow default, over the agent.
            (None, Some(Summary), Some(Full), Summary),
            (None, Some(Full), Some(Summary), Full),
            (None, Some(Summary), None, Summary),
            // Then the agent's frontmatter.
            (None, None, Some(Summary), Summary),
            (None, None, Some(Full), Full),
            // Then the built-in default.
            (None, None, None, Full),
        ];
        for &(step, defaults, agent, expected) in table {
            let agent_name = match name(agent) {
                None => "fp_none".to_string(),
                Some(p) => format!("fp_{p}"),
            };
            let mut wf = String::from("name: precedence\n");
            if let Some(d) = name(defaults) {
                wf.push_str(&format!("defaults:\n  findings_profile: {d}\n"));
            }
            wf.push_str(&format!(
                "steps:\n  - id: s\n    agent: {agent_name}\n    prompt: p\n"
            ));
            if let Some(p) = name(step) {
                wf.push_str(&format!("    findings_profile: {p}\n"));
            }
            let f = factory_with(&wf, context(tmp.path()));
            let run = assemble(launch(&f, tmp.path(), "s", &agent_name).await).await;
            let got = run
                .opts
                .tool_context
                .services
                .findings
                .as_ref()
                .expect("findings options always set")
                .profile;
            assert_eq!(
                got, expected,
                "step={step:?} defaults={defaults:?} agent={agent:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_runs_findings_base_and_engagement_reach_the_step() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ag", "");
        let engagement = Arc::new(
            rupu_coverage::builtin_registry()
                .unwrap()
                .active_set(&[rupu_coverage::DEFAULT_PROFILE.to_string()])
                .unwrap(),
        );
        let mut ctx = context(tmp.path());
        ctx.findings = rupu_coverage::FindingWriteOptions {
            artifact_root: Some(tmp.path().join("store")),
            artifact_max_bytes: 7,
            engagement: Some(engagement.clone()),
            ..Default::default()
        };
        let f = factory_with(WF, ctx);
        assert_eq!(
            f.engagement_profiles(),
            vec![rupu_coverage::DEFAULT_PROFILE.to_string()]
        );
        let run = assemble(launch(&f, tmp.path(), "unrestricted", "ag").await).await;
        let fo = run.opts.tool_context.services.findings.clone().unwrap();
        assert_eq!(fo.artifact_max_bytes, 7);
        assert_eq!(fo.artifact_root, Some(tmp.path().join("store")));
        // Dropping the engagement would route the step's findings as native
        // code findings.
        let got = fo.engagement.expect("engagement must reach the step");
        assert!(Arc::ptr_eq(&got, &engagement));
    }

    /// ISSUES.md I-18: `[bash]` reaches every workflow step (it once
    /// hardcoded 120 s and an empty allowlist).
    #[tokio::test]
    async fn bash_config_reaches_the_step() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ag", "");
        let mut ctx = context(tmp.path());
        ctx.config.bash.timeout_secs = Some(42);
        ctx.config.bash.env_allowlist = Some(vec!["FOO".to_string()]);
        let f = factory_with(WF, ctx);
        let run = assemble(launch(&f, tmp.path(), "unrestricted", "ag").await).await;
        let bash = &run.opts.tool_context.workspace.bash;
        assert_eq!(bash.timeout_secs, 42);
        assert_eq!(bash.env_allowlist, vec!["FOO".to_string()]);
    }

    #[tokio::test]
    async fn the_customer_reaches_every_step() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ag", "");
        let f = factory_with(WF, context(tmp.path()));
        assert_eq!(StepFactory::customer(&f), None);
        let mut ctx = context(tmp.path());
        ctx.customer = Some("acme".to_string());
        let f = factory_with(WF, ctx);
        assert_eq!(StepFactory::customer(&f), Some("acme"));
        let run = assemble(launch(&f, tmp.path(), "unrestricted", "ag").await).await;
        assert_eq!(
            run.opts.tool_context.services.customer.as_deref(),
            Some("acme")
        );
    }

    #[tokio::test]
    async fn workflow_concerns_win_over_the_agents() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "ag",
            "concerns:\n  - include: owasp-top10-2021\n",
        );
        let include = |run: &AssembledRun| match &run
            .opts
            .concerns
            .as_ref()
            .expect("concerns resolved")
            .entries[0]
        {
            rupu_coverage::ConcernsEntry::Include(d) => d.include.clone(),
            other => panic!("expected Include entry, got {other:?}"),
        };

        let with = format!("{WF}concerns:\n  - include: stride\n");
        let f = factory_with(&with, context(tmp.path()));
        let run = assemble(launch(&f, tmp.path(), "unrestricted", "ag").await).await;
        assert_eq!(include(&run), "stride", "the workflow's concerns win");

        let f = factory_with(WF, context(tmp.path()));
        let run = assemble(launch(&f, tmp.path(), "unrestricted", "ag").await).await;
        assert_eq!(
            include(&run),
            "owasp-top10-2021",
            "the agent's flow through when the workflow declares none"
        );
    }

    #[tokio::test]
    async fn the_run_target_and_the_agents_fallbacks_reach_the_step() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "ag", "fallbacks:\n  - model: claude-test-2\n");
        let mut f = factory_with(WF, context(tmp.path()));
        f.system_prompt_suffix = Some("repo: acme/widget".into());
        let run = assemble(launch(&f, tmp.path(), "unrestricted", "ag").await).await;
        assert!(
            run.opts
                .system_prompt
                .ends_with("## Run target\n\nrepo: acme/widget"),
            "{}",
            run.opts.system_prompt
        );
        assert_eq!(
            run.opts.recovery.chain,
            vec![rupu_config::FallbackEntry {
                provider: None,
                model: "claude-test-2".into(),
            }]
        );
        assert!(run.opts.recovery.hop_builder.is_some());
    }

    /// Delegates to the real factory, records the findings profile each
    /// launch resolves to, and runs it on a scripted provider.
    struct RecordingFactory {
        inner: DefaultStepFactory,
        seen: std::sync::Mutex<Vec<(String, rupu_coverage::FindingProfile)>>,
    }

    #[async_trait::async_trait]
    impl StepFactory for RecordingFactory {
        async fn launch_for_step(&self, request: StepRequest) -> Result<StepLaunch, AssembleError> {
            let step_id = request.step_id.clone();
            let mut launch = self.inner.launch_for_step(request).await?;
            let o = &launch.spec.overrides;
            let profile = rupu_coverage::FindingProfile::resolve(
                o.findings_profile,
                o.findings_default,
                launch.spec.agent.findings_profile,
            );
            self.seen.lock().unwrap().push((step_id, profile));
            launch.spec.services.provider = Some(PreparedProvider::injected(
                "mock",
                "mock-1",
                Box::new(rupu_agent::runner::MockProvider::new(vec![
                    rupu_agent::runner::ScriptedTurn::AssistantText {
                        text: "done".into(),
                        stop: rupu_providers::types::StopReason::EndTurn,
                        input_tokens: 1,
                        output_tokens: 1,
                    },
                ])),
                &launch.spec.agent,
                Some(rupu_providers::model_limits::ModelLimits::unknown()),
            ));
            Ok(launch)
        }
    }

    fn run_opts(
        wf: Workflow,
        dir: &std::path::Path,
        factory: Arc<RecordingFactory>,
    ) -> crate::runner::OrchestratorRunOpts {
        crate::runner::OrchestratorRunOpts {
            run_step: Default::default(),
            workflow: wf,
            inputs: Default::default(),
            workspace_id: "ws_profile".into(),
            naming: None,
            workspace_path: dir.to_path_buf(),
            transcript_dir: dir.join("transcripts"),
            factory,
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_services: None,
            pause: None,
        }
    }

    fn write_profile_agents(global: &std::path::Path) {
        write(
            global,
            "fp_full",
            "tools: [report_finding]\nfindingsProfile: full\n",
        );
        write(
            global,
            "fp_summary",
            "tools: [report_finding]\nfindingsProfile: summary\n",
        );
    }

    #[tokio::test]
    async fn for_each_units_resolve_their_steps_profile() {
        use rupu_coverage::FindingProfile::Summary;
        let tmp = tempfile::tempdir().unwrap();
        write_profile_agents(tmp.path());
        // The agent says full and the workflow default says full; only the
        // fan-out step says summary. Every unit must get summary.
        let wf_src = "name: fanout\ndefaults:\n  findings_profile: full\nsteps:\n  - id: fan\n    for_each: '[\"a.rs\", \"b.rs\", \"c.rs\"]'\n    agent: fp_full\n    prompt: \"assess {{ item }}\"\n    findings_profile: summary\n";
        let rec = Arc::new(RecordingFactory {
            inner: factory_with(wf_src, context(tmp.path())),
            seen: Default::default(),
        });
        let wf = Workflow::parse(wf_src).unwrap();
        crate::runner::run_workflow(run_opts(wf, tmp.path(), Arc::clone(&rec)))
            .await
            .expect("run completes");
        let seen = rec.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "one launch per unit: {seen:?}");
        for (step_id, profile) in seen {
            assert_eq!(step_id, "fan");
            assert_eq!(profile, Summary);
        }
    }

    #[tokio::test]
    async fn on_reject_cleanup_sub_steps_resolve_their_own_profile() {
        use rupu_coverage::FindingProfile::{Full, Summary};
        let tmp = tempfile::tempdir().unwrap();
        write_profile_agents(tmp.path());
        // The cleanup sub-step lives under the gate, not in `steps`; the
        // factory finds it there and resolves step → defaults → agent.
        let wf_src = "name: gated\ndefaults:\n  findings_profile: full\nsteps:\n  - id: gate\n    approval:\n      required: true\n      on_reject:\n        - id: triage\n          agent: fp_full\n          prompt: p\n          findings_profile: summary\n        - id: note\n          agent: fp_summary\n          prompt: p\n";
        let rec = Arc::new(RecordingFactory {
            inner: factory_with(wf_src, context(tmp.path())),
            seen: Default::default(),
        });
        let wf = Workflow::parse(wf_src).unwrap();
        let mut opts = run_opts(wf, tmp.path(), Arc::clone(&rec));
        opts.resume_from = Some(crate::runner::ResumeState::from_rejection(
            "run_gated".into(),
            Vec::new(),
            "gate".into(),
            "not today".into(),
        ));
        crate::runner::run_reject_cleanup(opts, "gate", "not today", "cli", None)
            .await
            .expect("cleanup completes");
        let seen = rec.seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![("triage".to_string(), Summary), ("note".to_string(), Full)],
            "the step's own profile, else the workflow default over the agent's"
        );
    }
}
