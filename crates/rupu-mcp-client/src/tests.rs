use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::Request,
    response::Response as AxumResponse,
    routing::any,
    Router,
};
use std::{collections::VecDeque, sync::Mutex as StdMutex};

#[derive(Clone)]
struct Seen {
    method: reqwest::Method,
    headers: HeaderMap,
    body: Value,
}
enum Reply {
    Result(Value),
    Raw(Value),
    Status(u16),
    Redirect(String),
    Events(Vec<Value>, Option<Value>),
    Disconnect,
    Bytes(Vec<u8>),
    Delayed(Duration, Value),
}
struct Script {
    replies: StdMutex<VecDeque<Reply>>,
    seen: StdMutex<Vec<Seen>>,
    observed: tokio::sync::Notify,
}
struct Fixture {
    url: String,
    state: Arc<Script>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn handler(State(script): State<Arc<Script>>, request: Request<Body>) -> AxumResponse {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, MAX_BYTES).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    script.seen.lock().unwrap().push(Seen {
        method: parts.method,
        headers: parts.headers,
        body: body.clone(),
    });
    script.observed.notify_one();
    let reply = script
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected HTTP request (possible retry)");
    let result = |value: Value| json!({"jsonrpc":"2.0","id":body["id"],"result":value});
    let mut builder = AxumResponse::builder();
    if body["method"] == "initialize" {
        builder = builder.header(SESSION, "fixture-session");
    }
    match reply {
        Reply::Result(value) => builder
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(result(value).to_string()))
            .unwrap(),
        Reply::Raw(value) => builder
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        Reply::Status(status) => builder.status(status).body(Body::empty()).unwrap(),
        Reply::Redirect(url) => builder
            .status(302)
            .header("location", url)
            .body(Body::empty())
            .unwrap(),
        Reply::Events(mut events, final_result) => {
            if let Some(value) = final_result {
                events.push(result(value));
            }
            let mut bytes = b"\xef\xbb\xbf: fixture\r\n\r\n".to_vec();
            for event in events {
                bytes.extend_from_slice(format!("data: {event}\r\n\r\n").as_bytes());
            }
            let chunks: Vec<_> = bytes
                .chunks(1)
                .map(|b| Ok::<_, std::io::Error>(b.to_vec()))
                .collect();
            builder
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                .unwrap()
        }
        Reply::Disconnect => builder
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(futures_util::stream::iter(vec![
                Ok(b": response lost\n\n".to_vec()),
                Err(std::io::Error::other("fixture disconnect")),
            ])))
            .unwrap(),
        Reply::Bytes(bytes) => builder
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .unwrap(),
        Reply::Delayed(delay, value) => {
            tokio::time::sleep(delay).await;
            builder
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(result(value).to_string()))
                .unwrap()
        }
    }
}
impl Fixture {
    async fn start(replies: Vec<Reply>) -> Self {
        let state = Arc::new(Script {
            replies: StdMutex::new(replies.into()),
            seen: StdMutex::new(Vec::new()),
            observed: tokio::sync::Notify::new(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/mcp", any(handler))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { url, state, task }
    }
    fn registry(&self, authenticated: bool) -> Registry {
        Registry::parse(&format!(
            "version=1\n[servers.fixture]\nlabel='Fixture'\nurl='{}'\nenabled=true\n{}",
            self.url,
            if authenticated {
                "bearer_env='RUPU_MCP_FIXTURE_TOKEN'"
            } else {
                ""
            }
        ))
        .unwrap()
    }
    fn calls(&self, method: &str) -> usize {
        self.state
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.body["method"] == method)
            .count()
    }
}
#[derive(Default)]
struct FakeCredentials(StdMutex<Vec<String>>);
impl Credentials for FakeCredentials {
    fn bearer(&self, reference: &str) -> Result<String, Error> {
        self.0.lock().unwrap().push(reference.into());
        Ok("fixture-secret-not-a-real-token".into())
    }
}
fn init() -> Reply {
    Reply::Result(
        json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{"tools":{}},
    "serverInfo":{"name":"fixture","version":"1"},"instructions":"untrusted fixture instructions"}),
    )
}
fn tool(name: &str) -> Value {
    json!({"name":name,"description":"Fixture", "inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}})
}
fn list() -> Reply {
    Reply::Result(json!({"tools":[tool("query")]}))
}
fn success() -> Value {
    json!({"content":[{"type":"text","text":"fixture result"}],"structuredContent":{"fixture":true}})
}
fn uncaptured() -> Capture {
    Capture {
        context: rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System),
        sink: Arc::new(rupu_netflow::NullSink),
    }
}
async fn connect(fixture: &Fixture) -> HttpToolsClient {
    HttpToolsClient::connect(
        &fixture.registry(false),
        "fixture",
        &FakeCredentials::default(),
        Deadlines::default(),
        uncaptured(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn handshake_headers_discovery_and_structured_call_are_real_http() {
    let fixture = Fixture::start(vec![
        init(),
        Reply::Status(202),
        list(),
        Reply::Result(success()),
        Reply::Status(405),
    ])
    .await;
    let credentials = FakeCredentials::default();
    let registry = fixture.registry(true);
    let client = HttpToolsClient::connect(
        &registry,
        "fixture",
        &credentials,
        Deadlines::default(),
        uncaptured(),
    )
    .await
    .unwrap();
    let tools = client.discover().await.unwrap();
    assert_eq!(client.discover().await.unwrap()[0].name(), tools[0].name());
    assert_eq!(
        fixture.calls("tools/list"),
        1,
        "reinspection keeps the frozen snapshot"
    );
    let result = client
        .call(&tools[0], json!({"question":"fixture"}))
        .await
        .unwrap();
    assert!(!result.is_error);
    assert_eq!(tools[0].server_id(), "fixture");
    assert_eq!(
        tools[0].local_effect(),
        rupu_config::mcp::ToolEffect::External
    );
    assert_eq!(result.structured_content.unwrap()["fixture"], true);
    assert_eq!(
        registry.servers["fixture"].effect(tools[0].name()),
        rupu_config::mcp::ToolEffect::External,
        "remote readOnlyHint is not local policy"
    );
    client.close().await.unwrap();
    {
        let seen = fixture.state.seen.lock().unwrap();
        assert_eq!(seen[0].body["method"], "initialize");
        assert_eq!(seen[0].body["params"]["capabilities"], json!({}));
        assert!(!seen[0].headers.contains_key(SESSION));
        assert_eq!(seen[1].body["method"], "notifications/initialized");
        for request in &seen[1..] {
            assert_eq!(request.headers[SESSION], "fixture-session");
            assert_eq!(request.headers[VERSION], PROTOCOL_VERSION);
            assert_eq!(
                request.headers[AUTHORIZATION],
                "Bearer fixture-secret-not-a-real-token"
            );
        }
        assert_eq!(seen.last().unwrap().method, reqwest::Method::DELETE);
    }
    assert_eq!(
        credentials.0.lock().unwrap().as_slice(),
        &["RUPU_MCP_FIXTURE_TOKEN"]
    );
    assert!(!format!("{client:?} {registry:?}").contains("fixture-secret"));
    assert!(matches!(
        client.call(&tools[0], json!({})).await,
        Err(Error::Expired)
    ));
}

#[tokio::test]
async fn paging_is_bounded_and_duplicate_or_repeated_cursors_fail() {
    let fixture = Fixture::start(vec![
        init(),
        Reply::Status(202),
        Reply::Result(json!({"tools":[tool("first")],"nextCursor":"next"})),
        Reply::Result(json!({"tools":[tool("second")]})),
    ])
    .await;
    let client = connect(&fixture).await;
    assert_eq!(client.discover().await.unwrap().len(), 2);
    assert_eq!(
        fixture.state.seen.lock().unwrap()[3].body["params"]["cursor"],
        "next"
    );
    for second in [
        json!({"tools":[tool("first")]}),
        json!({"tools":[],"nextCursor":"next"}),
    ] {
        let fixture = Fixture::start(vec![
            init(),
            Reply::Status(202),
            Reply::Result(json!({"tools":[tool("first")],"nextCursor":"next"})),
            Reply::Result(second),
        ])
        .await;
        assert!(matches!(
            connect(&fixture).await.discover().await,
            Err(Error::Protocol(_))
        ));
        assert_eq!(fixture.calls("tools/list"), 2);
    }
}

#[tokio::test]
async fn handles_cannot_cross_clients_and_invalid_arguments_never_dispatch() {
    let a = Fixture::start(vec![init(), Reply::Status(202), list()]).await;
    let b = Fixture::start(vec![init(), Reply::Status(202)]).await;
    let client_a = connect(&a).await;
    let tools = client_a.discover().await.unwrap();
    let client_b = connect(&b).await;
    assert!(matches!(
        client_b.call(&tools[0], json!({})).await,
        Err(Error::WrongOwner)
    ));
    assert!(matches!(
        client_a.call(&tools[0], json!([])).await,
        Err(Error::Configuration)
    ));
    assert_eq!(a.calls("tools/call") + b.calls("tools/call"), 0);
}

#[tokio::test]
async fn chunked_sse_handles_utf8_notifications_and_refuses_server_capability_requests() {
    let events = vec![
        json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"message":"évidence"}}),
        json!({"jsonrpc":"2.0","id":"server-request","method":"sampling/createMessage","params":{}}),
    ];
    let fixture = Fixture::start(vec![
        init(),
        Reply::Status(202),
        list(),
        Reply::Events(events, Some(success())),
        Reply::Status(202),
    ])
    .await;
    let client = connect(&fixture).await;
    let tools = client.discover().await.unwrap();
    assert!(client.call(&tools[0], json!({})).await.is_ok());
    let seen = fixture.state.seen.lock().unwrap();
    assert_eq!(seen[4].body["error"]["code"], -32601);
    assert_eq!(seen[4].body["id"], "server-request");
}

#[tokio::test]
async fn list_changed_invalidates_future_calls_without_mutating_completed_result() {
    let fixture = Fixture::start(vec![
        init(),
        Reply::Status(202),
        list(),
        Reply::Events(
            vec![json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})],
            Some(success()),
        ),
    ])
    .await;
    let client = connect(&fixture).await;
    let tools = client.discover().await.unwrap();
    assert!(client.call(&tools[0], json!({})).await.is_ok());
    assert!(matches!(
        client.call(&tools[0], json!({})).await,
        Err(Error::Stale)
    ));
    assert_eq!(fixture.calls("tools/call"), 1);
}

