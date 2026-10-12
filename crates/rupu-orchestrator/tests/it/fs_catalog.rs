//! `FsCatalog`: the agent/workflow catalog port every launch site hands its
//! assembler, driven through the roster catalog tools the way a run calls
//! them (W5 §3.1a).

use rupu_orchestrator::FsCatalog;
use rupu_tools::{ToolContext, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

const RECON: &str = "---\nname: recon\ndescription: Surveys the layout of a repository.\ntools: [read_file, grep]\n---\nYou survey a repository.\n";
const WRITER: &str = "---\nname: writer\n---\nYou write text.\n";
const ONE_STEP: &str = "name: Plain Flow\ndescription: A single placeholder step.\ninputs:\n  topic:\n    type: string\nsteps:\n  - id: only\n    agent: writer\n    prompt: hi\n";

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Two agents (one described) + one workflow whose stem differs from its
/// declared name, in the global layer.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    write(&global, "agents/recon.md", RECON);
    write(&global, "agents/writer.md", WRITER);
    write(&global, "workflows/plain-flow.yaml", ONE_STEP);
    (tmp, global)
}

fn ctx(catalog: FsCatalog) -> ToolContext {
    let mut ctx = ToolContext::default();
    ctx.services.catalog = Some(Arc::new(catalog));
    ctx
}

async fn call(ctx: &ToolContext, name: &str, input: Value) -> ToolOutput {
    rupu_tools::bodies::body(name, ctx)
        .unwrap()
        .invoke(input, ctx)
        .await
        .unwrap()
}

fn json_of(out: &ToolOutput) -> Value {
    assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
    serde_json::from_str(&out.stdout).unwrap()
}

#[tokio::test]
async fn agents_list_returns_names_descriptions_and_tools() {
    let (_tmp, global) = fixture();
    let v = json_of(
        &call(
            &ctx(FsCatalog::new(&global, None)),
            "agents.list",
            json!({}),
        )
        .await,
    );
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let recon = rows.iter().find(|r| r["name"] == "recon").unwrap();
    assert_eq!(recon["description"], "Surveys the layout of a repository.");
    assert_eq!(recon["tools"], json!(["read_file", "grep"]));
    let writer = rows.iter().find(|r| r["name"] == "writer").unwrap();
    assert!(writer["description"].is_null());
    // Compact: never the system prompt / raw file.
    assert!(!v.to_string().contains("You survey"));
}

#[tokio::test]
async fn a_bad_agent_file_is_reported_and_search_still_finds_workflows() {
    let (_tmp, global) = fixture();
    write(&global, "agents/broken.md", "no frontmatter here\n");
    let c = ctx(FsCatalog::new(&global, None));
    let out = call(&c, "agents.list", json!({})).await;
    let err = out.error.expect("a bad .md is reported, not propagated");
    assert!(err.starts_with("could not load agents:"), "{err}");
    assert!(err.contains("broken.md"), "names the bad file: {err}");

    let v = json_of(&call(&c, "catalog.search", json!({"query": "plain"})).await);
    assert_eq!(v["hits"].as_array().unwrap().len(), 1);
    let w = v["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 1);
    assert!(w[0].as_str().unwrap().contains("broken.md"));
}

#[tokio::test]
async fn workflows_list_still_lists_a_broken_file() {
    let (_tmp, global) = fixture();
    write(
        &global,
        "workflows/broken.yaml",
        "name: [unclosed\nsteps: {",
    );
    let v = json_of(
        &call(
            &ctx(FsCatalog::new(&global, None)),
            "workflows.list",
            json!({}),
        )
        .await,
    );
    let broken = v
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "broken")
        .unwrap();
    assert!(broken["parse_error"].is_string());
}

#[tokio::test]
async fn workflows_get_addresses_by_id_and_carries_its_inputs() {
    let (_tmp, global) = fixture();
    let c = ctx(FsCatalog::new(&global, None));
    let v = json_of(&call(&c, "workflows.get", json!({"id": "plain-flow"})).await);
    assert_eq!(v["id"], "plain-flow");
    assert_eq!(v["name"], "Plain Flow");
    assert_eq!(v["scope"], "global");
    assert_eq!(v["step_count"], 1);
    assert_eq!(v["input_keys"], json!(["topic"]));
}

#[tokio::test]
async fn the_project_rupu_dir_shadows_global() {
    let (tmp, global) = fixture();
    let project_root = tmp.path().join("proj");
    let project = project_root.join(".rupu");
    write(
        &project,
        "agents/recon.md",
        "---\nname: recon\ndescription: Project-local recon.\n---\nbody\n",
    );
    write(&project, "workflows/local-flow.yaml", ONE_STEP);
    let c = ctx(FsCatalog::for_project_root(&global, Some(&project_root)));
    let agents = json_of(&call(&c, "agents.list", json!({})).await);
    let recon = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "recon")
        .unwrap();
    assert_eq!(recon["description"], "Project-local recon.");
    let wfs = json_of(&call(&c, "workflows.list", json!({})).await);
    let ids: Vec<_> = wfs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, ["local-flow", "plain-flow"]);
}
