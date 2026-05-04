//! `rupu workflow list | show | run`.
//!
//! Lists workflows from `<global>/workflows/*.yaml` and (if any)
//! `<project>/.rupu/workflows/*.yaml`; project entries shadow global by
//! filename. `show` prints the YAML body. `run` parses the workflow,
//! builds a [`StepFactory`] that wires real providers via
//! [`provider_factory::build_for_provider`], and dispatches
//! [`rupu_orchestrator::run_workflow`].
//!
//! The factory carries a clone of the parsed [`Workflow`] so each
//! step's `agent:` field is honored (no hardcoded agent name).

use crate::paths;
use crate::provider_factory;
use async_trait::async_trait;
use clap::Subcommand;
use rupu_agent::runner::{AgentRunOpts, BypassDecider, PermissionDecider};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::Workflow;
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all workflows (global + project).
    List,
    /// Print a workflow's YAML body.
    Show {
        /// Workflow name (filename stem under `workflows/`).
        name: String,
    },
    /// Run a workflow.
    Run {
        /// Workflow name (filename stem under `workflows/`).
        name: String,
        /// `KEY=VALUE` template inputs (repeatable).
        #[arg(long, value_parser = parse_kv)]
        input: Vec<(String, String)>,
        /// Override permission mode (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
    },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE: {s}"))?;
    Ok((k.to_string(), v.to_string()))
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::List => list().await,
        Action::Show { name } => show(&name).await,
        Action::Run { name, input, mode } => run(&name, input, mode.as_deref()).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu workflow: {e}");
            ExitCode::from(1)
        }
    }
}

async fn list() -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // (name, scope) — project shadows global by name. We collect into
    // a BTreeMap to dedupe before printing.
    let mut by_name: BTreeMap<String, String> = BTreeMap::new();
    push_yaml_names(&global.join("workflows"), "global", &mut by_name);
    if let Some(p) = &project_root {
        // Project entries inserted second deliberately overwrite the
        // global scope chip for the same name.
        push_yaml_names(&p.join(".rupu/workflows"), "project", &mut by_name);
    }

    println!("{:<28} SCOPE", "NAME");
    for (n, s) in &by_name {
        println!("{n:<28} {s}");
    }
    Ok(())
}

fn push_yaml_names(dir: &Path, scope: &str, into: &mut BTreeMap<String, String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            into.insert(stem.to_string(), scope.to_string());
        }
    }
}

async fn show(name: &str) -> anyhow::Result<()> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;
    print!("{body}");
    Ok(())
}

fn locate_workflow(name: &str) -> anyhow::Result<PathBuf> {
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    if let Some(p) = &project_root {
        let candidate = p.join(".rupu/workflows").join(format!("{name}.yaml"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let global = paths::global_dir()?;
    let candidate = global.join("workflows").join(format!("{name}.yaml"));
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(anyhow::anyhow!("workflow not found: {name}"))
}

async fn run(name: &str, inputs: Vec<(String, String)>, mode: Option<&str>) -> anyhow::Result<()> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;
    let workflow = Workflow::parse(&body)?;

    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Workspace upsert (mirrors `rupu run`).
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &pwd)?;

    // Credential resolver (shared across all steps in this workflow run).
    let resolver = Arc::new(rupu_auth::KeychainResolver::new());

    let mode_str = mode.unwrap_or("ask").to_string();
    let transcripts = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts)?;

    let factory = Arc::new(CliStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str,
    });

    let inputs_map: BTreeMap<String, String> = inputs.into_iter().collect();
    let opts = OrchestratorRunOpts {
        workflow,
        inputs: inputs_map,
        workspace_id: ws.id,
        workspace_path: pwd.clone(),
        transcript_dir: transcripts,
        factory,
    };

    let result = run_workflow(opts).await?;
    for sr in &result.step_results {
        println!(
            "rupu: step {} run {} -> {}",
            sr.step_id,
            sr.run_id,
            sr.transcript_path.display()
        );
    }
    Ok(())
}

/// `StepFactory` impl that resolves each step's `agent:` against
/// the project- and global-scope `agents/` dirs and constructs a
/// real provider via [`provider_factory::build_for_provider`].
struct CliStepFactory {
    workflow: Workflow,
    global: PathBuf,
    project_root: Option<PathBuf>,
    resolver: Arc<rupu_auth::KeychainResolver>,
    mode_str: String,
}

#[async_trait]
impl StepFactory for CliStepFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
    ) -> AgentRunOpts {
        let step = self
            .workflow
            .steps
            .iter()
            .find(|s| s.id == step_id)
            .expect("step_id from orchestrator must match a workflow step");

        // The agent loader takes the parent of `agents/`. For the
        // project layer that's `<project>/.rupu`; the global layer is
        // `<global>` directly (which already contains `agents/`).
        let project_agents_parent = self.project_root.as_ref().map(|p| p.join(".rupu"));
        let spec =
            rupu_agent::load_agent(&self.global, project_agents_parent.as_deref(), &step.agent)
                .unwrap_or_else(|_| {
                    // Fallback: synthesize a minimal AgentSpec with the
                    // rendered prompt as system prompt so the factory contract
                    // is honored even when the agent file is missing. The
                    // agent loop will surface the failure via run_complete{
                    // status: Error}.
                    rupu_agent::AgentSpec {
                        name: step.agent.clone(),
                        description: None,
                        provider: Some("anthropic".to_string()),
                        model: Some("claude-sonnet-4-6".to_string()),
                        auth: None,
                        tools: None,
                        max_turns: Some(50),
                        permission_mode: Some(self.mode_str.clone()),
                        anthropic_oauth_prefix: None,
                        effort: None,
                        context_window: None,
                        system_prompt: rendered_prompt.clone(),
                    }
                });

        let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
        let model = spec
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".into());
        let auth_hint = spec.auth;
        let (_resolved_auth, provider) = provider_factory::build_for_provider(
            &provider_name,
            &model,
            auth_hint,
            self.resolver.as_ref(),
        )
        .await
        .expect("provider build failed in step factory");

        AgentRunOpts {
            agent_name: spec.name,
            agent_system_prompt: spec.system_prompt,
            agent_tools: spec.tools,
            provider,
            provider_name,
            model,
            run_id,
            workspace_id,
            workspace_path: workspace_path.clone(),
            transcript_path,
            max_turns: spec.max_turns.unwrap_or(50),
            decider: Arc::new(BypassDecider) as Arc<dyn PermissionDecider>,
            tool_context: ToolContext {
                workspace_path,
                bash_env_allowlist: Vec::new(),
                bash_timeout_secs: 120,
            },
            user_message: rendered_prompt,
            mode_str: self.mode_str.clone(),
            no_stream: false,
            effort: spec.effort,
            context_window: spec.context_window,
        }
    }
}