#[tokio::test]
async fn remote_tool_failure_and_rpc_failure_are_distinct() {
    let fixture = Fixture::start(vec![
        init(),
        Reply::Status(202),
        list(),
        Reply::Result(
            json!({"content":[{"type":"text","text":"invalid fixture input"}],"isError":true}),
        ),
        Reply::Raw(json!({"jsonrpc":"2.0","id":4,"error":{"code":-32602,"message":"fixture"}})),
    ])
    .await;
    let client = connect(&fixture).await;
    let tools = client.discover().await.unwrap();
    assert!(client.call(&tools[0], json!({})).await.unwrap().is_error);
    assert!(matches!(
        client.call(&tools[0], json!({})).await,
        Err(Error::Rpc(-32602))
    ));
}

#[tokio::test]
async fn disconnect_wrong_id_http_failure_and_oversize_are_uncertain_without_retry() {
    for reply in [
        Reply::Disconnect,
        Reply::Raw(json!({"jsonrpc":"2.0","id":999,"result":success()})),
        Reply::Status(503),
        Reply::Bytes(vec![b' '; MAX_BYTES + 1]),
    ] {
        let fixture = Fixture::start(vec![init(), Reply::Status(202), list(), reply]).await;
        let client = connect(&fixture).await;
        let tools = client.discover().await.unwrap();
        assert!(matches!(
            client.call(&tools[0], json!({})).await,
            Err(Error::Uncertain)
        ));
        assert_eq!(fixture.calls("tools/call"), 1);
        assert!(matches!(
            client.call(&tools[0], json!({})).await,
            Err(Error::Uncertain)
        ));
        assert_eq!(
            fixture.calls("tools/call"),
            1,
            "uncertain connections remain blocked"
        );
    }
}

