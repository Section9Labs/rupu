//! Opt-in authoring projection. Uses the owning definition serializer without
//! profile expansion or the run-detail DTO's trimming/omissions.
use crate::error::ApiError;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rupu_agentiflow::AgentiflowDef;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{self, Write};

pub(crate) const FEATURE: &str = "agentiflow.validation_definition.v1";
const MAX_SOURCE: usize = 256 * 1024;
const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 16_384;

#[derive(Debug)]
pub(super) enum ValidationError {
    Legacy(ApiError),
    SourceTooLarge,
    Unavailable,
}
impl From<ApiError> for ValidationError {
    fn from(value: ApiError) -> Self {
        Self::Legacy(value)
    }
}
impl IntoResponse for ValidationError {
    fn into_response(self) -> Response {
        match self {
            Self::Legacy(error) => error.into_response(),
            Self::SourceTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({
                "code":"definition_source_too_large",
                "error":"Structured validation accepts at most 256 KiB of source"
            }))).into_response(),
            Self::Unavailable => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({
                "code":"definition_echo_unavailable",
                "error":"This definition cannot be represented exactly within the structured editor's JSON limits; use Source"
            }))).into_response(),
        }
    }
}
pub(super) fn check_source(raw: &str) -> Result<(), ValidationError> {
    if raw.len() > MAX_SOURCE {
        Err(ValidationError::SourceTooLarge)
    } else {
        Ok(())
    }
}

/// Opaque scope coordinates may contain YAML-specific values. Refuse rather
/// than stringify map keys, discard tags, or turn nonfinite values into null.
fn json_safe_yaml(value: &serde_yaml::Value, depth: usize, nodes: &mut usize) -> bool {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return false;
    }
    match value {
        serde_yaml::Value::Null | serde_yaml::Value::Bool(_) | serde_yaml::Value::String(_) => true,
        serde_yaml::Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
        serde_yaml::Value::Sequence(rows) => {
            rows.iter().all(|v| json_safe_yaml(v, depth + 1, nodes))
        }
        serde_yaml::Value::Mapping(rows) => rows.iter().all(|(k, v)| {
            matches!(k, serde_yaml::Value::String(_)) && json_safe_yaml(v, depth + 1, nodes)
        }),
        serde_yaml::Value::Tagged(_) => false,
    }
}
fn bounded_tree(value: &Value, depth: usize, nodes: &mut usize) -> bool {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return false;
    }
    match value {
        Value::Array(rows) => rows.iter().all(|v| bounded_tree(v, depth + 1, nodes)),
        Value::Object(rows) => rows.values().all(|v| bounded_tree(v, depth + 1, nodes)),
        _ => true,
    }
}
#[derive(Default)]
struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_RESPONSE {
            return Err(io::Error::other("definition echo exceeds response bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Serialize)]
struct Echo<'a> {
    ok: bool,
    name: &'a str,
    lead: &'a str,
    engagement_profiles: &'a [String],
    definition: &'a AgentiflowDef,
    definition_schema: &'static str,
    source_sha256: String,
    scope_id: Option<&'a str>,
}
pub(super) fn response(
    def: &AgentiflowDef,
    raw: &str,
    scope_id: Option<&str>,
) -> Result<Value, ValidationError> {
    check_source(raw)?;
    let finite = def.coverage.as_ref().is_none_or(|c| c.reach.is_finite())
        && def.budget.as_ref().is_none_or(|b| {
            b.usd.is_none_or(f64::is_finite) && b.soft_at.is_none_or(f64::is_finite)
        });
    let mut nodes = 0;
    if !finite
        || !def.scope.roots.iter().all(|root| {
            root.fields
                .values()
                .all(|v| json_safe_yaml(v, 0, &mut nodes))
        })
    {
        return Err(ValidationError::Unavailable);
    }
    let echo = Echo {
        ok: true,
        name: &def.name,
        lead: &def.lead,
        engagement_profiles: &def.engagement_profiles,
        definition: def,
        definition_schema: "agentiflow.def.v1",
        source_sha256: format!("{:x}", Sha256::digest(raw.as_bytes())),
        scope_id,
    };
    // Prove the full serialized byte bound before building a JSON tree.
    let mut bytes = BoundedBytes::default();
    serde_json::to_writer(&mut bytes, &echo).map_err(|_| ValidationError::Unavailable)?;
    // Preserve typed integer/f64 values directly rather than introducing a
    // textual number parse whose rounding could change semantic comparison.
    let value = serde_json::to_value(&echo).map_err(|_| ValidationError::Unavailable)?;
    if !bounded_tree(&value["definition"], 0, &mut 0) {
        return Err(ValidationError::Unavailable);
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
