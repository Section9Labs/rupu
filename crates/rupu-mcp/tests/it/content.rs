//! Resources and prompts: `resources/*` and `prompts/*` over stub sources.

use crate::stub::StubSource;
use async_trait::async_trait;
use rupu_mcp::{
    EmbeddedResource, McpServer, PromptArgument, PromptDef, PromptReply, PromptSource, ResourceDef,
    ResourceSource, ResourceTemplate, ServeError, Sources, ToolFilter, LIST_PAGE,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `stub://doc/<n>` for `n` in `0..count` (markdown `# doc <n>`), plus
/// `stub://bin` (bytes that are not UTF-8).
struct Docs {
    count: usize,
}

#[async_trait]
impl ResourceSource for Docs {
    fn resources(&self) -> Vec<ResourceDef> {
        (0..self.count)
            .map(|n| ResourceDef {
                uri: format!("stub://doc/{n}"),
                name: format!("doc {n}"),
                description: format!("stub doc {n}"),
                mime_type: "text/markdown".into(),
            })
            .chain([ResourceDef {
                uri: "stub://bin".into(),
                name: "bin".into(),
                description: "stub bytes".into(),
                mime_type: "application/octet-stream".into(),
            }])
            .collect()
    }

    fn templates(&self) -> Vec<ResourceTemplate> {
        vec![ResourceTemplate {
            uri_template: "stub://doc/{n}".into(),
            name: "doc".into(),
            description: "a stub doc".into(),
            mime_type: Some("text/markdown".into()),
        }]
    }

    async fn read(&self, uri: &str) -> Option<EmbeddedResource> {
        if uri == "stub://bin" {
            return Some(EmbeddedResource {
                uri: uri.into(),
                mime_type: "application/octet-stream".into(),
                name: None,
                bytes: bytes::Bytes::from_static(&[0xff, 0x00]),
            });
        }
        let n: usize = uri.strip_prefix("stub://doc/")?.parse().ok()?;
        (n < self.count).then(|| EmbeddedResource {
            uri: uri.into(),
            mime_type: "text/markdown".into(),
            name: None,
            bytes: format!("# doc {n}").into(),
        })
    }
}

/// `greet {who (required), tone?}`.
struct Greet;

#[async_trait]
impl PromptSource for Greet {
    fn prompts(&self) -> Vec<PromptDef> {
        vec![PromptDef {
            name: "greet".into(),
            description: "Greet someone.".into(),
            arguments: vec![
                PromptArgument {
                    name: "who".into(),
                    description: "who to greet".into(),
                    required: true,
                },
                PromptArgument {
                    name: "tone".into(),
                    description: "how".into(),
                    required: false,
                },
            ],
        }]
    }

    async fn get(&self, name: &str, args: &BTreeMap<String, String>) -> Option<PromptReply> {
        (name == "greet").then(|| PromptReply {
            description: "Greet someone.".into(),
            text: format!(
                "Say hello to {} ({}).",
                args["who"],
                args.get("tone").map_or("plainly", String::as_str)
            ),
        })
    }
}

fn server(docs: usize) -> McpServer {
    McpServer::from_sources(
        Sources {
            tools: vec![Arc::new(StubSource)],
            resources: vec![Arc::new(Docs { count: docs })],
            prompts: vec![Arc::new(Greet)],
        },
        &ToolFilter::all(),
    )
    .unwrap()
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

#[tokio::test]
async fn a_server_without_content_sources_advertises_and_answers_tools_only() {
    let plain = McpServer::new(vec![Arc::new(StubSource)], &ToolFilter::all()).unwrap();
    let init = call(&plain, "initialize", json!({})).await;
    assert_eq!(init["result"]["capabilities"], json!({"tools": {}}));
    for method in [
        "resources/list",
        "resources/templates/list",
        "resources/read",
        "prompts/list",
        "prompts/get",
    ] {
        let r = call(&plain, method, json!({})).await;
        assert_eq!(r["error"]["code"], -32601, "{method}: {r}");
    }
}

#[tokio::test]
async fn initialize_advertises_resources_and_prompts_when_served() {
    let init = call(&server(1), "initialize", json!({})).await;
    assert_eq!(
        init["result"]["capabilities"],
        json!({"tools": {}, "resources": {}, "prompts": {}})
    );
}

#[tokio::test]
async fn resources_list_pages_with_a_cursor_until_the_end() {
    let s = server(2 * LIST_PAGE + 10);
    let mut uris = Vec::new();
    let mut cursor = Value::Null;
    let mut pages = 0;
    loop {
        let r = call(&s, "resources/list", json!({"cursor": cursor})).await;
        let page = r["result"]["resources"].as_array().unwrap();
        assert!(page.len() <= LIST_PAGE, "{r}");
        uris.extend(page.iter().map(|d| d["uri"].as_str().unwrap().to_string()));
        pages += 1;
        match r["result"].get("nextCursor") {
            Some(c) => cursor = c.clone(),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(uris.len(), 2 * LIST_PAGE + 11);
    assert_eq!(uris[0], "stub://doc/0");
    assert_eq!(uris.last().unwrap(), "stub://bin");
    let first = call(&s, "resources/list", json!({})).await;
    assert_eq!(
        first["result"]["resources"][0],
        json!({"uri": "stub://doc/0", "name": "doc 0", "description": "stub doc 0",
               "mimeType": "text/markdown"})
    );
    for bad in [json!("x"), json!("9999"), json!(3)] {
        let r = call(&s, "resources/list", json!({"cursor": bad})).await;
        assert_eq!(r["error"]["code"], -32602, "{bad}: {r}");
    }
}

#[tokio::test]
async fn resources_templates_list_names_each_template() {
    let r = call(&server(1), "resources/templates/list", json!({})).await;
    assert_eq!(
        r["result"],
        json!({"resourceTemplates": [{"uriTemplate": "stub://doc/{n}", "name": "doc",
               "description": "a stub doc", "mimeType": "text/markdown"}]})
    );
}

#[tokio::test]
async fn resources_read_answers_text_or_a_blob_and_refuses_an_unknown_uri() {
    let s = server(3);
    let r = call(&s, "resources/read", json!({"uri": "stub://doc/2"})).await;
    assert_eq!(
        r["result"],
        json!({"contents": [{"uri": "stub://doc/2", "mimeType": "text/markdown",
               "text": "# doc 2"}]})
    );
    let r = call(&s, "resources/read", json!({"uri": "stub://bin"})).await;
    assert_eq!(r["result"]["contents"][0]["blob"], "/wA=");
    let r = call(&s, "resources/read", json!({"uri": "stub://doc/7"})).await;
    assert_eq!(r["error"]["code"], -32602);
    assert_eq!(
        r["error"]["message"],
        "invalid arguments: unknown resource: stub://doc/7"
    );
    let r = call(&s, "resources/read", json!({})).await;
    assert_eq!(r["error"]["code"], -32602);
}

#[tokio::test]
async fn prompts_list_and_get_render_a_user_message() {
    let s = server(1);
    let r = call(&s, "prompts/list", json!({})).await;
    assert_eq!(
        r["result"],
        json!({"prompts": [{"name": "greet", "description": "Greet someone.", "arguments": [
            {"name": "who", "description": "who to greet", "required": true},
            {"name": "tone", "description": "how", "required": false},
        ]}]})
    );
    let r = call(
        &s,
        "prompts/get",
        json!({"name": "greet", "arguments": {"who": "matt"}}),
    )
    .await;
    assert_eq!(
        r["result"],
        json!({"description": "Greet someone.", "messages": [{"role": "user",
               "content": {"type": "text", "text": "Say hello to matt (plainly)."}}]})
    );
}

#[tokio::test]
async fn prompts_get_refuses_an_unknown_prompt_or_bad_arguments() {
    let s = server(1);
    let cases = [
        (
            json!({"name": "nope"}),
            "invalid arguments: unknown prompt: nope",
        ),
        (
            json!({"name": "greet"}),
            "invalid arguments: prompt `greet` needs the argument `who`",
        ),
        (
            json!({"name": "greet", "arguments": {"who": 1}}),
            "invalid arguments: prompt `greet`: argument `who` must be a string",
        ),
        (
            json!({"name": "greet", "arguments": ["matt"]}),
            "invalid arguments: prompt `greet`: `arguments` must be an object",
        ),
    ];
    for (params, message) in cases {
        let r = call(&s, "prompts/get", params.clone()).await;
        assert_eq!(r["error"]["code"], -32602, "{params}: {r}");
        assert_eq!(r["error"]["message"], message, "{params}");
    }
}

#[test]
fn a_uri_or_prompt_listed_twice_is_refused() {
    let r = McpServer::from_sources(
        Sources {
            resources: vec![Arc::new(Docs { count: 1 }), Arc::new(Docs { count: 1 })],
            ..Sources::default()
        },
        &ToolFilter::all(),
    );
    assert!(matches!(r, Err(ServeError::DuplicateResource(u)) if u == "stub://doc/0"));
    let r = McpServer::from_sources(
        Sources {
            prompts: vec![Arc::new(Greet), Arc::new(Greet)],
            ..Sources::default()
        },
        &ToolFilter::all(),
    );
    assert!(matches!(r, Err(ServeError::DuplicatePrompt(n)) if n == "greet"));
}
