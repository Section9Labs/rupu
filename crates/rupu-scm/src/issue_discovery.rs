//! Current, bounded project issue discovery. Provider pagination is live, not a
//! snapshot; a next token does not promise more rows or a complete total.
use crate::{IssueRef, IssueTracker, ScmError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PAGE_SIZE: usize = 25;
pub const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
pub const TITLE_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryState {
    Open,
    Closed,
    All,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryRequest {
    pub project: String,
    pub state: DiscoveryState,
    pub title_query: Option<String>,
    pub after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveryRow {
    pub number: String,
    pub title: String,
    pub state: String,
    pub updated_at: Option<String>,
    pub url: Option<String>,
    pub title_truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveryPage {
    pub project: String,
    pub provider_project_id: String,
    pub connector_epoch: String,
    pub rows: Vec<DiscoveryRow>,
    pub next: Option<String>,
    pub end_observed: bool,
    /// True when the bounded REST page window ended without observing exhaustion.
    pub truncated: bool,
    /// "none" or "provider_title_query". Provider search semantics apply; no
    /// local filter pretends to search beyond the returned page.
    pub search_mode: String,
}
pub(crate) fn unsupported() -> ScmError {
    ScmError::BadRequest {
        message: "issue_discovery_unsupported".into(),
    }
}
fn failed() -> ScmError {
    ScmError::BadRequest {
        message: "issue_discovery_response_unavailable".into(),
    }
}
fn invalid() -> ScmError {
    ScmError::BadRequest {
        message: "issue_discovery_invalid_request".into(),
    }
}
pub fn validate(request: &DiscoveryRequest, tracker: IssueTracker) -> Result<(), ScmError> {
    crate::operator_issues::validate_project(&IssueRef {
        tracker,
        project: request.project.clone(),
        number: 1,
    })?;
    if request
        .title_query
        .as_deref()
        .is_some_and(|s| !valid(s, 128) || s.trim().is_empty())
        || request.after.as_deref().is_some_and(|s| !valid(s, 4096))
    {
        return Err(invalid());
    }
    if request.title_query.is_some() && matches!(tracker, IssueTracker::Github | IssueTracker::Jira)
    {
        return Err(ScmError::BadRequest {
            message: "issue_discovery_title_search_unsupported".into(),
        });
    }
    Ok(())
}
fn valid(s: &str, cap: usize) -> bool {
    !s.is_empty() && s.len() <= cap && !s.chars().any(char::is_control)
}
fn text(v: &Value, cap: usize) -> Result<String, ScmError> {
    v.as_str()
        .filter(|s| valid(s, cap))
        .map(str::to_owned)
        .ok_or_else(failed)
}
fn decimal(v: &Value) -> Result<String, ScmError> {
    let s = v
        .as_str()
        .map(str::to_owned)
        .or_else(|| v.as_u64().map(|n| n.to_string()))
        .ok_or_else(failed)?;
    let n = s.parse::<u64>().map_err(|_| failed())?;
    if n == 0 || n.to_string() != s {
        return Err(failed());
    }
    Ok(s)
}
fn same(v: &Value, expected: &str) -> Result<(), ScmError> {
    if v.as_str() == Some(expected) {
        Ok(())
    } else {
        Err(failed())
    }
}
fn rows(v: &Value) -> Result<&Vec<Value>, ScmError> {
    v.as_array()
        .filter(|a| a.len() <= PAGE_SIZE)
        .ok_or_else(failed)
}
fn page(after: Option<&str>) -> Result<u32, ScmError> {
    let Some(s) = after else { return Ok(1) };
    let n = s.parse::<u32>().map_err(|_| invalid())?;
    if n == 0 || n > 10_000 || n.to_string() != s {
        return Err(invalid());
    }
    Ok(n)
}
fn rest_continuation(page: u32, count: usize) -> (Option<String>, bool, bool) {
    if count < PAGE_SIZE {
        (None, true, false)
    } else if page == 10_000 {
        (None, false, true)
    } else {
        (Some((page + 1).to_string()), false, false)
    }
}
fn opaque(next: &Value, more: bool, after: Option<&str>) -> Result<Option<String>, ScmError> {
    if !more {
        if !next.is_null() && next.as_str().is_none() {
            return Err(failed());
        }
        return Ok(None);
    }
    let next = text(next, 4096)?;
    if Some(next.as_str()) == after {
        return Err(failed());
    }
    Ok(Some(next))
}
fn summary(
    number: String,
    title: &Value,
    state: &str,
    updated: &Value,
    url: &Value,
) -> Result<DiscoveryRow, ScmError> {
    let title = title
        .as_str()
        .filter(|s| !s.chars().any(char::is_control))
        .ok_or_else(failed)?;
    let mut end = title.len().min(TITLE_BYTES);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    let updated_at = if updated.is_null() {
        None
    } else {
        let s = text(updated, 128)?;
        let parsed = chrono::DateTime::parse_from_rfc3339(&s)
            .or_else(|_| chrono::DateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f%z"))
            .map_err(|_| failed())?;
        Some(parsed.to_rfc3339())
    };
    let url = if url.is_null() {
        None
    } else {
        let s = text(url, 2048)?;
        let u = url::Url::parse(&s).map_err(|_| failed())?;
        if !matches!(u.scheme(), "http" | "https")
            || u.host_str().is_none()
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
        {
            return Err(failed());
        }
        Some(s)
    };
    Ok(DiscoveryRow {
        number,
        title: title[..end].into(),
        state: state.into(),
        updated_at,
        url,
        title_truncated: end < title.len(),
    })
}
fn selected(request: &DiscoveryRequest, state: &str) -> Result<(), ScmError> {
    match (request.state, state) {
        (DiscoveryState::All, "open" | "closed")
        | (DiscoveryState::Open, "open")
        | (DiscoveryState::Closed, "closed") => Ok(()),
        _ => Err(failed()),
    }
}
fn query(path: &str, pairs: &[(&str, String)]) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        q.append_pair(k, v);
    }
    format!("{path}?{}", q.finish())
}
mod providers;
#[cfg(test)]
mod tests;
