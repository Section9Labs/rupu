//! `/mcp`'s resources and prompts (spec M9): every resource lists, reads back
//! as shipped, and every prompt's links resolve.

#![allow(clippy::disallowed_methods)]

use rupu_cp::mcp::McpOpts;
use rupu_cp::server::{router_with, RouterOpts};
use rupu_cp::state::AppState;
use rupu_mcp::McpServer;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The docs the spec names (M9) plus `mcp`.
const DOCS: &[&str] = &[
    "using-rupu",
    "agent-format",
    "agent-authoring",
    "workflow-format",
    "workflow-authoring",
    "agentiflows",
    "triggers",
    "engagement-profiles",
    "coverage",
    "messaging",
    "skills",
    "transcript-schema",
    "cp-api",
    "control-plane",
    "response-outcomes",
    "netflow",
    "configuration",
    "mcp",
];

const QUERY_LISTS: &[&str] = &[
    "findings",
    "workflow_runs",
    "agent_runs",
    "sessions",
    "autoflow_runs",
    "events",
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn mcp_opts() -> McpOpts {
    McpOpts {
        catalog: None,
        tools: vec![],
    }
}

fn server() -> (Arc<McpServer>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(dir.path().to_path_buf(), Default::default());
    (
        rupu_cp::mcp::build_server(&state, &mcp_opts()).unwrap(),
        dir,
    )
}

async fn call(server: &McpServer, method: &str, params: Value) -> Value {
    server
        .handle(
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
            None,
        )
        .await
        .expect("a reply")
}

/// Every listed resource, across pages.
async fn list_all(server: &McpServer) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let r = call(server, "resources/list", json!({"cursor": cursor})).await;
        out.extend(
            r["result"]["resources"]
                .as_array()
                .expect("resources")
                .clone(),
        );
        match r["result"].get("nextCursor") {
            Some(c) => cursor = c.clone(),
            None => return out,
        }
    }
}

