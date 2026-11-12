//! Stateless authoring parse projection. Never resolves execution dependencies.
use axum::{
    extract::Json,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use rupu_orchestrator::Workflow;
use serde::{
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    io::{self, Write},
};
use tokio::sync::Semaphore;

pub(crate) const FEATURE: &str = "workflow.validation_projection.v1";
pub(crate) const MAX_REQUEST: usize = 2 * 1024 * 1024;
const MAX_SOURCE: usize = 256 * 1024;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 32_768;
const MAX_COLLECTION: usize = 2048;
const MAX_SCALAR_BYTES: usize = 1024 * 1024;
static PARSERS: Semaphore = Semaphore::const_new(2);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    schema_version: u32,
    nonce: String,
    raw: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    Invalid,
    Bound,
    Unsupported,
    Busy,
    Internal,
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Invalid => (
                StatusCode::BAD_REQUEST,
                "workflow_invalid",
                "Workflow source or projection request is invalid",
            ),
            Self::Bound => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "projection_limit",
                "Workflow projection exceeds a source, structure or response limit",
            ),
            Self::Unsupported => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "projection_unavailable",
                "Workflow uses YAML values that cannot be projected exactly as JSON; use Source",
            ),
            Self::Busy => (
                StatusCode::TOO_MANY_REQUESTS,
                "projection_busy",
                "Workflow parser is busy; validate again when ready",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "projection_failed",
                "Workflow projection could not be completed",
            ),
        };
        (
            status,
            Json(serde_json::json!({"code":code,"error":message})),
        )
            .into_response()
    }
}

/// The permit belongs to the actual blocking work, not the request waiter. A
/// disconnected caller cannot release admission while parsing still runs.
pub(super) async fn validate(
    request: Result<Json<Request>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(error) => {
            return if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                Failure::Bound.into_response()
            } else {
                Failure::Invalid.into_response()
            }
        }
    };
    respond(&PARSERS, move || project(request)).await
}

async fn respond(
    parsers: &'static Semaphore,
    work: impl FnOnce() -> Result<Vec<u8>, Failure> + Send + 'static,
) -> Response {
    let permit = match parsers.try_acquire() {
        Ok(permit) => permit,
        Err(_) => return Failure::Busy.into_response(),
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    {
        Ok(Ok(bytes)) => ([(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Ok(Err(error)) => error.into_response(),
        Err(_) => Failure::Internal.into_response(),
    }
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    scalars: usize,
    failure: Option<Failure>,
}
impl Budget {
    fn fail<E: de::Error>(&mut self, failure: Failure) -> E {
        self.failure = Some(failure);
        E::custom("workflow projection preflight refused")
    }
    fn scalar<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        self.scalars = self.scalars.saturating_add(bytes);
        if self.scalars > MAX_SCALAR_BYTES {
            return Err(self.fail(Failure::Bound));
        }
        Ok(())
    }
}
/// Count expanded nodes before recursively materializing Workflow. This visitor
/// allocates no YAML tree. Alias expansion is charged on every visited value;
/// source/event loading itself is bounded by MAX_SOURCE and serde_yaml's parser.
struct Probe<'a> {
    budget: &'a mut Budget,
    depth: usize,
    key: bool,
}
impl<'de> DeserializeSeed<'de> for Probe<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        self.budget.nodes += 1;
        if self.depth > MAX_DEPTH || self.budget.nodes > MAX_NODES {
            return Err(self.budget.fail(Failure::Bound));
        }
        deserializer.deserialize_any(self)
    }
}
impl Probe<'_> {
    fn plain<E: de::Error>(&mut self) -> Result<(), E> {
        if self.key {
            return Err(self.budget.fail(Failure::Unsupported));
        }
        Ok(())
    }
}
impl<'de> Visitor<'de> for Probe<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("bounded JSON-compatible YAML")
    }
    fn visit_unit<E: de::Error>(mut self) -> Result<(), E> {
        self.plain()
    }
    fn visit_none<E: de::Error>(mut self) -> Result<(), E> {
        self.plain()
    }
    fn visit_bool<E: de::Error>(mut self, _: bool) -> Result<(), E> {
        self.plain()
    }
    fn visit_i64<E: de::Error>(mut self, _: i64) -> Result<(), E> {
        self.plain()
    }
    fn visit_u64<E: de::Error>(mut self, _: u64) -> Result<(), E> {
        self.plain()
    }
    fn visit_f64<E: de::Error>(mut self, value: f64) -> Result<(), E> {
        self.plain()?;
        if !value.is_finite() {
            return Err(self.budget.fail(Failure::Unsupported));
        }
        Ok(())
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        self.budget.scalar(value.len())
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<(), E> {
        self.visit_str(&value)
    }
    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<(), A::Error> {
        self.plain()?;
        let mut count = 0;
        while seq
            .next_element_seed(Probe {
                budget: &mut *self.budget,
                depth: self.depth + 1,
                key: false,
            })?
            .is_some()
        {
            count += 1;
            if count > MAX_COLLECTION {
                return Err(self.budget.fail(Failure::Bound));
            }
        }
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<(), A::Error> {
        self.plain()?;
        let mut count = 0;
        while map
            .next_key_seed(Probe {
                budget: &mut *self.budget,
                depth: self.depth + 1,
                key: true,
            })?
            .is_some()
        {
            count += 1;
            if count > MAX_COLLECTION {
                return Err(self.budget.fail(Failure::Bound));
            }
            map.next_value_seed(Probe {
                budget: &mut *self.budget,
                depth: self.depth + 1,
                key: false,
            })?;
        }
        Ok(())
    }
    fn visit_enum<A: de::EnumAccess<'de>>(self, _: A) -> Result<(), A::Error> {
        Err(self.budget.fail(Failure::Unsupported))
    }
}
fn check_yaml(raw: &str) -> Result<(), Failure> {
    let mut budget = Budget::default();
    Probe {
        budget: &mut budget,
        depth: 0,
        key: false,
    }
    .deserialize(serde_yaml::Deserializer::from_str(raw))
    .map_err(|_| budget.failure.unwrap_or(Failure::Invalid))
}

struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_RESPONSE {
            return Err(io::Error::other("projection response bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Serialize)]
struct Projection<'a> {
    schema: &'static str,
    schema_version: u32,
    parser_contract: &'static str,
    nonce: &'a str,
    source_sha256: String,
    source_bytes: usize,
    ok: bool,
    workflow: &'a Workflow,
}
fn project(request: Request) -> Result<Vec<u8>, Failure> {
    if request.schema_version != 1
        || request.nonce.is_empty()
        || request.nonce.len() > 128
        || !request
            .nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Failure::Invalid);
    }
    if request.raw.len() > MAX_SOURCE {
        return Err(Failure::Bound);
    }
    check_yaml(&request.raw)?;
    let workflow = Workflow::parse(&request.raw).map_err(|_| Failure::Invalid)?;
    let response = Projection {
        schema: FEATURE,
        schema_version: 1,
        parser_contract: "rupu.workflow.v1",
        nonce: &request.nonce,
        source_sha256: format!("{:x}", Sha256::digest(request.raw.as_bytes())),
        source_bytes: request.raw.len(),
        ok: true,
        workflow: &workflow,
    };
    let mut bytes = BoundedBytes(Vec::new());
    serde_json::to_writer(&mut bytes, &response).map_err(|e| {
        if e.is_io() {
            Failure::Bound
        } else {
            Failure::Unsupported
        }
    })?;
    // Validate the serialized envelope too: struct/enum wrappers can add depth
    // beyond the YAML source. Return no partial or truncated semantic projection.
    let tree: serde_json::Value =
        serde_json::from_slice(&bytes.0).map_err(|_| Failure::Unsupported)?;
    let mut pending = vec![(&tree, 0)];
    let mut nodes = 0;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if depth > MAX_DEPTH || nodes > MAX_NODES * 2 {
            return Err(Failure::Bound);
        }
        match value {
            serde_json::Value::Array(v) => pending.extend(v.iter().map(|v| (v, depth + 1))),
            serde_json::Value::Object(v) => pending.extend(v.values().map(|v| (v, depth + 1))),
            _ => (),
        }
    }
    Ok(bytes.0)
}

#[cfg(test)]
#[path = "workflow_validation_projection_tests.rs"]
mod tests;