#[tokio::test]
async fn deadline_sends_best_effort_cancellation_but_never_repeats_call() {
    let fixture = Fixture::start(vec![
        init(),
        Reply::Status(202),
        list(),
        Reply::Delayed(Duration::from_millis(100), success()),
        Reply::Status(202),
    ])
    .await;
    let client = HttpToolsClient::connect(
        &fixture.registry(false),
        "fixture",
        &FakeCredentials::default(),
        Deadlines {
            metadata: Duration::from_secs(2),
            call: Duration::from_millis(20),
        },
        uncaptured(),
    )
    .await
    .unwrap();
    let tools = client.discover().await.unwrap();
    assert!(matches!(
        client.call(&tools[0], json!({})).await,
        Err(Error::Uncertain)
    ));
    assert_eq!(fixture.calls("tools/call"), 1);
    assert_eq!(fixture.calls("notifications/cancelled"), 1);
}

#[tokio::test]
async fn redirects_do_not_transfer_credentials_or_reissue_calls() {
    let destination = Fixture::start(vec![]).await;
    let fixture = Fixture::start(vec![
        init(),
        Reply::Status(202),
        list(),
        Reply::Redirect(destination.url.clone()),
    ])
    .await;
    let client = HttpToolsClient::connect(
        &fixture.registry(true),
        "fixture",
        &FakeCredentials::default(),
        Deadlines::default(),
        uncaptured(),
    )
    .await
    .unwrap();
    let tools = client.discover().await.unwrap();
    assert!(matches!(
        client.call(&tools[0], json!({})).await,
        Err(Error::Uncertain)
    ));
    assert!(destination.state.seen.lock().unwrap().is_empty());
    assert_eq!(fixture.calls("tools/call"), 1);
}