/// `(mimeType, text)` of a resource.
async fn read(server: &McpServer, uri: &str) -> (String, String) {
    let r = call(server, "resources/read", json!({"uri": uri})).await;
    let c = &r["result"]["contents"];
    assert_eq!(c.as_array().map(Vec::len), Some(1), "{uri}: {r}");
    assert_eq!(c[0]["uri"], uri);
    (
        c[0]["mimeType"].as_str().unwrap().to_string(),
        c[0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{uri}: no text: {r}"))
            .to_string(),
    )
}

/// Every file under `examples/`, relative, `/`-separated.
fn example_files() -> BTreeSet<String> {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeSet<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            // Not part of the repo: a Finder `.DS_Store`, an editor's swap file.
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                let rel = p.strip_prefix(base).unwrap();
                out.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let base = repo().join("examples");
    let mut out = BTreeSet::new();
    walk(&base, &base, &mut out);
    out
}

#[tokio::test]
async fn resources_list_names_every_resource_and_each_reads_back_with_its_type() {
    let (s, _tmp) = server();
    let listed = list_all(&s).await;
    let uris: BTreeSet<String> = listed
        .iter()
        .map(|d| d["uri"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(uris.len(), listed.len(), "a URI is listed twice");

    let mut want = BTreeSet::new();
    want.extend(DOCS.iter().map(|d| format!("rupu://docs/{d}")));
    want.insert("rupu://schema/finding-report".to_string());
    want.extend(QUERY_LISTS.iter().map(|l| format!("rupu://query/{l}")));
    want.extend(
        rupu_coverage::profile::builtin_profiles()
            .keys()
            .map(|id| format!("rupu://profiles/{id}")),
    );
    want.insert("rupu://tools".to_string());
    want.extend(
        example_files()
            .iter()
            .map(|p| format!("rupu://examples/{p}")),
    );
    assert_eq!(uris, want);

    for d in &listed {
        let uri = d["uri"].as_str().unwrap();
        let (mime, text) = read(&s, uri).await;
        assert_eq!(d["mimeType"], mime, "{uri}");
        assert!(!text.trim().is_empty(), "{uri} is empty");
        assert!(d["description"].as_str().is_some_and(|t| !t.is_empty()));
        let want_mime = match uri {
            u if u.starts_with("rupu://docs/") => "text/markdown",
            "rupu://schema/finding-report" => "application/schema+json",
            u if u.starts_with("rupu://query/") || u == "rupu://tools" => "application/json",
            u if u.starts_with("rupu://profiles/") => "application/toml",
            u if u.ends_with(".md") => "text/markdown",
            u if u.ends_with(".yaml") => "application/yaml",
            u if u.ends_with(".json") => "application/json",
            u => panic!("unexpected resource {u}"),
        };
        assert_eq!(mime, want_mime, "{uri}");
    }
}

#[tokio::test]
async fn docs_and_examples_read_back_byte_for_byte() {
    let (s, _tmp) = server();
    let (_, cp_api) = read(&s, "rupu://docs/cp-api").await;
    assert_eq!(
        cp_api,
        std::fs::read_to_string(repo().join("docs/cp-api.md")).unwrap()
    );
    for d in DOCS {
        let (_, text) = read(&s, &format!("rupu://docs/{d}")).await;
        let file = std::fs::read_to_string(repo().join(format!("docs/{d}.md"))).unwrap();
        assert!(text == file, "rupu://docs/{d} differs from docs/{d}.md");
    }
    for p in example_files() {
        let (_, text) = read(&s, &format!("rupu://examples/{p}")).await;
        let file = std::fs::read_to_string(repo().join("examples").join(&p)).unwrap();
        assert!(
            text == file,
            "rupu://examples/{p} differs from examples/{p}"
        );
    }
}

#[tokio::test]
async fn registries_schema_profiles_and_tools_match_their_sources() {
    let (s, _tmp) = server();
    let parse = |t: String| serde_json::from_str::<Value>(&t).unwrap();

    let (_, schema) = read(&s, "rupu://schema/finding-report").await;
    assert_eq!(
        parse(schema),
        rupu_coverage::report::schema::canonical_schema()
    );

    for list in QUERY_LISTS {
        let fixture = if *list == "findings" {
            repo().join("crates/rupu-coverage/tests/fixtures/finding_query/fields.json")
        } else {
            repo().join(format!(
                "crates/rupu-cp/tests/fixtures/list_query/{list}.fields.json"
            ))
        };
        let (_, text) = read(&s, &format!("rupu://query/{list}")).await;
        assert_eq!(
            parse(text),
            parse(std::fs::read_to_string(&fixture).unwrap()),
            "rupu://query/{list} vs {}",
            fixture.display()
        );
    }

    for (id, profile) in rupu_coverage::profile::builtin_profiles() {
        let (_, toml) = read(&s, &format!("rupu://profiles/{id}")).await;
        let back = rupu_coverage::profile::parse_profile(&toml).unwrap();
        assert_eq!(back.id, profile.id);
    }

    // The same rows as `GET /api/tools`.
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(dir.path().to_path_buf(), Default::default());
    let app = router_with(
        state,
        None,
        RouterOpts {
            web: false,
            mcp: None,
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let api: Value = reqwest::get(format!("http://{addr}/api/tools"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let (_, tools) = read(&s, "rupu://tools").await;
    assert_eq!(parse(tools), api);
    assert!(api["tools"].as_array().unwrap().len() > 10);
}

#[tokio::test]
async fn templates_cover_each_parameterized_uri() {
    let (s, _tmp) = server();
    let r = call(&s, "resources/templates/list", json!({})).await;
    let templates: Vec<&str> = r["result"]["resourceTemplates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["uriTemplate"].as_str().unwrap())
        .collect();
    assert_eq!(
        templates,
        [
            "rupu://docs/{name}",
            "rupu://query/{list}",
            "rupu://profiles/{id}",
            "rupu://examples/{+path}",
        ]
    );
}

#[tokio::test]
async fn an_unknown_uri_is_resource_not_found_naming_it() {
    let (s, _tmp) = server();
    for uri in ["rupu://docs/nope", "rupu://docs", "file:///etc/passwd"] {
        let r = call(&s, "resources/read", json!({"uri": uri})).await;
        assert_eq!(
            r["error"],
            json!({"code": -32002, "message": format!("resource not found: {uri}"),
                   "data": {"uri": uri}}),
            "{uri}"
        );
    }
}

/// The `rupu://` links in a prompt's text.
fn links(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|w| w.strip_prefix("rupu://"))
        .map(|rest| format!("rupu://{}", rest.trim_end_matches([':', ',', '.', ')'])))
        .collect()
}

#[tokio::test]
async fn every_prompt_renders_with_links_that_resolve() {
    let (s, _tmp) = server();
    let r = call(&s, "prompts/list", json!({})).await;
    let names: Vec<&str> = r["result"]["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "author-agent",
            "author-workflow",
            "author-agentiflow",
            "triage-findings"
        ]
    );
    for name in names {
        let r = call(&s, "prompts/get", json!({"name": name})).await;
        let msgs = r["result"]["messages"].as_array().expect("messages");
        assert_eq!(msgs.len(), 1, "{r}");
        assert_eq!(msgs[0]["role"], "user");
        let text = msgs[0]["content"]["text"].as_str().unwrap();
        let found = links(text);
        assert!(!found.is_empty(), "{name} links nothing");
        for uri in found {
            let r = call(&s, "resources/read", json!({"uri": uri})).await;
            assert!(
                r.get("result").is_some(),
                "{name}: {uri} does not resolve: {r}"
            );
        }
    }
}

#[tokio::test]
async fn triage_findings_starts_from_the_projects_query() {
    let (s, _tmp) = server();
    let text = |r: Value| {
        r["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let all = text(call(&s, "prompts/get", json!({"name": "triage-findings"})).await);
    assert!(all.contains("Start with `findings.list` {}."), "{all}");
    let one = text(
        call(
            &s,
            "prompts/get",
            json!({"name": "triage-findings", "arguments": {"project": "web \"app\""}}),
        )
        .await,
    );
    assert!(
        one.contains(r#"Start with `findings.list` {"q":"project:\"web \\\"app\\\"\""}."#),
        "{one}"
    );
    // The query the prompt names parses.
    let q = "project:\"web \\\"app\\\"\"";
    let parsed = rupu_coverage::ledger::query_lang::parse_query(q).unwrap();
    assert_eq!(
        serde_json::to_value(&parsed).unwrap()["terms"][0]["values"],
        json!(["web \"app\""])
    );
}

#[tokio::test]
async fn initialize_on_mcp_advertises_resources_and_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(dir.path().to_path_buf(), Default::default());
    let server = rupu_cp::mcp::build_server(&state, &mcp_opts()).unwrap();
    let app = router_with(
        state,
        None,
        RouterOpts {
            web: false,
            mcp: Some(server),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let v: Value = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "t", "version": "0"}}}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["result"]["capabilities"],
        json!({"tools": {}, "resources": {}, "prompts": {}}),
        "{v}"
    );
}
