//! The definitions operator tools: the workflows and agents the CP can run.

use super::{op, OpSpec};
use crate::api::{
    agents, autoflow_claims, autoflows, models, repo_scope::ScopeQuery, skills, workflows,
};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "workflows.list",
            Method::GET,
            "/api/workflows",
            "List the workflows this control plane can run.\n\nTakes no arguments. Returns a JSON array of workflow summaries (name, scope, description).",
        ),
        op(
            "workflows.run",
            Method::POST,
            "/api/workflows/:name/run",
            "Launch a workflow run.\n\n`name` is the workflow; `inputs` maps input names to string values; `mode` (ask|bypass|readonly), `target`, `working_dir`, `host`, `scope_kind`/`scope_id` and `engagement_profiles` (local launches only) are optional. For a workflow given as YAML, see workflows.run_yaml. Returns `{run_id, host_id}`.",
        )
        .body::<workflows::LaunchBody>()
        .open_world(),
        op(
            "agents.list",
            Method::GET,
            "/api/agents",
            "List the agents this control plane can run.\n\nTakes no arguments. Returns a JSON array of agent definitions (name, description, provider, model, tools).",
        ),
        op(
            "agents.run",
            Method::POST,
            "/api/agents/:name/run",
            "Launch a standalone agent run.\n\n`name` is the agent; `prompt` is its task; `mode`, `target`, `working_dir`, `host`, `skills`, `engagement_profiles`, `model` and `provider` (these two on local launches only) and the other launch options are optional. Returns `{run_id, host_id}`.",
        )
        .body::<agents::AgentRunBody>()
        .open_world(),
        op(
            "agents.get",
            Method::GET,
            "/api/agents/:name",
            "Read one agent definition.\n\n`name` is the agent's file stem. Returns the agent object (name, description, provider, model, tools, scope, ...) plus `system_prompt` and `raw`, the whole definition file.",
        ),
        op(
            "agents.create",
            Method::POST,
            "/api/agents",
            "Create a new agent definition in the global layer.\n\n`raw` is the whole `.md` file (frontmatter and body); the name comes from its `name:`. Returns the new agent as `agents.get` does. 400 if the file does not parse, 409 if an agent of that name already exists in any layer.",
        )
        .body::<agents::AgentWriteBody>(),
        op(
            "agents.save",
            Method::PUT,
            "/api/agents/:name",
            "Overwrite an agent definition with a new file.\n\n`raw` is the whole `.md` file and its `name:` must equal `name`. `scope_kind` (global|project) with `scope_id` (the project's workspace id) picks the layer; without them the project-first match is overwritten, or a new global file is made. Returns the saved agent as `agents.get` does.",
        )
        .query::<ScopeQuery>()
        .body::<agents::AgentWriteBody>(),
        op(
            "agents.delete",
            Method::DELETE,
            "/api/agents/:name",
            "Delete an agent definition file.\n\n`scope_kind` (global|project) with `scope_id` picks the layer; without them the project-first match is deleted. Returns `{deleted, scope, scope_kind}`; 404 if nothing matches.",
        )
        .query::<ScopeQuery>(),
        op(
            "agents.generate",
            Method::POST,
            "/api/agents/generate",
            "Draft an agent definition from a description with a model.\n\n`description` is required; `provider` and `model` are optional. Returns `{raw, provider, model, attempts}` and writes nothing: pass `raw` to `agents.create`. 501 when the control plane has no generator.",
        )
        .body::<agents::GenerateAgentBody>()
        .open_world(),
        op(
            "workflows.get",
            Method::GET,
            "/api/workflows/:name",
            "Read one workflow definition.\n\n`name` is the workflow's file stem. Returns `{workflow, yaml, usage, scope, scope_kind, scope_id}`: the parsed workflow, its raw YAML and its usage rollup.",
        ),
        op(
            "workflows.create",
            Method::POST,
            "/api/workflows",
            "Create a new workflow definition in the global layer.\n\n`raw` is the whole `.yaml` file; the name comes from its `name:`. Returns the new workflow as `workflows.get` does. 400 if the file does not parse, 409 if a workflow of that name already exists in any layer.",
        )
        .body::<workflows::WorkflowWriteBody>(),
        op(
            "workflows.save",
            Method::PUT,
            "/api/workflows/:name",
            "Overwrite a workflow definition with a new file.\n\n`raw` is the whole `.yaml` file and its `name:` must equal `name`. `scope_kind` (global|project) with `scope_id` (the project's workspace id) picks the layer; without them the project-first match is overwritten, or a new global file is made. Returns the saved workflow as `workflows.get` does.",
        )
        .query::<ScopeQuery>()
        .body::<workflows::WorkflowWriteBody>(),
        op(
            "workflows.delete",
            Method::DELETE,
            "/api/workflows/:name",
            "Delete a workflow definition file.\n\n`scope_kind` (global|project) with `scope_id` picks the layer; without them the project-first match is deleted. Returns `{deleted, scope, scope_kind}`; 404 if nothing matches.",
        )
        .query::<ScopeQuery>(),
        op(
            "workflows.validate",
            Method::POST,
            "/api/workflows/validate",
            "Check that workflow YAML parses, without saving it.\n\n`raw` is the whole `.yaml` file. Returns `{ok: true}`, or an error `HTTP 400` carrying the parse message when it does not parse.",
        )
        .body::<workflows::WorkflowWriteBody>()
        .read_only(),
        op(
            "workflows.generate",
            Method::POST,
            "/api/workflows/generate",
            "Draft a workflow definition from a description with a model.\n\n`description` is required; `provider` and `model` are optional. Returns `{raw, provider, model, attempts}` and writes nothing: pass `raw` to `workflows.create`. 501 when the control plane has no generator.",
        )
        .body::<workflows::GenerateWorkflowBody>()
        .open_world(),
        op(
            "generate.models",
            Method::GET,
            "/api/generate/models",
            "List the providers and models the definition generator can use.\n\nTakes no arguments. Returns a JSON array of `{provider, models, is_default}`; empty when the control plane has no generator.",
        ),
        op(
            "autoflows.list",
            Method::GET,
            "/api/autoflows",
            "List the workflows that declare an autoflow.\n\nTakes no arguments. Returns a JSON array of rows (name, slug, trigger, scope, scope_kind, enabled, ...), project definitions shadowing same-named global ones.",
        ),
        op(
            "autoflows.enable",
            Method::POST,
            "/api/autoflows/:name/enable",
            "Turn an autoflow on by editing its workflow file.\n\n`name` is the workflow's file stem; `scope_kind` (global|project) with `scope_id` picks the layer. Returns `{name, enabled}`; 501 on a read-only control plane.",
        )
        .query::<ScopeQuery>(),
        op(
            "autoflows.disable",
            Method::POST,
            "/api/autoflows/:name/disable",
            "Turn an autoflow off by editing its workflow file.\n\n`name` is the workflow's file stem; `scope_kind` (global|project) with `scope_id` picks the layer. Returns `{name, enabled}`; 501 on a read-only control plane.",
        )
        .query::<ScopeQuery>(),
        op(
            "claims.list",
            Method::GET,
            "/api/autoflows/claims",
            "List the issue claims the autoflow engine tracks.\n\nTakes no arguments. Returns a JSON array of claims (issue_ref, repo_ref, workflow, status, last_run_id, lease_expires_at, ...).",
        ),
        op(
            "claims.release",
            Method::POST,
            "/api/autoflows/claims/release",
            "Delete the claim an autoflow holds on an issue.\n\n`issue_ref` names the claim. Returns `{released}`, false when no claim was tracked; releasing twice is harmless.",
        )
        .body::<autoflow_claims::ReleaseBody>()
        .destructive(),
        op(
            "claims.requeue",
            Method::POST,
            "/api/autoflows/claims/requeue",
            "Queue a manual wake for a claimed issue.\n\n`issue_ref` names the claim; `not_before` (RFC 3339) defers the wake, and an unparseable value means now. Returns `{wake_id}`; 404 when no claim is tracked.",
        )
        .body::<autoflow_claims::RequeueBody>(),
        // Autoflow operations only the CLI had (Plan 3a, package E).
        op(
            "autoflows.get",
            Method::GET,
            "/api/autoflows/:name",
            "Show one autoflow's resolved metadata and its YAML (rupu autoflow show).\n\n`name` is the autoflow's workflow name; `repo` (`github:owner/repo`) picks one when several repos define it. Returns the `autoflow_show` report: repo, entity, source, selector, workspace, claim TTL, `body`; 404 when none is visible, 409 when several are.",
        )
        .query::<autoflows::AutoflowRepoQuery>(),
        op(
            "autoflows.explain",
            Method::GET,
            "/api/autoflows/explain",
            "Explain the autoflow state of one issue (rupu autoflow explain).\n\n`issue_ref` is the issue in full form (`github:owner/repo/issues/42`; a bare number is a 400); `repo` limits resolution to one bound repo. Returns the `autoflow_explain` report: status, claim, last run, queued wakes, recent cycle events.",
        )
        .query::<autoflows::ExplainQuery>(),
        op(
            "autoflows.doctor",
            Method::GET,
            "/api/autoflows/doctor",
            "Check autoflow claims, wakes and runs for inconsistencies (rupu autoflow doctor).\n\n`repo` limits the check to one tracked repo. Returns the `autoflow_doctor` report: `ok`, and one row (scope, problem, detail) per problem found.",
        )
        .query::<autoflows::AutoflowRepoQuery>(),
        op(
            "autoflows.wakes",
            Method::GET,
            "/api/autoflows/wakes",
            "List queued and recently processed autoflow wakes (rupu autoflow wakes).\n\n`repo` limits it to one tracked repo. Returns the `autoflow_wakes` report: rows (wake_id, state queued|processed, source, event, entity, not_before, repo).",
        )
        .query::<autoflows::AutoflowRepoQuery>(),
        op(
            "autoflows.run",
            Method::POST,
            "/api/autoflows/:name/run",
            "Run one autoflow cycle for an issue now, detached (rupu autoflow run).\n\n`name` is the autoflow; `issue` the issue in full form (`github:owner/repo/issues/42`); `repo` and `mode` (bypass|readonly) are optional. Returns 202 `{name, issue, started}` once the cycle's process starts; follow it with `autoflows.explain`. 404 for an unknown autoflow, 409 when another cycle holds the issue.",
        )
        .query::<crate::admin_ops::HostQuery>()
        .body::<autoflows::AutoflowRunBody>()
        .open_world(),
        op(
            "claims.repair",
            Method::POST,
            "/api/autoflows/claims/repair",
            "Repair one autoflow claim: drop bad wakes, rebuild its worktree (rupu autoflow repair).\n\n`issue_ref` names the claim, in full form (`github:owner/repo/issues/42`); `release` or `requeue` (at most one) also releases it or queues a repair wake. `dry_run` defaults to true and lists the repairs without applying them; pass false to apply. Returns `{issue_ref, dry_run, repairs}`; 404 when no claim is tracked.",
        )
        .query::<crate::admin_ops::HostQuery>()
        .body::<autoflow_claims::RepairBody>()
        .destructive(),
        op(
            "skills.list",
            Method::GET,
            "/api/skills",
            "List the installed Agent Skills.\n\n`ws_id` adds that project's skills to the global ones. Returns `{skills, warnings}`; a skills layer that could not be read is a `warnings` entry, not an error.",
        )
        .query::<skills::SkillsQuery>(),
        op(
            "skills.get",
            Method::GET,
            "/api/skills/:layer/:name",
            "Read one installed skill, body included.\n\n`layer` is global or project (project needs `ws_id`); `name` is the skill. Returns the skill's metadata, its `body` and its other files; 404 if not installed.",
        )
        .query::<skills::SkillsQuery>(),
        op(
            "tools.list",
            Method::GET,
            "/api/tools",
            "List the whole tool catalog.\n\nTakes no arguments. Returns `{tools}`: per tool its name, aliases, namespace, effect, needs, description, input_schema and action_eligible.",
        ),
        op(
            "models.list",
            Method::GET,
            "/api/models",
            "List the models each provider offers, with their limits.\n\nTakes no arguments. Returns a JSON array of providers, each with its models and when the list was fetched; 501 without `cp serve`.",
        ),
        op(
            "models.refresh",
            Method::POST,
            "/api/models/refresh",
            "Re-fetch the model lists from the providers.\n\n`provider` narrows it to one provider; none means all. Returns a JSON array of per-provider outcomes (`ok` or `error`); 400 for an unknown provider, 501 without `cp serve`.",
        )
        .body::<models::RefreshBody>()
        .open_world(),
        // Plan 3a, package B: the YAML launch and agent validation.
        op(
            "workflows.run_yaml",
            Method::POST,
            "/api/workflows/run",
            "Launch a workflow given as YAML, without saving it under a name (`rupu workflow run --file`).\n\n`yaml` is the whole workflow file; `inputs`, `mode` (ask|bypass|readonly), `working_dir` and `engagement_profiles` are optional. The YAML is checked first (HTTP 400 with the parse error), then kept under the control plane's global dir as the run's source. Returns `{run_id, host_id, file}`; local-only (a remote `host` is a 501).",
        )
        .body::<workflows::LaunchYamlBody>()
        .open_world(),
        op(
            "agents.validate",
            Method::POST,
            "/api/agents/validate",
            "Check an agent definition's text without saving it, as `rupu agent validate` checks a file.\n\n`raw` is the whole `.md` file; `ws_id` adds that project's skills layer to the global one. It must parse, its `tools:` must name known tools and its skills must resolve. Returns `{ok: true, name}`, or HTTP 400 naming what is wrong.",
        )
        .body::<agents::ValidateAgentBody>()
        .read_only(),
        op(
            "agents.validate_all",
            Method::GET,
            "/api/agents/validate",
            "Check every agent file, global and project, as `rupu agent validate --all` does.\n\n`ws_id` limits the projects to that one; absent, every registered project is checked. Returns `{ok, rows}`: one row per file with `name`, `layer`, `scope_id`, `path`, `ok` and `error`; `ok` is false when any file failed.",
        )
        .query::<agents::ValidateAgentsQuery>(),
    ]
}