#[tokio::test]
async fn expired_session_does_not_reinitialize_and_replay_a_call() {
    let fixture =
        Fixture::start(vec![init(), Reply::Status(202), list(), Reply::Status(404)]).await;
    let client = connect(&fixture).await;
    let tools = client.discover().await.unwrap();
    for _ in 0..2 {
        assert!(matches!(
            client.call(&tools[0], json!({})).await,
            Err(Error::Expired)
        ));
    }
    assert_eq!(fixture.calls("initialize"), 1);
    assert_eq!(fixture.calls("tools/call"), 1);
}

#[tokio::test]
async fn operator_server_and_unsupported_versions_stop_before_discovery() {
    for (name, version) in [("rupu", PROTOCOL_VERSION), ("fixture", "unsupported")] {
        let fixture = Fixture::start(vec![Reply::Result(json!({"protocolVersion":version,
            "capabilities":{"tools":{}},"serverInfo":{"name":name,"version":"1"}}))])
        .await;
        assert!(HttpToolsClient::connect(
            &fixture.registry(false),
            "fixture",
            &FakeCredentials::default(),
            Deadlines::default(),
            uncaptured()
        )
        .await
        .is_err());
        assert_eq!(fixture.state.seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn disabled_configuration_does_not_read_credentials_or_connect() {
    let fixture = Fixture::start(vec![]).await;
    let mut registry = fixture.registry(true);
    registry.servers.get_mut("fixture").unwrap().enabled = false;
    let credentials = FakeCredentials::default();
    assert!(matches!(
        HttpToolsClient::connect(
            &registry,
            "fixture",
            &credentials,
            Deadlines::default(),
            uncaptured()
        )
        .await,
        Err(Error::Configuration)
    ));
    assert!(credentials.0.lock().unwrap().is_empty());
    assert!(fixture.state.seen.lock().unwrap().is_empty());
}

#[test]
fn sse_all_byte_boundaries_and_cr_only_framing() {
    for delimiter in ["\r\n", "\r", "\n"] {
        let expected = json!({"jsonrpc":"2.0","id":1,"result":{"text":"évidence"}});
        let bytes = format!("\u{feff}data: {expected}{delimiter}{delimiter}").into_bytes();
        let mut decoder = sse::Decoder::default();
        let mut actual = Vec::new();
        for byte in bytes {
            actual.extend(decoder.push(&[byte]).unwrap());
        }
        actual.extend(decoder.finish().unwrap());
        assert_eq!(actual, [expected]);
    }
}

#[tokio::test]
async fn discovery_refuses_oversized_catalog_and_unsupported_execution_contract() {
    let too_many = (0..257)
        .map(|n| tool(&format!("tool{n}")))
        .collect::<Vec<_>>();
    let mut required = tool("requires-task");
    required["execution"] = json!({"taskSupport":"required"});
    for tools in [too_many, vec![required]] {
        let fixture = Fixture::start(vec![
            init(),
            Reply::Status(202),
            Reply::Result(json!({"tools":tools})),
        ])
        .await;
        assert!(connect(&fixture).await.discover().await.is_err());
        assert_eq!(fixture.calls("tools/call"), 0);
    }
}

#[tokio::test]
async fn returned_resource_link_is_data_and_never_fetched() {
    let fixture = Fixture::start(vec![init(), Reply::Status(202), list(), Reply::Result(json!({
        "content":[{"type":"resource_link","uri":"https://unreachable.invalid/fixture","name":"Fixture"}]
    }))]).await;
    let client = connect(&fixture).await;
    let tools = client.discover().await.unwrap();
    let result = client.call(&tools[0], json!({})).await.unwrap();
    assert_eq!(result.content[0]["type"], "resource_link");
    assert_eq!(fixture.state.seen.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn abort_after_dispatch_latches_uncertainty_before_another_caller_can_send() {
    let fixture = Fixture::start(vec![
        init(),
        Reply::Status(202),
        list(),
        Reply::Delayed(Duration::from_secs(1), success()),
    ])
    .await;
    let client = Arc::new(connect(&fixture).await);
    let tool = client.discover().await.unwrap().remove(0);
    let pending = {
        let client = client.clone();
        let tool = tool.clone();
        tokio::spawn(async move { client.call(&tool, json!({})).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let notified = fixture.state.observed.notified();
            if fixture.calls("tools/call") == 1 {
                break;
            }
            notified.await;
        }
    })
    .await
    .unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert!(matches!(
        client.call(&tool, json!({})).await,
        Err(Error::Uncertain)
    ));
    assert_eq!(fixture.calls("tools/call"), 1);
}

#[tokio::test]
async fn malformed_rpc_error_is_uncertain_and_blocks_another_dispatch() {
    for error in [json!({"code":-32603}), json!({"code":-32603,"message":3})] {
        let fixture = Fixture::start(vec![
            init(),
            Reply::Status(202),
            list(),
            Reply::Raw(json!({"jsonrpc":"2.0","id":3,"error":error})),
        ])
        .await;
        let client = connect(&fixture).await;
        let tool = client.discover().await.unwrap().remove(0);
        for _ in 0..2 {
            assert!(matches!(
                client.call(&tool, json!({})).await,
                Err(Error::Uncertain)
            ));
        }
        assert_eq!(fixture.calls("tools/call"), 1);
    }
}

#[tokio::test]
async fn capture_uses_each_run_sink_and_never_records_headers_or_payloads() {
    let sink_a = Arc::new(rupu_netflow::MemorySink::default());
    let sink_b = Arc::new(rupu_netflow::MemorySink::default());
    for (run, sink) in [
        ("run_fixture_a", sink_a.clone()),
        ("run_fixture_b", sink_b.clone()),
    ] {
        let fixture = Fixture::start(vec![
            init(),
            Reply::Status(202),
            list(),
            Reply::Result(success()),
        ])
        .await;
        let mut context = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System);
        context.run_id = Some(run.into());
        context.workspace_id = Some("ws_fixture".into());
        let client = HttpToolsClient::connect(
            &fixture.registry(true),
            "fixture",
            &FakeCredentials::default(),
            Deadlines::default(),
            Capture {
                context,
                sink: sink.clone(),
            },
        )
        .await
        .unwrap();
        let tool = client.discover().await.unwrap().remove(0);
        client
            .call(&tool, json!({"private":"private-fixture-argument"}))
            .await
            .unwrap();
        let records = sink.records();
        assert_eq!(records.len(), 4);
        assert!(records
            .iter()
            .all(|record| record.ctx.run_id.as_deref() == Some(run)
                && record.ctx.workspace_id.as_deref() == Some("ws_fixture")
                && record.ctx.origin == rupu_netflow::Origin::Mcp("fixture".into())));
        let recorded = serde_json::to_string(&records).unwrap();
        for private in [
            "fixture-secret-not-a-real-token",
            "fixture-session",
            "private-fixture-argument",
        ] {
            assert!(!recorded.contains(private));
        }
    }
    assert!(sink_a
        .records()
        .iter()
        .all(|r| r.ctx.run_id.as_deref() == Some("run_fixture_a")));
    assert!(sink_b
        .records()
        .iter()
        .all(|r| r.ctx.run_id.as_deref() == Some("run_fixture_b")));
}

#[tokio::test]
async fn host_store_resolves_exact_binding_and_never_falls_back() {
    let fixture = Fixture::start(vec![init(), Reply::Status(202)]).await;
    let tmp = tempfile::tempdir().unwrap();
    let generation = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let operation = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    let mut registry = fixture.registry(false);
    registry.servers.get_mut("fixture").unwrap().bearer_store = Some(generation.into());
    let owner = rupu_auth::mcp_credentials::Binding {
        server_id: "fixture".into(),
        url: fixture.url.clone(),
        generation: generation.into(),
    };
    let credentials = HostCredentials::for_global(tmp.path());
    assert!(matches!(
        HttpToolsClient::connect(
            &registry,
            "fixture",
            &credentials,
            Deadlines::default(),
            uncaptured()
        )
        .await,
        Err(Error::Credential)
    ));
    assert!(matches!(
        HttpToolsClient::connect(
            &registry,
            "fixture",
            &EnvironmentCredentials,
            Deadlines::default(),
            uncaptured()
        )
        .await,
        Err(Error::Credential)
    ));
    assert_eq!(fixture.calls("initialize"), 0);
    let store = rupu_auth::mcp_credentials::Store::for_global(tmp.path());
    let receipt = store
        .mutate(&owner, "absent", operation, Some("fixture-host-token"))
        .unwrap();
    let client = HttpToolsClient::connect(
        &registry,
        "fixture",
        &credentials,
        Deadlines::default(),
        uncaptured(),
    )
    .await
    .unwrap();
    assert_eq!(
        fixture.state.seen.lock().unwrap()[0].headers[AUTHORIZATION],
        "Bearer fixture-host-token"
    );
    assert!(!format!("{client:?} {registry:?}").contains("fixture-host-token"));
    let wrong = rupu_auth::mcp_credentials::Binding {
        server_id: "other".into(),
        ..owner.clone()
    };
    assert!(matches!(
        credentials.host_bearer(&wrong),
        Err(Error::Credential)
    ));
    store
        .mutate(
            &owner,
            &receipt.credential_revision,
            "01ARZ3NDEKTSV4RRFFQ69G5FAX",
            None,
        )
        .unwrap();
    assert!(matches!(
        credentials.host_bearer(&owner),
        Err(Error::Credential)
    ));
}
