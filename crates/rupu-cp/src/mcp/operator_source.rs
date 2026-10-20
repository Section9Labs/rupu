//! The operator tools as a [`ToolSource`]: each call is built into an HTTP
//! request and answered by the CP's own API router, in process.

use super::ops::{ops, OpSpec};
use super::ExpectedRoute;
use async_trait::async_trait;
use rupu_mcp::{CallCx, EmbeddedResource, ToolDef, ToolResult, ToolSource};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};
use tower::ServiceExt as _;

/// Largest API response a tool call reads.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// An op with its schemas lifted once.
struct Prepared {
    op: OpSpec,
    schema: Value,
    query_keys: Vec<String>,
    has_body: bool,
}

pub struct OperatorSource {
    inner: axum::Router,
    prepared: Vec<Prepared>,
}

impl OperatorSource {
    /// Every operator tool ([`ops`]) over `inner`.
    pub fn new(inner: axum::Router) -> Self {
        Self::with_ops(inner, ops().to_vec())
    }

    /// `ops` over `inner`.
    pub(crate) fn with_ops(inner: axum::Router, ops: Vec<OpSpec>) -> Self {
        let prepared = ops
            .into_iter()
            .map(|op| Prepared {
                schema: input_schema(&op),
                query_keys: lift((op.query)()).0.keys().cloned().collect(),
                has_body: (op.body)().is_some(),
                op,
            })
            .collect();
        Self { inner, prepared }
    }
}

/// `s` as a `&'static str`, each distinct value allocated once: the
/// `{root}`-expanded route templates (a handful) an [`ExpectedRoute`] names.
fn intern(s: String) -> &'static str {
    static SEEN: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(known) = seen.get(s.as_str()) {
        return known;
    }
    let leaked: &'static str = Box::leak(s.into_boxed_str());
    seen.insert(leaked);
    leaked
}

/// The `filename` of a `Content-Disposition` value (quoted or bare; the
/// `filename*` form is not read).
fn disposition_filename(v: &str) -> Option<String> {
    const KEY: &str = "filename=";
    let at = v.to_ascii_lowercase().find(KEY)?;
    let rest = &v[at + KEY.len()..];
    let name = match rest.strip_prefix('"') {
        Some(quoted) => &quoted[..quoted.find('"')?],
        None => rest.split(';').next().unwrap_or("").trim(),
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// What a note on an `x-rupu-*` response header adds to its value.
fn header_explanation(name: &str) -> Option<&'static str> {
    match name {
        "x-rupu-hosts-unfilterable" => {
            Some("these hosts were left out: they can't filter on the named field (host:field)")
        }
        "x-rupu-hosts-without-customer" => {
            Some("these hosts were left out: they can't report a customer")
        }
        _ => None,
    }
}

/// `definitions` of a schemars root, as a lookup for `$ref` inlining.
fn inline_refs(v: &Value, defs: &Map<String, Value>, depth: usize) -> Value {
    match v {
        Value::Object(o) => {
            if let Some(Value::String(r)) = o.get("$ref") {
                if let (Some(name), true) = (r.strip_prefix("#/definitions/"), depth < 16) {
                    if let Some(d) = defs.get(name) {
                        return inline_refs(d, defs, depth + 1);
                    }
                }
            }
            Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), inline_refs(x, defs, depth)))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| inline_refs(x, defs, depth)).collect()),
        other => other.clone(),
    }
}

/// A schema's `(properties, required)`, with definitions inlined.
fn lift(root: Option<schemars::schema::RootSchema>) -> (Map<String, Value>, Vec<String>) {
    let Some(root) = root else {
        return Default::default();
    };
    let v = serde_json::to_value(&root).unwrap_or(Value::Null);
    let defs = v
        .get("definitions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let props = v
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| {
            p.iter()
                .map(|(k, x)| (k.clone(), inline_refs(x, &defs, 0)))
                .collect()
        })
        .unwrap_or_default();
    let required = v
        .get("required")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    (props, required)
}

/// The path parameters of a template: `:name` segments, plus `id` for a
/// `{root}` template (whose root comes from it).
fn path_params(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|s| s.strip_prefix(':'))
        .collect()
}

