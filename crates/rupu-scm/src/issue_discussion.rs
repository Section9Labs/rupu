//! Bounded operator discussion reads. This port does not alter runtime issue tools.
use crate::{IssueRef, IssueTracker, ScmError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PAGE_SIZE: usize = 20;
pub const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_IDENTITY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscussionIdentity {
    pub connector_epoch: String,
    pub api_profile: String,
    pub project_id: String,
    pub issue_id: String,
    pub identifier: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscussionComment {
    pub id: String,
    pub kind: String,
    pub author: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub parent_comment_id: Option<String>,
    pub body: String,
    pub format: String,
    pub source_format: String,
    pub projection_complete: bool,
    pub unsupported_nodes: u32,
    pub unsupported_marks: u32,
    pub metadata_truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscussionPage {
    pub identity: DiscussionIdentity,
    pub items: Vec<DiscussionComment>,
    pub next: Option<String>,
    pub order: String,
    pub provider_exhausted: bool,
    pub input_bytes: usize,
}
pub(crate) fn failed() -> ScmError {
    ScmError::BadRequest {
        message: "discussion_response_unavailable".into(),
    }
}
pub(crate) fn changed() -> ScmError {
    ScmError::BadRequest {
        message: "discussion_selection_changed".into(),
    }
}
pub(crate) fn unsupported() -> ScmError {
    ScmError::BadRequest {
        message: "discussion_unsupported".into(),
    }
}
pub(crate) fn valid(value: &str, cap: usize) -> bool {
    !value.is_empty() && value.len() <= cap && !value.chars().any(char::is_control)
}
fn text(value: &Value, cap: usize) -> Result<String, ScmError> {
    value
        .as_str()
        .filter(|s| valid(s, cap))
        .map(str::to_owned)
        .ok_or_else(failed)
}
fn id(value: &Value) -> Result<String, ScmError> {
    let value = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .ok_or_else(failed)?;
    if !valid(&value, 256)
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-=+/".contains(&b))
    {
        return Err(failed());
    }
    Ok(value)
}
fn decimal(value: &Value) -> Result<String, ScmError> {
    let s = id(value)?;
    let n = s
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(failed)?;
    if n.to_string() != s {
        return Err(failed());
    }
    Ok(s)
}
fn timestamp(value: &Value) -> Result<String, ScmError> {
    let s = text(value, 128)?;
    if chrono::DateTime::parse_from_rfc3339(&s).is_err()
        && chrono::DateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f%z").is_err()
    {
        return Err(failed());
    }
    Ok(s)
}
fn optional_timestamp(value: &Value) -> Result<Option<String>, ScmError> {
    if value.is_null() {
        Ok(None)
    } else {
        timestamp(value).map(Some)
    }
}
fn author(value: &Value) -> Result<(Option<String>, bool), ScmError> {
    if value.is_null() {
        return Ok((None, false));
    }
    let s = value.as_str().ok_or_else(failed)?;
    if s.chars().any(char::is_control) {
        return Err(failed());
    }
    let mut end = s.len().min(512);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Ok((Some(s[..end].to_owned()), end < s.len()))
}
fn page_number(after: Option<&str>, initial: u32) -> Result<u32, ScmError> {
    let Some(s) = after else { return Ok(initial) };
    let n = s.parse::<u32>().map_err(|_| failed())?;
    if n.to_string() != s || n < initial || n > 1_000_000 {
        return Err(failed());
    }
    Ok(n)
}
fn unchanged(a: &str, b: &str) -> Result<(), ScmError> {
    if a == b {
        Ok(())
    } else {
        Err(changed())
    }
}

mod adf;
mod providers;
mod transport;
pub(crate) use transport::{http, rest_next, Session};
#[cfg(test)]
mod tests;
