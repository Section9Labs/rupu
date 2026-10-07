//! One guard for every `/api/*` path parameter.
//!
//! Axum percent-decodes a path parameter, so `/api/agents/..%2F..%2Fx` hands
//! a handler `../../x`, and `%2Fetc%2Fpasswd` an absolute path (which
//! `Path::join` lets replace the base). Path parameters here are ids, names,
//! slugs and digests: each is joined onto a store directory, put into an
//! `ssh` argv, or spliced into a remote host's URL. Rather than trust every
//! handler and store to validate its own, [`reject_unsafe_segments`] refuses —
//! with a 400, before routing — any `/api/*` path with a segment that decodes
//! to something that cannot be a single, plain path component:
//!
//! - `.` or `..`, or anything starting with `.` or `-` (hidden names; an
//!   argv option);
//! - a `/` or `\` (an encoded separator), or a NUL / control character;
//! - a `?` or `#` (would re-shape a forwarded URL);
//! - an invalid percent-escape or non-UTF-8 bytes.
//!
//! Handlers still validate their own parameters (`validate_id`,
//! `validate_name`, …); this guard is the floor under all of them.

use crate::error::ApiError;
use axum::{extract::Request, middleware::Next, response::IntoResponse, response::Response};

/// Percent-decode one path segment; `None` on a malformed escape or bytes
/// that are not UTF-8.
fn decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Why `segment` (raw, still percent-encoded) is refused, if it is.
pub(crate) fn unsafe_segment(segment: &str) -> Option<&'static str> {
    let Some(d) = decode(segment) else {
        return Some("a malformed percent-escape");
    };
    if d.starts_with('.') {
        return Some("a `.`-prefixed or `..` component");
    }
    if d.starts_with('-') {
        return Some("a `-`-prefixed component");
    }
    if d.contains(['/', '\\']) {
        return Some("an encoded path separator");
    }
    if d.contains(['?', '#']) {
        return Some("an encoded `?` or `#`");
    }
    if d.chars().any(char::is_control) {
        return Some("a control character");
    }
    None
}

/// Refuse an `/api/*` request whose path has an unsafe segment (see the
/// module doc). Empty segments (`//`, a trailing `/`) are left to the router.
pub async fn reject_unsafe_segments(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            if let Some(why) = unsafe_segment(seg) {
                return ApiError::bad_request(format!(
                    "invalid path segment {seg:?}: {why} is not allowed in an API path"
                ))
                .into_response();
            }
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::unsafe_segment;

    #[test]
    fn plain_ids_names_and_digests_pass() {
        for ok in [
            "run_01J9ZK3",
            "oracle-recon",
            "af_a-b_c",
            "my%20agent",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "v1.2",
        ] {
            assert_eq!(unsafe_segment(ok), None, "{ok}");
        }
    }

    #[test]
    fn traversal_separators_and_injection_are_refused() {
        for bad in [
            "..",
            ".",
            "%2e%2E",
            ".hidden",
            "..%2Fetc",
            "%2Fetc%2Fpasswd",
            "a%2fb",
            "a%5Cb",
            "%00",
            "a%0Ab",
            "x%3Fy",
            "x%23y",
            "-oProxyCommand",
            "%zz",
            "%ff",
        ] {
            assert!(unsafe_segment(bad).is_some(), "{bad} must be refused");
        }
    }
}
