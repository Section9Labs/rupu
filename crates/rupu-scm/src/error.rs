//! ScmError + per-platform classification.
//!
//! Spec §4b + §9d. Recoverable variants surface to the agent as JSON
//! tool errors (the agent decides what to do). Unrecoverable variants
//! abort the run with an actionable message (mirrors Plan 1's
//! ProviderError::Unauthorized UX).

use std::time::Duration;

use reqwest::header::HeaderMap;
use thiserror::Error;

use crate::platform::Platform;

#[derive(Debug, Error)]
pub enum ScmError {
    // ─── Recoverable ───
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    #[error("transient: {0}")]
    Transient(#[source] anyhow::Error),

    #[error("conflict: {message}")]
    Conflict { message: String },

    #[error("not found: {what}")]
    NotFound { what: String },

    // ─── Unrecoverable ───
    #[error("unauthorized for {platform}: {hint}")]
    Unauthorized { platform: String, hint: String },

    #[error("missing scope `{scope}` for {platform}: {hint}")]
    MissingScope {
        platform: String,
        scope: String,
        hint: String,
    },

    #[error("network unreachable: {0}")]
    Network(#[source] anyhow::Error),

    #[error("bad request: {message}")]
    BadRequest { message: String },
}

impl ScmError {
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Transient(_)
                | Self::Conflict { .. }
                | Self::NotFound { .. }
        )
    }
}

/// Map an HTTP failure into the structured ScmError vocabulary. Pure
/// for testability; per-platform adapters call this at the boundary
/// between raw HTTP and trait return values. Spec §9d table.
pub fn classify_scm_error(
    platform: Platform,
    status: u16,
    body: &str,
    headers: &HeaderMap,
) -> ScmError {
    match status {
        401 => ScmError::Unauthorized {
            platform: platform.as_str().into(),
            hint: format!(
                "run: rupu auth login --provider {} --mode sso",
                platform.as_str()
            ),
        },
        403 => {
            // GitHub uses 403 both for missing-scope AND primary rate limits.
            // Differentiate by the X-Accepted-OAuth-Scopes header.
            if let Some(missing) = scope_missing(headers) {
                ScmError::MissingScope {
                    platform: platform.as_str().into(),
                    scope: missing,
                    hint: format!(
                        "re-login to grant the missing scope: rupu auth login --provider {} --mode sso",
                        platform.as_str()
                    ),
                }
            } else {
                ScmError::RateLimited {
                    retry_after: parse_retry_after(headers),
                }
            }
        }
        404 => ScmError::NotFound {
            what: extract_message(body).unwrap_or_else(|| "(unknown)".into()),
        },
        409 | 422 => {
            let message = extract_message(body).unwrap_or_else(|| truncate(body, 200));
            // 422 is split: GitHub uses it for both validation errors AND merge conflicts.
            // Bias toward Conflict only when the message hints at a write conflict.
            let lower = message.to_lowercase();
            if lower.contains("already exists")
                || lower.contains("conflict")
                || lower.contains("not mergeable")
            {
                ScmError::Conflict { message }
            } else if status == 422 {
                ScmError::BadRequest { message }
            } else {
                ScmError::Conflict { message }
            }
        }
        400 => ScmError::BadRequest {
            message: extract_message(body).unwrap_or_else(|| truncate(body, 200)),
        },
        429 => ScmError::RateLimited {
            retry_after: parse_retry_after(headers),
        },
        500..=599 => ScmError::Transient(anyhow::anyhow!(
            "{platform} {status}: {}",
            truncate(body, 200)
        )),
        _ => ScmError::Transient(anyhow::anyhow!(
            "{platform} {status}: {}",
            truncate(body, 200)
        )),
    }
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let v = headers.get("Retry-After")?.to_str().ok()?.trim();
    v.parse::<u64>().ok().map(Duration::from_secs)
}

fn scope_missing(headers: &HeaderMap) -> Option<String> {
    let granted: std::collections::HashSet<_> = headers
        .get("X-OAuth-Scopes")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();
    let needed: Vec<String> = headers
        .get("X-Accepted-OAuth-Scopes")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();
    let missing: Vec<&String> = needed.iter().filter(|s| !granted.contains(*s)).collect();
    if missing.is_empty() && needed.is_empty() {
        None
    } else if missing.is_empty() {
        None
    } else {
        Some(
            missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

fn extract_message(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("message")?
        .as_str()
        .map(String::from)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}
