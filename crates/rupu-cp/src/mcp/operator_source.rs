//! The operator tools as a [`ToolSource`]: each call is built into an HTTP
//! request and answered by the CP's own API router, in process.

use super::ops::{ops, OpSpec};
use async_trait::async_trait;
use rupu_mcp::{CallCx, ToolDef, ToolResult, ToolSource};
use serde_json::{json, Map, Value};
use tower::ServiceExt as _;

/// Largest API response a tool call reads.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub struct OperatorSource {
    inner: axum::Router,
}

impl OperatorSource {
    pub fn new(inner: axum::Router) -> Self {
        Self { inner }
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
    let (q, _) = lift((op.query)());
    for (k, v) in q {
        props.entry(k).or_insert(v);
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
    json!({"type": "object", "properties": props, "required": required})
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
        ops()
            .iter()
            .map(|op| ToolDef {
                name: op.name.to_string(),
                description: op.description.to_string(),
                input_schema: input_schema(op),
                hints: op.hints,
            })
            .collect()
    }

    async fn call(&self, name: &str, args: Value, _cx: &CallCx) -> Option<ToolResult> {
        let op = ops().iter().find(|o| o.name == name)?;
        Some(self.invoke(op, args).await)
    }
}

impl OperatorSource {
    async fn invoke(&self, op: &OpSpec, args: Value) -> ToolResult {
        let mut args = match args {
            Value::Object(o) => o,
            Value::Null => Map::new(),
            _ => return ToolResult::Error("arguments must be an object".into()),
        };
        let schema = input_schema(op);
        if let Some(req) = schema["required"].as_array() {
            for r in req.iter().filter_map(Value::as_str) {
                if args.get(r).is_none_or(Value::is_null) {
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
                id_for_root = Some(val.clone());
            }
            path = path.replace(&format!(":{p}"), &encode_segment(&val));
        }
        if path.contains("{root}") {
            let id = id_for_root.unwrap_or_default();
            let Some(route) = crate::host::messages::MessageRoute::of_id(&id) else {
                return ToolResult::Error(format!("`{id}` is not a run, session or agentiflow id"));
            };
            path = path.replace("{root}", route.segment());
        }

        // Query: the query schema's properties, taken out of the arguments.
        let (qprops, _) = lift((op.query)());
        let query = {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for key in qprops.keys() {
                if let Some(v) = args.remove(key) {
                    if let Some(s) = scalar(&v) {
                        ser.append_pair(key, &s);
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
        let body = if (op.body)().is_some() {
            axum::body::Body::from(Value::Object(args).to_string())
        } else {
            axum::body::Body::empty()
        };
        let request = match http::Request::builder()
            .method(op.method.clone())
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-rupu-client", "mcp")
            .body(body)
        {
            Ok(r) => r,
            Err(e) => return ToolResult::Error(format!("bad request: {e}")),
        };

        let resp = match self.inner.clone().oneshot(request).await {
            Ok(r) => r,
            Err(e) => match e {},
        };
        let status = resp.status();
        let bytes = match axum::body::to_bytes(resp.into_body(), MAX_RESPONSE_BYTES).await {
            Ok(b) => b,
            Err(e) => return ToolResult::Error(format!("HTTP {}: {e}", status.as_u16())),
        };
        if !status.is_success() {
            return ToolResult::Error(format!(
                "HTTP {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&bytes)
            ));
        }
        if bytes.is_empty() {
            return ToolResult::Json(json!({"status": status.as_u16()}));
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => ToolResult::Json(v),
            Err(_) => ToolResult::Text(String::from_utf8_lossy(&bytes).into_owned()),
        }
    }
}
