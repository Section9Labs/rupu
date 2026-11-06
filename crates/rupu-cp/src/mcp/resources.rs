//! The read-only resources `/mcp` serves (spec M9): the docs, the
//! finding-report schema, the query registries, the built-in engagement
//! profiles, the tool catalog and the repo's `examples/`. Everything is
//! embedded in the binary or built from its registries, never read from
//! `RUPU_HOME`, so an agent learns the formats of the binary it talks to.

use async_trait::async_trait;
use bytes::Bytes;
use rupu_mcp::{EmbeddedResource, ResourceDef, ResourceSource, ResourceTemplate};
use std::sync::LazyLock;

/// One served resource.
struct Entry {
    def: ResourceDef,
    body: Bytes,
}

/// `(name, description, text)` of each doc under `docs/`.
macro_rules! docs {
    ($(($name:literal, $description:literal)),* $(,)?) => {
        &[$(($name, $description, include_str!(concat!("../../../../docs/", $name, ".md")))),*]
    };
}

/// Every doc served as `rupu://docs/<name>`.
static DOCS: &[(&str, &str, &str)] = docs![
    (
        "using-rupu",
        "Using rupu: projects, agents, workflows, runs and the day-to-day CLI."
    ),
    (
        "agent-format",
        "Agent file format reference: every frontmatter key of an agent `.md`."
    ),
    (
        "agent-authoring",
        "How to write a good agent: scope, prompts, tools and failure modes."
    ),
    (
        "workflow-format",
        "Workflow file format reference: every key of a workflow YAML."
    ),
    (
        "workflow-authoring",
        "How to write a good workflow: steps, fan-out, gates and actions."
    ),
    (
        "agentiflows",
        "Agentiflows: goal-directed fleet runs, their definition file and envelope."
    ),
    (
        "triggers",
        "Workflow triggers: cron, polled SCM events and webhooks."
    ),
    (
        "engagement-profiles",
        "Engagement profiles: asset kinds, evidence blocks and coverage ladders."
    ),
    (
        "coverage",
        "The coverage harness and findings: ledgers, reports, the findings query language."
    ),
    (
        "messaging",
        "Messaging: message spaces, addresses, board posts and operator messages."
    ),
    (
        "skills",
        "Agent Skills: SKILL.md skills, grants and loading them in runs and sessions."
    ),
    (
        "transcript-schema",
        "Transcript schema reference: every JSONL transcript event."
    ),
    (
        "cp-api",
        "The control-plane HTTP API (`/api/*`) that the operator tools carry."
    ),
    (
        "control-plane",
        "Control-plane operator guide: `rupu cp serve`, hosts and the web UI."
    ),
    (
        "response-outcomes",
        "Response outcomes and recovery: how a bad model reply is classified and retried."
    ),
    (
        "netflow",
        "Netflow: the network connections a run makes and where they are recorded."
    ),
    (
        "configuration",
        "Configuration reference: `config.toml` layers and every key."
    ),
    (
        "mcp",
        "rupu's MCP server: `/mcp`, its tools, `wait`, resources and prompts."
    ),
];

/// `(path, text)` of each file under `examples/`.
macro_rules! examples {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../../../examples/", $path)))),*]
    };
}

/// Every file under `examples/`, served as `rupu://examples/<path>`.
/// `tests/it/mcp_resources.rs` fails when this and the directory differ.
static EXAMPLES: &[(&str, &str)] = examples![
    "README.md",
    "agents/code-reviewer.md",
    "agents/finding-fixer.md",
    "agents/issue-commenter.md",
    "agents/issue-understander.md",
    "agents/maintainability-reviewer.md",
    "agents/performance-reviewer.md",
    "agents/phase-planner.md",
    "agents/pr-author.md",
    "agents/repo-implementer.md",
    "agents/repo-investigator.md",
    "agents/security-assessor.md",
    "agents/security-reviewer.md",
    "agents/spec-writer.md",
    "agents/writer.md",
    "contracts/autoflow_outcome_v1.json",
    "contracts/phase_plan_v1.json",
    "contracts/review_packet_v1.json",
    "contracts/workflow_dispatch_v1.json",
    "workflows/code-review-panel.yaml",
    "workflows/issue-supervisor-dispatch.yaml",
    "workflows/issue-to-spec-and-plan.yaml",
    "workflows/phase-delivery-cycle.yaml",
    "workflows/phase-ready-autoflow.yaml",
    "workflows/quick-bugfix.yaml",
    "workflows/review-changed-files.yaml",
];

