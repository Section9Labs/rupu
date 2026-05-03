//! Pure functions mapping vendor HTTP/error responses to ProviderError.
//!
//! Each adapter calls its corresponding `classify_*` at the boundary
//! between raw HTTP and the agent loop. Spec §7b. Pure for testability.

use std::time::Duration;

use crate::auth_mode::AuthMode;
use crate::error::ProviderError;

fn parse_retry_after(_body: &str) -> Option<Duration> {
    // Plan 1 keeps it simple — body parsing varies by vendor and isn't
    // observed by the tests. Plan 2 wires the actual Retry-After header
    // via the call site (which has access to reqwest::Response::headers).
    None
}

pub fn classify_anthropic(status: u16, body: &str, _vendor_code: Option<&str>) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized {
            provider: "anthropic".into(),
            auth_mode: AuthMode::ApiKey,
            hint: "run: rupu auth login --provider anthropic".into(),
        },
        404 => ProviderError::ModelUnavailable {
            model: "(unknown)".into(),
        },
        400 => ProviderError::BadRequest {
            message: truncate(body, 500),
        },
        429 | 529 => ProviderError::RateLimited {
            retry_after: parse_retry_after(body),
        },
        500..=503 => ProviderError::Transient(anyhow::anyhow!(
            "anthropic transient {status}: {}",
            truncate(body, 200)
        )),
        _ => ProviderError::Other(anyhow::anyhow!(
            "anthropic {status}: {}",
            truncate(body, 200)
        )),
    }
}

pub fn classify_openai(status: u16, body: &str, vendor_code: Option<&str>) -> ProviderError {
    match (status, vendor_code) {
        (401, _) => ProviderError::Unauthorized {
            provider: "openai".into(),
            auth_mode: AuthMode::ApiKey,
            hint: "run: rupu auth login --provider openai".into(),
        },
        (403, Some(c)) if c.contains("billing") || c.contains("quota") => {
            ProviderError::QuotaExceeded {
                provider: "openai".into(),
            }
        }
        (404, _) => ProviderError::ModelUnavailable {
            model: "(unknown)".into(),
        },
        (400, _) => ProviderError::BadRequest {
            message: truncate(body, 500),
        },
        (429, _) => ProviderError::RateLimited {
            retry_after: parse_retry_after(body),
        },
        (500..=503, _) => ProviderError::Transient(anyhow::anyhow!(
            "openai transient {status}: {}",
            truncate(body, 200)
        )),
        _ => ProviderError::Other(anyhow::anyhow!("openai {status}: {}", truncate(body, 200))),
    }
}

pub fn classify_gemini(status: u16, body: &str, _vendor_code: Option<&str>) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized {
            provider: "gemini".into(),
            auth_mode: AuthMode::ApiKey,
            hint: "run: rupu auth login --provider gemini".into(),
        },
        404 => ProviderError::ModelUnavailable {
            model: "(unknown)".into(),
        },
        400 => ProviderError::BadRequest {
            message: truncate(body, 500),
        },
        429 => ProviderError::RateLimited {
            retry_after: parse_retry_after(body),
        },
        500..=503 => ProviderError::Transient(anyhow::anyhow!(
            "gemini transient {status}: {}",
            truncate(body, 200)
        )),
        _ => ProviderError::Other(anyhow::anyhow!("gemini {status}: {}", truncate(body, 200))),
    }
}

pub fn classify_copilot(status: u16, body: &str, _vendor_code: Option<&str>) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized {
            provider: "copilot".into(),
            auth_mode: AuthMode::ApiKey,
            hint: "run: rupu auth login --provider copilot".into(),
        },
        404 => ProviderError::ModelUnavailable {
            model: "(unknown)".into(),
        },
        400 => ProviderError::BadRequest {
            message: truncate(body, 500),
        },
        429 => ProviderError::RateLimited {
            retry_after: parse_retry_after(body),
        },
        500..=503 => ProviderError::Transient(anyhow::anyhow!(
            "copilot transient {status}: {}",
            truncate(body, 200)
        )),
        _ => ProviderError::Other(anyhow::anyhow!("copilot {status}: {}", truncate(body, 200))),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