fn input_schema(op: &OpSpec) -> Value {
    let mut props = Map::new();
    let mut required: Vec<String> = Vec::new();
    for p in path_params(op.path) {
        props.insert(
            p.to_string(),
            json!({"type": "string", "description": format!("The `{p}` path parameter.")}),
        );
        required.push(p.to_string());
    }
    let (q, q_required) = lift((op.query)());
    for (k, v) in q {
        props.entry(k).or_insert(v);
    }
    // A query field the handler requires (`runs.source`'s `path`) is as
    // required here as a path parameter.
    for r in q_required {
        if !required.contains(&r) {
            required.push(r);
        }
    }
    let (b, b_required) = lift((op.body)());
    for (k, v) in b {
        props.entry(k).or_insert(v);
    }
    for r in b_required {
        if !required.contains(&r) {
            required.push(r);
        }
    }
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

/// Percent-encode one path segment (everything but unreserved bytes).
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[async_trait]
impl ToolSource for OperatorSource {
    fn tools(&self) -> Vec<ToolDef> {
        self.prepared
            .iter()
            .map(|p| ToolDef {
                name: p.op.name.to_string(),
                description: p.op.description.to_string(),
                input_schema: p.schema.clone(),
                hints: p.op.hints,
            })
            .collect()
    }

    async fn call(&self, name: &str, args: Value, cx: &CallCx) -> Option<ToolResult> {
        let p = self.prepared.iter().find(|p| p.op.name == name)?;
        Some(self.invoke(p, args, cx).await)
    }
}

impl OperatorSource {
    async fn invoke(&self, p: &Prepared, args: Value, cx: &CallCx) -> ToolResult {
        let mut args = match args {
            Value::Object(o) => o,
            Value::Null => Map::new(),
            _ => return ToolResult::Error("arguments must be an object".into()),
        };
        let op = &p.op;
        // Agents send `null` for optional fields: treat it as absent.
        args.retain(|_, v| !v.is_null());
        let props = p.schema["properties"].as_object();
        if let Some(key) = args
            .keys()
            .find(|k| !props.is_some_and(|m| m.contains_key(k.as_str())))
        {
            return ToolResult::Error(format!("unknown argument `{key}`"));
        }
        if let Some(req) = p.schema["required"].as_array() {
            for r in req.iter().filter_map(Value::as_str) {
                if args.get(r).is_none_or(|v| v.as_str() == Some("")) {
                    return ToolResult::Error(format!("missing required argument `{r}`"));
                }
            }
        }

        // Path.
        let mut path = op.path.to_string();
        let mut id_for_root = None;
        for p in path_params(op.path) {
            let Some(val) = args.remove(p).as_ref().and_then(scalar) else {
                return ToolResult::Error(format!("argument `{p}` must be a string"));
            };
            if p == "id" {
                // A run id is `run_` + word characters: anything else (a
                // sibling route's literal segment such as `workflows`) would
                // reach another handler.
                if op.name.starts_with("runs.") && !crate::host::connector::valid_run_id(&val) {
                    return ToolResult::Error(format!("`{val}` is not a run id"));
                }
                id_for_root = Some(val.clone());
            }
            path = path.replace(&format!(":{p}"), &encode_segment(&val));
        }
        let mut expected = op.path;
        if path.contains("{root}") {
            let id = id_for_root.unwrap_or_default();
            let Some(route) = crate::host::messages::MessageRoute::of_id(&id) else {
                return ToolResult::Error(format!("`{id}` is not a run, session or agentiflow id"));
            };
            path = path.replace("{root}", route.segment());
            expected = intern(op.path.replace("{root}", route.segment()));
        }

        // Query: the query schema's properties, taken out of the arguments.
        let query = {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for key in &p.query_keys {
                if let Some(v) = args.remove(key) {
                    match scalar(&v) {
                        Some(s) => {
                            ser.append_pair(key, &s);
                        }
                        None => {
                            return ToolResult::Error(format!(
                                "argument `{key}` must be a string, number or boolean"
                            ))
                        }
                    }
                }
            }
            ser.finish()
        };
        let uri = if query.is_empty() {
            path
        } else {
            format!("{path}?{query}")
        };

        // Body: the rest, only for a route that has one.
        let body = if p.has_body {
            axum::body::Body::from(Value::Object(args).to_string())
        } else {
            axum::body::Body::empty()
        };
        let mut builder = http::Request::builder()
            .method(op.method.clone())
            .uri(&uri)
            .header("content-type", "application/json");
        // The caller's `Host`: a handler that derives an address from it
        // (`POST /api/hosts/node`) sees the one the client reached.
        if let Some(host) = &cx.host {
            builder = builder.header(http::header::HOST, host);
        }
        let mut request = match builder.body(body) {
            Ok(r) => r,
            Err(e) => return ToolResult::Error(format!("bad request: {e}")),
        };
        // How the handlers know the call came through `/mcp` (attribution).
        request.extensions_mut().insert(super::McpClient);
        // The route this call is for; `guarded` refuses any other.
        request.extensions_mut().insert(ExpectedRoute(expected));

        let resp = match self.inner.clone().oneshot(request).await {
            Ok(r) => r,
            Err(e) => match e {},
        };
        let (parts, body) = resp.into_parts();
        let status = parts.status;
        let bytes = match axum::body::to_bytes(body, MAX_RESPONSE_BYTES).await {
            Ok(b) => b,
            Err(e) => {
                let e = e.into_inner();
                if e.downcast_ref::<http_body_util::LengthLimitError>()
                    .is_some()
                {
                    return ToolResult::Error(format!(
                        "HTTP {}: the response is over 16 MiB; fetch {} {uri} from the API with its token instead",
                        status.as_u16(),
                        op.method
                    ));
                }
                return ToolResult::Error(format!("HTTP {}: {e}", status.as_u16()));
            }
        };
        if !status.is_success() {
            return ToolResult::Error(format!(
                "HTTP {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&bytes)
            ));
        }
        if op.bytes {
            let header = |name| {
                parts
                    .headers
                    .get(name)
                    .and_then(|v: &http::HeaderValue| v.to_str().ok())
            };
            return ToolResult::Resource(EmbeddedResource {
                // `rupu://api` stands for the API root: `/api/x` is `rupu://api/x`.
                uri: format!(
                    "rupu://api{}",
                    uri.strip_prefix("/api")
                        .filter(|rest| rest.starts_with('/'))
                        .unwrap_or(&uri)
                ),
                mime_type: header(http::header::CONTENT_TYPE)
                    .unwrap_or("application/octet-stream")
                    .to_string(),
                name: header(http::header::CONTENT_DISPOSITION).and_then(disposition_filename),
                bytes,
            });
        }
        // A body that isn't JSON is text.
        let (value, text) = if bytes.is_empty() {
            (json!({"status": status.as_u16()}), false)
        } else {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => (v, false),
                Err(_) => (
                    Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                    true,
                ),
            }
        };
        // `x-rupu-*` headers say something the body doesn't (hosts left out
        // of a fan-out): the caller sees each as a note.
        let mut notes = Vec::new();
        let mut headers = Map::new();
        for (name, v) in &parts.headers {
            let name = name.as_str();
            if !name.starts_with("x-rupu-") {
                continue;
            }
            let v = String::from_utf8_lossy(v.as_bytes()).into_owned();
            notes.push(match header_explanation(name) {
                Some(why) => format!("{name}: {v} — {why}"),
                None => format!("{name}: {v}"),
            });
            headers.insert(name.to_string(), Value::String(v));
        }
        if notes.is_empty() {
            match value {
                Value::String(t) if text => ToolResult::Text(t),
                v => ToolResult::Json(v),
            }
        } else {
            ToolResult::Annotated {
                value,
                notes,
                meta: json!({ "rupu/headers": headers }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::ops::op;
    use axum::routing::{get, post};
    use http::{header, HeaderMap, Method};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Call `op` over `router` (with the route guard) as the operator
    /// source does.
    async fn call(router: axum::Router, op: OpSpec, args: Value, cx: CallCx) -> ToolResult {
        let name = op.name;
        OperatorSource::with_ops(crate::mcp::guarded(router), vec![op])
            .call(name, args, &cx)
            .await
            .expect("the op is this source's")
    }

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct FormatQuery {
        format: Option<String>,
    }

    #[tokio::test]
    async fn the_guard_refuses_before_the_handler_runs() {
        let hits = Arc::new(AtomicUsize::new(0));
        let (a, b) = (hits.clone(), hits.clone());
        let router = axum::Router::new()
            .route(
                "/t/:id",
                post(move || async move {
                    a.fetch_add(1, Ordering::SeqCst);
                    "param"
                }),
            )
            .route(
                "/t/literal",
                post(move || async move {
                    b.fetch_add(1, Ordering::SeqCst);
                    "literal"
                }),
            );
        let r = call(
            router,
            op("t.post", Method::POST, "/t/:id", "Post one."),
            json!({"id": "literal"}),
            CallCx::default(),
        )
        .await;
        match r {
            ToolResult::Error(t) => {
                assert!(t.contains("routes to"), "{t}");
                assert!(t.starts_with("HTTP 400"), "{t}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_guard_passes_a_matching_route() {
        let hits = Arc::new(AtomicUsize::new(0));
        let a = hits.clone();
        let router = axum::Router::new().route(
            "/t/:id",
            post(move || async move {
                a.fetch_add(1, Ordering::SeqCst);
                "param"
            }),
        );
        let r = call(
            router,
            op("t.post", Method::POST, "/t/:id", "Post one."),
            json!({"id": "x"}),
            CallCx::default(),
        )
        .await;
        assert!(matches!(&r, ToolResult::Text(t) if t == "param"), "{r:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_callers_host_is_the_requests_host() {
        let router = axum::Router::new().route(
            "/t/x",
            get(|h: HeaderMap| async move {
                axum::Json(
                    h.get(header::HOST)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string(),
                )
            }),
        );
        let cx = CallCx {
            host: Some("10.1.2.3:7879".into()),
            ..Default::default()
        };
        let r = call(
            router,
            op("t.get", Method::GET, "/t/x", "Get it."),
            json!({}),
            cx,
        )
        .await;
        assert!(
            matches!(&r, ToolResult::Json(Value::String(h)) if h == "10.1.2.3:7879"),
            "{r:?}"
        );
    }

    #[tokio::test]
    async fn a_bytes_op_returns_a_resource_named_from_content_disposition() {
        let router = axum::Router::new().route(
            "/t/x",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "application/pdf"),
                        (
                            header::CONTENT_DISPOSITION,
                            "attachment; filename=\"r.pdf\"",
                        ),
                    ],
                    b"%PDF-1.7".to_vec(),
                )
            }),
        );
        let r = call(
            router,
            op("t.export", Method::GET, "/t/x", "Export it.")
                .query::<FormatQuery>()
                .bytes(),
            json!({"format": "pdf"}),
            CallCx::default(),
        )
        .await;
        let ToolResult::Resource(res) = r else {
            panic!("{r:?}")
        };
        assert_eq!(res.mime_type, "application/pdf");
        assert_eq!(res.name.as_deref(), Some("r.pdf"));
        assert_eq!(res.uri, "rupu://api/t/x?format=pdf");
        assert_eq!(&res.bytes[..], b"%PDF-1.7");
    }

    #[tokio::test]
    async fn an_oversize_body_names_the_api_path() {
        let router =
            axum::Router::new().route("/t/x", get(|| async { vec![b'a'; 17 * 1024 * 1024] }));
        let r = call(
            router,
            op("t.get", Method::GET, "/t/x", "Get it."),
            json!({}),
            CallCx::default(),
        )
        .await;
        match r {
            ToolResult::Error(t) => {
                assert!(t.contains("over 16 MiB"), "{t}");
                assert!(t.contains("/t/x"), "{t}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn x_rupu_headers_become_notes() {
        let router = axum::Router::new().route(
            "/t/x",
            get(|| async {
                (
                    [("x-rupu-hosts-unfilterable", "peer:status")],
                    axum::Json(json!([1, 2])),
                )
            }),
        );
        let r = call(
            router,
            op("t.get", Method::GET, "/t/x", "Get it."),
            json!({}),
            CallCx::default(),
        )
        .await;
        let ToolResult::Annotated { value, notes, meta } = r else {
            panic!("{r:?}")
        };
        assert_eq!(value, json!([1, 2]));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0]
                .starts_with("x-rupu-hosts-unfilterable: peer:status — these hosts were left out"),
            "{notes:?}"
        );
        assert_eq!(
            meta["rupu/headers"]["x-rupu-hosts-unfilterable"],
            "peer:status"
        );
    }

    #[tokio::test]
    async fn a_body_without_x_rupu_headers_stays_plain_json() {
        let router = axum::Router::new().route(
            "/t/x",
            get(|| async { ([("x-other", "1")], axum::Json(json!({"a": 1}))) }),
        );
        let r = call(
            router,
            op("t.get", Method::GET, "/t/x", "Get it."),
            json!({}),
            CallCx::default(),
        )
        .await;
        assert!(
            matches!(&r, ToolResult::Json(v) if v == &json!({"a": 1})),
            "{r:?}"
        );
    }

    #[test]
    fn filenames_are_read_from_quoted_and_bare_dispositions() {
        assert_eq!(
            disposition_filename("attachment; filename=\"a; b.txt\"; filename*=UTF-8''x")
                .as_deref(),
            Some("a; b.txt")
        );
        assert_eq!(
            disposition_filename("inline; FILENAME=r.pdf").as_deref(),
            Some("r.pdf")
        );
        assert_eq!(disposition_filename("attachment"), None);
        assert_eq!(
            disposition_filename("attachment; filename*=UTF-8''x.pdf"),
            None
        );
    }

    #[test]
    fn segment_encoding_leaves_no_reserved_byte_raw() {
        let e = encode_segment("a/b?c#d%e");
        assert_eq!(e, "a%2Fb%3Fc%23d%25e");
        assert!(!e.contains(['/', '?', '#']) && e.matches('%').count() == 4);
    }

    #[test]
    fn no_op_has_overlapping_property_names() {
        for op in ops() {
            let path: Vec<String> = path_params(op.path).iter().map(|s| s.to_string()).collect();
            let q: Vec<String> = lift((op.query)()).0.keys().cloned().collect();
            let b: Vec<String> = lift((op.body)()).0.keys().cloned().collect();
            for (an, a, bn, bb) in [
                ("path", &path, "query", &q),
                ("path", &path, "body", &b),
                ("query", &q, "body", &b),
            ] {
                for k in a {
                    assert!(!bb.contains(k), "{}: `{k}` in {an} and {bn}", op.name);
                }
            }
        }
    }
}