/// Each list query's field registry, served as `rupu://query/<list>`.
fn query_registries() -> Vec<(&'static str, &'static str, serde_json::Value)> {
    use crate::query::{agent_runs, autoflow_runs, events, sessions, workflow_runs};
    let json = |fields: crate::query::Fields| serde_json::to_value(fields).expect("registry");
    vec![
        (
            "findings",
            "`q` of `findings.list`",
            serde_json::to_value(rupu_coverage::ledger::query_lang::FIELDS).expect("registry"),
        ),
        (
            "workflow_runs",
            "`q` of the workflow run lists",
            json(workflow_runs::FIELDS),
        ),
        (
            "agent_runs",
            "`q` of the agent run lists",
            json(agent_runs::FIELDS),
        ),
        (
            "sessions",
            "`q` of the session lists",
            json(sessions::FIELDS),
        ),
        (
            "autoflow_runs",
            "`q` of the autoflow run list",
            json(autoflow_runs::FIELDS),
        ),
        ("events", "`q` of the event feed", json(events::FIELDS)),
    ]
}

/// The MIME type an example is served with, by extension.
fn example_mime(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("md") => "text/markdown",
        Some("yaml" | "yml") => "application/yaml",
        Some("json") => "application/json",
        _ => "text/plain",
    }
}

fn entry(uri: String, description: String, mime: &str, body: Bytes) -> Entry {
    Entry {
        def: ResourceDef {
            name: uri.trim_start_matches("rupu://").to_string(),
            uri,
            description,
            mime_type: mime.to_string(),
        },
        body,
    }
}

fn pretty(v: &impl serde::Serialize) -> Bytes {
    Bytes::from(serde_json::to_string_pretty(v).expect("serializable"))
}

static ENTRIES: LazyLock<Vec<Entry>> = LazyLock::new(|| {
    let mut out = Vec::new();
    for (name, description, text) in DOCS {
        out.push(entry(
            format!("rupu://docs/{name}"),
            description.to_string(),
            "text/markdown",
            Bytes::from_static(text.as_bytes()),
        ));
    }
    out.push(entry(
        "rupu://schema/finding-report".into(),
        "JSON Schema (draft-07) of a structured finding report, the `report` of `findings.report`."
            .into(),
        "application/schema+json",
        pretty(&rupu_coverage::report::schema::canonical_schema()),
    ));
    for (list, used_by, fields) in query_registries() {
        out.push(entry(
            format!("rupu://query/{list}"),
            format!(
                "The query fields of {used_by}: key, aliases, kind, match mode and allowed values."
            ),
            "application/json",
            pretty(&fields),
        ));
    }
    for (id, toml) in rupu_coverage::profile::builtin_profile_sources() {
        out.push(entry(
            format!("rupu://profiles/{id}"),
            format!("The built-in `{id}` engagement profile, as its TOML."),
            "application/toml",
            Bytes::from_static(toml.as_bytes()),
        ));
    }
    out.push(entry(
        "rupu://tools".into(),
        "The agent tool catalog (`GET /api/tools`): every tool an agent's `tools:` can grant."
            .into(),
        "application/json",
        pretty(&crate::api::tools::catalog()),
    ));
    for (path, text) in EXAMPLES {
        out.push(entry(
            format!("rupu://examples/{path}"),
            format!("Example from the rupu repo: `examples/{path}`."),
            example_mime(path),
            Bytes::from_static(text.as_bytes()),
        ));
    }
    out
});

/// The resources above, as an MCP resource source.
pub struct RupuResources;

#[async_trait]
impl ResourceSource for RupuResources {
    fn resources(&self) -> Vec<ResourceDef> {
        ENTRIES.iter().map(|e| e.def.clone()).collect()
    }

    fn templates(&self) -> Vec<ResourceTemplate> {
        let template =
            |uri: &str, name: &str, description: &str, mime: Option<&str>| ResourceTemplate {
                uri_template: uri.into(),
                name: name.into(),
                description: description.into(),
                mime_type: mime.map(str::to_string),
            };
        vec![
            template(
                "rupu://docs/{name}",
                "docs",
                "A rupu doc, as embedded in this binary.",
                Some("text/markdown"),
            ),
            template(
                "rupu://query/{list}",
                "query",
                "A list query's field registry: findings, workflow_runs, agent_runs, sessions, autoflow_runs, events.",
                Some("application/json"),
            ),
            template(
                "rupu://profiles/{id}",
                "profiles",
                "A built-in engagement profile, as its TOML.",
                Some("application/toml"),
            ),
            template(
                "rupu://examples/{+path}",
                "examples",
                "A file under the repo's `examples/`: agents, workflows and contracts.",
                None,
            ),
        ]
    }

    async fn read(&self, uri: &str) -> Option<EmbeddedResource> {
        let e = ENTRIES.iter().find(|e| e.def.uri == uri)?;
        Some(EmbeddedResource {
            uri: e.def.uri.clone(),
            mime_type: e.def.mime_type.clone(),
            name: None,
            bytes: e.body.clone(),
        })
    }
}
